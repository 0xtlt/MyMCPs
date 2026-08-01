import { test } from '@japa/runner'
import AccessToken from '#models/access_token'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('access tokens', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('creates a token and flashes plaintext only once', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client.post('/tokens').loginAs(admin).withCsrfToken().redirects(0).form({
      name: 'Production agent',
      scopeMode: 'all',
    })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/tokens')
    response.assertFlashMessage(
      'success',
      'Access token created — copy it now, it will not be shown again'
    )

    const plaintext = response.flashMessage('createdPlaintext')
    assert.isString(plaintext)
    assert.match(String(plaintext), /^mcp_[A-Za-z0-9_-]{43}$/)

    const token = await AccessToken.findByOrFail('name', 'Production agent')
    assert.notEqual(token.tokenHash, plaintext)
    assert.isFalse(token.isRevoked)
  })

  test('rejects selected MCP ids that do not exist', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id)

    const response = await client
      .post('/tokens')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        name: 'Selected agent',
        scopeMode: 'selected',
        mcpIds: [mcp.id, 999999],
      })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/tokens')
    response.assertFlashMessage('error', 'One or more selected MCPs do not exist')
    assert.isNull(await AccessToken.findBy('name', 'Selected agent'))
  })

  test('revokes a token', async ({ client, assert }) => {
    const admin = await createAdmin()
    const created = await client.post('/tokens').loginAs(admin).withCsrfToken().redirects(0).form({
      name: 'Revocable agent',
      scopeMode: 'all',
    })
    const token = await AccessToken.findByOrFail('name', 'Revocable agent')

    const response = await client
      .post(`/tokens/${token.id}/revoke`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    created.assertStatus(302)
    response.assertStatus(302)
    assertRedirectTo(assert, response, '/tokens')
    response.assertFlashMessage('success', 'Token revoked')
    const revokedToken = await AccessToken.findOrFail(token.id)
    assert.isTrue(revokedToken.isRevoked)
  })
})
