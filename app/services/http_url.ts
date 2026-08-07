/**
 * Parse an MCP or OAuth endpoint. Query parameters and URL credentials are
 * intentionally preserved because some providers require them.
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
  if (url.hash) {
    throw new Error(`${label} must not include a fragment`)
  }
  return url
}
