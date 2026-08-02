import env from '#start/env'

/**
 * Normalize the configured public base URL for links shown in the UI.
 */
export function normalizePublicAppUrl(value: string | undefined) {
  return value?.replace(/\/$/, '') || null
}

export function publicAppUrl() {
  return normalizePublicAppUrl(env.get('APP_URL'))
}

export function requirePublicAppUrl() {
  const configured = publicAppUrl()
  if (!configured) {
    throw new Error('APP_URL is not configured. Set it to the public HTTPS origin and redeploy.')
  }
  return configured
}
