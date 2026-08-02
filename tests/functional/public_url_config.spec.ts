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
    tokensResponse.assertTextIncludes('&quot;appUrlConfigured&quot;:false')
    tokensResponse.assertTextIncludes('&quot;gatewayUrl&quot;:null')

    const invitesResponse = await client.get('/invites').loginAs(admin)
    invitesResponse.assertStatus(200)
    invitesResponse.assertTextIncludes('&quot;appUrl&quot;:null')
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
})
