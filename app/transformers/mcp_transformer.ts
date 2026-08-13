import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { readCachedNpmPackageVersion } from '#services/upstream/deno_runner'
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
      hasOauthAccessToken: McpSecretStore.hasSecret(this.resource.oauthAccessToken),
      oauthRequired: Boolean(this.resource.oauthRequired),
      npmCachedVersion:
        this.resource.transport === 'npm'
          ? readCachedNpmPackageVersion(this.resource.npmPackage, this.resource.npmVersion)
          : null,
    }
  }

  /**
   * Compact option row for token scope pickers.
   */
  toOption() {
    return this.pick(this.resource, ['id', 'name', 'slug', 'enabled'])
  }
}
