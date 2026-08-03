import type { HttpContext } from '@adonisjs/core/http'
import logger from '@adonisjs/core/services/logger'
import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js'
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import McpCallLogService from '#services/mcp_call_log_service'
import {
  callUpstreamTool,
  listNamespacedTools,
  parseNamespacedTool,
} from '#services/upstream/manager'

/**
 * Agent-facing MCP gateway: aggregate tools from allowed upstreams behind one URL.
 */
export default class GatewayController {
  async handle(ctx: HttpContext) {
    const mcps = ctx.allowedMcps ?? []
    const callerIp = ctx.request.ip()
    const bySlug = new Map<string, Mcp>()
    for (const mcp of mcps) {
      bySlug.set(mcp.slug, mcp)
    }

    const tools = await listNamespacedTools(mcps)

    const server = new Server({ name: 'mymcps', version: '0.1.0' }, { capabilities: { tools: {} } })

    server.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools: tools.map((tool) => ({
        name: tool.namespacedName,
        description: tool.description ?? `Tool ${tool.name} from ${tool.mcpSlug}`,
        inputSchema: tool.inputSchema,
      })),
    }))

    server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const startedAt = performance.now()
      const accessToken = ctx.accessToken!
      const requestedToolName = request.params.name
      const args = request.params.arguments
      const parsed = parseNamespacedTool(request.params.name)
      if (!parsed) {
        McpCallLogService.record({
          accessToken,
          callerIp,
          requestedToolName,
          toolName: null,
          args,
          outcome: 'error',
          errorCategory: 'invalid_tool',
          errorSummary: 'Invalid tool name',
          durationMs: performance.now() - startedAt,
        })
        return {
          content: [{ type: 'text' as const, text: 'Invalid tool name' }],
          isError: true,
        }
      }

      const mcp = bySlug.get(parsed.slug)
      if (!mcp) {
        McpCallLogService.record({
          accessToken,
          callerIp,
          mcpSlug: parsed.slug,
          requestedToolName,
          toolName: parsed.toolName,
          args,
          outcome: 'error',
          errorCategory: 'disallowed_mcp',
          errorSummary: 'MCP not allowed for this token',
          durationMs: performance.now() - startedAt,
        })
        return {
          content: [{ type: 'text' as const, text: 'MCP not allowed for this token' }],
          isError: true,
        }
      }

      try {
        // CallToolRequestSchema already types arguments as Record<string, unknown> | undefined
        const result = await callUpstreamTool(mcp, parsed.toolName, args)
        const isError = result.isError === true
        McpCallLogService.record({
          accessToken,
          callerIp,
          mcp,
          requestedToolName,
          toolName: parsed.toolName,
          args,
          response: result,
          outcome: isError ? 'error' : 'success',
          errorCategory: isError ? 'tool_error' : null,
          errorSummary: isError ? 'Upstream tool returned an error' : null,
          durationMs: performance.now() - startedAt,
        })
        return result
      } catch (error) {
        logger.warn(
          { err: error, mcpId: mcp.id, tool: parsed.toolName },
          'Upstream tool call failed'
        )
        McpCallLogService.record({
          accessToken,
          callerIp,
          mcp,
          requestedToolName,
          toolName: parsed.toolName,
          args,
          outcome: 'error',
          errorCategory: 'upstream_exception',
          errorSummary: error instanceof Error ? error : 'Upstream tool call failed',
          durationMs: performance.now() - startedAt,
        })
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

    void McpCallLogService.pruneExpired()

    nodeRes.on('finish', () => {
      void server.close().catch((error) => {
        logger.debug({ err: error }, 'Gateway MCP server close failed')
      })
    })

    await transport.handleRequest(nodeReq, nodeRes, body)
  }
}
