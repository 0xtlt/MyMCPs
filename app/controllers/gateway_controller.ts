import type { HttpContext } from '@adonisjs/core/http'
import logger from '@adonisjs/core/services/logger'
import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js'
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js'
import type Mcp from '#models/mcp'
import McpCallLogService from '#services/mcp_call_log_service'
import {
  LAZY_GATEWAY_TOOLS,
  lazyGatewayInstructions,
  mcpCatalog,
  parseCallToolInput,
  parseGatewayToolMode,
  parseToolSearchInput,
  searchUpstreamTools,
} from '#services/gateway_lazy_tools'
import {
  callUpstreamTool,
  listNamespacedTools,
  parseNamespacedTool,
  probeUpstream,
} from '#services/upstream/manager'
import { sanitizeDiagnostic, sanitizeMcpDiagnostic } from '#services/security_redaction'

/**
 * Agent-facing MCP gateway: aggregate tools from allowed upstreams behind one URL.
 */
export default class GatewayController {
  async handle(ctx: HttpContext) {
    const toolMode = parseGatewayToolMode(ctx.request.header('x-mymcps-tool-mode'))
    if (!toolMode) {
      return ctx.response.status(400).json({
        error: 'invalid_tool_mode',
        message: 'X-MyMCPs-Tool-Mode must be either eager or lazy',
      })
    }

    const mcps = ctx.allowedMcps ?? []
    const callerIp = ctx.request.ip()
    const bySlug = new Map<string, Mcp>()
    for (const mcp of mcps) {
      bySlug.set(mcp.slug, mcp)
    }

    const tools = toolMode === 'eager' ? await listNamespacedTools(mcps) : []

    const server = new Server(
      { name: 'mymcps', version: '0.1.0' },
      {
        capabilities: { tools: {} },
        ...(toolMode === 'lazy' ? { instructions: lazyGatewayInstructions(mcps) } : {}),
      }
    )

    server.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools:
        toolMode === 'lazy'
          ? LAZY_GATEWAY_TOOLS
          : tools.map((tool) => ({
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

      if (toolMode === 'lazy') {
        if (requestedToolName === 'list_mcps') {
          const catalog = { mcps: mcpCatalog(mcps) }
          return {
            content: [{ type: 'text' as const, text: JSON.stringify(catalog) }],
            structuredContent: catalog,
          }
        }

        if (requestedToolName === 'tool_search') {
          const input = parseToolSearchInput(args)
          if (typeof input === 'string') {
            return {
              content: [{ type: 'text' as const, text: input }],
              isError: true,
            }
          }

          const mcp = bySlug.get(input.mcp)
          if (!mcp) {
            return {
              content: [
                {
                  type: 'text' as const,
                  text: `MCP "${input.mcp}" is not available to this access token`,
                },
              ],
              isError: true,
            }
          }

          try {
            const upstreamTools = await probeUpstream(mcp)
            const matches = searchUpstreamTools(upstreamTools, input.query, input.limit)
            const result = {
              mcp: mcpCatalog([mcp])[0],
              query: input.query,
              tools: matches,
            }
            return {
              content: [{ type: 'text' as const, text: JSON.stringify(result) }],
              structuredContent: result,
            }
          } catch (error) {
            logger.warn(
              { error: sanitizeMcpDiagnostic(error, mcp), mcpId: mcp.id, slug: mcp.slug },
              'Lazy gateway tool search failed'
            )
            return {
              content: [
                {
                  type: 'text' as const,
                  text: `Unable to search tools for MCP "${mcp.slug}"`,
                },
              ],
              isError: true,
            }
          }
        }

        if (requestedToolName === 'call_tool') {
          const input = parseCallToolInput(args)
          if (typeof input === 'string') {
            McpCallLogService.record({
              accessToken,
              callerIp,
              requestedToolName,
              toolName: null,
              args,
              outcome: 'error',
              errorCategory: 'invalid_tool',
              errorSummary: input,
              durationMs: performance.now() - startedAt,
            })
            return {
              content: [{ type: 'text' as const, text: input }],
              isError: true,
            }
          }

          const mcp = bySlug.get(input.mcp)
          const targetToolName = `${input.mcp}__${input.tool}`
          if (!mcp) {
            McpCallLogService.record({
              accessToken,
              callerIp,
              mcpSlug: input.mcp,
              requestedToolName: targetToolName,
              toolName: input.tool,
              args: input.arguments,
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

          return this.callAndRecord({
            accessToken,
            callerIp,
            mcp,
            requestedToolName: targetToolName,
            toolName: input.tool,
            args: input.arguments,
            startedAt,
          })
        }

        return {
          content: [{ type: 'text' as const, text: 'Invalid lazy gateway tool name' }],
          isError: true,
        }
      }

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

      return this.callAndRecord({
        accessToken,
        callerIp,
        mcp,
        requestedToolName,
        toolName: parsed.toolName,
        args,
        startedAt,
      })
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
        logger.debug({ error: sanitizeDiagnostic(error) }, 'Gateway MCP server close failed')
      })
    })

    await transport.handleRequest(nodeReq, nodeRes, body)
  }

  private async callAndRecord(params: {
    accessToken: NonNullable<HttpContext['accessToken']>
    callerIp: string | null
    mcp: Mcp
    requestedToolName: string
    toolName: string
    args: Record<string, unknown> | undefined
    startedAt: number
  }) {
    try {
      const result = await callUpstreamTool(params.mcp, params.toolName, params.args)
      const isError = result.isError === true
      McpCallLogService.record({
        accessToken: params.accessToken,
        callerIp: params.callerIp,
        mcp: params.mcp,
        requestedToolName: params.requestedToolName,
        toolName: params.toolName,
        args: params.args,
        response: result,
        outcome: isError ? 'error' : 'success',
        errorCategory: isError ? 'tool_error' : null,
        errorSummary: isError ? 'Upstream tool returned an error' : null,
        durationMs: performance.now() - params.startedAt,
      })
      return result
    } catch (error) {
      logger.warn(
        {
          error: sanitizeMcpDiagnostic(error, params.mcp),
          mcpId: params.mcp.id,
          tool: params.toolName,
        },
        'Upstream tool call failed'
      )
      McpCallLogService.record({
        accessToken: params.accessToken,
        callerIp: params.callerIp,
        mcp: params.mcp,
        requestedToolName: params.requestedToolName,
        toolName: params.toolName,
        args: params.args,
        outcome: 'error',
        errorCategory: 'upstream_exception',
        errorSummary: error instanceof Error ? error : 'Upstream tool call failed',
        durationMs: performance.now() - params.startedAt,
      })
      return {
        content: [{ type: 'text' as const, text: 'Upstream tool call failed' }],
        isError: true,
      }
    }
  }
}
