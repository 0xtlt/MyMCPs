import type AccessToken from '#models/access_token'
import { BaseTransformer } from '@adonisjs/core/transformers'

export default class AccessTokenTransformer extends BaseTransformer<AccessToken> {
  toObject() {
    const mcpIds = this.resource.mcps ? this.resource.mcps.map((mcp) => mcp.id) : []

    return {
      ...this.pick(this.resource, [
        'id',
        'name',
        'tokenPrefix',
        'source',
        'scopeMode',
        'expiresAt',
        'revokedAt',
        'lastUsedAt',
        'createdAt',
      ]),
      mcpIds,
      isUsable: this.resource.isUsable,
      isActive: this.resource.isActive,
      canRevoke:
        this.resource.source === 'oauth' ? !this.resource.isRevoked : this.resource.isUsable,
      displayExpiresAt:
        this.resource.source === 'oauth'
          ? this.resource.oauthRefreshExpiresAt
          : this.resource.expiresAt,
      oauthClientName: this.resource.oauthClient?.clientName ?? null,
    }
  }
}
