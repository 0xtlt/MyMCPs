import type { HttpContext } from '@adonisjs/core/http'
import env from '#start/env'

/**
 * Public base URL for links shown in the UI (gateway, invite links).
 * Prefer the browser host on localhost so cloud/dev ports stay correct.
 */
export function publicAppUrl(request: HttpContext['request']) {
  const configured = env.get('APP_URL').replace(/\/$/, '')
  if (request.host()?.includes('localhost')) {
    return `${request.protocol()}://${request.host()}`
  }
  return configured
}
