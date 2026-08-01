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
        'scopeMode',
        'expiresAt',
        'revokedAt',
        'lastUsedAt',
        'createdAt',
      ]),
      mcpIds,
      isUsable: this.resource.isUsable,
    }
  }
}
