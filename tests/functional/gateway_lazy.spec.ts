import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import InstanceSetting from '#models/instance_setting'
import McpCallLog from '#models/mcp_call_log'
import { applicationVersion } from '#services/application_version'
import McpCallLogService from '#services/mcp_call_log_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp } from '#tests/helpers/factories'

type RpcResponse = {
  jsonrpc: '2.0'
  id: string | number
  result?: Record<string, unknown>
  error?: Record<string, unknown>
}

function parseRpcResponse(response: { text: () => string; body: () => unknown }): RpcResponse {
  const text = response.text().trim()
  const data = text
    .split('\n')
    .find((line) => line.startsWith('data:'))
    ?.slice(5)
    .trim()
  if (data) {
    return JSON.parse(data) as RpcResponse
  }
  if (text) {
    return JSON.parse(text) as RpcResponse
  }
  return response.body() as RpcResponse
}

async function gatewayRpc(
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

function initializeRequest(id = 1) {
  return {
    jsonrpc: '2.0',
    id,
    method: 'initialize',
    params: {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'gateway-lazy-test', version: '1.0.0' },
    },
  }
}

function toolsListRequest(id = 2) {
  return { jsonrpc: '2.0', id, method: 'tools/list', params: {} }
}

function toolCallRequest(name: string, args?: Record<string, unknown>, id = 3) {
  return {
    jsonrpc: '2.0',
    id,
    method: 'tools/call',
    params: { name, ...(args === undefined ? {} : { arguments: args }) },
  }
}

function jsonRpcResponse(body: unknown, sessionId: string) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'Content-Type': 'application/json',
      'Mcp-Session-Id': sessionId,
    },
  })
}

function mockUpstreams() {
  const originalFetch = globalThis.fetch
  const requests: Array<{ clientVersion?: string; host: string; method: string }> = []

  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    const host = new URL(request.url).host
    const rawBody = await request.clone().text()
    const message = rawBody
      ? (JSON.parse(rawBody) as {
          id?: string | number
          method?: string
          params?: {
            name?: string
            arguments?: Record<string, unknown>
            clientInfo?: { version?: string }
          }
        })
      : {}
    requests.push({
      clientVersion: message.params?.clientInfo?.version,
      host,
      method: message.method ?? request.method,
    })
    const sessionId = `${host.replace(/[^a-z0-9]/gi, '-')}-session`

    if (request.method === 'DELETE') return new Response(null, { status: 200 })
    if (message.method === 'initialize') {
      return jsonRpcResponse(
        {
          jsonrpc: '2.0',
          id: message.id,
          result: {
            protocolVersion: '2025-06-18',
            capabilities: { tools: {} },
            serverInfo: { name: host, version: '1.0.0' },
          },
        },
        sessionId
      )
    }
    if (message.method === 'notifications/initialized') {
      return new Response(null, { status: 202, headers: { 'Mcp-Session-Id': sessionId } })
    }
    if (message.method === 'tools/list') {
      const tools = host.startsWith('issues.')
        ? [
            {
              name: 'create_issue',
              description: 'Create a new project issue',
              inputSchema: {
                type: 'object',
                properties: { title: { type: 'string' } },
                required: ['title'],
              },
            },
            {
              name: 'list_issues',
              description: 'List project work items',
              inputSchema: { type: 'object' },
            },
          ]
        : [
            {
              name: 'create_event',
              description: 'Create a calendar event',
              inputSchema: { type: 'object' },
            },
          ]
      return jsonRpcResponse({ jsonrpc: '2.0', id: message.id, result: { tools } }, sessionId)
    }
    if (message.method === 'tools/call') {
      return jsonRpcResponse(
        {
          jsonrpc: '2.0',
          id: message.id,
          result: {
            content: [
              {
                type: 'text',
                text: JSON.stringify({
                  name: message.params?.name,
                  args: message.params?.arguments,
                }),
              },
            ],
          },
        },
        sessionId
      )
    }

    return new Response('Not found', { status: 404 })
  }

  return {
    requests,
    restore() {
      globalThis.fetch = originalFetch
    },
  }
}

