import User from '#models/user'
import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'

/**
 * When the instance has no users yet, force every app route to /onboarding.
 */
export default class NeedsSetupMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    const setupComplete = await User.setupComplete()
    if (setupComplete) {
      return next()
    }

    return ctx.response.redirect().toPath('/onboarding')
  }
}
