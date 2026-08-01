/**
 * Vine schemas for OAuth session payload and token endpoint JSON.
 */
import vine from '@vinejs/vine'

/**
 * Values we put in the session during the PKCE authorize redirect.
 */
export const oauthSessionValidator = vine.create({
  mcpId: vine.number(),
  codeVerifier: vine.string().minLength(1),
  state: vine.string().minLength(1),
})

/**
 * Token endpoint JSON (snake_case as returned by OAuth providers).
 */
export const oauthTokenResponseValidator = vine.create({
  access_token: vine.string().minLength(1),
  refresh_token: vine.string().optional(),
  expires_in: vine.number().optional(),
})
