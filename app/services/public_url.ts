import env from '#start/env'
import app from '@adonisjs/core/services/app'

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])

/**
 * Normalize the configured public base URL for links shown in the UI.
 */
export function normalizePublicAppUrl(value: string | undefined) {
  return value?.replace(/\/$/, '') || null
}

export function publicAppUrl() {
  return normalizePublicAppUrl(env.get('APP_URL'))
}

export function validatePublicAppUrl(value: string, allowInsecureLoopback: boolean) {
  const url = new URL(value)
  const isOriginOnly =
    url.pathname === '/' && !url.search && !url.hash && !url.username && !url.password
  const isSecure = url.protocol === 'https:'
  const isAllowedLoopback =
    allowInsecureLoopback && url.protocol === 'http:' && LOOPBACK_HOSTS.has(url.hostname)

  if (!isOriginOnly || (!isSecure && !isAllowedLoopback)) {
    throw new Error(
      'APP_URL must be a public HTTPS origin (HTTP loopback is allowed only in development and tests).'
    )
  }

  return url.origin
}

export function requirePublicAppUrl() {
  const configured = publicAppUrl()
  if (!configured) {
    throw new Error('APP_URL is not configured. Set it to the public HTTPS origin and redeploy.')
  }
  return validatePublicAppUrl(configured, app.inDev || app.inTest)
}

/**
 * Return the public origin only when it is safe to use for OAuth endpoints.
 */
export function publicOauthAppUrl() {
  try {
    return requirePublicAppUrl()
  } catch {
    return null
  }
}
