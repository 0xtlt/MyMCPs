import { test } from '@japa/runner'
import type { ApiClient, ApiResponse } from '@japa/api-client'
import InstanceSetting from '#models/instance_setting'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp } from '#tests/helpers/factories'

const MCP_MESSAGE = 'mcp must be a non-empty MCP slug of at most 120 characters'
const QUERY_MESSAGE = 'query must be non-empty and at most 200 characters'
const LIMIT_MESSAGE = 'limit must be an integer between 1 and 20'
const TOOL_MESSAGE = 'tool must be a non-empty upstream tool name of at most 128 characters'
const ARGUMENTS_MESSAGE = 'arguments must be an object when provided'

type ToolResult = { content: Array<{ text: string }>; isError?: boolean }

function rpcResult(response: ApiResponse) {
  const data = response
    .text()
    .split('\n')
    .find((line) => line.startsWith('data:'))
    ?.slice(5)
  return (JSON.parse(data ?? response.text()) as { result: Record<string, unknown> }).result
}

function gatewayRpc(
  client: ApiClient,
  plaintext: string,
  body: Record<string, unknown>,
  mode?: string
) {
  const request = client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')
  if (mode !== undefined) {
    request.header('X-MyMCPs-Tool-Mode', mode)
  }
  return request.json(body)
}

function toolsList() {
  return { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} }
}

function toolCall(name: string, args?: Record<string, unknown>) {
  return {
    jsonrpc: '2.0',
    id: 2,
    method: 'tools/call',
    params: { name, ...(args === undefined ? {} : { arguments: args }) },
  }
}

/** One upstream MCP that answers every tool call with the call it received. */
function mockUpstream() {
  const originalFetch = globalThis.fetch
  const calls: unknown[] = []

  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    const rawBody = await request.clone().text()
    const message = rawBody
      ? (JSON.parse(rawBody) as { id?: number; method?: string; params?: unknown })
      : {}
    const headers = { 'Content-Type': 'application/json', 'Mcp-Session-Id': 'session' }
    const reply = (result: unknown) =>
      new Response(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }), {
        status: 200,
        headers,
      })

    if (request.method === 'DELETE') return new Response(null, { status: 200 })
    if (message.method === 'initialize') {
      return reply({
        protocolVersion: '2025-06-18',
        capabilities: { tools: {} },
        serverInfo: { name: 'issues', version: '1.0.0' },
      })
    }
    if (message.method === 'notifications/initialized') {
      return new Response(null, { status: 202, headers })
    }
    if (message.method === 'tools/list') {
      return reply({
        tools: [
          { name: 'create_issue', description: 'Create an issue', inputSchema: { type: 'object' } },
          { name: 'list_issues', description: 'List issues', inputSchema: { type: 'object' } },
        ],
      })
    }
    if (message.method === 'tools/call') {
      calls.push(message.params)
      return reply({ content: [{ type: 'text', text: 'called' }] })
    }
    return new Response('Not found', { status: 404 })
  }

  return {
    calls,
    restore() {
      globalThis.fetch = originalFetch
    },
  }
}

