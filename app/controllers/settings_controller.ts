import { updateEmailValidator, updatePasswordValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'

export default class SettingsController {
  async index({ inertia }: HttpContext) {
    return inertia.render('settings/index', {})
  }

  async updateEmail({ request, auth, response, session }: HttpContext) {
    const user = auth.user!
    const payload = await request.validateUsing(updateEmailValidator, {
      meta: { userId: user.id },
    })

    await user.validatePassword(payload.currentPassword)
    user.email = payload.email
    await user.save()

    session.flash('success', 'Email updated')
    return response.redirect().toRoute('settings.index')
  }

  async updatePassword({ request, auth, response, session }: HttpContext) {
    const user = auth.user!
    const payload = await request.validateUsing(updatePasswordValidator)

    await user.validatePassword(payload.currentPassword)
    user.password = payload.newPassword
    await user.save()

    session.flash('success', 'Password updated')
    return response.redirect().toRoute('settings.index')
  }
}
