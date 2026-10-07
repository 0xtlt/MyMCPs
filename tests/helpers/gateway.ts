import type { ApiClient } from '@japa/api-client'
import McpCallLogService from '#services/mcp_call_log_service'

export type RpcResponse = {
  result?: {
    tools?: Array<{ name: string; description?: string }>
    content?: Array<{ type: string; text: string }>
    isError?: boolean
  }
}

function parseRpcResponse(response: { text: () => string; body: () => unknown }): RpcResponse {
  const text = response.text().trim()
  const data = text
    .split('\n')
    .find((line) => line.startsWith('data:'))
    ?.slice(5)
    .trim()
  if (data) return JSON.parse(data) as RpcResponse
  return text ? (JSON.parse(text) as RpcResponse) : (response.body() as RpcResponse)
}

/** Send one JSON-RPC request to the gateway as an agent would, and wait for its call log. */
export async function gatewayRpc(
  client: ApiClient,
  plaintext: string,
  method: string,
  params: Record<string, unknown>,
  mode: 'eager' | 'lazy' = 'eager'
) {
  const response = await client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')
    .header('X-MyMCPs-Tool-Mode', mode)
    .json({ jsonrpc: '2.0', id: 1, method, params })
  response.assertStatus(200)
  await McpCallLogService.flush()
  return parseRpcResponse(response)
}

/** The text of a tool result, which is all that built-in tools and the gateway return. */
export function resultText(response: RpcResponse) {
  return response.result?.content?.[0]?.text ?? ''
}

type UpstreamTool = { name: string; description?: string }

function jsonRpc(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json', 'Mcp-Session-Id': 'test-session' },
  })
}

/**
 * Replace `fetch` with an MCP server reached over HTTP at `origin`. It lists
 * `tools` and answers every call with the arguments it was given, which
 * `calls` also keeps.
 */
export function mockHttpMcp(origin: string, tools: UpstreamTool[]) {
  const originalFetch = globalThis.fetch
  const calls: Array<{ name: string; arguments: unknown }> = []

  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    if (new URL(request.url).origin !== origin) {
      return originalFetch(input, init)
    }
    if (request.method === 'DELETE') return new Response(null, { status: 200 })

    const message = JSON.parse(await request.text()) as {
      id?: number
      method?: string
      params?: { name: string; arguments?: unknown }
    }
    switch (message.method) {
      case 'initialize':
        return jsonRpc({
          jsonrpc: '2.0',
          id: message.id,
          result: {
            protocolVersion: '2025-06-18',
            capabilities: { tools: {} },
            serverInfo: { name: 'Test MCP', version: '1.0.0' },
          },
        })
      case 'notifications/initialized':
        return new Response(null, { status: 202, headers: { 'Mcp-Session-Id': 'test-session' } })
      case 'tools/list':
        return jsonRpc({
          jsonrpc: '2.0',
          id: message.id,
          result: { tools: tools.map((tool) => ({ ...tool, inputSchema: { type: 'object' } })) },
        })
      case 'tools/call':
        calls.push({ name: message.params!.name, arguments: message.params!.arguments })
        return jsonRpc({
          jsonrpc: '2.0',
          id: message.id,
          result: { content: [{ type: 'text', text: `ran ${message.params!.name}` }] },
        })
      default:
        return new Response('Not found', { status: 404 })
    }
  }

  return {
    calls,
    restore: () => {
      globalThis.fetch = originalFetch
    },
  }
}
