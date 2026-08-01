import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { refreshOauthAccessToken } from '#services/upstream/oauth'

export type UpstreamTool = {
  name: string
  description?: string
  inputSchema: Tool['inputSchema']
}

export type ConnectedHttpUpstream = {
  client: Client
  transport: StreamableHTTPClientTransport
  close: () => Promise<void>
}

function buildAuthHeaders(mcp: Mcp): Record<string, string> {
  const headers: Record<string, string> = {}

  if (mcp.authType === 'bearer') {
    const token = McpSecretStore.decrypt(mcp.authBearer)
    if (token) {
      headers.Authorization = `Bearer ${token}`
    }
  }

  if (mcp.authType === 'header') {
    const name = mcp.authHeaderName
    const value = McpSecretStore.decrypt(mcp.authHeaderValue)
    if (name && value) {
      headers[name] = value
    }
  }

  if (mcp.authType === 'oauth') {
    const token = McpSecretStore.decrypt(mcp.oauthAccessToken)
    if (token) {
      headers.Authorization = `${mcp.oauthTokenType || 'Bearer'} ${token}`
    }
  }

  return headers
}

export async function connectHttpUpstream(mcp: Mcp): Promise<ConnectedHttpUpstream> {
  if (!mcp.httpUrl) {
    throw new Error('HTTP MCP is missing a URL')
  }

  if (mcp.authType === 'oauth') {
    await refreshOauthAccessToken(mcp)
  }

  const headers = buildAuthHeaders(mcp)
  const transport = new StreamableHTTPClientTransport(new URL(mcp.httpUrl), {
    requestInit: { headers },
  })
  const client = new Client({ name: 'mymcps-gateway', version: '0.1.0' })
  await client.connect(transport)

  return {
    client,
    transport,
    close: async () => {
      try {
        await client.close()
      } catch {
        // Connection teardown is best-effort.
      }
      try {
        await transport.close()
      } catch {
        // Connection teardown is best-effort.
      }
    },
  }
}

export async function listHttpTools(mcp: Mcp): Promise<UpstreamTool[]> {
  const connected = await connectHttpUpstream(mcp)
  try {
    const result = await connected.client.listTools()
    return (result.tools ?? []).map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: tool.inputSchema,
    }))
  } finally {
    await connected.close()
  }
}
