import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import type { UpstreamTool } from '#services/upstream/http_client'
import {
  callToolValidator,
  gatewayToolModeValidator,
  toolSearchValidator,
} from '#validators/gateway'

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

/** The tool mode a request asks for, or null when its header names no mode the gateway has. */
export async function parseGatewayToolMode(
  value: string | undefined,
  defaultMode: GatewayToolMode = 'eager'
): Promise<GatewayToolMode | null> {
  const [unsupported, mode] = await gatewayToolModeValidator.tryValidate(value)
  return unsupported ? null : (mode ?? defaultMode)
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

/**
 * The arguments of `tool_search`, or the sentence that tells the agent which
 * argument to correct: the first one that is wrong.
 */
export async function parseToolSearchInput(
  args: Record<string, unknown> | undefined
): Promise<ToolSearchInput | string> {
  const [error, input] = await toolSearchValidator.tryValidate(args ?? {})
  return error ? error.messages[0].message : input
}

export type CallToolInput = {
  mcp: string
  tool: string
  arguments: Record<string, unknown> | undefined
}

/**
 * The arguments of `call_tool`, or the sentence that tells the agent which
 * argument to correct: the first one that is wrong.
 */
export async function parseCallToolInput(
  args: Record<string, unknown> | undefined
): Promise<CallToolInput | string> {
  const [error, input] = await callToolValidator.tryValidate(args ?? {})
  if (error) {
    return error.messages[0].message
  }

  return {
    mcp: input.mcp,
    tool: input.tool,
    // The upstream tool receives the object the agent sent, not a copy of it.
    arguments: args?.arguments as CallToolInput['arguments'],
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
