import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

test.group('gateway OAuth consent', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('navigates the browser to the OAuth client callback after approval', async ({
    assert,
    browserContext,
    client,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)

    const registration = await client.post('/register').json({
      client_name: 'Browser callback client',
      redirect_uris: ['http://127.0.0.1/callback'],
      token_endpoint_auth_method: 'none',
      grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'],
      scope: 'mcp:tools',
    })
    registration.assertStatus(201)

    const verifier = 'browser-oauth-verifier-for-mymcps-tests-123456789'
    const challenge = createHash('sha256').update(verifier).digest('base64url')
    const authorizationPath = `/authorize?${new URLSearchParams({
      client_id: registration.body().client_id,
      redirect_uri: 'http://127.0.0.1:49152/callback',
      response_type: 'code',
      code_challenge: challenge,
      code_challenge_method: 'S256',
      scope: 'mcp:tools',
      resource: 'http://localhost:3333/mcp',
      state: 'browser-state',
    })}`

    const page = await visit(authorizationPath)
    await page.route('http://127.0.0.1:49152/callback**', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'text/plain',
        body: 'OAuth callback reached',
      })
    })

    await page.getByRole('button', { name: 'Authorize client' }).click()
    await page.waitForURL((url) => url.origin === 'http://127.0.0.1:49152')

    const callback = new URL(page.url())
    assert.equal(callback.searchParams.get('state'), 'browser-state')
    assert.match(callback.searchParams.get('code') ?? '', /^[A-Za-z0-9_-]{43}$/)
    await page.assertTextContains('body', 'OAuth callback reached')
  })
})
