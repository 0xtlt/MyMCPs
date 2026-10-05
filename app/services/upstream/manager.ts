import type Mcp from '#models/mcp'
import logger from '@adonisjs/core/services/logger'
import McpSecretStore from '#services/mcp_secret_store'
import { UnauthorizedError } from '@modelcontextprotocol/sdk/client/auth.js'
import { StreamableHTTPError } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import {
  connectHttpUpstream,
  listHttpTools,
  UpstreamUnauthorizedError,
  type ConnectedHttpUpstream,
  type UpstreamTool,
} from '#services/upstream/http_client'
import {
  connectDenoUpstream,
  listDenoTools,
  type ConnectedDenoUpstream,
} from '#services/upstream/deno_runner'
import { sanitizeMcpDiagnostic } from '#services/security_redaction'
import { BuiltinAuthorizationError } from '#services/builtin/definition'
import { builtinMcp } from '#services/builtin/registry'
import { callBuiltinTool, listBuiltinTools, verifyBuiltin } from '#services/builtin/runtime'
import { namespacedToolValidator } from '#validators/gateway'

export type ConnectedUpstream = ConnectedHttpUpstream | ConnectedDenoUpstream

export function namespaceTool(slug: string, toolName: string) {
  return `${slug}__${toolName}`
}

export async function parseNamespacedTool(namespaced: string) {
  const [malformed, tool] = await namespacedToolValidator.tryValidate(namespaced)
  return malformed ? null : tool
}

export async function connectUpstream(mcp: Mcp): Promise<ConnectedUpstream> {
  if (mcp.transport === 'http') {
    return connectHttpUpstream(mcp)
  }
  if (mcp.transport === 'npm') {
    return connectDenoUpstream(mcp)
  }
  throw new Error(`Unsupported transport: ${mcp.transport}`)
}

export async function probeUpstream(mcp: Mcp): Promise<UpstreamTool[]> {
  if (mcp.transport === 'http') {
    return listHttpTools(mcp)
  }
  if (mcp.transport === 'npm') {
    return listDenoTools(mcp)
  }
  if (mcp.transport === 'builtin') {
    return listBuiltinTools(mcp)
  }
  throw new Error(`Unsupported transport: ${mcp.transport}`)
}

export async function listNamespacedTools(mcps: Mcp[]) {
  const tools: Array<UpstreamTool & { mcpId: number; mcpSlug: string; namespacedName: string }> = []

  for (const mcp of mcps) {
    try {
      const listed = await probeUpstream(mcp)
      for (const tool of listed) {
        tools.push({
          ...tool,
          mcpId: mcp.id,
          mcpSlug: mcp.slug,
          namespacedName: namespaceTool(mcp.slug, tool.name),
        })
      }
    } catch (error) {
      logger.warn(
        { error: sanitizeMcpDiagnostic(error, mcp), mcpId: mcp.id, slug: mcp.slug },
        'Skipping unhealthy upstream while listing gateway tools'
      )
    }
  }

  return tools
}

export async function callUpstreamTool(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined
) {
  if (mcp.transport === 'builtin') {
    return callBuiltinTool(mcp, toolName, args)
  }

  const connected = await connectUpstream(mcp)
  try {
    return await connected.client.callTool({
      name: toolName,
      arguments: args ?? {},
    })
  } finally {
    await connected.close()
  }
}

/**
 * Listing built-in tools is local, so health comes from one authenticated
 * provider request instead.
 */
async function testBuiltinAndUpdateStatus(mcp: Mcp) {
  const hadAccessToken = Boolean(McpSecretStore.decrypt(mcp.oauthAccessToken))
  try {
    await verifyBuiltin(mcp)
    mcp.status = 'ready'
    mcp.lastError = null
    mcp.oauthRequired = false
  } catch (error) {
    // Connect only repairs an OAuth sign-in. A rejected password is an error
    // to fix in the form.
    const authorizationRequired =
      error instanceof BuiltinAuthorizationError && Boolean(builtinMcp(mcp.builtinKey)?.oauth)
    mcp.status = authorizationRequired && !hadAccessToken ? 'draft' : 'error'
    mcp.lastError =
      authorizationRequired && !hadAccessToken
        ? 'OAuth authorization required'
        : (sanitizeMcpDiagnostic(error, mcp) ?? 'Unknown error')
    mcp.oauthRequired = authorizationRequired
  }
  await mcp.save()
  return mcp
}

export async function testAndUpdateStatus(mcp: Mcp) {
  if (mcp.transport === 'builtin') {
    return testBuiltinAndUpdateStatus(mcp)
  }

  try {
    await probeUpstream(mcp)
    mcp.status = 'ready'
    mcp.lastError = null
    mcp.oauthRequired = false
    await mcp.save()
  } catch (error) {
    const authorizationRequired =
      error instanceof UnauthorizedError ||
      error instanceof UpstreamUnauthorizedError ||
      (error instanceof StreamableHTTPError && error.code === 401)

    if (mcp.transport === 'http' && mcp.authType === 'auto' && authorizationRequired) {
      const hasOauthAccessToken = Boolean(McpSecretStore.decrypt(mcp.oauthAccessToken))
      mcp.status = hasOauthAccessToken ? 'error' : 'draft'
      mcp.lastError = hasOauthAccessToken
        ? error instanceof UpstreamUnauthorizedError
          ? sanitizeMcpDiagnostic(`OAuth token rejected. ${error.message}`, mcp)!
          : 'OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP.'
        : 'OAuth authorization required'
      mcp.oauthRequired = true
    } else {
      mcp.status = 'error'
      mcp.lastError = sanitizeMcpDiagnostic(error, mcp) ?? 'Unknown error'
      mcp.oauthRequired = false
    }
    await mcp.save()
  }
  return mcp
}
