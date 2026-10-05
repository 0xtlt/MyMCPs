/**
 * Fixed values of the gateway's OAuth server that its service and its
 * validators both need. Import-free so validators can use it.
 */

/** The one scope the gateway grants. */
export const GATEWAY_OAUTH_SCOPE = 'mcp:tools'

/** Hosts that name the user's own device, as `URL` spells them. */
export const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])
