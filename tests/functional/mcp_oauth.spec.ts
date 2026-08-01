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

function mockNotionOAuthServer() {
  const originalFetch = globalThis.fetch

  globalThis.fetch = async (input, init) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
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
        token_type: 'Bearer',
        expires_in: 3600,
        refresh_token: 'refresh-token',
        scope: 'notion',
      })
    }

    if (url === 'https://mcp.notion.com/mcp' && body) {
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
    }

    return new Response('not found', { status: 404 })
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
      const admin = await createAdmin()
      const mcp = await createMcp(admin.id, {
        name: 'Notion',
        authType: 'oauth',
        httpUrl: 'https://mcp.notion.com/mcp',
        status: 'draft',
      })

      const startResponse = await client
        .get(`/mcps/${mcp.id}/oauth/start`)
        .loginAs(admin)
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
        .loginAs(admin)
        .redirects(0)

      callbackResponse.assertStatus(302)
      assert.equal(
        new URL(callbackResponse.header('location')!, 'http://localhost').pathname,
        '/mcps'
      )
      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'access-token')
      assert.equal(saved.status, 'ready')
    } finally {
      restoreFetch()
    }
  })
})
