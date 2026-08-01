/**
 * Shared rules for email and password.
 */
import vine from '@vinejs/vine'

const email = () => vine.string().email().maxLength(254)
const password = () => vine.string().minLength(8).maxLength(32)

/**
 * First-run onboarding: create the instance admin.
 */
export const onboardingValidator = vine.create({
  fullName: vine.string().trim().minLength(1).maxLength(120),
  email: email().unique({ table: 'users', column: 'email' }),
  password: password().confirmed({
    confirmationField: 'passwordConfirmation',
  }),
})

/**
 * Accept an invite and create a member account.
 */
export const acceptInviteValidator = vine.create({
  fullName: vine.string().trim().minLength(1).maxLength(120),
  password: password().confirmed({
    confirmationField: 'passwordConfirmation',
  }),
})

/**
 * Admin creates an invite for an email address.
 */
export const createInviteValidator = vine.create({
  email: email(),
})

/**
 * Login credentials.
 */
export const loginValidator = vine.create({
  email: email(),
  password: vine.string().minLength(1),
})
