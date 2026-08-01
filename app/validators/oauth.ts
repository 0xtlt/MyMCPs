/**
 * Vine schemas for OAuth flows (session, provider JSON, browser callback).
 */
import vine from '@vinejs/vine'

/**
 * Values we put in the session during the PKCE authorize redirect.
 */
export const oauthSessionValidator = vine.create({
  mcpId: vine.number(),
  codeVerifier: vine.string().minLength(1),
  state: vine.string().minLength(1),
  redirectUri: vine.string().url(),
  authorizationServerUrl: vine.string().url(),
  resource: vine.string().url().optional(),
  clientId: vine.string().minLength(1),
})

/**
 * Token endpoint JSON (snake_case as returned by OAuth providers).
 */
export const oauthTokenResponseValidator = vine.create({
  access_token: vine.string().minLength(1),
  refresh_token: vine.string().optional(),
  expires_in: vine.number().optional(),
})

/**
 * Query params on `/mcps/oauth/callback` from the authorization server.
 */
export const oauthCallbackValidator = vine.create({
  code: vine.string().optional(),
  state: vine.string().optional(),
  error: vine.string().optional(),
})
