import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import logger from '@adonisjs/core/services/logger'
import limiter from '@adonisjs/limiter/services/main'
import InstanceSetting from '#models/instance_setting'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { GATEWAY_REQUESTS_PER_MINUTE, gatewayRateLimiter } from '#services/gateway_rate_limiter'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createMcp,
  createStoredAccessToken,
} from '#tests/helpers/factories'

function jsonRpcResponse(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json', 'Mcp-Session-Id': 'hardening-session' },
  })
}

/** Stand in for every HTTP upstream and record which ones the gateway contacts. */
function mockUpstreams() {
  const originalFetch = globalThis.fetch
  const requests: Array<{ host: string; method: string }> = []

  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    const rawBody = await request.clone().text()
    const message = rawBody ? (JSON.parse(rawBody) as { id?: number; method?: string }) : {}
    requests.push({ host: new URL(request.url).host, method: message.method ?? request.method })

    if (request.method === 'DELETE') return new Response(null, { status: 200 })
    if (message.method === 'initialize') {
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          protocolVersion: '2025-06-18',
          capabilities: { tools: {} },
          serverInfo: { name: 'Hardening upstream', version: '1.0.0' },
        },
      })
    }
    if (message.method === 'notifications/initialized') {
      return new Response(null, { status: 202 })
    }
    if (message.method === 'tools/list') {
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: { tools: [{ name: 'echo', inputSchema: { type: 'object' } }] },
      })
    }
    if (message.method === 'tools/call') {
      return jsonRpcResponse({
        jsonrpc: '2.0',
        id: message.id,
        result: { content: [{ type: 'text', text: 'ok' }] },
      })
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

function gatewayRpc(
  client: ApiClient,
  plaintext: string,
  body: Record<string, unknown>,
  headers: Record<string, string> = {}
) {
  const request = client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')
  for (const [name, value] of Object.entries(headers)) {
    request.header(name, value)
  }
  return request.json({ jsonrpc: '2.0', ...body })
}

function toolCall(name: string, args?: Record<string, unknown>) {
  return {
    id: 7,
    method: 'tools/call',
    params: { name, ...(args === undefined ? {} : { arguments: args }) },
  }
}

const initialize = {
  id: 1,
  method: 'initialize',
  params: {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'hardening-test', version: '1.0.0' },
  },
}

test.group('hardening: gateway requests', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(async () => {
    await McpCallLogService.flush()
    await limiter.clear(['memory'])
    await rollbackTestTransaction()
  })

  test('contacts upstreams in eager mode only to list tools or to call one', async ({
    client,
    assert,
  }) => {
    const mock = mockUpstreams()
    try {
      const admin = await createAdmin()
      await createMcp(admin.id, { slug: 'issues', httpUrl: 'https://issues.example/mcp' })
      await createMcp(admin.id, { slug: 'calendar', httpUrl: 'https://calendar.example/mcp' })
      const { plaintext } = await createAccessToken(admin.id)

      const initialized = await gatewayRpc(client, plaintext, initialize)
      initialized.assertStatus(200)
      const notified = await gatewayRpc(client, plaintext, { method: 'notifications/initialized' })
      notified.assertStatus(202)
      const pinged = await gatewayRpc(client, plaintext, { id: 2, method: 'ping' })
      pinged.assertStatus(200)
      assert.lengthOf(mock.requests, 0)

      const called = await gatewayRpc(client, plaintext, toolCall('issues__echo'))
      called.assertStatus(200)
      assert.include(called.text(), 'ok')
      assert.isTrue(mock.requests.some((request) => request.method === 'tools/call'))
      assert.isFalse(mock.requests.some((request) => request.method === 'tools/list'))
      assert.isTrue(mock.requests.every((request) => request.host === 'issues.example'))

      mock.requests.length = 0
      const listed = await gatewayRpc(client, plaintext, { id: 3, method: 'tools/list' })
      listed.assertStatus(200)
      assert.include(listed.text(), 'issues__echo')
      assert.include(listed.text(), 'calendar__echo')
      assert.deepEqual([...new Set(mock.requests.map((request) => request.host))].sort(), [
        'calendar.example',
        'issues.example',
      ])
    } finally {
      mock.restore()
    }
  })

  test('limits the requests of one access token without affecting another', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const busy = await createAccessToken(admin.id)
    const other = await createAccessToken(admin.id)
    await gatewayRateLimiter.set(
      `mcp:${busy.token.id}`,
      GATEWAY_REQUESTS_PER_MINUTE - 1,
      '1 minute'
    )

    const last = await gatewayRpc(client, busy.plaintext, initialize)
    last.assertStatus(200)

    const refused = await gatewayRpc(client, busy.plaintext, initialize)
    refused.assertStatus(429)
    refused.assertBody({
      error: 'rate_limited',
      message: 'Too many requests for this access token',
    })
    const retryAfter = Number(refused.header('retry-after'))
    assert.isAbove(retryAfter, 0)
    assert.isAtMost(retryAfter, 60)

    const unaffected = await gatewayRpc(client, other.plaintext, initialize)
    unaffected.assertStatus(200)
  })
})