test.group('vine: gateway input', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(async () => {
    await McpCallLogService.flush()
    await rollbackTestTransaction()
  })

  test('falls back to the instance tool mode for a blank header and refuses unknown modes', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const settings = await InstanceSetting.current()
    settings.gatewayToolMode = 'lazy'
    await settings.save()
    const listed = async (mode?: string) => {
      const response = await gatewayRpc(client, plaintext, toolsList(), mode)
      response.assertStatus(200)
      return (rpcResult(response).tools as Array<{ name: string }>).map((tool) => tool.name)
    }
    const lazyTools = ['list_mcps', 'tool_search', 'call_tool']

    assert.deepEqual(await listed(), lazyTools)
    assert.deepEqual(await listed(''), lazyTools)
    assert.deepEqual(await listed('   '), lazyTools)
    assert.deepEqual(await listed('LAZY'), lazyTools)
    assert.deepEqual(await listed(' Eager '), [])

    for (const mode of ['sometimes', 'lazy, eager', 'eager lazy', '"lazy"', 'lazyy']) {
      const response = await gatewayRpc(client, plaintext, toolsList(), mode)
      response.assertStatus(400)
      response.assertBody({
        error: 'invalid_tool_mode',
        message: 'X-MyMCPs-Tool-Mode must be either eager or lazy',
      })
    }
  })

  test('tells the agent which tool_search argument to correct', async ({ client, assert }) => {
    const mock = mockUpstream()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [mcp.id],
      })
      const search = async (args?: Record<string, unknown>) => {
        const response = await gatewayRpc(client, plaintext, toolCall('tool_search', args), 'lazy')
        response.assertStatus(200)
        return rpcResult(response) as ToolResult
      }

      for (const [args, message] of [
        [undefined, MCP_MESSAGE],
        [{}, MCP_MESSAGE],
        [{ mcp: 5, query: 'issue' }, MCP_MESSAGE],
        [{ mcp: ['issues'], query: 'issue' }, MCP_MESSAGE],
        [{ mcp: 'x'.repeat(121), query: 'issue' }, MCP_MESSAGE],
        [{ mcp: '', query: '', limit: 0 }, MCP_MESSAGE],
        [{ mcp: 'issues' }, QUERY_MESSAGE],
        [{ mcp: 'issues', query: null }, QUERY_MESSAGE],
        [{ mcp: 'issues', query: 'q'.repeat(201), limit: 0 }, QUERY_MESSAGE],
        [{ mcp: 'issues', query: 'issue', limit: 0 }, LIMIT_MESSAGE],
        [{ mcp: 'issues', query: 'issue', limit: 21 }, LIMIT_MESSAGE],
        [{ mcp: 'issues', query: 'issue', limit: 1.5 }, LIMIT_MESSAGE],
        [{ mcp: 'issues', query: 'issue', limit: '5' }, LIMIT_MESSAGE],
        [{ mcp: 'issues', query: 'issue', limit: null }, LIMIT_MESSAGE],
      ] as Array<[Record<string, unknown> | undefined, string]>) {
        const result = await search(args)
        assert.isTrue(result.isError)
        assert.equal(result.content[0].text, message)
      }

      const found = await search({ mcp: ' issues ', query: ' issue ', surplus: true })
      assert.notProperty(found, 'isError')
      assert.deepEqual(
        (found as unknown as { structuredContent: { query: string; tools: unknown[] } })
          .structuredContent.query,
        'issue'
      )
      const limited = await search({ mcp: 'issues', query: 'issue', limit: 1 })
      assert.lengthOf(
        (limited as unknown as { structuredContent: { tools: unknown[] } }).structuredContent.tools,
        1
      )
    } finally {
      mock.restore()
    }
  })

  test('tells the agent which call_tool argument to correct and logs the refusal', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstream()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [mcp.id],
      })
      const call = async (args?: Record<string, unknown>) => {
        const response = await gatewayRpc(client, plaintext, toolCall('call_tool', args), 'lazy')
        response.assertStatus(200)
        return rpcResult(response) as ToolResult
      }
      const refusals: Array<[Record<string, unknown> | undefined, string]> = [
        [undefined, MCP_MESSAGE],
        [{ tool: 'create_issue' }, MCP_MESSAGE],
        [{ mcp: '', tool: '', arguments: [] }, MCP_MESSAGE],
        [{ mcp: 'issues' }, TOOL_MESSAGE],
        [{ mcp: 'issues', tool: 't'.repeat(129), arguments: [] }, TOOL_MESSAGE],
        [{ mcp: 'issues', tool: 'create_issue', arguments: null }, ARGUMENTS_MESSAGE],
        [{ mcp: 'issues', tool: 'create_issue', arguments: [] }, ARGUMENTS_MESSAGE],
        [{ mcp: 'issues', tool: 'create_issue', arguments: 'title' }, ARGUMENTS_MESSAGE],
        [{ mcp: 'issues', tool: 'create_issue', arguments: 0 }, ARGUMENTS_MESSAGE],
      ]

      for (const [args, message] of refusals) {
        const result = await call(args)
        assert.isTrue(result.isError)
        assert.equal(result.content[0].text, message)
      }
      assert.lengthOf(mock.calls, 0)

      await call({ mcp: ' issues ', tool: ' create_issue ' })
      await call({
        mcp: 'issues',
        tool: 'create_issue',
        arguments: { title: 'Hello', labels: ['a', { deep: [1, null] }], count: 0 },
      })
      assert.deepEqual(mock.calls, [
        { name: 'create_issue', arguments: {} },
        {
          name: 'create_issue',
          arguments: { title: 'Hello', labels: ['a', { deep: [1, null] }], count: 0 },
        },
      ])

      await McpCallLogService.flush()
      const logs = await McpCallLog.query().orderBy('id', 'asc')
      assert.deepEqual(
        logs.map((log) => [log.outcome, log.errorCategory, log.errorSummary]),
        [
          ...refusals.map(([, message]) => ['error', 'invalid_tool', message]),
          ['success', null, null],
          ['success', null, null],
        ]
      )
    } finally {
      mock.restore()
    }
  })

  test('splits an eager tool name at its first separator and refuses names without a slug', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstream()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [mcp.id],
      })
      const call = async (name: string) => {
        const response = await gatewayRpc(client, plaintext, toolCall(name, { title: 'Hello' }))
        response.assertStatus(200)
        return rpcResult(response) as ToolResult
      }

      for (const name of ['__create_issue', '___create_issue', 'issues_create_issue', 'issues']) {
        const result = await call(name)
        assert.isTrue(result.isError)
        assert.equal(result.content[0].text, 'Invalid tool name')
      }
      for (const name of ['missing__create_issue', '_issues__create_issue', 'issues __tool']) {
        const result = await call(name)
        assert.isTrue(result.isError)
        assert.equal(result.content[0].text, 'MCP not allowed for this token')
      }

      for (const name of [
        'issues__create_issue',
        'issues__create__issue',
        'issues___x',
        'issues__',
      ]) {
        assert.notProperty(await call(name), 'isError')
      }
      assert.deepEqual(
        mock.calls.map((sent) => (sent as { name: string }).name),
        ['create_issue', 'create__issue', '_x', '']
      )
    } finally {
      mock.restore()
    }
  })
})
