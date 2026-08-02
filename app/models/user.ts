import { UserSchema } from '#database/schema'
import hash from '@adonisjs/core/services/hash'
import { compose } from '@adonisjs/core/helpers'
import { withAuthFinder } from '@adonisjs/auth/mixins/lucid'
import { DbRememberMeTokensProvider } from '@adonisjs/auth/session'
import { hasMany } from '@adonisjs/lucid/orm'
import type { HasMany } from '@adonisjs/lucid/types/relations'
import Invite from '#models/invite'

export type UserRole = 'admin' | 'member'

export default class User extends compose(UserSchema, withAuthFinder(hash)) {
  static rememberMeTokens = DbRememberMeTokensProvider.forModel(User)

  declare role: UserRole

  @hasMany(() => Invite, { foreignKey: 'createdBy' })
  declare invites: HasMany<typeof Invite>

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
}
