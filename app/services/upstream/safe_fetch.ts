const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])

/**
 * Fetch an upstream endpoint without forwarding a request across redirects.
 * Redirecting authenticated POST requests can disclose headers, authorization
 * codes, refresh tokens, or client secrets to a different destination.
 */
export async function fetchWithoutRedirects(
  input: Parameters<typeof fetch>[0],
  init: Parameters<typeof fetch>[1],
  endpointLabel: string
) {
  const response = await fetch(input, { ...init, redirect: 'manual' })
  if (!REDIRECT_STATUSES.has(response.status)) {
    return response
  }

  await response.body?.cancel().catch(() => undefined)
  throw new Error(`${endpointLabel} redirected the request. Configure the final URL directly.`)
}
