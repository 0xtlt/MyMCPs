import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import Mcp from '#models/mcp'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'
import { createStravaMcp, mockStrava, stravaJson } from '#tests/helpers/strava'

type RpcResponse = {
  result?: {
    tools?: Array<{ name: string }>
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

async function gatewayRpc(
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
  return parseRpcResponse(response)
}

const stravaForm = {
  name: 'Strava',
  description: 'Training log',
  transport: 'builtin',
  builtinKey: 'strava',
  authType: 'auto',
  oauthClientId: '123456',
  oauthClientSecret: 'strava-client-secret',
  enabled: 'on',
}

test.group('Built-in Strava MCP: setup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('creates the MCP with encrypted credentials and waits for authorization', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      const response = await client
        .post('/mcps')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form(stravaForm)

      response.assertStatus(302)
      response.assertFlashMessage('success', 'MCP created')
      const mcp = await Mcp.findByOrFail('slug', 'strava')
      assert.equal(mcp.transport, 'builtin')
      assert.equal(mcp.builtinKey, 'strava')
      assert.equal(mcp.authType, 'auto')
      assert.isNull(mcp.httpUrl)
      assert.equal(mcp.oauthClientId, '123456')
      assert.notInclude(mcp.oauthClientSecret!, 'strava-client-secret')
      assert.equal(McpSecretStore.decrypt(mcp.oauthClientSecret), 'strava-client-secret')
      assert.equal(mcp.status, 'draft')
      assert.isTrue(Boolean(mcp.oauthRequired))
      assert.equal(response.flashMessage('editingMcpId'), mcp.id)
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('asks for both application credentials and a numeric Client ID', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const post = (overrides: Record<string, string>) =>
      client
        .post('/mcps')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form({ ...stravaForm, ...overrides })

    const missing = await post({ oauthClientId: '', oauthClientSecret: '' })
    assert.deepEqual(missing.flashMessage('inputErrorsBag'), {
      oauthClientId: ['Enter the Client ID of your Strava API application'],
      oauthClientSecret: ['Enter the Client Secret of your Strava API application'],
    })

    const swapped = await post({ oauthClientId: 'a1b2c3d4e5' })
    assert.deepEqual(swapped.flashMessage('inputErrorsBag'), {
      oauthClientId: ['The Strava Client ID is a number, such as 123456'],
    })

    const unknown = await post({ builtinKey: 'garmin' })
    assert.property(unknown.flashMessage('inputErrorsBag'), 'builtinKey')

    assert.lengthOf(await Mcp.all(), 0)
  })

  test('shares the callback domain and never the Client Secret with the page', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createStravaMcp(admin.id)

    const response = await client.get('/mcps').loginAs(admin).withInertia()

    response.assertInertiaPropsContains({
      publicApp: { url: 'http://localhost:3333', hostname: 'localhost' },
      mcps: [
        {
          id: mcp.id,
          transport: 'builtin',
          builtinKey: 'strava',
          oauthClientId: '123456',
          hasOauthClientSecret: true,
          hasOauthAccessToken: true,
        },
      ],
    })
    assert.notInclude(response.text(), 'strava-client-secret')
    assert.notInclude(response.text(), 'strava-access-token')
    assert.notInclude(response.text(), mcp.oauthClientSecret!)
  })

  test('keeps the connection when saving without new credentials and drops it when they change', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      const mcp = await createStravaMcp(admin.id)
      const put = (overrides: Record<string, string>) =>
        client
          .put(`/mcps/${mcp.id}`)
          .loginAs(admin)
          .withCsrfToken()
          .redirects(0)
          .form({ ...stravaForm, oauthClientSecret: '', ...overrides })

      const renamed = await put({ name: 'Strava training' })
      renamed.assertFlashMessage('success', 'MCP updated')
      const kept = await Mcp.findOrFail(mcp.id)
      assert.equal(kept.name, 'Strava training')
      assert.equal(McpSecretStore.decrypt(kept.oauthClientSecret), 'strava-client-secret')
      assert.equal(McpSecretStore.decrypt(kept.oauthAccessToken), 'strava-access-token')
      assert.equal(kept.status, 'ready')

      await put({ name: 'Strava training', oauthClientSecret: 'strava-client-secret' })
      const sameSecret = await Mcp.findOrFail(mcp.id)
      assert.isNotNull(sameSecret.oauthAccessToken)

      await put({ name: 'Strava training', oauthClientId: '654321' })
      const disconnected = await Mcp.findOrFail(mcp.id)
      assert.equal(disconnected.oauthClientId, '654321')
      assert.equal(McpSecretStore.decrypt(disconnected.oauthClientSecret), 'strava-client-secret')
      assert.isNull(disconnected.oauthAccessToken)
      assert.isNull(disconnected.oauthRefreshToken)
      assert.isNull(disconnected.oauthScopes)
      assert.equal(disconnected.status, 'draft')
      assert.isTrue(Boolean(disconnected.oauthRequired))
    } finally {
      strava.restore()
    }
  })
})

