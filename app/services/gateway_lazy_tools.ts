import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import type { UpstreamTool } from '#services/upstream/http_client'

export type GatewayToolMode = 'eager' | 'lazy'

export type McpCatalogEntry = {
  name: string
  slug: string
  description: string | null
  status: string
}

export const LAZY_GATEWAY_TOOLS: Tool[] = [
  {
    name: 'list_mcps',
    description:
      'List the MCP servers available to this access token. Use a returned slug with tool_search and call_tool.',
    inputSchema: {
      type: 'object',
      properties: {},
      additionalProperties: false,
    },
  },
  {
    name: 'tool_search',
    description:
      'Search tool definitions from one available MCP server without invoking them. Select the MCP by slug from the server catalog or list_mcps.',
    inputSchema: {
      type: 'object',
      properties: {
        mcp: {
          type: 'string',
          description: 'Exact MCP slug from the available MCP catalog.',
        },
        query: {
          type: 'string',
          description: 'Words describing the tool capability to find.',
        },
        limit: {
          type: 'integer',
          minimum: 1,
          maximum: 20,
          default: 10,
          description: 'Maximum number of matching tool definitions to return.',
        },
      },
      required: ['mcp', 'query'],
      additionalProperties: false,
    },
  },
  {
    name: 'call_tool',
    description:
      'Invoke an exact upstream tool. Use the MCP slug and tool name returned by tool_search; arguments must match that tool input schema.',
    inputSchema: {
      type: 'object',
      properties: {
        mcp: {
          type: 'string',
          description: 'Exact MCP slug from the available MCP catalog.',
        },
        tool: {
          type: 'string',
          description: 'Exact upstream tool name returned by tool_search.',
        },
        arguments: {
          type: 'object',
          description: 'Arguments matching the selected upstream tool input schema.',
          additionalProperties: true,
        },
      },
      required: ['mcp', 'tool'],
      additionalProperties: false,
    },
  },
]

export function parseGatewayToolMode(
  value: string | undefined,
  defaultMode: GatewayToolMode = 'eager'
): GatewayToolMode | null {
  if (value === undefined || value.trim() === '') {
    return defaultMode
  }

  const normalized = value.trim().toLowerCase()
  return normalized === 'eager' || normalized === 'lazy' ? normalized : null
}

export function mcpCatalog(mcps: Mcp[]): McpCatalogEntry[] {
  return mcps.map((mcp) => ({
    name: mcp.name,
    slug: mcp.slug,
    description: mcp.description,
    status: mcp.status,
  }))
}

function singleLine(value: string) {
  return value
    .replace(/[\r\n\t]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

export function lazyGatewayInstructions(mcps: Mcp[]) {
  const catalog = mcpCatalog(mcps)
  const entries = catalog.map((mcp) => {
    const description = mcp.description ? singleLine(mcp.description) : singleLine(mcp.name)
    return `- ${mcp.slug}: ${description}`
  })

  return [
    'Available MCPs:',
    ...(entries.length > 0 ? entries : ['- None available for this access token.']),
    '',
    "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools.",
  ].join('\n')
}

export type ToolSearchInput = {
  mcp: string
  query: string
  limit: number
}

export function parseToolSearchInput(
  args: Record<string, unknown> | undefined
): ToolSearchInput | string {
  const mcp = typeof args?.mcp === 'string' ? args.mcp.trim() : ''
  const query = typeof args?.query === 'string' ? args.query.trim() : ''
  const limit = args?.limit === undefined ? 10 : args.limit

  if (!mcp || mcp.length > 120) {
    return 'mcp must be a non-empty MCP slug of at most 120 characters'
  }
  if (!query || query.length > 200) {
    return 'query must be non-empty and at most 200 characters'
  }
  if (typeof limit !== 'number' || !Number.isInteger(limit) || limit < 1 || limit > 20) {
    return 'limit must be an integer between 1 and 20'
  }

  return { mcp, query, limit }
}

export type CallToolInput = {
  mcp: string
  tool: string
  arguments: Record<string, unknown> | undefined
}

export function parseCallToolInput(
  args: Record<string, unknown> | undefined
): CallToolInput | string {
  const mcp = typeof args?.mcp === 'string' ? args.mcp.trim() : ''
  const tool = typeof args?.tool === 'string' ? args.tool.trim() : ''
  const toolArguments = args?.arguments

  if (!mcp || mcp.length > 120) {
    return 'mcp must be a non-empty MCP slug of at most 120 characters'
  }
  if (!tool || tool.length > 128) {
    return 'tool must be a non-empty upstream tool name of at most 128 characters'
  }
  if (
    toolArguments !== undefined &&
    (toolArguments === null || Array.isArray(toolArguments) || typeof toolArguments !== 'object')
  ) {
    return 'arguments must be an object when provided'
  }

  return {
    mcp,
    tool,
    arguments: toolArguments as Record<string, unknown> | undefined,
  }
}

function searchTokens(query: string) {
  return query
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean)
}

function toolSearchScore(tool: UpstreamTool, query: string, tokens: string[]) {
  const name = tool.name.toLowerCase()
  const description = tool.description?.toLowerCase() ?? ''
  const normalizedQuery = query.toLowerCase()
  let score = 0

  if (name === normalizedQuery) score += 1_000
  else if (name.startsWith(normalizedQuery)) score += 600
  else if (name.includes(normalizedQuery)) score += 400
  if (description.includes(normalizedQuery)) score += 200

  for (const token of tokens) {
    if (name === token) score += 100
    else if (name.includes(token)) score += 50
    if (description.includes(token)) score += 10
  }

  return score
}

export function searchUpstreamTools(tools: UpstreamTool[], query: string, limit: number) {
  const tokens = searchTokens(query)
  return tools
    .map((tool) => ({ tool, score: toolSearchScore(tool, query, tokens) }))
    .filter((match) => match.score > 0)
    .sort(
      (left, right) => right.score - left.score || left.tool.name.localeCompare(right.tool.name)
    )
    .slice(0, limit)
    .map(({ tool }) => tool)
}
