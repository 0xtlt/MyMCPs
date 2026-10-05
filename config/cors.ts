import app from '@adonisjs/core/services/app'
import { defineConfig } from '@adonisjs/cors'

export function resolveCorsOrigin(requestUrl: string, isDevelopment: boolean) {
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
    return '*'
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
   * called by installed clients from any origin. They authenticate with a header the client
   * sets itself, never with cookies, so they answer with a plain wildcard.
   */
  origin: (_requestOrigin, ctx) => {
    const allowed = resolveCorsOrigin(ctx.request.url(), app.inDev)
    // In development the caller's origin is echoed, so the response depends on it.
    if (allowed === true) ctx.response.vary('Origin')
    return allowed
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
   * Never let another origin send the browser's cookies along. Every origin
   * allowed above is allowed without knowing who it is.
   */
  credentials: false,

  /**
   * Cache CORS preflight response for N seconds.
   */
  maxAge: 90,
})

export default corsConfig
