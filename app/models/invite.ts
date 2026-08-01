import { InviteSchema } from '#database/schema'
import { belongsTo } from '@adonisjs/lucid/orm'
import type { BelongsTo } from '@adonisjs/lucid/types/relations'
import { DateTime } from 'luxon'
import { randomBytes } from 'node:crypto'
import User from '#models/user'
import type { UserRole } from '#models/user'

export default class Invite extends InviteSchema {
  declare role: UserRole

  @belongsTo(() => User, { foreignKey: 'createdBy' })
  declare creator: BelongsTo<typeof User>

  get isAccepted() {
    return this.acceptedAt !== null
  }

  get isExpired() {
    return this.expiresAt < DateTime.utc()
  }

  get isUsable() {
    return !this.isAccepted && !this.isExpired
  }

  static generateToken() {
    return randomBytes(32).toString('hex')
  }
}
