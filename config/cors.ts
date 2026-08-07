import app from '@adonisjs/core/services/app'
import { defineConfig } from '@adonisjs/cors'

export function resolveCorsOrigin(
  requestOrigin: string | undefined,
  requestUrl: string,
  isDevelopment: boolean
) {
  const pathname = requestUrl.split('?', 1)[0]
  if (
    [
      '/mcp',
      '/register',
      '/token',
      '/revoke',
      '/.well-known/oauth-authorization-server',
      '/.well-known/oauth-protected-resource',
      '/.well-known/oauth-protected-resource/mcp',
    ].includes(pathname)
  ) {
    return requestOrigin || true
  }
  return isDevelopment ? true : []
}

/**
 * Configuration options to tweak the CORS policy. The following
 * options are documented on the official documentation website.
 *
 * https://docs.adonisjs.com/guides/security/cors
 */
const corsConfig = defineConfig({
  /**
   * Enable or disable CORS handling globally.
   */
  enabled: true,

  /**
   * Session UI stays locked down in production. The MCP and OAuth protocol endpoints may be
   * called by installed clients, so those routes reflect the caller's origin.
   */
  origin: (requestOrigin, ctx) => {
    return resolveCorsOrigin(requestOrigin, ctx.request.url(), app.inDev)
  },

  /**
   * HTTP methods accepted for cross-origin requests.
   */
  methods: ['GET', 'HEAD', 'POST', 'PUT', 'DELETE', 'OPTIONS'],

  /**
   * Reflect request headers by default. Use a string array to restrict
   * allowed headers.
   */
  headers: true,

  /**
   * Response headers exposed to the browser.
   */
  exposeHeaders: ['WWW-Authenticate'],

  /**
   * Allow cookies/authorization headers on cross-origin requests.
   */
  credentials: true,

  /**
   * Cache CORS preflight response for N seconds.
   */
  maxAge: 90,
})

export default corsConfig
