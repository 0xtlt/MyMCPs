/**
 * Vine schema for the OAuth sign-in of built-in MCPs.
 */
import vine from '@vinejs/vine'
import { stravaFaults } from '#validators/builtin_strava'

/**
 * The body of a refused token request: the error of RFC 6749, or the
 * `{ message, errors }` Strava answers with instead.
 */
export const builtinTokenFailureValidator = vine.create({
  error: vine.string().optional(),
  error_description: vine.string().optional(),
  message: vine.string().optional(),
  errors: stravaFaults().optional(),
})
