import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import McpSecretStore from '#services/mcp_secret_store'
import { testAndUpdateStatus } from '#services/upstream/manager'
import {
  clearOauthSession,
  exchangeAuthorizationCode,
  readOauthSession,
  refreshOauthAccessToken,
  startOauthFlow,
} from '#services/upstream/oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function fakeSession() {
  const values = new Map<string, unknown>()
  const session = {
    get(key: string) {
      return values.get(key)
    },
    put(key: string, value: unknown) {
      values.set(key, value)
    },
    forget(key: string) {
      values.delete(key)
    },
  } as unknown as HttpContext['session']

  return { session, values }
}

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function mockNotionOAuthServer() {
  const originalFetch = globalThis.fetch
  const calls: Array<{ url: string; method: string; body: string }> = []

  globalThis.fetch = async (input, init) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    const method = init?.method ?? 'GET'
    const body = String(init?.body ?? '')
    calls.push({ url, method, body })

    if (url.includes('/.well-known/oauth-protected-resource')) {
      return jsonResponse({
        resource: 'https://mcp.notion.com/mcp',
        authorization_servers: ['https://auth.example'],
        scopes_supported: ['notion'],
      })
    }

    if (url === 'https://auth.example/.well-known/oauth-authorization-server') {
      return jsonResponse({
        issuer: 'https://auth.example',
        authorization_endpoint: 'https://auth.example/authorize',
        token_endpoint: 'https://auth.example/token',
        registration_endpoint: 'https://auth.example/register',
        response_types_supported: ['code'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
        token_endpoint_auth_methods_supported: ['none'],
        code_challenge_methods_supported: ['S256'],
      })
    }

    if (url === 'https://auth.example/register') {
      return jsonResponse({
        client_id: 'notion-client-123',
        redirect_uris: ['http://localhost:3333/mcps/oauth/callback'],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        token_endpoint_auth_method: 'none',
        client_name: 'MyMCPs',
      })
    }

    if (url === 'https://auth.example/token') {
      if (body.includes('grant_type=refresh_token')) {
        return jsonResponse({
          access_token: 'refreshed-access-token',
          token_type: 'Bearer',
          expires_in: 3600,
          refresh_token: 'refreshed-refresh-token',
        })
      }

      return jsonResponse({
        access_token: 'access-token',
        token_type: 'Bearer',
        expires_in: 3600,
        refresh_token: 'refresh-token',
        scope: 'notion',
      })
    }

    return new Response('not found', { status: 404 })
  }

  return {
    calls,
    restore() {
      globalThis.fetch = originalFetch
    },
  }
}

