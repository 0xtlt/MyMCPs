import type Mcp from '#models/mcp'
import {
  connectHttpUpstream,
  listHttpTools,
  type ConnectedHttpUpstream,
  type UpstreamTool,
} from '#services/upstream/http_client'
import {
  connectDenoUpstream,
  listDenoTools,
  type ConnectedDenoUpstream,
} from '#services/upstream/deno_runner'

export type ConnectedUpstream = ConnectedHttpUpstream | ConnectedDenoUpstream

export function namespaceTool(slug: string, toolName: string) {
  return `${slug}__${toolName}`
}

export function parseNamespacedTool(namespaced: string) {
  const idx = namespaced.indexOf('__')
  if (idx <= 0) {
    return null
  }
  return {
    slug: namespaced.slice(0, idx),
    toolName: namespaced.slice(idx + 2),
  }
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
  throw new Error(`Unsupported transport: ${mcp.transport}`)
}

export async function listNamespacedTools(mcps: Mcp[]) {
  const tools: Array<UpstreamTool & { mcpId: number; mcpSlug: string; namespacedName: string }> =
    []

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
    } catch {
      // Skip unhealthy upstreams for aggregation; status is tracked separately.
    }
  }

  return tools
}

export async function callUpstreamTool(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined
) {
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

export async function testAndUpdateStatus(mcp: Mcp) {
  try {
    if (mcp.authType === 'oauth' && !mcp.oauthAccessToken) {
      mcp.status = 'draft'
      mcp.lastError = 'OAuth authorization required'
      await mcp.save()
      return mcp
    }

    await probeUpstream(mcp)
    mcp.status = 'ready'
    mcp.lastError = null
    await mcp.save()
  } catch (error) {
    mcp.status = 'error'
    mcp.lastError = error instanceof Error ? error.message : String(error)
    await mcp.save()
  }
  return mcp
}
