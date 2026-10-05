import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createInvite, createMcp } from '#tests/helpers/factories'
import { inviteTokenParamsValidator, recordIdParamsValidator } from '#validators/route_params'
import Invite from '#models/invite'

test.group('route params: validators', () => {
  test('a record id is a whole, positive number')
    .with([
      { id: '12', valid: true },
      { id: 7, valid: true },
      { id: 'abc', valid: false },
      { id: '1.5', valid: false },
      { id: '-3', valid: false },
      { id: '12abc', valid: false },
      { id: '', valid: false },
      { id: undefined, valid: false },
    ])
    .run(async ({ assert }, { id, valid }) => {
      const [error, params] = await recordIdParamsValidator.tryValidate({ id })
      assert.equal(error === null, valid)
      if (valid) assert.equal(params!.id, Number(id))
    })

  test('an invite token is 64 hexadecimal characters')
    .with([
      { token: Invite.generateToken(), valid: true },
      { token: 'a'.repeat(63), valid: false },
      { token: 'g'.repeat(64), valid: false },
      { token: 'A'.repeat(64), valid: false },
      { token: `${'a'.repeat(64)} `, valid: false },
      { token: '', valid: false },
    ])
    .run(async ({ assert }, { token, valid }) => {
      const [error] = await inviteTokenParamsValidator.tryValidate({ token })
      assert.equal(error === null, valid)
    })
})

test.group('route params: lookups', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('an id that is not a number is answered like a missing record')
    .with([
      { method: 'delete', path: '/mcps/abc', message: 'MCP not found' },
      { method: 'post', path: '/mcps/1.5/probe', message: 'MCP not found' },
      { method: 'post', path: '/tokens/abc/revoke', message: 'Token not found' },
      { method: 'delete', path: '/invites/abc', message: 'Invite not found' },
      { method: 'delete', path: '/members/abc', message: 'Member not found' },
    ] as const)
    .run(async ({ client }, { method, path, message }) => {
      const admin = await createAdmin()

      const response = await client[method](path).loginAs(admin).withCsrfToken().redirects(0)

      response.assertStatus(302)
      response.assertFlashMessage('error', message)
    })

  test('a record is still found by its id', async ({ client }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id)

    const response = await client
      .delete(`/mcps/${mcp.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    response.assertFlashMessage('success', 'MCP deleted')
  })

  test('an invite link with a malformed token is refused like an unknown one', async ({
    client,
  }) => {
    await createAdmin()

    const response = await client.get('/invite/not-a-token').redirects(0)

    response.assertStatus(302)
    response.assertFlashMessage('error', 'This invite is invalid or has expired')
  })

  test('an invite link with its token still opens', async ({ client }) => {
    const admin = await createAdmin()
    const invite = await createInvite(admin.id)

    const response = await client.get(`/invite/${invite.token}`).redirects(0)

    response.assertStatus(200)
  })
})
