import type { HttpContext } from '@adonisjs/core/http'
import env from '#start/env'

/**
 * Public base URL for links shown in the UI (gateway, invite links).
 * Prefer the browser host on loopback during development so cloud/dev ports
 * stay correct. Production deployments use APP_URL as the trusted origin.
 */
export function publicAppUrl(request: HttpContext['request']) {
  const configured = env.get('APP_URL').replace(/\/$/, '')
  const host = request.host()
  if (host) {
    try {
      const hostname = new URL(`${request.protocol()}://${host}`).hostname
      const isLoopback =
        hostname === 'localhost' ||
        hostname === '127.0.0.1' ||
        hostname === '::1' ||
        hostname === '[::1]'
      if (isLoopback) {
        return `${request.protocol()}://${host}`
      }
    } catch {
      // Fall back to the configured origin for malformed request hosts.
    }
  }
  return configured
}
