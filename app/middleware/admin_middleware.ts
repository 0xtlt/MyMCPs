import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'

/**
 * Ensures the authenticated user is an admin.
 */
export default class AdminMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    const user = ctx.auth.user
    if (!user || user.role !== 'admin') {
      ctx.session.flash('error', 'Admin access required')
      return ctx.response.redirect().toRoute('home')
    }

    return next()
  }
}
