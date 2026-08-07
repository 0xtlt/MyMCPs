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

export const loginRateLimiter = limiter.use({
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