test.group('Built-in Strava MCP: OAuth', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  async function startAuthorization(client: ApiClient, email: string) {
    const admin = await createAdmin({ email })
    const mcp = await createStravaMcp(admin.id, { connected: false })
    const login = await client
      .post('/login')
      .withCsrfToken()
      .redirects(0)
      .form({ email, password: 'password123' })
    const start = await client
      .get(`/mcps/${mcp.id}/oauth/start`)
      .withSession(login.session())
      .redirects(0)
    start.assertStatus(302)
    return { mcp, start, authorizationUrl: new URL(start.header('location')!) }
  }

  test('sends the admin to Strava and stores the tokens and granted scopes on return', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const { mcp, start, authorizationUrl } = await startAuthorization(client, 'ok@example.com')

      assert.equal(
        authorizationUrl.origin + authorizationUrl.pathname,
        'https://www.strava.com/oauth/authorize'
      )
      assert.equal(authorizationUrl.searchParams.get('client_id'), '123456')
      assert.equal(
        authorizationUrl.searchParams.get('redirect_uri'),
        'http://localhost:3333/mcps/oauth/callback'
      )
      assert.equal(
        authorizationUrl.searchParams.get('scope'),
        'read,read_all,profile:read_all,activity:read_all'
      )
      assert.equal(authorizationUrl.searchParams.get('approval_prompt'), 'force')
      assert.notInclude(authorizationUrl.toString(), 'strava-client-secret')
      assert.lengthOf(strava.requests, 0)

      const state = authorizationUrl.searchParams.get('state')!
      const callback = await client
        .get('/mcps/oauth/callback')
        .qs({ state, code: 'strava-code', scope: 'read,activity:read_all' })
        .withSession(start.session())
        .redirects(0)

      callback.assertStatus(302)
      callback.assertFlashMessage('success', 'OAuth connected')
      assert.equal(new URL(callback.header('location')!, 'http://localhost').pathname, '/mcps')
      assert.equal(new URL(callback.header('location')!, 'http://localhost').search, '')

      const [exchange] = strava.tokenRequests()
      assert.equal(exchange.method, 'POST')
      assert.deepEqual(Object.fromEntries(exchange.form!), {
        client_id: '123456',
        client_secret: 'strava-client-secret',
        grant_type: 'authorization_code',
        code: 'strava-code',
      })
      assert.deepEqual(
        strava.apiRequests().map((request) => [request.url.pathname, request.authorization]),
        [['/api/v3/athlete', 'Bearer strava-access-token']]
      )

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'strava-access-token')
      assert.equal(McpSecretStore.decrypt(saved.oauthRefreshToken), 'strava-refresh-token')
      assert.notInclude(saved.oauthAccessToken!, 'strava-access-token')
      assert.equal(saved.oauthScopes, 'read activity:read_all')
      assert.isNotNull(saved.oauthTokenExpiresAt)
      assert.equal(saved.status, 'ready')
      assert.isFalse(Boolean(saved.oauthRequired))
    } finally {
      strava.restore()
    }
  })

  test('explains rejected application credentials without echoing them', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/oauth/token'
        ? stravaJson(
            {
              message: 'Authorization Error',
              errors: [{ resource: 'Application', field: '', code: 'invalid' }],
            },
            401
          )
        : undefined
    )
    try {
      const { mcp, start, authorizationUrl } = await startAuthorization(client, 'bad@example.com')
      const callback = await client
        .get('/mcps/oauth/callback')
        .qs({ state: authorizationUrl.searchParams.get('state')!, code: 'strava-sensitive-code' })
        .withSession(start.session())
        .redirects(0)

      const error = String(callback.flashMessage('error'))
      assert.include(error, 'Strava rejected the token request (HTTP 401)')
      assert.include(error, 'Check the Client ID and Client Secret')
      assert.notInclude(error, 'strava-client-secret')
      assert.notInclude(error, 'strava-sensitive-code')

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(saved.status, 'error')
      assert.isNull(saved.oauthAccessToken)
      assert.lengthOf(strava.apiRequests(), 0)
    } finally {
      strava.restore()
    }
  })

  test('leaves the MCP disconnected when access is denied on Strava', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const { mcp, start, authorizationUrl } = await startAuthorization(client, 'no@example.com')
      const callback = await client
        .get('/mcps/oauth/callback')
        .qs({ state: authorizationUrl.searchParams.get('state')!, error: 'access_denied' })
        .withSession(start.session())
        .redirects(0)

      callback.assertFlashMessage('error', 'OAuth error: access_denied')
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.oauthAccessToken)
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('rejects a callback whose state was not issued to this session', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const { mcp, start } = await startAuthorization(client, 'forged@example.com')
      const callback = await client
        .get('/mcps/oauth/callback')
        .qs({ state: 'forged-state', code: 'strava-code' })
        .withSession(start.session())
        .redirects(0)

      callback.assertFlashMessage('error', 'Invalid OAuth callback')
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.oauthAccessToken)
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })
})

