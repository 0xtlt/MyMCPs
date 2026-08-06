import app from '@adonisjs/core/services/app'
import { defineConfig } from '@adonisjs/cors'

export function resolveCorsOrigin(
  requestOrigin: string | undefined,
  requestUrl: string,
  isDevelopment: boolean
) {
  const pathname = requestUrl.split('?', 1)[0]
  if (pathname === '/mcp') {
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
   * Session UI stays locked down in production.
   * Only the bearer `/mcp` gateway reflects arbitrary origins (agents, no cookies).
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
  exposeHeaders: [],

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
