import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import {
  BuiltinAuthorizationError,
  BuiltinToolError,
  type ApprovalSummary,
  type BuiltinFile,
  type BuiltinMcpDefinition,
  type BuiltinMcpProvider,
  type BuiltinPasswordContext,
  type BuiltinTool,
  type BuiltinToolContext,
  type BuiltinUploadTarget,
} from '#services/builtin/definition'
import { parseOauthScopes } from '#services/builtin/oauth'
import { requireBuiltinMcp } from '#services/builtin/registry'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import type { UpstreamTool } from '#services/upstream/http_client'
import { refreshOauthAccessToken } from '#services/upstream/oauth'

function notConnected(definition: BuiltinMcpDefinition) {
  return new BuiltinAuthorizationError(
    `${definition.name} is not connected. Connect it from the MCPs page in MyMCPs.`
  )
}

/**
 * What the saved sign-in may do. An OAuth provider reports the scopes it
 * granted, or nothing at all (`null`, which allows every tool). A password
 * may do exactly what the admin allowed in MyMCPs.
 */
function grantedScopes(definition: BuiltinMcpDefinition, mcp: Mcp) {
  if (definition.password) {
    return parseOauthScopes(mcp.builtinPermissions)
  }
  const scopes = parseOauthScopes(mcp.oauthScopes)
  return scopes.length > 0 ? scopes : null
}

function isGranted(tool: { requiresAnyScope?: readonly string[] }, scopes: string[] | null) {
  return (
    !tool.requiresAnyScope ||
    scopes === null ||
    tool.requiresAnyScope.some((scope) => scopes.includes(scope))
  )
}

/**
 * Whether the provider granted any write scope. Unknown scopes count as
 * granted so the UI does not ask to re-authorize on a guess. A password
 * sign-in has nothing to re-authorize, and neither has a provider whose one
 * scope both reads and writes.
 */
export function builtinWriteGranted(mcp: Mcp) {
  const definition = requireBuiltinMcp(mcp)
  if (!definition.oauth || definition.oauth.writeScopes.length === 0) return true

  const scopes = grantedScopes(definition, mcp)
  return scopes === null || definition.oauth.writeScopes.some((scope) => scopes.includes(scope))
}

/**
 * What the admin entered for the provider's settings. Values saved under a
 * key the provider no longer has, or that can no longer be decrypted, are
 * left out: the tool that needs one says so.
 */
export function builtinSettings(definition: BuiltinMcpDefinition, mcp: Mcp) {
  let saved: Record<string, string>
  try {
    saved = McpEnvironmentStore.decrypt(mcp.builtinSettings)
  } catch {
    saved = {}
  }
  return Object.fromEntries(
    (definition.settings ?? [])
      .filter(({ key }) => Object.hasOwn(saved, key))
      .map(({ key }) => [key, saved[key]])
  )
}

function toolError(message: string): CallToolResult {
  return { content: [{ type: 'text', text: message }], isError: true }
}

/** Whether a sign-in was saved. The provider may have revoked it since. */
function hasSignIn(definition: BuiltinMcpDefinition, mcp: Mcp) {
  return definition.oauth
    ? Boolean(mcp.oauthAccessToken)
    : Boolean(mcp.builtinUsername && mcp.builtinPassword)
}

async function authorizedContext(
  definition: BuiltinMcpDefinition,
  mcp: Mcp
): Promise<BuiltinToolContext> {
  if (!mcp.oauthAccessToken) {
    throw notConnected(definition)
  }
  await refreshOauthAccessToken(mcp)

  // The refresh reloads the model, so read the token it left behind.
  const accessToken = McpSecretStore.decrypt(mcp.oauthAccessToken)
  if (!accessToken) {
    throw notConnected(definition)
  }
  return {
    mcpId: mcp.id,
    accessToken,
    grantedScopes: grantedScopes(definition, mcp),
    settings: builtinSettings(definition, mcp),
  }
}

async function passwordContext(
  definition: BuiltinMcpDefinition,
  mcp: Mcp
): Promise<BuiltinPasswordContext> {
  const password = McpSecretStore.decrypt(mcp.builtinPassword)
  if (!mcp.builtinUsername || !password) {
    throw notConnected(definition)
  }
  return {
    mcpId: mcp.id,
    username: mcp.builtinUsername,
    password,
    permissions: grantedScopes(definition, mcp) ?? [],
    aliases: mcp.builtinAliases?.split(' ') ?? [],
    settings: builtinSettings(definition, mcp),
  }
}

function notGranted(definition: BuiltinMcpDefinition, toolName: string, needs: readonly string[]) {
  const permission = `"${needs.join('" or "')}"`
  return definition.oauth
    ? `${toolName} needs the ${definition.name} permission ${permission}, which was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked.`
    : `${toolName} needs the ${permission} permission, which is not allowed for this ${definition.name} MCP. An administrator can allow it from the MCPs page in MyMCPs.`
}

