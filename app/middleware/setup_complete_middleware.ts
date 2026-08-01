import User from '#models/user'
import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'

/**
 * Onboarding is only available before the first admin exists.
 * Afterwards, send people to the app (or login).
 */
export default class SetupCompleteMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    const setupComplete = await User.setupComplete()
    if (!setupComplete) {
      return next()
    }

    if (await ctx.auth.use('web').check()) {
      return ctx.response.redirect().toRoute('home')
    }

    return ctx.response.redirect().toRoute('session.create')
  }
}
