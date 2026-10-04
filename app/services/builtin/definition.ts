import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type { BuiltinMcpKey } from '#services/builtin/keys'

/**
 * A failure the calling agent can act on. Its message becomes the tool result,
 * so it must never contain credentials.
 */
export class BuiltinToolError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options)
    this.name = 'BuiltinToolError'
  }
}

/** The provider no longer accepts the saved authorization. */
export class BuiltinAuthorizationError extends BuiltinToolError {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options)
    this.name = 'BuiltinAuthorizationError'
  }
}

/**
 * OAuth 2.0 authorization-code settings for a provider whose API application
 * the admin registers themselves, then pastes its client ID and secret.
 */
export type BuiltinOauthConfig = {
  issuer: string
  authorizeUrl: string
  tokenUrl: string
  scopes: readonly string[]
  /** RFC 6749 separates scopes with spaces. Some providers expect commas. */
  scopeSeparator: string
  authorizeParams?: Readonly<Record<string, string>>
  /** Catches a secret pasted into the client ID field before the provider does. */
  clientIdPattern?: RegExp
  clientIdHint?: string
}

export type BuiltinToolContext = {
  accessToken: string
  /** `null` when the provider did not report which scopes were granted. */
  grantedScopes: string[] | null
}

export type BuiltinTool = {
  name: string
  description: string
  inputSchema: Tool['inputSchema']
  /** The tool needs at least one of these. Omit when any authorization works. */
  requiresAnyScope?: readonly string[]
  /** Returns JSON-serializable data. Throw `BuiltinToolError` for expected failures. */
  run: (args: Record<string, unknown>, context: BuiltinToolContext) => Promise<unknown>
}

export type BuiltinMcpDefinition = {
  key: BuiltinMcpKey
  /** Provider name used in messages, such as "Strava". */
  name: string
  oauth: BuiltinOauthConfig
  tools: readonly BuiltinTool[]
  /** One cheap authenticated request proving the saved authorization still works. */
  verify: (context: BuiltinToolContext) => Promise<void>
}
