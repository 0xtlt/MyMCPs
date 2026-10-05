const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])
const MAX_REDIRECTS = 5

/**
 * Most one upstream response may deliver, counted after decompression.
 * Generous for real tool results (base64 screenshots and file contents run to
 * a few MiB) while keeping one upstream from filling the memory of the single
 * gateway process.
 */
export const MAX_UPSTREAM_RESPONSE_BYTES = 32 * 1024 * 1024

/** Error responses only become diagnostics, so far less of them is read. */
export const MAX_UPSTREAM_ERROR_RESPONSE_BYTES = 64 * 1024

export type UpstreamResponseLimits = {
  /** Limit for a 2xx body. Reading past it fails. */
  maxResponseBytes?: number
  /** Limit for any other body. It is cut there, since callers only quote it. */
  maxErrorResponseBytes?: number
}

function decodedUrlCredential(value: string) {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function requestWithUrlCredentials(url: URL, init?: RequestInit) {
  const requestUrl = new URL(url)
  const headers = new Headers(init?.headers)

  if (requestUrl.username || requestUrl.password) {
    if (!headers.has('Authorization')) {
      const username = decodedUrlCredential(requestUrl.username)
      const password = decodedUrlCredential(requestUrl.password)
      headers.set(
        'Authorization',
        `Basic ${Buffer.from(`${username}:${password}`).toString('base64')}`
      )
    }
    requestUrl.username = ''
    requestUrl.password = ''
  }

  return new Request(requestUrl, { ...init, headers })
}

function initialRequest(input: Parameters<typeof fetch>[0], init?: RequestInit) {
  return input instanceof Request
    ? new Request(input, init)
    : requestWithUrlCredentials(new URL(input.toString()), init)
}

function switchesToGet(status: number, method: string) {
  return (
    (status === 303 && method !== 'GET' && method !== 'HEAD') ||
    ((status === 301 || status === 302) && method === 'POST')
  )
}

/**
 * Hand the response back with a body that stops at its limit. The SDK and our
 * own callers read bodies whole (`json()`, `text()`, the SSE parser), so the
 * limit has to sit in the stream they read from.
 */
function withLimitedBody(
  response: Response,
  endpointLabel: string,
  limits: UpstreamResponseLimits
) {
  if (!response.body) {
    return response
  }

  const truncates = !response.ok
  const maxBytes = truncates
    ? (limits.maxErrorResponseBytes ?? MAX_UPSTREAM_ERROR_RESPONSE_BYTES)
    : (limits.maxResponseBytes ?? MAX_UPSTREAM_RESPONSE_BYTES)
  const reader = response.body.getReader()
  let received = 0

  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      const { done, value } = await reader.read()
      if (done) {
        controller.close()
        return
      }

      received += value.byteLength
      if (received <= maxBytes) {
        controller.enqueue(value)
        return
      }

      await reader.cancel().catch(() => undefined)
      if (!truncates) {
        controller.error(new Error(`${endpointLabel} response exceeded ${maxBytes} bytes`))
        return
      }
      const kept = value.byteLength - (received - maxBytes)
      if (kept > 0) {
        controller.enqueue(value.subarray(0, kept))
      }
      controller.close()
    },
    cancel(reason) {
      return reader.cancel(reason)
    },
  })

  const limited = new Response(body, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  })
  // A constructed Response has no URL; the SDK reads it to describe redirects.
  Object.defineProperty(limited, 'url', { value: response.url })
  return limited
}

/**
 * Follow ordinary endpoint redirects without forwarding credentials to another
 * origin. A small redirect cap avoids loops while supporting canonical paths.
 * Response bodies are size-limited, see `UpstreamResponseLimits`.
 */
export async function fetchWithSameOriginRedirects(
  input: Parameters<typeof fetch>[0],
  init: Parameters<typeof fetch>[1],
  endpointLabel: string,
  limits: UpstreamResponseLimits = {}
) {
  let request = initialRequest(input, init)
  // Every hop, and the body of the final response, stays cancellable by the
  // caller. It has to be the caller's own signal: the one a Request derives
  // stops following it once that Request object is garbage-collected.
  const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined)

  for (let redirects = 0; ; redirects++) {
    const replay = request.clone()
    const response = await fetch(request, { redirect: 'manual', signal })
    if (!REDIRECT_STATUSES.has(response.status)) {
      return withLimitedBody(response, endpointLabel, limits)
    }

    const location = response.headers.get('location')
    if (!location) {
      return withLimitedBody(response, endpointLabel, limits)
    }

    const nextUrl = new URL(location, request.url)
    if (nextUrl.origin !== new URL(request.url).origin) {
      await response.body?.cancel().catch(() => undefined)
      throw new Error(`${endpointLabel} redirected to a different origin`)
    }
    if (redirects >= MAX_REDIRECTS) {
      await response.body?.cancel().catch(() => undefined)
      throw new Error(`${endpointLabel} exceeded ${MAX_REDIRECTS} redirects`)
    }

    await response.body?.cancel().catch(() => undefined)

    let method = replay.method.toUpperCase()
    const headers = new Headers(replay.headers)
    let body: ArrayBuffer | undefined
    if (switchesToGet(response.status, method)) {
      method = 'GET'
      for (const header of [
        'content-encoding',
        'content-language',
        'content-length',
        'content-location',
        'content-type',
      ]) {
        headers.delete(header)
      }
    } else if (method !== 'GET' && method !== 'HEAD') {
      body = await replay.arrayBuffer()
    }

    request = requestWithUrlCredentials(nextUrl, { method, headers, body })
  }
}
