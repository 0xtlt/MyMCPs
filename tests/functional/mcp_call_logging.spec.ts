import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import InstanceSetting from '#models/instance_setting'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp } from '#tests/helpers/factories'

function jsonRpcResponse(body: unknown, session = true) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'Content-Type': 'application/json',
      ...(session ? { 'Mcp-Session-Id': 'logging-test-session' } : {}),
    },
  })
}

function mockToolServer() {
  const originalFetch = globalThis.fetch

  globalThis.fetch = async (_input, init) => {
    const method = init?.method ?? 'GET'
    if (method === 'DELETE') return new Response(null, { status: 200 })
    const rawBody = String(init?.body ?? '')
    if (!rawBody) return new Response('Missing body', { status: 400 })
    const message = JSON.parse(rawBody) as {
      id?: string | number
      method?: string
      params?: { name?: string }
    }

    if (message.method === 'initialize') {
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          protocolVersion: '2025-06-18',
          capabilities: { tools: {} },
          serverInfo: { name: 'Logging test', version: '1.0.0' },
        },
      })
    }
    if (message.method === 'notifications/initialized') {
      return new Response(null, {
        status: 202,
        headers: { 'Mcp-Session-Id': 'logging-test-session' },
      })
    }
    if (message.method === 'tools/list') {
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          tools: [
            { name: 'echo', inputSchema: { type: 'object' } },
            { name: 'fail', inputSchema: { type: 'object' } },
            { name: 'explode', inputSchema: { type: 'object' } },
          ],
        },
      })
    }
    if (message.method === 'tools/call' && message.params?.name === 'explode') {
      throw new Error('Bearer very-secret-token failed at https://user:pass@example.test')
    }
    if (message.method === 'tools/call') {
      const isError = message.params?.name === 'fail'
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          content: [{ type: 'text', text: isError ? 'private tool failure' : 'ok' }],
          ...(isError ? { isError: true } : {}),
        },
      })
    }

    return new Response('Not found', { status: 404 })
  }

  return () => {
    globalThis.fetch = originalFetch
  }
}

async function callGateway(
  client: ApiClient,
  plaintext: string,
  name: string,
  args?: Record<string, unknown>,
  options: { flush?: boolean; headers?: Record<string, string> } = {}
) {
  const request = client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')

  for (const [header, value] of Object.entries(options.headers ?? {})) {
    request.header(header, value)
  }

  const response = await request.json({
    jsonrpc: '2.0',
    id: Math.floor(Math.random() * 1_000_000),
    method: 'tools/call',
    params: { name, ...(args === undefined ? {} : { arguments: args }) },
  })

  if (options.flush !== false) {
    await McpCallLogService.flush()
  }
  return response
}