test.group('Built-in Strava MCP: gateway', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('exposes namespaced Strava tools and runs them through the gateway', async ({
    client,
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      const mcp = await createStravaMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const listed = await gatewayRpc(client, plaintext, 'tools/list', {})
      const names = listed.result!.tools!.map((tool) => tool.name)
      assert.include(names, 'strava__list_activities')
      assert.include(names, 'strava__get_athlete')
      assert.lengthOf(names, 17)
      assert.lengthOf(strava.requests, 0)

      const called = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__list_activities',
        arguments: { per_page: 1 },
      })
      assert.isUndefined(called.result!.isError)
      assert.equal(JSON.parse(called.result!.content![0].text)[0].name, 'Morning Run')

      const failed = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__get_activity',
        arguments: { activity_id: 404 },
      })
      assert.isTrue(failed.result!.isError)
      assert.include(failed.result!.content![0].text, 'Strava could not find this resource')

      await McpCallLogService.flush()
      const logs = await McpCallLog.query().where('mcp_id', mcp.id).orderBy('id', 'asc')
      assert.deepEqual(
        logs.map((log) => [log.toolName, log.outcome, log.errorCategory]),
        [
          ['list_activities', 'success', null],
          ['get_activity', 'error', 'tool_error'],
        ]
      )
    } finally {
      strava.restore()
    }
  })

  test('finds and calls Strava tools in lazy mode', async ({ client, assert }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      await createStravaMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const search = await gatewayRpc(
        client,
        plaintext,
        'tools/call',
        { name: 'tool_search', arguments: { mcp: 'strava', query: 'heart rate zones' } },
        'lazy'
      )
      const found = JSON.parse(search.result!.content![0].text) as {
        tools: Array<{ name: string }>
      }
      assert.include(
        found.tools.map((tool) => tool.name),
        'get_athlete_zones'
      )

      const called = await gatewayRpc(
        client,
        plaintext,
        'tools/call',
        { name: 'call_tool', arguments: { mcp: 'strava', tool: 'get_athlete', arguments: {} } },
        'lazy'
      )
      const athlete = JSON.parse(called.result!.content![0].text)
      assert.equal(athlete.firstname, 'Test')
      assert.notProperty(athlete, 'profile')
    } finally {
      strava.restore()
    }
  })

  test('skips a Strava MCP that is not connected yet', async ({ client, assert }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      await createStravaMcp(admin.id, { connected: false })
      const { plaintext } = await createAccessToken(admin.id)

      const listed = await gatewayRpc(client, plaintext, 'tools/list', {})
      assert.deepEqual(listed.result!.tools, [])

      const called = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__get_athlete',
        arguments: {},
      })
      assert.isTrue(called.result!.isError)
      assert.include(called.result!.content![0].text, 'Strava is not connected')
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })
})
