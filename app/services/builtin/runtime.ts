import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import {
  BuiltinAuthorizationError,
  BuiltinToolError,
  type BuiltinMcpDefinition,
  type BuiltinTool,
  type BuiltinToolContext,
} from '#services/builtin/definition'
import { parseOauthScopes } from '#services/builtin/oauth'
import { requireBuiltinMcp } from '#services/builtin/registry'
import McpSecretStore from '#services/mcp_secret_store'
import type { UpstreamTool } from '#services/upstream/http_client'
import { refreshOauthAccessToken } from '#services/upstream/oauth'

function notConnected(definition: BuiltinMcpDefinition) {
  return new BuiltinAuthorizationError(
    `${definition.name} is not connected. Connect it from the MCPs page in MyMCPs.`
  )
}

function grantedScopes(mcp: Mcp) {
  const scopes = parseOauthScopes(mcp.oauthScopes)
  return scopes.length > 0 ? scopes : null
}

function isGranted(tool: BuiltinTool, scopes: string[] | null) {
  return (
    !tool.requiresAnyScope ||
    scopes === null ||
    tool.requiresAnyScope.some((scope) => scopes.includes(scope))
  )
}

/**
 * Whether the provider granted any write scope. Unknown scopes count as
 * granted so the UI does not ask to re-authorize on a guess.
 */
export function builtinWriteGranted(mcp: Mcp) {
  const scopes = grantedScopes(mcp)
  return (
    scopes === null ||
    requireBuiltinMcp(mcp).oauth.writeScopes.some((scope) => scopes.includes(scope))
  )
}

function toolError(message: string): CallToolResult {
  return { content: [{ type: 'text', text: message }], isError: true }
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
  return { accessToken, grantedScopes: grantedScopes(mcp) }
}

/**
 * Tool definitions are static, so listing them never calls the provider.
 * Write tools are left out until the admin allows write access, and so are
 * tools whose permission the user unchecked while authorizing.
 */
export function listBuiltinTools(mcp: Mcp): UpstreamTool[] {
  const definition = requireBuiltinMcp(mcp)
  if (!mcp.oauthAccessToken) {
    throw notConnected(definition)
  }

  const scopes = grantedScopes(mcp)
  return definition.tools
    .filter((tool) => (!tool.write || mcp.builtinWriteEnabled) && isGranted(tool, scopes))
    .map(({ name, description, inputSchema }) => ({ name, description, inputSchema }))
}

export async function callBuiltinTool(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined
): Promise<CallToolResult> {
  const definition = requireBuiltinMcp(mcp)
  const tool = definition.tools.find((candidate) => candidate.name === toolName)
  if (!tool) {
    return toolError(`Unknown ${definition.name} tool: ${toolName}`)
  }
  if (tool.write && !mcp.builtinWriteEnabled) {
    return toolError(
      `${toolName} changes ${definition.name} data, and write access is turned off for this MCP. An administrator can allow it from the MCPs page in MyMCPs.`
    )
  }

  try {
    const context = await authorizedContext(definition, mcp)
    if (!isGranted(tool, context.grantedScopes)) {
      return toolError(
        `${toolName} needs the ${definition.name} permission "${tool.requiresAnyScope!.join('" or "')}", which was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked.`
      )
    }
    const data = await tool.run(args ?? {}, context)
    return { content: [{ type: 'text', text: JSON.stringify(data) }] }
  } catch (error) {
    if (error instanceof BuiltinToolError) {
      return toolError(error.message)
    }
    throw error
  }
}

/** Throws `BuiltinAuthorizationError` when the provider must be (re)authorized. */
export async function verifyBuiltin(mcp: Mcp) {
  const definition = requireBuiltinMcp(mcp)
  await definition.verify(await authorizedContext(definition, mcp))
}
