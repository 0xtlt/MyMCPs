import User from '#models/user'
import { loginRateLimiter } from '#start/limiter'
import { loginValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'

export default class SessionController {
  async create({ inertia }: HttpContext) {
    return inertia.render('auth/login', {})
  }

  async store(ctx: HttpContext) {
    const { request, auth, response, session } = ctx
    const { email, password } = await request.validateUsing(loginValidator)
    const [rateLimitError, user] = await loginRateLimiter.penalize(`login:${request.ip()}`, () =>
      User.verifyCredentials(email, password)
    )

    if (rateLimitError) {
      throw rateLimitError
    }

    await auth.use('web').login(user, true)
    const oauthReturnTo = session.pull('oauthReturnTo')
    if (typeof oauthReturnTo === 'string' && oauthReturnTo.startsWith('/authorize?')) {
      return response.redirect(oauthReturnTo)
    }
    response.redirect().toRoute('home')
  }

  async destroy({ auth, response }: HttpContext) {
    await auth.use('web').logout()
    response.redirect().toRoute('session.create')
  }
}