test.group('hardening: MCP call log', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(async () => {
    await McpCallLogService.flush()
    await rollbackTestTransaction()
  })

  test('stores only a well-formed slug and bounded names for refused calls', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const forwardedFor = { 'x-forwarded-for': 'a'.repeat(300) }

    await gatewayRpc(client, plaintext, toolCall(`${'x'.repeat(4000)}__tool`), forwardedFor)
    await gatewayRpc(client, plaintext, toolCall('Not A Slug__tool'), forwardedFor)
    await gatewayRpc(client, plaintext, toolCall('missing-mcp__tool'), forwardedFor)
    await gatewayRpc(
      client,
      plaintext,
      toolCall('call_tool', { mcp: 'Not A Slug', tool: 'tool' }),
      { ...forwardedFor, 'x-mymcps-tool-mode': 'lazy' }
    )
    await McpCallLogService.flush()

    const logs = await McpCallLog.query().orderBy('id', 'asc')
    assert.deepEqual(
      logs.map((log) => log.errorCategory),
      ['disallowed_mcp', 'disallowed_mcp', 'disallowed_mcp', 'disallowed_mcp']
    )
    assert.deepEqual(
      logs.map((log) => log.mcpSlug),
      [null, null, 'missing-mcp', null]
    )
    assert.lengthOf(logs[0].requestedToolName, 512)
    for (const log of logs) {
      assert.isAtMost(log.callerIp?.length ?? 0, 64)
    }
  })

  test('builds the MCP filter of the Logs page from the MCPs that exist', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id)
    await createMcp(admin.id, { name: 'Search MCP', slug: 'search' })
    await McpCallLog.create({
      accessTokenId: token.id,
      accessTokenName: token.name,
      accessTokenPrefix: token.tokenPrefix,
      mcpSlug: 'made-up-by-a-caller',
      requestedToolName: 'made-up-by-a-caller__tool',
      toolName: 'tool',
      outcome: 'error',
      errorCategory: 'disallowed_mcp',
      argumentsCaptured: false,
      responseCaptured: false,
      durationMs: 1,
    })

    const page = await client.get('/logs?range=all').withInertia().loginAs(admin)

    page.assertStatus(200)
    page.assertInertiaPropsContains({ pagination: { total: 1 } })
    assert.deepEqual(page.inertiaProps.options.mcps, [{ value: 'search', label: 'Search MCP' }])
  })

  test('logs a failed log write without the statement or its values', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { plaintext } = await createAccessToken(admin.id)
    const settings = await InstanceSetting.findOrFail(1)
    settings.mcpLogLevel = 'arguments'
    await settings.save()

    const warnings: unknown[][] = []
    const originalWarn = logger.warn
    const originalCreate = McpCallLog.create
    logger.warn = ((...entry: unknown[]) => {
      warnings.push(entry)
    }) as typeof logger.warn
    McpCallLog.create = ((values: { arguments: string }) => {
      // Knex puts the bound values in the message of the errors it throws.
      throw Object.assign(
        new Error(
          `insert into \`mcp_call_logs\` (\`arguments\`) values ('${values.arguments}') - SQLITE_FULL: database or disk is full`
        ),
        { code: 'SQLITE_FULL' }
      )
    }) as unknown as typeof McpCallLog.create

    try {
      const response = await gatewayRpc(
        client,
        plaintext,
        toolCall('invalid', { password: 'captured-argument-secret' })
      )
      response.assertStatus(200)
      await McpCallLogService.flush()
    } finally {
      logger.warn = originalWarn
      McpCallLog.create = originalCreate
    }

    assert.lengthOf(warnings, 1)
    const [fields, message] = warnings[0] as [Record<string, unknown>, string]
    assert.equal(message, 'MCP call log could not be persisted')
    assert.notProperty(fields, 'err')
    assert.equal(fields.code, 'SQLITE_FULL')
    assert.include(fields.error as string, '[REDACTED]')
    assert.notInclude(JSON.stringify(fields), 'captured-argument-secret')
  })
})

test.group('hardening: protocol endpoint CORS', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('answers other origins without credentials and without echoing them', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const origin = 'https://agent.example'

    const preflight = await client
      .options('/mcp')
      .header('origin', origin)
      .header('access-control-request-method', 'POST')
      .header('access-control-request-headers', 'authorization,content-type')
    preflight.assertStatus(204)
    assert.equal(preflight.header('access-control-allow-origin'), '*')
    assert.isUndefined(preflight.header('access-control-allow-credentials'))
    assert.include(preflight.header('access-control-allow-headers'), 'authorization')

    for (const path of ['/mcp', '/.well-known/oauth-authorization-server']) {
      const response = await client.get(path).header('origin', origin)
      assert.equal(response.header('access-control-allow-origin'), '*')
      assert.isUndefined(response.header('access-control-allow-credentials'))
    }

    const sessionPage = await client.get('/login').header('origin', origin)
    assert.isUndefined(sessionPage.header('access-control-allow-origin'))
    assert.isUndefined(sessionPage.header('access-control-allow-credentials'))
  })
})