test.group('MCP OAuth', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('discovers, registers, redirects, exchanges, and refreshes a Notion-style MCP', async ({
    assert,
  }) => {
    const mock = mockNotionOAuthServer()

    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Notion',
        authType: 'auto',
        httpUrl: 'https://mcp.notion.com/mcp',
        status: 'draft',
      })
      const { session, values } = fakeSession()

      const redirect = await startOauthFlow(session, mcp)
      const authorizationUrl = new URL(redirect)
      const state = authorizationUrl.searchParams.get('state')
      const oauth = await readOauthSession(session, state ?? undefined)

      assert.equal(authorizationUrl.origin, 'https://auth.example')
      assert.equal(authorizationUrl.pathname, '/authorize')
      assert.equal(authorizationUrl.searchParams.get('client_id'), 'notion-client-123')
      assert.equal(
        authorizationUrl.searchParams.get('redirect_uri'),
        'http://localhost:3333/mcps/oauth/callback'
      )
      assert.equal(authorizationUrl.searchParams.get('resource'), 'https://mcp.notion.com/mcp')
      assert.equal(authorizationUrl.searchParams.get('code_challenge_method'), 'S256')
      assert.isNotNull(oauth)
      assert.equal(oauth?.redirectUri, 'http://localhost:3333/mcps/oauth/callback')
      assert.equal(mcp.oauthIssuer, 'https://auth.example')
      assert.equal(mcp.oauthResource, 'https://mcp.notion.com/mcp')
      assert.equal(mcp.oauthClientId, 'notion-client-123')
      assert.equal(mcp.oauthClientAuthMethod, 'none')
      assert.isTrue(values.has(`mcp_oauth:${state}`))

      await exchangeAuthorizationCode(mcp, oauth!, 'authorization-code')

      assert.equal(McpSecretStore.decrypt(mcp.oauthAccessToken), 'access-token')
      assert.equal(McpSecretStore.decrypt(mcp.oauthRefreshToken), 'refresh-token')
      assert.equal(mcp.oauthTokenType, 'Bearer')
      assert.equal(mcp.oauthScopes, 'notion')
      assert.isFalse(mcp.oauthRequired)
      assert.isNotNull(mcp.oauthTokenExpiresAt)

      mcp.oauthTokenExpiresAt = DateTime.utc().minus({ minutes: 1 })
      await mcp.save()
      await refreshOauthAccessToken(mcp)

      assert.equal(McpSecretStore.decrypt(mcp.oauthAccessToken), 'refreshed-access-token')
      assert.equal(McpSecretStore.decrypt(mcp.oauthRefreshToken), 'refreshed-refresh-token')

      const tokenBodies = mock.calls
        .filter((call) => call.url === 'https://auth.example/token')
        .map((call) => call.body)
      assert.lengthOf(tokenBodies, 2)
      assert.include(
        tokenBodies[0],
        'redirect_uri=http%3A%2F%2Flocalhost%3A3333%2Fmcps%2Foauth%2Fcallback'
      )
      assert.include(tokenBodies[1], 'grant_type=refresh_token')

      clearOauthSession(session, state ?? undefined)
      assert.isNull(await readOauthSession(session, state ?? undefined))
    } finally {
      mock.restore()
    }
  })

  test('reports an actionable error when an MCP has no OAuth metadata or client registration', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = async () => new Response('', { status: 404 })

    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Undiscoverable',
        authType: 'auto',
        httpUrl: 'https://mcp.example/mcp',
        status: 'draft',
      })

      await assert.rejects(
        () => startOauthFlow(fakeSession().session, mcp),
        'OAuth provider metadata could not be discovered'
      )
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})

test.group('MCP automatic authentication', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('marks only Auto HTTP MCPs as requiring OAuth after an unauthorized response', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = async () =>
      new Response('', {
        status: 401,
        headers: {
          'WWW-Authenticate':
            'Bearer resource_metadata="https://mcp.example/.well-known/oauth-protected-resource"',
        },
      })

    try {
      const admin = await createAdmin()
      const automatic = await createMcp(admin.id, {
        name: 'Automatic auth',
        authType: 'auto',
        status: 'draft',
      })
      const rejectedToken = await createMcp(admin.id, {
        name: 'Rejected OAuth token',
        authType: 'auto',
        status: 'ready',
      })
      rejectedToken.oauthAccessToken = McpSecretStore.encrypt('rejected-access-token')
      await rejectedToken.save()
      const manual = await createMcp(admin.id, {
        name: 'Manual bearer',
        authType: 'bearer',
        status: 'draft',
      })

      await testAndUpdateStatus(automatic)
      await testAndUpdateStatus(rejectedToken)
      await testAndUpdateStatus(manual)

      assert.equal(automatic.status, 'draft')
      assert.equal(automatic.lastError, 'OAuth authorization required')
      assert.isTrue(automatic.oauthRequired)
      assert.equal(rejectedToken.status, 'error')
      assert.equal(
        rejectedToken.lastError,
        'OAuth authorization was rejected. Re-authorize this MCP.'
      )
      assert.isTrue(rejectedToken.oauthRequired)
      assert.equal(manual.status, 'error')
      assert.isFalse(manual.oauthRequired)
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})
