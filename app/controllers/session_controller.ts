import User from '#models/user'
import { refundRateLimit } from '#middleware/rate_limit_middleware'
import { loginValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'

export default class SessionController {
  async create({ inertia }: HttpContext) {
    return inertia.render('auth/login', {})
  }

  async store(ctx: HttpContext) {
    const { request, auth, response } = ctx
    const { email, password } = await request.validateUsing(loginValidator)
    const user = await User.verifyCredentials(email, password)

    await auth.use('web').login(user, true)
    refundRateLimit(ctx)
    response.redirect().toRoute('home')
  }

  async destroy({ auth, response }: HttpContext) {
    await auth.use('web').logout()
    response.redirect().toRoute('session.create')
  }
}
