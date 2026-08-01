import { McpSchema } from '#database/schema'
import { belongsTo, manyToMany } from '@adonisjs/lucid/orm'
import type { BelongsTo, ManyToMany } from '@adonisjs/lucid/types/relations'
import User from '#models/user'
import AccessToken from '#models/access_token'

export type McpTransport = 'http' | 'npm'
export type McpAuthType = 'none' | 'bearer' | 'header' | 'oauth'
export type McpStatus = 'draft' | 'ready' | 'error'

export default class Mcp extends McpSchema {
  declare transport: McpTransport
  declare authType: McpAuthType
  declare status: McpStatus

  @belongsTo(() => User, { foreignKey: 'createdBy' })
  declare creator: BelongsTo<typeof User>

  @manyToMany(() => AccessToken, {
    pivotTable: 'access_token_mcps',
    pivotForeignKey: 'mcp_id',
    pivotRelatedForeignKey: 'access_token_id',
  })
  declare accessTokens: ManyToMany<typeof AccessToken>

  get npmArgsList(): string[] {
    if (!this.npmArgs) {
      return []
    }
    try {
      const parsed = JSON.parse(this.npmArgs)
      return Array.isArray(parsed) ? parsed.map(String) : []
    } catch {
      return []
    }
  }

  setNpmArgsList(args: string[]) {
    this.npmArgs = args.length ? JSON.stringify(args) : null
  }

  static slugify(name: string) {
    const base = name
      .toLowerCase()
      .trim()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 80)
    return base || 'mcp'
  }
}
