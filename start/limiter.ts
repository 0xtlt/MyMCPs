/*
|--------------------------------------------------------------------------
| Define application limiters
|--------------------------------------------------------------------------
|
| Adonis's limiter service owns storage, atomic counters, exceptions, and
| response headers. Route keys keep each anonymous workflow isolated.
|
*/

import limiter from '@adonisjs/limiter/services/main'
import type { MiddlewareFn } from '@adonisjs/core/types/http'

const anonymousWriteLimit = 5
const anonymousWriteWindow = '15 minutes'

const anonymousWriteLimiter = limiter.use({
  requests: anonymousWriteLimit,
  duration: anonymousWriteWindow,
})

function throttleAnonymousWrites(name: string): MiddlewareFn {
  return async (ctx, next) => {
    await anonymousWriteLimiter.consume(`${name}:${ctx.request.ip()}`)
    return next()
  }
}

export const onboardingThrottle = throttleAnonymousWrites('onboarding')
export const inviteAcceptanceThrottle = throttleAnonymousWrites('invite-accept')

/**
 * `penalize` consumes a point only when credential verification throws and
 * clears prior failures after a successful login.
 */
export const loginRateLimiter = anonymousWriteLimiter
