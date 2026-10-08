/*
|--------------------------------------------------------------------------
| Define application limiters
|--------------------------------------------------------------------------
|
| Adonis's limiter service owns storage, atomic counters, exceptions, and
| response headers for repeated credential failures.
|
*/

import limiter from '@adonisjs/limiter/services/main'

/** Sign-in attempts on one account from one client address. */
export const loginRateLimiter = limiter.use({
  requests: 5,
  duration: '15 minutes',
})

/** Failed sign-in attempts from one client address, whichever accounts they target. */
export const loginAddressRateLimiter = limiter.use({
  requests: 30,
  duration: '15 minutes',
})

/** Current-password confirmations by one signed-in user. */
export const currentPasswordRateLimiter = limiter.use({
  requests: 5,
  duration: '15 minutes',
})

export const oauthAuthorizationRateLimiter = limiter.use({
  requests: 100,
  duration: '15 minutes',
})

export const oauthTokenRateLimiter = limiter.use({
  requests: 50,
  duration: '15 minutes',
})

export const oauthRegistrationRateLimiter = limiter.use({
  requests: 20,
  duration: '1 hour',
})

/** Each download of a built-in MCP file signs in to its provider. */
export const builtinFileRateLimiter = limiter.use({
  requests: 60,
  duration: '15 minutes',
})

/** Each upload to a built-in MCP writes a file to the instance's disk. */
export const builtinUploadRateLimiter = limiter.use({
  requests: 60,
  duration: '15 minutes',
})

/**
 * Each import of a backup writes a file of up to 4 GiB to the instance's
 * disk and derives a key from a password, for a visitor without an account.
 */
export const backupImportRateLimiter = limiter.use({
  requests: 10,
  duration: '15 minutes',
})
