import type { HttpContext } from '@adonisjs/core/http'
import logger from '@adonisjs/core/services/logger'
import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js'
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import {
  callUpstreamTool,
  listNamespacedTools,
  parseNamespacedTool,
} from '#services/upstream/manager'
import { asToolArguments } from '#services/unknown'

/**
 * Agent-facing MCP gateway: aggregate tools from allowed upstreams behind one URL.
 */
export default class GatewayController {
  async handle(ctx: HttpContext) {
    const mcps = ctx.allowedMcps ?? []
    const bySlug = new Map<string, Mcp>()
    for (const mcp of mcps) {
      bySlug.set(mcp.slug, mcp)
    }

    const tools = await listNamespacedTools(mcps)

    const server = new Server(
      { name: 'mymcps', version: '0.1.0' },
      { capabilities: { tools: {} } }
    )

    server.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools: tools.map((tool) => ({
        name: tool.namespacedName,
        description: tool.description ?? `Tool ${tool.name} from ${tool.mcpSlug}`,
        inputSchema: tool.inputSchema,
      })),
    }))

    server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const parsed = parseNamespacedTool(request.params.name)
      if (!parsed) {
        return {
          content: [{ type: 'text' as const, text: 'Invalid tool name' }],
          isError: true,
        }
      }

      const mcp = bySlug.get(parsed.slug)
      if (!mcp) {
        return {
          content: [{ type: 'text' as const, text: 'MCP not allowed for this token' }],
          isError: true,
        }
      }

      try {
        return await callUpstreamTool(
          mcp,
          parsed.toolName,
          asToolArguments(request.params.arguments)
        )
      } catch (error) {
        logger.warn(
          { err: error, mcpId: mcp.id, tool: parsed.toolName },
          'Upstream tool call failed'
        )
        return {
          content: [{ type: 'text' as const, text: 'Upstream tool call failed' }],
          isError: true,
        }
      }
    })

    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
    })

    await server.connect(transport)

    const nodeReq = ctx.request.request
    const nodeRes = ctx.response.response
    const body = ctx.request.all()

    nodeRes.on('finish', () => {
      void server.close().catch((error) => {
        logger.debug({ err: error }, 'Gateway MCP server close failed')
      })
    })

    await transport.handleRequest(nodeReq, nodeRes, body)
  }
}
