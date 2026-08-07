const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])
const MAX_REDIRECTS = 5

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
  let request = new Request(input, init)

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

    request = new Request(nextUrl, { method, headers, body })
  }
}
