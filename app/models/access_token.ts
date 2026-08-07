import { AccessTokenSchema } from '#database/schema'
import { belongsTo, manyToMany } from '@adonisjs/lucid/orm'
import type { BelongsTo, ManyToMany } from '@adonisjs/lucid/types/relations'
import { DateTime } from 'luxon'
import User from '#models/user'
import Mcp from '#models/mcp'
import OauthClient from '#models/oauth_client'

export type AccessTokenScopeMode = 'all' | 'selected'
export type AccessTokenSource = 'manual' | 'oauth'

export default class AccessToken extends AccessTokenSchema {
  declare scopeMode: AccessTokenScopeMode
  declare source: AccessTokenSource

  @belongsTo(() => User, { foreignKey: 'createdBy' })
  declare creator: BelongsTo<typeof User>

  @belongsTo(() => OauthClient, { foreignKey: 'oauthClientId' })
  declare oauthClient: BelongsTo<typeof OauthClient>

  @manyToMany(() => Mcp, {
    pivotTable: 'access_token_mcps',
    pivotForeignKey: 'access_token_id',
    pivotRelatedForeignKey: 'mcp_id',
    pivotTimestamps: true,
  })
  declare mcps: ManyToMany<typeof Mcp>

  get isRevoked() {
    return this.revokedAt !== null
  }

  get isExpired() {
    return this.expiresAt !== null && this.expiresAt < DateTime.utc()
  }

  get isUsable() {
    return !this.isRevoked && !this.isExpired
  }

  get isOauthSessionActive() {
    if (this.source !== 'oauth' || this.isRevoked) return false
    const connectionExpiresAt = this.oauthRefreshExpiresAt ?? this.expiresAt
    return connectionExpiresAt === null || connectionExpiresAt > DateTime.utc()
  }

  get isActive() {
    return this.source === 'oauth' ? this.isOauthSessionActive : this.isUsable
  }
}
