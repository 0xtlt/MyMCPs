import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import AccessToken from '#models/access_token'
import Invite from '#models/invite'
import Mcp from '#models/mcp'
import User from '#models/user'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createInvite,
  createMember,
  createMcp,
} from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'
import limiter from '@adonisjs/limiter/services/main'

test.group('invites and member administration', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('restricts invite administration to admins', async ({ client, assert }) => {
    const member = await createMember()

    const response = await client.get('/invites').loginAs(member).redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/')
    response.assertFlashMessage('error', 'Admin access required')
  })

  test('creates an invite and rejects duplicate emails', async ({ client, assert }) => {
    const admin = await createAdmin()

    const created = await client
      .post('/invites')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'new-member@example.com' })

    created.assertStatus(302)
    assertRedirectTo(assert, created, '/invites')
    created.assertFlashMessage('success', 'Invite created')

    const invite = await Invite.findByOrFail('email', 'new-member@example.com')
    assert.equal(invite.createdBy, admin.id)
    assert.isTrue(invite.isUsable)

    const duplicate = await client
      .post('/invites')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'new-member@example.com' })

    duplicate.assertStatus(302)
    duplicate.assertFlashMessage('error', 'A pending invite already exists for that email')
  })

  test('accepts a usable invite and signs in the new member', async ({ client, assert }) => {
    const admin = await createAdmin()
    const invite = await createInvite(admin.id, { email: 'invited@example.com' })

    const response = await client
      .post(`/invite/${invite.token}`)
      .withCsrfToken()
      .redirects(0)
      .form({
        fullName: 'Invited Member',
        password: 'password123',
        passwordConfirmation: 'password123',
      })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/')
    response.assertFlashMessage('success', 'Welcome to MyMCPs')

    const member = await User.findByOrFail('email', 'invited@example.com')
    assert.equal(member.fullName, 'Invited Member')
    assert.equal(member.role, 'member')
    response.assertCookie('remember_web')
    assert.lengthOf(await User.rememberMeTokens.all(member), 1)
    const acceptedInvite = await Invite.findOrFail(invite.id)
    assert.isTrue(acceptedInvite.isAccepted)
  })

  test('rate-limits repeated invite acceptance attempts', async ({ client, assert }) => {
    const admin = await createAdmin()
    const invite = await createInvite(admin.id, { email: 'limited-invite@example.com' })

    for (let attempt = 0; attempt < 5; attempt++) {
      const response = await client
        .post(`/invite/${invite.token}`)
        .withCsrfToken()
        .form({ fullName: '', password: '', passwordConfirmation: '' })
      assert.notEqual(response.status(), 429)
    }

    const limited = await client
      .post(`/invite/${invite.token}`)
      .withCsrfToken()
      .form({ fullName: '', password: '', passwordConfirmation: '' })

    limited.assertStatus(429)
    limited.assertHeader('retry-after')
    limited.assertTextIncludes('Too many requests')
  })

  test('rejects expired invites', async ({ client, assert }) => {
    const admin = await createAdmin()
    const invite = await createInvite(admin.id, {
      expiresAt: DateTime.utc().minus({ minutes: 1 }),
    })

    const response = await client.get(`/invite/${invite.token}`).redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/')
    response.assertFlashMessage('error', 'This invite is invalid or has expired')
  })

  test('reassigns owned records and revokes tokens before removing a member', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const member = await createMember()
    const mcp = await createMcp(member.id)
    const token = await createAccessToken(member.id)
    const invite = await createInvite(member.id)

    const response = await client
      .delete(`/members/${member.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/invites')
    response.assertFlashMessage('success', 'Member removed')
    assert.isNull(await User.find(member.id))
    const reassignedMcp = await Mcp.findOrFail(mcp.id)
    const reassignedToken = await AccessToken.findOrFail(token.token.id)
    const reassignedInvite = await Invite.findOrFail(invite.id)
    assert.equal(reassignedMcp.createdBy, admin.id)
    assert.equal(reassignedToken.createdBy, admin.id)
    assert.isTrue(reassignedToken.isRevoked)
    assert.equal(reassignedInvite.createdBy, admin.id)
  })

  test('prevents an admin from removing their own account', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .delete(`/members/${admin.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/invites')
    response.assertFlashMessage('error', 'You cannot remove your own account')
    assert.isNotNull(await User.find(admin.id))
  })
})
