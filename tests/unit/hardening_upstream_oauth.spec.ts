import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { addressGuardRuntime, resetAddressGuardRuntime } from '#services/upstream/address_guard'
import {
  exchangeAuthorizationCode,
  readOauthSession,
  refreshOauthAccessToken,
  startOauthFlow,
} from '#services/upstream/oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

const CALLBACK = 'http://localhost:3333/mcps/oauth/callback'

function fakeSession() {
  const values = new Map<string, unknown>()
  const session = {
    all: () => Object.fromEntries(values),
    get: (key: string) => values.get(key),
    put: (key: string, value: unknown) => values.set(key, value),
    forget: (key: string) => values.delete(key),
  } as unknown as HttpContext['session']
  return { session, values }
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function authorizationServer(issuer: string) {
  return {
    issuer,
    authorization_endpoint: `${issuer}/authorize`,
    token_endpoint: `${issuer}/token`,
    registration_endpoint: `${issuer}/register`,
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code', 'refresh_token'],
    token_endpoint_auth_methods_supported: ['none'],
    code_challenge_methods_supported: ['S256'],
  }
}

/**
 * The MCP at mcp.example names `advertised` as its authorization server. Both
 * auth.example and other.example answer as complete providers.
 */
function mockProviders(advertised: string, tokens: Record<string, unknown> = {}) {
  const original = globalThis.fetch
  const calls: Array<{ method: string; url: string; body: string }> = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    const url = new URL(request.url)
    const body = await request.clone().text()
    calls.push({ method: request.method, url: request.url, body })

    if (url.pathname.startsWith('/.well-known/oauth-protected-resource')) {
      return json({ resource: 'https://mcp.example/mcp', authorization_servers: [advertised] })
    }
    if (url.pathname === '/.well-known/oauth-authorization-server') {
      return json(authorizationServer(url.origin))
    }
    if (url.pathname === '/register') {
      return json({
        client_id: `client-of-${url.hostname}`,
        redirect_uris: [CALLBACK],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        token_endpoint_auth_method: 'none',
      })
    }
    if (url.pathname === '/token') {
      return json({ access_token: 'access-new', token_type: 'Bearer', ...tokens })
    }
    return new Response('not found', { status: 404 })
  }
  return {
    calls,
    tokenRequests: () => calls.filter((call) => new URL(call.url).pathname === '/token'),
    restore() {
      globalThis.fetch = original
    },
  }
}

async function connectedMcp(overrides: Partial<Mcp> = {}) {
  const admin = await createAdmin()
  const mcp = await createMcp(admin.id, { authType: 'auto', httpUrl: 'https://mcp.example/mcp' })
  mcp.merge({
    oauthIssuer: 'https://auth.example',
    oauthAuthorizeUrl: 'https://auth.example/authorize',
    oauthTokenUrl: 'https://auth.example/token',
    oauthRedirectUri: CALLBACK,
    oauthClientId: 'client-of-auth.example',
    oauthClientAuthMethod: 'none',
    oauthResource: 'https://mcp.example/mcp',
    oauthAccessToken: McpSecretStore.encrypt('access-old'),
    oauthRefreshToken: McpSecretStore.encrypt('refresh-old'),
    oauthTokenExpiresAt: DateTime.utc().plus({ hours: 1 }),
    oauthTokenType: 'Bearer',
    ...overrides,
  })
  await mcp.save()
  return mcp
}

test.group('Re-authorizing an upstream MCP', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    addressGuardRuntime.lookup = async () => ['203.0.113.10']
  })
  group.each.teardown(resetAddressGuardRuntime)

  test('forgets the tokens of the previous provider when the MCP names another one', async ({
    assert,
  }) => {
    const mcp = await connectedMcp()
    const mock = mockProviders('https://other.example')

    try {
      const redirect = new URL(await startOauthFlow(fakeSession().session, mcp))
      assert.equal(redirect.origin, 'https://other.example')

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(saved.oauthIssuer, 'https://other.example')
      assert.equal(saved.oauthClientId, 'client-of-other.example')
      assert.isNull(saved.oauthAccessToken)
      assert.isNull(saved.oauthRefreshToken)
      assert.isNull(saved.oauthTokenExpiresAt)
      assert.isTrue(Boolean(saved.oauthRequired))

      // The flow may be abandoned here. Nothing is left to send to the new provider.
      await refreshOauthAccessToken(saved)
      assert.lengthOf(mock.tokenRequests(), 0)
      assert.isFalse(mock.calls.some((call) => call.body.includes('refresh-old')))
    } finally {
      mock.restore()
    }
  })

  test('forgets them too when only an inferred provider was saved', async ({ assert }) => {
    // Rows from before the issuer was stored: it is inferred from the endpoints.
    const mcp = await connectedMcp({
      oauthIssuer: null,
      oauthAuthorizeUrl: 'https://legacy.example/authorize',
      oauthTokenUrl: 'https://legacy.example/token',
    })
    const mock = mockProviders('https://auth.example')

    try {
      await startOauthFlow(fakeSession().session, mcp)

      assert.equal(mcp.oauthIssuer, 'https://auth.example')
      assert.equal(mcp.oauthClientId, 'client-of-auth.example')
      assert.isNull(mcp.oauthRefreshToken)
      assert.isNull(mcp.oauthAccessToken)
    } finally {
      mock.restore()
    }
  })

  test('forgets them when a new client had to be registered with the same provider', async ({
    assert,
  }) => {
    const mcp = await connectedMcp({ oauthRedirectUri: 'https://old-gateway.example/callback' })
    const mock = mockProviders('https://auth.example')

    try {
      await startOauthFlow(fakeSession().session, mcp)

      assert.isTrue(mock.calls.some((call) => call.url === 'https://auth.example/register'))
      assert.isNull(mcp.oauthRefreshToken)
      assert.isNull(mcp.oauthAccessToken)
      assert.isTrue(mcp.oauthRequired)
    } finally {
      mock.restore()
    }
  })

  test('keeps the connection while the same provider and client are authorized again', async ({
    assert,
  }) => {
    const mcp = await connectedMcp()
    const mock = mockProviders('https://auth.example')

    try {
      const redirect = new URL(await startOauthFlow(fakeSession().session, mcp))

      assert.equal(redirect.searchParams.get('client_id'), 'client-of-auth.example')
      assert.isFalse(mock.calls.some((call) => call.method === 'POST'))
      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'access-old')
      assert.equal(McpSecretStore.decrypt(saved.oauthRefreshToken), 'refresh-old')
      assert.isFalse(Boolean(saved.oauthRequired))
    } finally {
      mock.restore()
    }
  })
})

