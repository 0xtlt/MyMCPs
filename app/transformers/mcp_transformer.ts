import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { readCachedNpmPackageVersion } from '#services/upstream/deno_runner'
import { usesPastedOauthCallback } from '#services/upstream/oauth'
import { builtinMcp } from '#services/builtin/registry'
import { builtinSettings, builtinWriteGranted } from '#services/builtin/runtime'
import { BaseTransformer } from '@adonisjs/core/transformers'

export default class McpTransformer extends BaseTransformer<Mcp> {
  /**
   * Full MCP payload for the admin registry UI (never includes decrypted secrets).
   */
  toObject() {
    const definition = builtinMcp(this.resource.builtinKey)

    return {
      ...this.pick(this.resource, [
        'id',
        'name',
        'slug',
        'description',
        'transport',
        'builtinKey',
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
      // Other transports register their OAuth client automatically.
      oauthClientId: this.resource.transport === 'builtin' ? this.resource.oauthClientId : null,
      hasOauthClientSecret:
        this.resource.transport === 'builtin' &&
        McpSecretStore.hasSecret(this.resource.oauthClientSecret),
      builtinUsername: this.resource.transport === 'builtin' ? this.resource.builtinUsername : null,
      hasBuiltinPassword: McpSecretStore.hasSecret(this.resource.builtinPassword),
      builtinPermissions: this.resource.builtinPermissions?.split(' ') ?? [],
      builtinAliases: this.resource.builtinAliases?.split(' ') ?? [],
      builtinSettings: definition ? builtinSettings(definition, this.resource) : {},
      builtinWriteEnabled: Boolean(this.resource.builtinWriteEnabled),
      // False when write access is on but the saved authorization predates it.
      builtinWriteGranted: Boolean(definition) && builtinWriteGranted(this.resource),
      oauthRequired: Boolean(this.resource.oauthRequired),
      oauthPastedCallback: usesPastedOauthCallback(this.resource),
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
