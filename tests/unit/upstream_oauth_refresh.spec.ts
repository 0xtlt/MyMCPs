import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { refreshOauthAccessToken } from '#services/upstream/oauth'
import { connectHttpUpstream } from '#services/upstream/http_client'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

async function connection() {
  const admin = await createAdmin()
  const mcp = await createMcp(admin.id, { authType: 'auto' })
  mcp.oauthIssuer = 'https://oauth.example'
  mcp.oauthAuthorizeUrl = 'https://oauth.example/authorize'
  mcp.oauthTokenUrl = 'https://oauth.example/token'
  mcp.oauthClientId = 'client-test'
  mcp.oauthClientAuthMethod = 'none'
  mcp.oauthResource = 'https://mcp.example/mcp'
  mcp.oauthAccessToken = McpSecretStore.encrypt('access-old')
  mcp.oauthRefreshToken = McpSecretStore.encrypt('refresh-old')
  mcp.oauthTokenExpiresAt = DateTime.utc().minus({ minutes: 1 })
  await mcp.save()
  return mcp
}

function oauthServer(token: (body: URLSearchParams) => Promise<Response>) {
  const original = globalThis.fetch
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    if (request.url === 'https://oauth.example/.well-known/oauth-authorization-server') {
      return json({
        issuer: 'https://oauth.example',
        authorization_endpoint: 'https://oauth.example/authorize',
        token_endpoint: 'https://oauth.example/token',
        response_types_supported: ['code'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
        token_endpoint_auth_methods_supported: ['none'],
      })
    }
    if (request.url === 'https://oauth.example/token') {
      return token(new URLSearchParams(await request.text()))
    }
    throw new Error(`Unexpected test request: ${request.url}`)
  }
  return () => {
    globalThis.fetch = original
  }
}

test.group('Upstream OAuth refresh concurrency', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('uses the reloaded endpoint with the reloaded credentials', async ({ assert }) => {
    const mcp = await connection()
    const stale = await Mcp.findOrFail(mcp.id)
    mcp.httpUrl = 'https://new-mcp.example/mcp'
    mcp.oauthAccessToken = McpSecretStore.encrypt('new-endpoint-access')
    mcp.oauthTokenExpiresAt = DateTime.utc().plus({ minutes: 10 })
    await mcp.save()
    const original = globalThis.fetch
    const destinations: string[] = []
    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init)
      destinations.push(request.url)
      assert.equal(request.url, 'https://new-mcp.example/mcp')
      assert.equal(request.headers.get('Authorization'), 'Bearer new-endpoint-access')
      if (request.method !== 'POST') return new Response(null, { status: 405 })
      const message = (await request.json()) as {
        id?: number
        method: string
        params?: { protocolVersion: string }
      }
      if (message.method === 'initialize')
        return json({
          jsonrpc: '2.0',
          id: message.id,
          result: {
            protocolVersion: message.params?.protocolVersion,
            capabilities: {},
            serverInfo: { name: 'test', version: '1.0' },
          },
        })
      return new Response(null, { status: 202 })
    }
    try {
      const connected = await connectHttpUpstream(stale)
      await connected.close()
      assert.isAbove(destinations.length, 0)
    } finally {
      globalThis.fetch = original
    }
  })

  test('rotates once for concurrent calls and reloads waiting and stale model instances', async ({
    assert,
  }) => {
    const mcp = await connection()
    const callers = await Promise.all(Array.from({ length: 8 }, () => Mcp.findOrFail(mcp.id)))
    const stale = await Mcp.findOrFail(mcp.id)
    let calls = 0
    const restore = oauthServer(async (body) => {
      calls++
      assert.equal(body.get('resource'), 'https://mcp.example/mcp')
      assert.equal(body.get('refresh_token'), 'refresh-old')
      // Like Cygnus, reject a repeated use of the rotating refresh token.
      if (calls > 1) return json({ error: 'invalid_grant' }, 400)
      return json({
        access_token: 'access-new',
        refresh_token: 'refresh-new',
        token_type: 'Bearer',
        expires_in: 600,
      })
    })
    try {
      await Promise.all(callers.map(refreshOauthAccessToken))
      await refreshOauthAccessToken(stale)
      assert.equal(calls, 1)
      for (const caller of [...callers, stale]) {
        assert.equal(McpSecretStore.decrypt(caller.oauthAccessToken), 'access-new')
        assert.equal(McpSecretStore.decrypt(caller.oauthRefreshToken), 'refresh-new')
      }
    } finally {
      restore()
    }
  })

  test('shares refresh failures, then releases the failed operation for a later attempt', async ({
    assert,
  }) => {
    const mcp = await connection()
    const callers = await Promise.all(Array.from({ length: 3 }, () => Mcp.findOrFail(mcp.id)))
    let calls = 0
    let reject = true
    const restore = oauthServer(async () => {
      calls++
      if (reject) return json({ error: 'temporarily_unavailable' }, 503)
      return json({
        access_token: 'recovered-access',
        refresh_token: 'recovered-refresh',
        token_type: 'Bearer',
        expires_in: 600,
      })
    })
    try {
      const results = await Promise.allSettled(callers.map(refreshOauthAccessToken))
      assert.isTrue(results.every((result) => result.status === 'rejected'))
      assert.equal(calls, 1)
      reject = false
      await refreshOauthAccessToken(mcp)
      assert.equal(calls, 2)
      assert.equal(McpSecretStore.decrypt(mcp.oauthAccessToken), 'recovered-access')
    } finally {
      restore()
    }
  })

  test('does not block a different MCP while one token endpoint is pending', async ({ assert }) => {
    const first = await connection()
    const second = await connection()
    second.oauthRefreshToken = McpSecretStore.encrypt('second-refresh')
    await second.save()
    const started = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const restore = oauthServer(async (body) => {
      if (body.get('refresh_token') === 'refresh-old') {
        started.resolve()
        await release.promise
      }
      return json({
        access_token: 'access-new',
        refresh_token: 'refresh-new',
        token_type: 'Bearer',
        expires_in: 600,
      })
    })
    const pending = refreshOauthAccessToken(first)
    try {
      await started.promise
      await refreshOauthAccessToken(second)
      assert.equal(McpSecretStore.decrypt(second.oauthAccessToken), 'access-new')
    } finally {
      release.resolve()
      await pending
      restore()
    }
  })
})
