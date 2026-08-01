import { McpSchema } from '#database/schema'
import McpSecretStore from '#services/mcp_secret_store'
import { belongsTo, manyToMany } from '@adonisjs/lucid/orm'
import type { BelongsTo, ManyToMany } from '@adonisjs/lucid/types/relations'
import User from '#models/user'
import AccessToken from '#models/access_token'

export type McpTransport = 'http' | 'npm'
export type McpAuthType = 'none' | 'bearer' | 'header' | 'oauth'
export type McpStatus = 'draft' | 'ready' | 'error'
export type McpNpmEnvMap = Record<string, string>

/**
 * The generated schema stores npm args as JSON text (`string | null`).
 * Expose them as string[] through this accessor so callers never JSON.parse.
 */
function parseNpmArgsJson(value: string | null): string[] {
  if (!value) {
    return []
  }
  try {
    const parsed = JSON.parse(value)
    if (!Array.isArray(parsed)) {
      return []
    }
    return parsed.map((part) => String(part))
  } catch {
    return []
  }
}

function parseNpmEnvJson(value: string | null): McpNpmEnvMap {
  const decrypted = McpSecretStore.decrypt(value)
  if (!decrypted) {
    return {}
  }

  try {
    const parsed: unknown = JSON.parse(decrypted)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return {}
    }

    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, string] => typeof entry[1] === 'string')
    )
  } catch {
    return {}
  }
}

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
    pivotTimestamps: true,
  })
  declare accessTokens: ManyToMany<typeof AccessToken>

  get npmArgsList(): string[] {
    return parseNpmArgsJson(this.npmArgs)
  }

  set npmArgsList(args: string[]) {
    this.npmArgs = args.length > 0 ? JSON.stringify(args) : null
  }

  get npmEnvMap(): McpNpmEnvMap {
    return parseNpmEnvJson(this.npmEnv)
  }

  set npmEnvMap(env: McpNpmEnvMap) {
    this.npmEnv =
      Object.keys(env).length > 0 ? McpSecretStore.encrypt(JSON.stringify(env)) : null
  }

  get npmEnvEntries(): Array<{ name: string; hasValue: boolean }> {
    return Object.keys(this.npmEnvMap).map((name) => ({ name, hasValue: true }))
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
