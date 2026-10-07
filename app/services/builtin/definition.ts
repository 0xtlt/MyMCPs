import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type { VineValidator } from '@vinejs/vine'
import type { SchemaTypes } from '@vinejs/vine/types'
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
  /** Requested on top of `scopes` once the admin allows write access. */
  writeScopes: readonly string[]
  /** RFC 6749 separates scopes with spaces. Some providers expect commas. */
  scopeSeparator: string
  authorizeParams?: Readonly<Record<string, string>>
  /** Catches a secret pasted into the client ID field before the provider does. */
  clientIdPattern?: RegExp
  clientIdHint?: string
}

/**
 * Sign-in with an account name and a password the provider issues for apps,
 * for services that have no OAuth a self-hosted gateway can use.
 */
export type BuiltinPasswordConfig = {
  usernamePattern: RegExp
  usernameHint: string
  /** Rejects the account's main password before it is stored. */
  passwordPattern: RegExp
  passwordHint: string
  /**
   * What the admin can allow agents to do. The provider cannot restrict such
   * a password, so MyMCPs enforces these itself: a tool is only available
   * when one of its `requiresAnyScope` is allowed.
   */
  permissions: readonly string[]
  /** Explains what the other addresses of the account must look like. */
  aliasHint: string
}

export type BuiltinToolContext = {
  accessToken: string
  /** `null` when the provider did not report which scopes were granted. */
  grantedScopes: string[] | null
}

export type BuiltinPasswordContext = {
  mcpId: number
  username: string
  password: string
  /** What the admin allowed for this MCP. */
  permissions: string[]
  /** Other addresses of the same account that the admin lets agents act as. */
  aliases: string[]
}

/** A file a tool linked to, such as a mail attachment. */
export type BuiltinFile = {
  filename: string
  contentType: string
  /** The bytes in the pieces they arrived in, so that a large file is held in memory once. */
  content: Buffer[]
}

/** Where to keep a file sent to an upload link, as the tool that made the link described it. */
export type BuiltinUploadTarget = {
  /** Names the stored file, and is how tools refer to it afterwards. A UUID. */
  id: string
  filename: string
  /** Left out, it follows from the filename. */
  contentType?: string
  maxBytes: number
}

export type BuiltinTool<Context = BuiltinToolContext> = {
  name: string
  description: string
  /** The arguments as the agent reads them. */
  inputSchema: Tool['inputSchema']
  /** The arguments as `run` checks them. Both must describe the same ones. */
  input: VineValidator<SchemaTypes, any>
  /**
   * The tool needs at least one of these provider scopes, or of these
   * permissions for a password sign-in. Omit when any authorization works.
   */
  requiresAnyScope?: readonly string[]
  /** Changes data at the provider. Unavailable until the admin allows write access. */
  write?: true
  /** Returns JSON-serializable data. Throw `BuiltinToolError` for expected failures. */
  run: (args: Record<string, unknown>, context: Context) => Promise<unknown>
}

export type BuiltinMcpProvider<Context> = {
  key: BuiltinMcpKey
  /** Provider name used in messages, such as "Strava". */
  name: string
  tools: readonly BuiltinTool<Context>[]
  /** One cheap authenticated request proving the saved sign-in still works. */
  verify: (context: Context) => Promise<void>
  /**
   * Serves a file one of the tools handed out as a temporary signed link.
   * `reference` is what the tool put in the link, unchanged.
   */
  download?: (reference: unknown, context: Context) => Promise<BuiltinFile>
  /**
   * Says where to keep the file sent to a temporary signed link one of the
   * tools handed out. `reference` is what the tool put in the link, unchanged.
   * Throws `BuiltinToolError` when the link may no longer be used.
   */
  upload?: (reference: unknown, context: Context) => Promise<BuiltinUploadTarget>
}

export type BuiltinOauthMcpDefinition = BuiltinMcpProvider<BuiltinToolContext> & {
  oauth: BuiltinOauthConfig
  password?: undefined
}

export type BuiltinPasswordMcpDefinition = BuiltinMcpProvider<BuiltinPasswordContext> & {
  password: BuiltinPasswordConfig
  oauth?: undefined
}

export type BuiltinMcpDefinition = BuiltinOauthMcpDefinition | BuiltinPasswordMcpDefinition
