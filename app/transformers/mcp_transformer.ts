import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { BaseTransformer } from '@adonisjs/core/transformers'

export default class McpTransformer extends BaseTransformer<Mcp> {
  /**
   * Full MCP payload for the admin registry UI (never includes decrypted secrets).
   */
  toObject() {
    return {
      ...this.pick(this.resource, [
        'id',
        'name',
        'slug',
        'description',
        'transport',
        'httpUrl',
        'npmPackage',
        'npmVersion',
        'authType',
        'authHeaderName',
        'oauthAuthorizeUrl',
        'oauthTokenUrl',
        'oauthScopes',
        'oauthClientId',
        'status',
        'lastError',
        'enabled',
        'createdAt',
        'updatedAt',
      ]),
      npmArgs: this.resource.npmArgsList.join(' '),
      npmEnv: this.resource.npmEnvNames.map((name) => ({ name, hasValue: true })),
      hasAuthBearer: McpSecretStore.hasSecret(this.resource.authBearer),
      hasAuthHeaderValue: McpSecretStore.hasSecret(this.resource.authHeaderValue),
      hasOauthClientSecret: McpSecretStore.hasSecret(this.resource.oauthClientSecret),
      hasOauthAccessToken: McpSecretStore.hasSecret(this.resource.oauthAccessToken),
    }
  }

  /**
   * Compact option row for token scope pickers.
   */
  toOption() {
    return this.pick(this.resource, ['id', 'name', 'slug', 'enabled'])
  }
}
