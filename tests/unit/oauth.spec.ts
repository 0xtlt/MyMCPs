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
    const request = new Request(input, init)
    const url = request.url
    const method = request.method
    const body = await request.clone().text()
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
        token_type: 'bearer',
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

      const registration = mock.calls.find((call) => call.url === 'https://auth.example/register')
      assert.equal(JSON.parse(registration!.body).client_name, 'MyMCPs')

      clearOauthSession(session, state ?? undefined)
      assert.isNull(await readOauthSession(session, state ?? undefined))
    } finally {
      mock.restore()
    }
  })

  test('registers the Figma MCP with an allowlisted client name and loopback redirect', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    const requests: Array<{ url: string; body: string; authorization: string | null }> = []

    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init)
      requests.push({
        url: request.url,
        body: await request.clone().text(),
        authorization: request.headers.get('Authorization'),
      })

      if (request.url.includes('/.well-known/oauth-protected-resource')) {
        return jsonResponse({
          resource: 'https://mcp.figma.com/mcp',
          authorization_servers: ['https://api.figma.com'],
          scopes_supported: ['mcp:connect'],
        })
      }
      if (request.url === 'https://api.figma.com/.well-known/oauth-authorization-server') {
        return jsonResponse({
          issuer: 'https://api.figma.com',
          authorization_endpoint: 'https://www.figma.com/oauth/mcp',
          token_endpoint: 'https://api.figma.com/v1/oauth/token',
          registration_endpoint: 'https://api.figma.com/v1/oauth/mcp/register',
          response_types_supported: ['code'],
          grant_types_supported: ['authorization_code', 'refresh_token'],
          token_endpoint_auth_methods_supported: ['client_secret_basic', 'client_secret_post'],
          code_challenge_methods_supported: ['S256'],
          scopes_supported: ['mcp:connect'],
        })
      }
      if (request.url === 'https://api.figma.com/v1/oauth/mcp/register') {
        return jsonResponse({
          client_id: 'figma-client-123',
          client_secret: 'figma-client-secret',
          redirect_uris: ['http://localhost:45873/callback'],
          grant_types: ['authorization_code', 'refresh_token'],
          response_types: ['code'],
          token_endpoint_auth_method: 'none',
          client_name: 'Codex',
        })
      }
      if (request.url === 'https://api.figma.com/v1/oauth/token') {
        return jsonResponse({
          access_token: 'figma-access-token',
          token_type: 'bearer',
          expires_in: 3600,
          refresh_token: 'figma-refresh-token',
        })
      }

      return new Response('not found', { status: 404 })
    }

    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Figma',
        authType: 'auto',
        httpUrl: 'https://mcp.figma.com/mcp',
        status: 'draft',
      })
      const { session } = fakeSession()

      const redirect = new URL(await startOauthFlow(session, mcp))
      const registration = requests.find(
        (request) => request.url === 'https://api.figma.com/v1/oauth/mcp/register'
      )

      const registered = JSON.parse(registration!.body)
      assert.equal(registered.client_name, 'Codex')
      assert.deepEqual(registered.redirect_uris, ['http://localhost:45873/callback'])
      assert.equal(redirect.origin, 'https://www.figma.com')
      assert.equal(redirect.pathname, '/oauth/mcp')
      assert.equal(redirect.searchParams.get('client_id'), 'figma-client-123')
      assert.equal(redirect.searchParams.get('redirect_uri'), 'http://localhost:45873/callback')
      assert.equal(mcp.oauthRedirectUri, 'http://localhost:45873/callback')
      assert.equal(McpSecretStore.decrypt(mcp.oauthClientSecret), 'figma-client-secret')

      const oauth = await readOauthSession(session, redirect.searchParams.get('state') ?? undefined)
      await exchangeAuthorizationCode(mcp, oauth!, 'authorization-code')

      const tokenRequest = requests.find(
        (request) => request.url === 'https://api.figma.com/v1/oauth/token'
      )
      assert.equal(
        tokenRequest!.authorization,
        `Basic ${Buffer.from('figma-client-123:figma-client-secret').toString('base64')}`
      )
      assert.include(tokenRequest!.body, 'redirect_uri=http%3A%2F%2Flocalhost%3A45873%2Fcallback')
      assert.equal(McpSecretStore.decrypt(mcp.oauthAccessToken), 'figma-access-token')
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('verifies a same-origin path issuer discovered through legacy root metadata', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    const calls: string[] = []
    const metadata = {
      issuer: 'https://mcp.example/mcp',
      authorization_endpoint: 'https://mcp.example/mcp/authorize',
      token_endpoint: 'https://mcp.example/mcp/token',
      registration_endpoint: 'https://mcp.example/mcp/register',
      response_types_supported: ['code'],
      grant_types_supported: ['authorization_code', 'refresh_token'],
      token_endpoint_auth_methods_supported: ['none'],
      code_challenge_methods_supported: ['S256'],
    }

    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init)
      calls.push(request.url)

      if (request.url.includes('/.well-known/oauth-protected-resource')) {
        return new Response('', { status: 404 })
      }
      if (
        request.url === 'https://mcp.example/.well-known/oauth-authorization-server' ||
        request.url === 'https://mcp.example/.well-known/oauth-authorization-server/mcp'
      ) {
        return jsonResponse(metadata)
      }
      if (request.url === 'https://mcp.example/mcp/register') {
        return jsonResponse({
          client_id: 'path-client-123',
          redirect_uris: ['http://localhost:3333/mcps/oauth/callback'],
          grant_types: ['authorization_code', 'refresh_token'],
          response_types: ['code'],
          token_endpoint_auth_method: 'none',
          client_name: 'MyMCPs',
        })
      }

      return new Response('not found', { status: 404 })
    }

    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Path issuer',
        authType: 'auto',
        httpUrl: 'https://mcp.example/mcp',
        status: 'draft',
      })

      const redirect = new URL(await startOauthFlow(fakeSession().session, mcp))

      assert.equal(redirect.origin, 'https://mcp.example')
      assert.equal(redirect.pathname, '/mcp/authorize')
      assert.equal(mcp.oauthIssuer, 'https://mcp.example/mcp')
      assert.include(calls, 'https://mcp.example/.well-known/oauth-authorization-server/mcp')
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('rejects a cross-origin issuer from legacy root metadata without requesting it', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    const calls: string[] = []

    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init)
      calls.push(request.url)

      if (request.url.includes('/.well-known/oauth-protected-resource')) {
        return new Response('', { status: 404 })
      }
      if (request.url === 'https://mcp.example/.well-known/oauth-authorization-server') {
        return jsonResponse({
          issuer: 'https://untrusted.example/oauth',
          authorization_endpoint: 'https://untrusted.example/oauth/authorize',
          token_endpoint: 'https://untrusted.example/oauth/token',
          registration_endpoint: 'https://untrusted.example/oauth/register',
          response_types_supported: ['code'],
          code_challenge_methods_supported: ['S256'],
        })
      }

      return new Response('not found', { status: 404 })
    }

    try {
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Cross-origin issuer',
        authType: 'auto',
        httpUrl: 'https://mcp.example/mcp',
        status: 'draft',
      })

      await assert.rejects(
        () => startOauthFlow(fakeSession().session, mcp),
        'OAuth issuer metadata does not match the authorization server'
      )
      assert.isFalse(calls.some((url) => url.startsWith('https://untrusted.example/')))
    } finally {
      globalThis.fetch = originalFetch
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
    const authorizationHeaders: Array<string | null> = []
    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init)
      authorizationHeaders.push(request.headers.get('Authorization'))
      return new Response(
        JSON.stringify({
          error: 'invalid_token',
          error_description: 'The access token audience is invalid',
        }),
        {
          status: 401,
          headers: {
            'Content-Type': 'application/json',
            'WWW-Authenticate':
              'Bearer error="invalid_token", error_description="The access token audience is invalid"',
          },
        }
      )
    }

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
      rejectedToken.oauthTokenType = 'bearer'
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
      assert.include(rejectedToken.lastError!, 'The access token audience is invalid')
      assert.notInclude(rejectedToken.lastError!, 'rejected-access-token')
      assert.include(authorizationHeaders, 'Bearer rejected-access-token')
      assert.notInclude(authorizationHeaders, 'bearer rejected-access-token')
      assert.isTrue(rejectedToken.oauthRequired)
      assert.equal(manual.status, 'error')
      assert.isFalse(manual.oauthRequired)
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})