test.group('gateway lazy tool mode', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(async () => {
    await McpCallLogService.flush()
    await rollbackTestTransaction()
  })

  test('rejects an unsupported tool mode header on POST and GET', async ({ client }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)

    const postResponse = await gatewayRpc(client, plaintext, toolsListRequest(), 'sometimes')
    const getResponse = await client
      .get('/mcp')
      .bearerToken(plaintext)
      .header('X-MyMCPs-Tool-Mode', 'sometimes')

    for (const response of [postResponse, getResponse]) {
      response.assertStatus(400)
      response.assertBody({
        error: 'invalid_tool_mode',
        message: 'X-MyMCPs-Tool-Mode must be either eager or lazy',
      })
    }
  })

  test('uses the instance default when the header is absent and lets the header override it', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const issues = await createMcp(admin.id, {
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [issues.id],
      })
      const settings = await InstanceSetting.current()
      settings.gatewayToolMode = 'lazy'
      await settings.save()

      const defaultResponse = await gatewayRpc(client, plaintext, toolsListRequest())
      defaultResponse.assertStatus(200)
      const defaultTools = parseRpcResponse(defaultResponse).result?.tools as Array<{
        name: string
      }>

      assert.deepEqual(
        defaultTools.map((tool) => tool.name),
        ['list_mcps', 'tool_search', 'call_tool']
      )
      assert.lengthOf(mock.requests, 0)

      const overriddenResponse = await gatewayRpc(client, plaintext, toolsListRequest(), 'eager')
      overriddenResponse.assertStatus(200)
      const overriddenTools = parseRpcResponse(overriddenResponse).result?.tools as Array<{
        name: string
      }>

      assert.deepEqual(
        overriddenTools.map((tool) => tool.name),
        ['issues__create_issue', 'issues__list_issues']
      )
      assert.isTrue(mock.requests.some((request) => request.method === 'tools/list'))
    } finally {
      mock.restore()
    }
  })

  test('shares only allowed MCP summaries during lazy initialization without upstream calls', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const allowed = await createMcp(admin.id, {
        name: 'Issue Tracker',
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
        status: 'ready',
      })
      allowed.description = 'Project issues\nwithout exposing credentials'
      await allowed.save()
      const blocked = await createMcp(admin.id, {
        name: 'Private Calendar',
        slug: 'calendar',
        httpUrl: 'https://calendar.example/mcp',
      })
      blocked.lastError = 'Bearer top-secret-value'
      await blocked.save()
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [allowed.id],
      })

      const response = await gatewayRpc(client, plaintext, initializeRequest(), ' LaZy ')
      response.assertStatus(200)
      const rpc = parseRpcResponse(response)
      const instructions = String(rpc.result?.instructions)

      assert.deepEqual(rpc.result?.serverInfo, { name: 'mymcps', version: applicationVersion })
      assert.equal(
        instructions,
        [
          'Available MCPs:',
          '- issues: Project issues without exposing credentials',
          '',
          "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools.",
        ].join('\n')
      )
      assert.notInclude(instructions, 'Private Calendar')
      assert.notInclude(instructions, 'top-secret-value')
      assert.lengthOf(mock.requests, 0)
    } finally {
      mock.restore()
    }
  })

  test('lists only lazy gateway tools without probing upstream MCPs', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
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

      const response = await gatewayRpc(client, plaintext, toolsListRequest(), 'lazy')
      response.assertStatus(200)
      const rpc = parseRpcResponse(response)
      const tools = rpc.result?.tools as Array<{ name: string }>

      assert.deepEqual(
        tools.map((tool) => tool.name),
        ['list_mcps', 'tool_search', 'call_tool']
      )
      assert.lengthOf(mock.requests, 0)
    } finally {
      mock.restore()
    }
  })

  test('returns the allowed MCP catalog through list_mcps without upstream calls', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const issues = await createMcp(admin.id, {
        name: 'Issues',
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      issues.description = 'Issue tracking'
      await issues.save()
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [issues.id],
      })

      const response = await gatewayRpc(client, plaintext, toolCallRequest('list_mcps'), 'lazy')
      response.assertStatus(200)
      const rpc = parseRpcResponse(response)
      const result = rpc.result?.structuredContent as {
        mcps: Array<{ name: string; slug: string; description: string; status: string }>
      }

      assert.deepEqual(result.mcps, [
        { name: 'Issues', slug: 'issues', description: 'Issue tracking', status: 'ready' },
      ])
      assert.lengthOf(mock.requests, 0)
    } finally {
      mock.restore()
    }
  })

  test('searches only the selected allowed MCP and returns matching schemas', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const issues = await createMcp(admin.id, {
        name: 'Issues',
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const calendar = await createMcp(admin.id, {
        name: 'Calendar',
        slug: 'calendar',
        httpUrl: 'https://calendar.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [issues.id, calendar.id],
      })

      const response = await gatewayRpc(
        client,
        plaintext,
        toolCallRequest('tool_search', { mcp: 'issues', query: 'create issue', limit: 1 }),
        'lazy'
      )
      response.assertStatus(200)
      const rpc = parseRpcResponse(response)
      const result = rpc.result?.structuredContent as {
        mcp: { slug: string }
        tools: Array<{ name: string; inputSchema: Record<string, unknown> }>
      }

      assert.equal(result.mcp.slug, 'issues')
      assert.lengthOf(result.tools, 1)
      assert.equal(result.tools[0].name, 'create_issue')
      assert.property(result.tools[0].inputSchema, 'properties')
      assert.isTrue(mock.requests.every((request) => request.host === 'issues.example'))
      assert.isTrue(mock.requests.some((request) => request.method === 'tools/list'))
      assert.equal(
        mock.requests.find((request) => request.method === 'initialize')?.clientVersion,
        applicationVersion
      )
    } finally {
      mock.restore()
    }
  })

  test('calls an allowed upstream tool and logs its real target', async ({ client, assert }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const issues = await createMcp(admin.id, {
        name: 'Issues',
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [issues.id],
      })

      const response = await gatewayRpc(
        client,
        plaintext,
        toolCallRequest('call_tool', {
          mcp: 'issues',
          tool: 'create_issue',
          arguments: { title: 'Lazy discovery works' },
        }),
        'lazy'
      )
      response.assertStatus(200)
      await McpCallLogService.flush()

      const rpc = parseRpcResponse(response)
      assert.include(JSON.stringify(rpc.result), 'create_issue')
      const log = await McpCallLog.firstOrFail()
      assert.equal(log.mcpSlug, 'issues')
      assert.equal(log.requestedToolName, 'issues__create_issue')
      assert.equal(log.toolName, 'create_issue')
      assert.isTrue(mock.requests.some((request) => request.method === 'tools/call'))
    } finally {
      mock.restore()
    }
  })

  test('preserves eager namespaced discovery when the header is absent', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      const issues = await createMcp(admin.id, {
        slug: 'issues',
        httpUrl: 'https://issues.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [issues.id],
      })

      const response = await gatewayRpc(client, plaintext, toolsListRequest())
      response.assertStatus(200)
      const rpc = parseRpcResponse(response)
      const tools = rpc.result?.tools as Array<{ name: string }>

      assert.deepEqual(
        tools.map((tool) => tool.name),
        ['issues__create_issue', 'issues__list_issues']
      )
      assert.isTrue(mock.requests.some((request) => request.method === 'tools/list'))
    } finally {
      mock.restore()
    }
  })
})
