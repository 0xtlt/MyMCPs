import { OauthAuthorizationCodeSchema } from '#database/schema'
import { belongsTo } from '@adonisjs/lucid/orm'
import type { BelongsTo } from '@adonisjs/lucid/types/relations'
import OauthClient from '#models/oauth_client'
import User from '#models/user'

export default class OauthAuthorizationCode extends OauthAuthorizationCodeSchema {
  @belongsTo(() => OauthClient, { foreignKey: 'oauthClientId' })
  declare oauthClient: BelongsTo<typeof OauthClient>

  @belongsTo(() => User, { foreignKey: 'userId' })
  declare user: BelongsTo<typeof User>
}
