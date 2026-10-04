import type { BuiltinMcpDefinition } from '#services/builtin/definition'
import { stravaGet } from '#services/builtin/strava/api'
import { stravaTools } from '#services/builtin/strava/tools'

/**
 * Strava's own MCP (mcp.strava.com) only issues tokens to first-party clients,
 * so this one talks to the public API v3 through an API application the admin
 * creates at https://www.strava.com/settings/api.
 */
export const stravaMcp: BuiltinMcpDefinition = {
  key: 'strava',
  name: 'Strava',
  oauth: {
    issuer: 'https://www.strava.com',
    authorizeUrl: 'https://www.strava.com/oauth/authorize',
    tokenUrl: 'https://www.strava.com/api/v3/oauth/token',
    // The `_all` scopes add activities and profile data that are visible to
    // Only You, which the athlete can still uncheck on Strava.
    scopes: ['read', 'read_all', 'profile:read_all', 'activity:read_all'],
    writeScopes: ['activity:write', 'profile:write'],
    scopeSeparator: ',',
    // Always show the consent screen so re-authorizing can restore a
    // permission that was unchecked the first time.
    authorizeParams: { approval_prompt: 'force' },
    clientIdPattern: /^\d+$/,
    clientIdHint: 'The Strava Client ID is a number, such as 123456',
  },
  tools: stravaTools,
  verify: async ({ accessToken }) => {
    await stravaGet(accessToken, '/athlete')
  },
}
