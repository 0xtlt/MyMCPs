/**
 * Vine schemas for route parameters.
 */
import vine from '@vinejs/vine'

/**
 * `:id` of a stored record.
 */
export const recordIdParamsValidator = vine.create({
  id: vine.number().withoutDecimals().positive(),
})

/**
 * `:token` of an invite link, in the form `Invite.generateToken()` writes:
 * 32 random bytes in hexadecimal.
 */
export const inviteTokenParamsValidator = vine.create({
  token: vine.string().regex(/^[0-9a-f]{64}$/),
})
