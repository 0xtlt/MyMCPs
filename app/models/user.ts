import { UserSchema } from '#database/schema'
import hash from '@adonisjs/core/services/hash'
import { compose } from '@adonisjs/core/helpers'
import { withAuthFinder } from '@adonisjs/auth/mixins/lucid'
import { DbRememberMeTokensProvider } from '@adonisjs/auth/session'
import db from '@adonisjs/lucid/services/db'
import { beforeCreate, hasMany } from '@adonisjs/lucid/orm'
import type { HasMany } from '@adonisjs/lucid/types/relations'
import Invite from '#models/invite'

export type UserRole = 'admin' | 'member'

export default class User extends compose(UserSchema, withAuthFinder(hash)) {
  static rememberMeTokens = DbRememberMeTokensProvider.forModel(User)

  declare role: UserRole

  @hasMany(() => Invite, { foreignKey: 'createdBy' })
  declare invites: HasMany<typeof Invite>

  /**
   * The column default is not read back on insert; mirror it so a user can
   * be signed in right after being created.
   */
  @beforeCreate()
  static assignSessionVersion(user: User) {
    user.sessionVersion ??= 1
  }

  get initials() {
    const [first, last] = this.fullName ? this.fullName.split(' ') : this.email.split('@')
    if (first && last) {
      return `${first.charAt(0)}${last.charAt(0)}`.toUpperCase()
    }
    return `${first.slice(0, 2)}`.toUpperCase()
  }

  get isAdmin() {
    return this.role === 'admin'
  }

  static async setupComplete() {
    const count = await this.query().count('* as total')
    return Number(count[0].$extras.total) > 0
  }

  /**
   * Retire every session issued to this user so far: a session only
   * authenticates while it carries the current version. Joins the
   * transaction the model is bound to, if any.
   */
  async invalidateSessions() {
    // Incremented in SQL so that concurrent calls never settle on the same version.
    await User.query({ client: this.$trx }).where('id', this.id).increment('session_version', 1)
    await this.refresh()
  }

  /**
   * Set a new password and sign the account out everywhere. One transaction,
   * so the password never changes without the revocation, nor the reverse.
   */
  async changePassword(newPassword: string) {
    await db.transaction(async (trx) => {
      this.useTransaction(trx)
      this.password = newPassword
      await this.save()
      await this.invalidateSessions()

      // The token provider uses its own client, so revoke through this transaction.
      await trx.from('remember_me_tokens').where('tokenable_id', this.id).delete()
    })
  }
}
