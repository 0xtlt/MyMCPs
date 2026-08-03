import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp } from '#tests/helpers/factories'
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

  test('updates token settings without rotating its secret', async ({ client, assert }) => {
    const admin = await createAdmin()
    const firstMcp = await createMcp(admin.id, { name: 'First MCP' })
    const secondMcp = await createMcp(admin.id, { name: 'Second MCP' })
    const created = await createAccessToken(admin.id, { name: 'Original token' })
    const originalHash = created.token.tokenHash
    const originalPrefix = created.token.tokenPrefix
    const expiresAt = DateTime.utc().plus({ days: 7 }).startOf('minute')

    const response = await client
      .put(`/tokens/${created.token.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        name: 'Updated token',
        scopeMode: 'selected',
        mcpIds: [firstMcp.id, secondMcp.id],
        expiresAt: expiresAt.toISO(),
      })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/tokens')
    response.assertFlashMessage('success', 'Token updated')

    const updated = await AccessToken.findOrFail(created.token.id)
    await updated.load('mcps')
    assert.equal(updated.name, 'Updated token')
    assert.equal(updated.scopeMode, 'selected')
    assert.equal(updated.expiresAt?.toMillis(), expiresAt.toMillis())
    assert.deepEqual(updated.mcps.map((mcp) => mcp.id).sort(), [firstMcp.id, secondMcp.id].sort())
    assert.equal(updated.tokenHash, originalHash)
    assert.equal(updated.tokenPrefix, originalPrefix)
    assert.equal(updated.createdBy, admin.id)
    assert.isNull(updated.revokedAt)
  })

  test('clears selected MCPs and reactivates an expired token', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id)
    const created = await createAccessToken(admin.id, {
      name: 'Expired token',
      scopeMode: 'selected',
      mcpIds: [mcp.id],
      expiresAt: DateTime.utc().minus({ days: 1 }),
    })
    assert.isFalse(created.token.isUsable)

    const response = await client
      .put(`/tokens/${created.token.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        name: 'Reactivated token',
        scopeMode: 'all',
      })

    response.assertStatus(302)
    response.assertFlashMessage('success', 'Token updated')

    const updated = await AccessToken.findOrFail(created.token.id)
    await updated.load('mcps')
    assert.equal(updated.scopeMode, 'all')
    assert.isNull(updated.expiresAt)
    assert.lengthOf(updated.mcps, 0)
    assert.isTrue(updated.isUsable)
  })

  test('rejects invalid MCP ids without changing token settings', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id)
    const created = await createAccessToken(admin.id, { name: 'Unchanged token' })

    const response = await client
      .put(`/tokens/${created.token.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        name: 'Rejected update',
        scopeMode: 'selected',
        mcpIds: [mcp.id, 999999],
      })

    response.assertStatus(302)
    response.assertFlashMessage('error', 'One or more selected MCPs do not exist')

    const unchanged = await AccessToken.findOrFail(created.token.id)
    assert.equal(unchanged.name, 'Unchanged token')
    assert.equal(unchanged.scopeMode, 'all')
  })

  test('rejects updates to missing and revoked tokens', async ({ client, assert }) => {
    const admin = await createAdmin()
    const created = await createAccessToken(admin.id, { name: 'Revoked token' })
    created.token.revokedAt = DateTime.utc()
    await created.token.save()

    const revokedResponse = await client
      .put(`/tokens/${created.token.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ name: 'Rejected update', scopeMode: 'all' })

    revokedResponse.assertStatus(302)
    revokedResponse.assertFlashMessage('error', 'Revoked tokens cannot be edited')

    const missingResponse = await client
      .put('/tokens/999999')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ name: 'Missing token', scopeMode: 'all' })

    missingResponse.assertStatus(302)
    missingResponse.assertFlashMessage('error', 'Token not found')
    const revoked = await AccessToken.findOrFail(created.token.id)
    assert.equal(revoked.name, 'Revoked token')
  })

  test('validates the token route parameter with Vine', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .put('/tokens/not-a-token-id')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ name: 'Invalid token id', scopeMode: 'all' })

    response.assertStatus(302)
    assert.property(response.flashMessage('inputErrorsBag'), 'id')
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
