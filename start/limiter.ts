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
