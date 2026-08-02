import User from '#models/user'
import { onboardingValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'
import db from '@adonisjs/lucid/services/db'

export default class OnboardingController {
  async show({ inertia }: HttpContext) {
    return inertia.render('onboarding/index', {})
  }

  async store({ request, response, auth }: HttpContext) {
    const payload = await request.validateUsing(onboardingValidator)

    const user = await db.transaction(async (trx) => {
      const count = await User.query({ client: trx }).count('* as total')
      if (Number(count[0].$extras.total) > 0) {
        return null
      }

      return User.create(
        {
          fullName: payload.fullName,
          email: payload.email,
          password: payload.password,
          role: 'admin',
        },
        { client: trx }
      )
    })

    if (!user) {
      return response.redirect().toRoute('home')
    }

    await auth.use('web').login(user, true)
    return response.redirect().toRoute('home')
  }
}
