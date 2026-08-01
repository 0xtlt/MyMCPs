import { AccessTokenMcpSchema } from '#database/schema'
import { belongsTo } from '@adonisjs/lucid/orm'
import type { BelongsTo } from '@adonisjs/lucid/types/relations'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'

export default class AccessTokenMcp extends AccessTokenMcpSchema {
  @belongsTo(() => AccessToken, { foreignKey: 'accessTokenId' })
  declare accessToken: BelongsTo<typeof AccessToken>

  @belongsTo(() => Mcp, { foreignKey: 'mcpId' })
  declare mcp: BelongsTo<typeof Mcp>
}
