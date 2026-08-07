const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])
const MAX_REDIRECTS = 5

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
 * Follow ordinary endpoint redirects without forwarding credentials to another
 * origin. A small redirect cap avoids loops while supporting canonical paths.
 */
export async function fetchWithSameOriginRedirects(
  input: Parameters<typeof fetch>[0],
  init: Parameters<typeof fetch>[1],
  endpointLabel: string
) {
  let request = initialRequest(input, init)

  for (let redirects = 0; ; redirects++) {
    const replay = request.clone()
    const response = await fetch(request, { redirect: 'manual' })
    if (!REDIRECT_STATUSES.has(response.status)) {
      return response
    }

    const location = response.headers.get('location')
    if (!location) {
      return response
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