/**
 * Pair a provider with the loader of its own kind of sign-in, so its tools
 * only ever run with the context they were written for.
 */
function withProvider<Result>(
  definition: BuiltinMcpDefinition,
  mcp: Mcp,
  use: <Context>(provider: BuiltinMcpProvider<Context>, signIn: () => Promise<Context>) => Result
): Result {
  return definition.oauth
    ? use(definition, () => authorizedContext(definition, mcp))
    : use(definition, () => passwordContext(definition, mcp))
}

/**
 * Tool definitions are static, so listing them never calls the provider.
 * Write tools are left out until the admin allows write access, and so are
 * tools whose permission was unchecked: on the provider's consent screen, or
 * in MyMCPs for a password sign-in.
 */
export function listBuiltinTools(mcp: Mcp): UpstreamTool[] {
  const definition = requireBuiltinMcp(mcp)
  if (!hasSignIn(definition, mcp)) {
    throw notConnected(definition)
  }

  const scopes = grantedScopes(definition, mcp)
  return definition.tools
    .filter((tool) => (!tool.write || mcp.builtinWriteEnabled) && isGranted(tool, scopes))
    .map(({ name, description, inputSchema }) => ({ name, description, inputSchema }))
}

/**
 * Run `use` with a tool and its sign-in, once the call is known to be one
 * this MCP allows. Throws `BuiltinToolError` when it is not.
 */
function withTool<Result>(
  mcp: Mcp,
  toolName: string,
  use: <Context>(tool: BuiltinTool<Context>, context: Context) => Promise<Result>
): Promise<Result> {
  const definition = requireBuiltinMcp(mcp)
  return withProvider(definition, mcp, async (provider, signIn) => {
    const tool = provider.tools.find((candidate) => candidate.name === toolName)
    if (!tool) {
      throw new BuiltinToolError(`Unknown ${provider.name} tool: ${toolName}`)
    }
    if (tool.write && !mcp.builtinWriteEnabled) {
      throw new BuiltinToolError(
        `${toolName} changes ${provider.name} data, and write access is turned off for this MCP. An administrator can allow it from the MCPs page in MyMCPs.`
      )
    }

    const context = await signIn()
    if (!isGranted(tool, grantedScopes(definition, mcp))) {
      throw new BuiltinToolError(notGranted(definition, toolName, tool.requiresAnyScope!))
    }
    return use(tool, context)
  })
}

export async function callBuiltinTool(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined
): Promise<CallToolResult> {
  try {
    const data = await withTool(mcp, toolName, (tool, context) => tool.run(args ?? {}, context))
    return { content: [{ type: 'text', text: JSON.stringify(data) }] }
  } catch (error) {
    if (error instanceof BuiltinToolError) {
      return toolError(error.message)
    }
    throw error
  }
}

/**
 * What a call would do, for the person asked to approve it, or `null` when
 * the tool only has its arguments to show. Nothing is changed at the
 * provider. Throws `BuiltinToolError` for a call that would be refused, so
 * that nobody is asked to approve one.
 */
export function describeBuiltinCall(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined
): Promise<ApprovalSummary | null> {
  return withTool(mcp, toolName, (tool, context) => tool.describe(args ?? {}, context))
}

/**
 * The file behind a link one of the MCP's tools handed out. Throws
 * `BuiltinToolError` when it cannot be served any more.
 */
export async function downloadBuiltinFile(mcp: Mcp, reference: unknown): Promise<BuiltinFile> {
  return withProvider(requireBuiltinMcp(mcp), mcp, async (provider, signIn) => {
    if (!provider.download) {
      throw new BuiltinToolError(`${provider.name} has no files to download`)
    }
    return provider.download(reference, await signIn())
  })
}

/**
 * Where to keep the file sent to an upload link one of the MCP's tools handed
 * out. Throws `BuiltinToolError` when the link can no longer be used.
 */
export async function builtinUploadTarget(
  mcp: Mcp,
  reference: unknown
): Promise<BuiltinUploadTarget> {
  const definition = requireBuiltinMcp(mcp)
  return withProvider(definition, mcp, async (provider, signIn) => {
    if (!provider.upload) {
      throw new BuiltinToolError(`${provider.name} takes no files`)
    }
    // The link outlives the call that made it. Where write access is one
    // switch, a file is only taken while it is on.
    if (definition.oauth && !mcp.builtinWriteEnabled) {
      throw new BuiltinToolError(`Write access is turned off for this ${provider.name} MCP`)
    }
    return provider.upload(reference, await signIn())
  })
}

/** Throws `BuiltinAuthorizationError` when the provider must be (re)authorized. */
export async function verifyBuiltin(mcp: Mcp) {
  await withProvider(requireBuiltinMcp(mcp), mcp, async (provider, signIn) =>
    provider.verify(await signIn())
  )
}
