import { createHash, randomBytes } from 'node:crypto'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'
import db from '@adonisjs/lucid/services/db'

export type CreatedAccessToken = {
  token: AccessToken
  plaintext: string
}

export default class AccessTokenService {
  static generatePlaintext() {
    return `mcp_${randomBytes(32).toString('base64url')}`
  }

  static hash(plaintext: string) {
    return createHash('sha256').update(plaintext).digest('hex')
  }

  static prefix(plaintext: string) {
    return plaintext.slice(0, 12)
  }

  static async create(params: {
    name: string
    scopeMode: 'all' | 'selected'
    mcpIds: number[]
    expiresAt: DateTime | null
    createdBy: number
  }): Promise<CreatedAccessToken> {
    const plaintext = this.generatePlaintext()
    const token = await AccessToken.create({
      name: params.name,
      tokenPrefix: this.prefix(plaintext),
      tokenHash: this.hash(plaintext),
      scopeMode: params.scopeMode,
      expiresAt: params.expiresAt,
      createdBy: params.createdBy,
    })

    if (params.scopeMode === 'selected' && params.mcpIds.length > 0) {
      await token.related('mcps').attach(params.mcpIds)
    }

    return { token, plaintext }
  }

  static async findUsableByPlaintext(plaintext: string) {
    const token = await AccessToken.query().where('token_hash', this.hash(plaintext)).first()
    if (!token || !token.isUsable) {
      return null
    }
    return token
  }

  static async touchLastUsed(token: AccessToken) {
    if (token.lastUsedAt && token.lastUsedAt > DateTime.utc().minus({ minutes: 5 })) {
      return
    }
    token.lastUsedAt = DateTime.utc()
    await token.save()
  }

  /**
   * Resolve MCPs allowed for this token.
   * scope_mode=all → every enabled MCP (including ones added after token creation).
   */
  static async resolveAllowedMcps(token: AccessToken) {
    if (token.scopeMode === 'all') {
      return Mcp.query().where('enabled', true).orderBy('name', 'asc')
    }

    await token.load('mcps', (query) => {
      query.where('enabled', true).orderBy('name', 'asc')
    })
    return token.mcps
  }

  static async update(
    token: AccessToken,
    params: {
      name: string
      scopeMode: 'all' | 'selected'
      mcpIds: number[]
      expiresAt: DateTime | null
    }
  ) {
    await db.transaction(async (trx) => {
      token.useTransaction(trx)
      token.name = params.name
      token.scopeMode = params.scopeMode
      token.expiresAt = params.expiresAt
      await token.save()
      await token.related('mcps').sync(params.scopeMode === 'selected' ? params.mcpIds : [])
    })
  }

  static async revoke(token: AccessToken) {
    token.revokedAt = DateTime.utc()
    await token.save()
  }
}
