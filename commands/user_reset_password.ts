import { args, BaseCommand } from '@adonisjs/core/ace'
import type { CommandOptions } from '@adonisjs/core/types/ace'
import { errors } from '@vinejs/vine'

export default class UserResetPassword extends BaseCommand {
  static commandName = 'user:reset-password'
  static description = 'Reset a user password using hidden prompts (requires server access)'

  static options: CommandOptions = {
    startApp: true,
  }

  @args.string({ description: 'Email address of the account to recover' })
  declare email: string

  async run() {
    // Ace discovers commands before providers register the Lucid validation rules.
    const { default: User } = await import('#models/user')
    const { resetPasswordValidator } = await import('#validators/user')

    const user = await User.findBy('email', this.email.trim())
    if (!user) {
      this.logger.error('No user found with that email address')
      this.exitCode = 1
      return
    }

    const newPassword = await this.prompt.secure('New password')
    const passwordConfirmation = await this.prompt.secure('Confirm new password')

    let payload
    try {
      payload = await resetPasswordValidator.validate({ newPassword, passwordConfirmation })
    } catch (error) {
      if (!(error instanceof errors.E_VALIDATION_ERROR)) {
        throw error
      }
      this.logger.error('Passwords must match and contain between 8 and 32 characters')
      this.exitCode = 1
      return
    }

    await user.changePassword(payload.newPassword)

    this.logger.success(
      `Password reset for ${user.email}. Sessions and remember-me tokens revoked.`
    )
  }
}
