import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { refreshOauthAccessToken } from '#services/upstream/oauth'
import { fetchWithoutRedirects } from '#services/upstream/safe_fetch'
import { ensureSafeHttpUrl } from '#services/http_url'

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

export class UpstreamUnauthorizedError extends Error {
  readonly status = 401

  constructor(options?: ErrorOptions) {
    super('MCP server rejected the configured credentials (HTTP 401).', options)
    this.name = 'UpstreamUnauthorizedError'
  }
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

  if (mcp.authType === 'auto') {
    const token = McpSecretStore.decrypt(mcp.oauthAccessToken)
    if (token) {
      // OAuth token type names are case-insensitive, but some upstreams (including
      // the provider behind Notion MCP) parse the Bearer scheme case-sensitively.
      headers.Authorization = `Bearer ${token}`
    }
  }

  return headers
}

export async function connectHttpUpstream(mcp: Mcp): Promise<ConnectedHttpUpstream> {
  if (!mcp.httpUrl) {
    throw new Error('HTTP MCP is missing a URL')
  }
  const endpoint = ensureSafeHttpUrl(mcp.httpUrl, 'MCP URL')

  if (mcp.authType === 'auto' && mcp.oauthAccessToken) {
    await refreshOauthAccessToken(mcp)
  }

  const headers = buildAuthHeaders(mcp)
  let unauthorizedResponse = false
  const diagnosticFetch: typeof fetch = async (input, init) => {
    const response = await fetchWithoutRedirects(input, init, 'MCP endpoint')
    if (response.status === 401) {
      unauthorizedResponse = true
    }
    return response
  }
  const transport = new StreamableHTTPClientTransport(endpoint, {
    fetch: diagnosticFetch,
    requestInit: { headers },
  })
  const client = new Client({ name: 'mymcps-gateway', version: '0.1.0' })
  try {
    await client.connect(transport)
  } catch (error) {
    if (unauthorizedResponse) {
      throw new UpstreamUnauthorizedError({ cause: error })
    }
    throw error
  }

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
