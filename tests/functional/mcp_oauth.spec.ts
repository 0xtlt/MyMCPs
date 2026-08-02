import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function mockNotionOAuthServer(options: { rejectMcpToken?: boolean } = {}) {
  const originalFetch = globalThis.fetch

  globalThis.fetch = async (input, init) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    const method = init?.method ?? 'GET'
    const body = String(init?.body ?? '')

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
      return jsonResponse({
        access_token: 'access-token',
        token_type: 'bearer',
        expires_in: 3600,
        refresh_token: 'refresh-token',
        scope: 'notion',
      })
    }

    if (
      url.startsWith('https://mcp.notion.com/mcp') &&
      new Headers(init?.headers).get('Authorization') !== 'Bearer access-token'
    ) {
      return new Response(
        JSON.stringify({
          error: 'invalid_token',
          error_description: 'Missing or invalid access token',
        }),
        {
          status: 401,
          headers: {
            'Content-Type': 'application/json',
            'WWW-Authenticate': 'Bearer error="invalid_token"',
          },
        }
      )
    }

    if (url.startsWith('https://mcp.notion.com/mcp') && options.rejectMcpToken) {
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

    if (url.startsWith('https://mcp.notion.com/mcp') && body) {
      const message = JSON.parse(body) as { id?: string | number; method?: string }
      if (message.method === 'initialize') {
        return new Response(
          JSON.stringify({
            jsonrpc: '2.0',
            id: message.id,
            result: {
              protocolVersion: '2025-06-18',
              capabilities: {},
              serverInfo: { name: 'Notion mock', version: '1.0.0' },
            },
          }),
          {
            headers: {
              'Content-Type': 'application/json',
              'Mcp-Session-Id': 'notion-test-session',
            },
          }
        )
      }
      if (message.method === 'tools/list') {
        return new Response(
          JSON.stringify({
            jsonrpc: '2.0',
            id: message.id,
            result: { tools: [] },
          }),
          {
            headers: {
              'Content-Type': 'application/json',
              'Mcp-Session-Id': 'notion-test-session',
            },
          }
        )
      }
      if (message.method === 'notifications/initialized') {
        return new Response(null, {
          status: 202,
          headers: { 'Mcp-Session-Id': 'notion-test-session' },
        })
      }
    }

    return new Response(`not found: ${method} ${url} body=${body}`, { status: 404 })
  }

  return () => {
    globalThis.fetch = originalFetch
  }
}

test.group('MCP OAuth routes', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('uses a full-page redirect and completes the browser callback', async ({
    client,
    assert,
  }) => {
    const restoreFetch = mockNotionOAuthServer()

    try {
      const admin = await createAdmin({ email: 'admin@example.com' })
      const mcp = await createMcp(admin.id, {
        name: 'Notion',
        authType: 'auto',
        oauthRequired: true,
        httpUrl: 'https://mcp.notion.com/mcp',
        status: 'draft',
      })

      const loginResponse = await client
        .post('/login')
        .withCsrfToken()
        .redirects(0)
        .form({ email: 'admin@example.com', password: 'password123' })

      const startResponse = await client
        .get(`/mcps/${mcp.id}/oauth/start`)
        .withSession(loginResponse.session())
        .redirects(0)

      startResponse.assertStatus(302)
      const authorizationUrl = new URL(startResponse.header('location')!)
      assert.equal(authorizationUrl.origin, 'https://auth.example')
      assert.equal(authorizationUrl.pathname, '/authorize')
      assert.equal(authorizationUrl.searchParams.get('client_id'), 'notion-client-123')

      const callbackUrl = new URL(authorizationUrl.searchParams.get('redirect_uri')!)
      callbackUrl.searchParams.set('code', 'authorization-code')
      callbackUrl.searchParams.set('state', authorizationUrl.searchParams.get('state')!)
      const callbackResponse = await client
        .get(`${callbackUrl.pathname}${callbackUrl.search}`)
        .withSession(startResponse.session())
        .redirects(0)

      callbackResponse.assertStatus(302)
      assert.equal(
        new URL(callbackResponse.header('location')!, 'http://localhost').pathname,
        '/mcps'
      )
      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'access-token')
      assert.equal(saved.status, 'ready')
      assert.isFalse(Boolean(saved.oauthRequired))
    } finally {
      restoreFetch()
    }
  })

  test('reports a rejected OAuth token instead of claiming the MCP connected', async ({
    client,
    assert,
  }) => {
    const restoreFetch = mockNotionOAuthServer({ rejectMcpToken: true })

    try {
      const admin = await createAdmin({ email: 'rejected@example.com' })
      const mcp = await createMcp(admin.id, {
        name: 'Rejected Notion token',
        authType: 'auto',
        oauthRequired: true,
        httpUrl: 'https://mcp.notion.com/mcp',
        status: 'draft',
      })

      const loginResponse = await client
        .post('/login')
        .withCsrfToken()
        .redirects(0)
        .form({ email: 'rejected@example.com', password: 'password123' })

      const startResponse = await client
        .get(`/mcps/${mcp.id}/oauth/start`)
        .withSession(loginResponse.session())
        .redirects(0)
      const authorizationUrl = new URL(startResponse.header('location')!)
      const callbackUrl = new URL(authorizationUrl.searchParams.get('redirect_uri')!)
      callbackUrl.searchParams.set('code', 'authorization-code')
      callbackUrl.searchParams.set('state', authorizationUrl.searchParams.get('state')!)

      const callbackResponse = await client
        .get(`${callbackUrl.pathname}${callbackUrl.search}`)
        .withSession(startResponse.session())
        .redirects(0)

      callbackResponse.assertStatus(302)
      callbackResponse.assertFlashMessage(
        'error',
        'OAuth token rejected. MCP server returned HTTP 401. Response: {"error":"invalid_token","error_description":"The access token audience is invalid"} | WWW-Authenticate: Bearer error="invalid_token", error_description="The access token audience is invalid"'
      )
      assert.isUndefined(callbackResponse.flashMessage('success'))

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'access-token')
      assert.equal(saved.status, 'error')
      assert.isTrue(Boolean(saved.oauthRequired))
    } finally {
      restoreFetch()
    }
  })
})