test.group('MCP call capture', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('captures successful calls as metadata without arguments', async ({ client, assert }) => {
    const restoreFetch = mockToolServer()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Logging MCP',
        slug: 'logging',
        httpUrl: 'https://logging.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, {
        scopeMode: 'selected',
        mcpIds: [mcp.id],
      })

      const response = await callGateway(
        client,
        plaintext,
        'logging__echo',
        { message: 'secret' },
        {
          headers: { 'x-forwarded-for': '192.0.2.10' },
        }
      )
      response.assertStatus(200)

      const log = await McpCallLog.firstOrFail()
      assert.equal(log.outcome, 'success')
      assert.equal(log.mcpName, 'Logging MCP')
      assert.equal(log.toolName, 'echo')
      assert.equal(log.callerIp, '192.0.2.10')
      assert.isFalse(log.argumentsCaptured)
      assert.isNull(log.arguments)
      assert.isFalse(log.responseCaptured)
      assert.isNull(log.response)
      assert.isAtLeast(log.durationMs, 0)
    } finally {
      restoreFetch()
    }
  })

  test('captures exact arguments and tool-returned errors at arguments level', async ({
    client,
    assert,
  }) => {
    const restoreFetch = mockToolServer()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'logging',
        httpUrl: 'https://logging.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, { mcpIds: [mcp.id] })
      const settings = await InstanceSetting.findOrFail(1)
      settings.mcpLogLevel = 'arguments'
      await settings.save()
      const args = { password: 'stored-exactly', nested: { token: 'also-stored' } }

      const response = await callGateway(client, plaintext, 'logging__fail', args)
      response.assertStatus(200)

      const log = await McpCallLog.firstOrFail()
      assert.equal(log.outcome, 'error')
      assert.equal(log.errorCategory, 'tool_error')
      assert.equal(log.errorSummary, 'Upstream tool returned an error')
      assert.isTrue(log.argumentsCaptured)
      assert.deepEqual(JSON.parse(log.arguments!), args)
      assert.isFalse(log.responseCaptured)
      assert.isNull(log.response)
      assert.notInclude(log.errorSummary!, 'private tool failure')
    } finally {
      restoreFetch()
    }
  })

  test('captures exact arguments and MCP responses only at responses level', async ({
    client,
    assert,
  }) => {
    const restoreFetch = mockToolServer()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'logging',
        httpUrl: 'https://logging.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, { mcpIds: [mcp.id] })
      const settings = await InstanceSetting.findOrFail(1)
      settings.mcpLogLevel = 'responses'
      await settings.save()
      const args = { query: 'stored request' }

      const response = await callGateway(client, plaintext, 'logging__fail', args)
      response.assertStatus(200)

      const log = await McpCallLog.firstOrFail()
      assert.isTrue(log.argumentsCaptured)
      assert.deepEqual(JSON.parse(log.arguments!), args)
      assert.isTrue(log.responseCaptured)
      assert.deepEqual(JSON.parse(log.response!), {
        content: [{ type: 'text', text: 'private tool failure' }],
        isError: true,
      })
    } finally {
      restoreFetch()
    }
  })

  test('sanitizes upstream exceptions and preserves the gateway response', async ({
    client,
    assert,
  }) => {
    const restoreFetch = mockToolServer()
    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        slug: 'logging',
        httpUrl: 'https://logging.example/mcp',
      })
      const { plaintext } = await createAccessToken(admin.id, { mcpIds: [mcp.id] })

      const response = await callGateway(client, plaintext, 'logging__explode')
      response.assertStatus(200)
      assert.include(response.text(), 'Upstream tool call failed')

      const log = await McpCallLog.firstOrFail()
      assert.equal(log.errorCategory, 'upstream_exception')
      assert.include(log.errorSummary!, 'Bearer [REDACTED]')
      assert.include(log.errorSummary!, 'https://[REDACTED]@example.test')
      assert.notInclude(log.errorSummary!, 'very-secret-token')
      assert.notInclude(log.errorSummary!, 'user:pass')
    } finally {
      restoreFetch()
    }
  })

  test('records invalid and disallowed attempts but never tools/list', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)

    await callGateway(client, plaintext, 'invalid', undefined, {
      headers: { 'x-forwarded-for': '192.0.2.11' },
    })
    await callGateway(client, plaintext, 'missing__tool', undefined, {
      headers: { 'x-forwarded-for': '192.0.2.11' },
    })

    const logs = await McpCallLog.query().orderBy('id', 'asc')
    assert.deepEqual(
      logs.map((log) => log.errorCategory),
      ['invalid_tool', 'disallowed_mcp']
    )
    assert.deepEqual(
      logs.map((log) => log.callerIp),
      ['192.0.2.11', '192.0.2.11']
    )
    assert.equal(logs[1].mcpSlug, 'missing')
    assert.equal(logs.length, 2)
  })

  test('creates no records when capture is off', async ({ client, assert }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const settings = await InstanceSetting.findOrFail(1)
    settings.mcpLogLevel = 'off'
    await settings.save()

    const response = await callGateway(client, plaintext, 'invalid')
    response.assertStatus(200)
    assert.isNull(await McpCallLog.first())
  })

  test('does not alter MCP responses when log persistence fails', async ({ client, assert }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const originalCreate = McpCallLog.create
    McpCallLog.create = (() => {
      throw new Error('database unavailable')
    }) as typeof McpCallLog.create

    try {
      const response = await callGateway(client, plaintext, 'invalid')
      response.assertStatus(200)
      assert.include(response.text(), 'Invalid tool name')
    } finally {
      McpCallLog.create = originalCreate
    }
  })

  test('returns the MCP response before a queued log write completes', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const originalCreate = McpCallLog.create
    let releaseWrite!: () => void
    const blockedWrite = new Promise<void>((resolve) => {
      releaseWrite = resolve
    })

    McpCallLog.create = (async (values, options) => {
      await blockedWrite
      return originalCreate.call(McpCallLog, values, options)
    }) as typeof McpCallLog.create

    try {
      const response = await callGateway(client, plaintext, 'invalid', undefined, { flush: false })
      response.assertStatus(200)
      assert.include(response.text(), 'Invalid tool name')

      releaseWrite()
      await McpCallLogService.flush()
      assert.isNotNull(await McpCallLog.first())
    } finally {
      releaseWrite()
      await McpCallLogService.flush()
      McpCallLog.create = originalCreate
    }
  })
})
