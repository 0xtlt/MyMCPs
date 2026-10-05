/**
 * Vine schemas for OAuth flows (session, provider JSON, browser callback).
 */
import vine from '@vinejs/vine'

/**
 * Values we put in the session during the authorize redirect. Built-in MCPs
 * authenticate with a client secret instead of PKCE and have no code verifier.
 */
export const oauthSessionValidator = vine.create({
  mcpId: vine.number(),
  codeVerifier: vine.string().minLength(1).optional(),
  state: vine.string().minLength(1),
  redirectUri: vine.string().url({ require_tld: false }),
  authorizationServerUrl: vine.string().url({ require_tld: false }),
  resource: vine.string().url({ require_tld: false }).optional(),
  clientId: vine.string().minLength(1),
})

/**
 * Token endpoint JSON (snake_case as returned by OAuth providers).
 */
export const oauthTokenResponseValidator = vine.create({
  access_token: vine.string().minLength(1),
  token_type: vine.string().optional(),
  refresh_token: vine.string().optional(),
  expires_in: vine.number().optional(),
  scope: vine.string().optional(),
})

/**
 * Query params on `/mcps/oauth/callback` from the authorization server.
 */
export const oauthCallbackValidator = vine.create({
  code: vine.string().maxLength(8192).optional(),
  state: vine.string().maxLength(512).optional(),
  error: vine.string().maxLength(1024).optional(),
})

/**
 * Request headers on `/mcps/:id/oauth/start`. Browsers say where a request
 * comes from: the flow may be started from this app or from the address bar,
 * not from another site.
 */
export const oauthStartValidator = vine.create({
  headers: vine.object({
    'sec-fetch-site': vine.enum(['same-origin', 'none'] as const).optional(),
  }),
})