test.group('Upstream OAuth tokens without a stated lifetime', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    addressGuardRuntime.lookup = async () => ['203.0.113.10']
  })
  group.each.teardown(resetAddressGuardRuntime)

  function assertAboutAnHourAway(
    assert: { isTrue(value: boolean): void },
    expiry: DateTime | null
  ) {
    const minutes = expiry?.diff(DateTime.utc(), 'minutes').minutes ?? Number.NaN
    assert.isTrue(minutes > 58 && minutes <= 60)
  }

  test('are given an hour instead of being refreshed on every connection', async ({ assert }) => {
    const mcp = await connectedMcp({ oauthTokenExpiresAt: DateTime.utc().minus({ minutes: 1 }) })
    // No `expires_in` in the token responses.
    const mock = mockProviders('https://auth.example', { refresh_token: 'refresh-new' })

    try {
      await refreshOauthAccessToken(mcp)
      assert.lengthOf(mock.tokenRequests(), 1)
      assert.equal(McpSecretStore.decrypt(mcp.oauthAccessToken), 'access-new')
      assertAboutAnHourAway(assert, mcp.oauthTokenExpiresAt)

      // Every later connection asks for a refresh first.
      await refreshOauthAccessToken(mcp)
      await refreshOauthAccessToken(await Mcp.findOrFail(mcp.id))
      assert.lengthOf(mock.tokenRequests(), 1)
    } finally {
      mock.restore()
    }
  })

  test('are given the same hour when first issued', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      authType: 'auto',
      httpUrl: 'https://mcp.example/mcp',
      status: 'draft',
    })
    const mock = mockProviders('https://auth.example', { refresh_token: 'refresh-new' })
    const { session } = fakeSession()

    try {
      const redirect = new URL(await startOauthFlow(session, mcp))
      const oauth = await readOauthSession(session, redirect.searchParams.get('state')!)
      await exchangeAuthorizationCode(mcp, oauth!, 'authorization-code')

      assertAboutAnHourAway(assert, mcp.oauthTokenExpiresAt)
      await refreshOauthAccessToken(mcp)
      assert.isFalse(mock.tokenRequests().some((call) => call.body.includes('refresh_token')))
    } finally {
      mock.restore()
    }
  })

  test('keep the lifetime the provider states', async ({ assert }) => {
    const mcp = await connectedMcp({ oauthTokenExpiresAt: DateTime.utc().minus({ minutes: 1 }) })
    const mock = mockProviders('https://auth.example', { expires_in: 600 })

    try {
      await refreshOauthAccessToken(mcp)
      const minutes = mcp.oauthTokenExpiresAt!.diff(DateTime.utc(), 'minutes').minutes
      assert.isTrue(minutes > 9 && minutes <= 10)
    } finally {
      mock.restore()
    }
  })
})

test.group('Pending upstream OAuth authorizations', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    addressGuardRuntime.lookup = async () => ['203.0.113.10']
  })
  group.each.teardown(resetAddressGuardRuntime)

  test('keeps the five most recent starts of a session and nothing else of theirs', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      authType: 'auto',
      httpUrl: 'https://mcp.example/mcp',
      status: 'draft',
    })
    const mock = mockProviders('https://auth.example')
    const { session, values } = fakeSession()
    values.set('auth_web', 42)

    try {
      const states: string[] = []
      for (let start = 0; start < 8; start++) {
        const redirect = new URL(await startOauthFlow(session, mcp))
        states.push(redirect.searchParams.get('state')!)
      }

      const pending = [...values.keys()].filter((key) => key.startsWith('mcp_oauth:'))
      // At most five, and fewer when five would not fit in the session cookie.
      assert.isAtMost(pending.length, 5)
      assert.isAbove(pending.length, 1)
      assert.deepEqual(
        pending,
        states.slice(-pending.length).map((state) => `mcp_oauth:${state}`)
      )
      for (const abandoned of states.slice(0, 3)) {
        assert.isNull(await readOauthSession(session, abandoned))
      }
      assert.isNotNull(await readOauthSession(session, states.at(-1)))
      assert.equal(values.get('auth_web'), 42)
    } finally {
      mock.restore()
    }
  })
})
