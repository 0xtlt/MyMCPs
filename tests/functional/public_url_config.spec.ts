import { test } from '@japa/runner'
import env from '#start/env'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createInvite, createMcp } from '#tests/helpers/factories'

test.group('missing public app URL', (group) => {
  let originalAppUrl: string | undefined

  group.each.setup(async () => {
    await beginTestTransaction()
    originalAppUrl = env.get('APP_URL')
    env.set('APP_URL', undefined)
    delete process.env.APP_URL
  })

  group.each.teardown(async () => {
    env.set('APP_URL', originalAppUrl)
    if (originalAppUrl === undefined) delete process.env.APP_URL
    await rollbackTestTransaction()
  })

  test('shares the warning state and omits generated public URLs', async ({ client }) => {
    const admin = await createAdmin()
    await createInvite(admin.id)

    const tokensResponse = await client.get('/tokens').loginAs(admin)
    tokensResponse.assertStatus(200)
    tokensResponse.assertTextIncludes('"appUrlConfigured":false')
    tokensResponse.assertTextIncludes('"gatewayUrl":null')

    const invitesResponse = await client.get('/invites').loginAs(admin)
    invitesResponse.assertStatus(200)
    invitesResponse.assertTextIncludes('"appUrl":null')
  })

  test('rejects a direct OAuth start before making an upstream request', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'OAuth MCP',
      authType: 'auto',
      oauthRequired: true,
      httpUrl: 'https://mcp.example/mcp',
      status: 'draft',
    })

    const originalFetch = globalThis.fetch
    let upstreamCalled = false
    globalThis.fetch = async () => {
      upstreamCalled = true
      throw new Error('OAuth discovery should not run without APP_URL')
    }

    try {
      const response = await client.get(`/mcps/${mcp.id}/oauth/start`).loginAs(admin).redirects(0)

      response.assertStatus(302)
      assert.isFalse(upstreamCalled)
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('refuses gateway OAuth endpoints with an insecure public origin', async ({
    client,
    assert,
  }) => {
    env.set('APP_URL', 'http://mcp.example.com')
    const admin = await createAdmin()

    const tokens = await client.get('/tokens').withInertia().loginAs(admin)
    tokens.assertStatus(200)
    tokens.assertInertiaPropsContains({
      appUrlConfigured: false,
      gatewayUrl: null,
    })

    const responses = [
      {
        label: 'authorization metadata',
        response: await client.get('/.well-known/oauth-authorization-server'),
      },
      {
        label: 'resource metadata',
        response: await client.get('/.well-known/oauth-protected-resource/mcp'),
      },
      { label: 'registration', response: await client.post('/register').json({}) },
      { label: 'authorization', response: await client.get('/authorize') },
      { label: 'token exchange', response: await client.post('/token').form({}) },
      { label: 'revocation', response: await client.post('/revoke').form({}) },
    ]
    for (const { label, response } of responses) {
      assert.equal(response.status(), 503, label)
      assert.equal(
        response.body().error_description,
        'OAuth requires APP_URL to be a public HTTPS origin'
      )
    }

    const challenge = await client.get('/mcp')
    challenge.assertStatus(401)
    assert.notInclude(challenge.header('www-authenticate'), 'resource_metadata=')
  })
})
