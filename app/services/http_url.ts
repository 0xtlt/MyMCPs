/**
 * Parse an MCP or OAuth endpoint and reject URL forms unsupported by Fetch.
 * Query parameters are intentionally preserved because some providers require
 * them as part of the endpoint configuration.
 */
export function parseHttpUrl(value: string, label: string) {
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw new Error(`${label} must be a valid URL`)
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`${label} must use HTTP or HTTPS`)
  }
  if (url.username || url.password) {
    throw new Error(`${label} must not include URL credentials`)
  }
  if (url.hash) {
    throw new Error(`${label} must not include a fragment`)
  }
  return url
}
