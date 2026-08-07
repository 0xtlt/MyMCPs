import { createHash, randomBytes } from 'node:crypto'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'
import db from '@adonisjs/lucid/services/db'
import type { TransactionClientContract } from '@adonisjs/lucid/types/database'

export type CreatedAccessToken = {
  token: AccessToken
  plaintext: string
}

export type CreatedOauthTokens = CreatedAccessToken & {
  refreshToken: string | null
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

  static generateRefreshToken() {
    return `mcp_refresh_${randomBytes(32).toString('base64url')}`
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
      source: 'manual',
    })

    if (params.scopeMode === 'selected' && params.mcpIds.length > 0) {
      await token.related('mcps').attach(params.mcpIds)
    }

    return { token, plaintext }
  }

  static async createOauthGrant(params: {
    name: string
    clientId: number
    clientSupportsRefresh: boolean
    scopes: string
    resource: string
    createdBy: number
    trx?: TransactionClientContract
  }): Promise<CreatedOauthTokens> {
    const plaintext = this.generatePlaintext()
    const refreshToken = params.clientSupportsRefresh ? this.generateRefreshToken() : null
    const token = await AccessToken.create(
      {
        name: params.name,
        tokenPrefix: this.prefix(plaintext),
        tokenHash: this.hash(plaintext),
        scopeMode: 'all',
        source: 'oauth',
        expiresAt: DateTime.utc().plus({ hours: 1 }),
        createdBy: params.createdBy,
        oauthClientId: params.clientId,
        oauthScopes: params.scopes,
        oauthResource: params.resource,
        oauthRefreshTokenHash: refreshToken ? this.hash(refreshToken) : null,
        oauthRefreshTokenPrefix: refreshToken ? this.prefix(refreshToken) : null,
        oauthRefreshExpiresAt: refreshToken ? DateTime.utc().plus({ days: 30 }) : null,
      },
      params.trx ? { client: params.trx } : undefined
    )

    return { token, plaintext, refreshToken }
  }

  static async rotateOauthGrant(params: {
    refreshToken: string
    clientId: number
    resource: string
  }): Promise<CreatedOauthTokens | null> {
    const refreshHash = this.hash(params.refreshToken)
    const current = await AccessToken.query()
      .where('oauth_refresh_token_hash', refreshHash)
      .where('oauth_client_id', params.clientId)
      .where('oauth_resource', params.resource)
      .where('source', 'oauth')
      .whereNull('revoked_at')
      .first()

    if (
      !current ||
      !current.oauthRefreshExpiresAt ||
      current.oauthRefreshExpiresAt <= DateTime.utc()
    ) {
      return null
    }

    const plaintext = this.generatePlaintext()
    const nextRefreshToken = this.generateRefreshToken()
    const nextExpiresAt = DateTime.utc().plus({ hours: 1 }).toSQL({ includeOffset: false })

    return db.transaction(async (trx) => {
      const updated = await AccessToken.query({ client: trx })
        .where('id', current.id)
        .where('oauth_refresh_token_hash', refreshHash)
        .whereNull('revoked_at')
        .update({
          tokenHash: this.hash(plaintext),
          tokenPrefix: this.prefix(plaintext),
          expiresAt: nextExpiresAt,
          oauthRefreshTokenHash: this.hash(nextRefreshToken),
          oauthRefreshTokenPrefix: this.prefix(nextRefreshToken),
          lastUsedAt: null,
        })
        .returning('id')

      if (updated.length !== 1) {
        return null
      }

      const token = await AccessToken.query({ client: trx }).where('id', current.id).firstOrFail()
      return { token, plaintext, refreshToken: nextRefreshToken }
    })
  }

  static async revokeOauthToken(clientId: number, plaintext: string) {
    const hash = this.hash(plaintext)
    const token = await AccessToken.query()
      .where('oauth_client_id', clientId)
      .where('source', 'oauth')
      .where((query) => {
        query.where('token_hash', hash).orWhere('oauth_refresh_token_hash', hash)
      })
      .first()

    if (token && !token.isRevoked) {
      await this.revoke(token)
    }
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
