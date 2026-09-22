import { test } from '@japa/runner'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import app from '@adonisjs/core/services/app'
import ace from '@adonisjs/core/services/ace'
import hash from '@adonisjs/core/services/hash'
import db from '@adonisjs/lucid/services/db'
import User from '#models/user'
import { createUser } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import UserResetPassword from '../../commands/user_reset_password.js'

test.group('user:reset-password', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    ace.ui.switchMode('raw')
    return () => ace.ui.switchMode('normal')
  })

  test('can be discovered before application providers boot', async ({ assert }) => {
    const { stdout } = await promisify(execFile)(
      process.execPath,
      ['ace.js', 'user:reset-password', '--help'],
      { cwd: app.appRoot.pathname }
    )
    assert.include(stdout, 'user:reset-password')
    assert.include(stdout, 'Email address of the account to recover')
  })

  test('recovers a {$self} account and revokes only its remember-me tokens')
    .with(['admin', 'member'] as const)
    .run(async ({ assert, client }, role) => {
      const user = await createUser({ role })
      const other = await createUser()
      await User.rememberMeTokens.create(user, '1 year')
      await User.rememberMeTokens.create(user, '1 year')
      await User.rememberMeTokens.create(other, '1 year')

      const command = await ace.create(UserResetPassword, [user.email])
      command.prompt.trap('New password').replyWith('new-password-123')
      command.prompt.trap('Confirm new password').replyWith('new-password-123')
      await command.exec()

      command.assertSucceeded()
      await user.refresh()
      assert.notEqual(user.password, 'new-password-123')
      assert.isTrue(await hash.verify(user.password, 'new-password-123'))
      assert.isFalse(await hash.verify(user.password, 'password123'))
      assert.equal(user.role, role)
      assert.lengthOf(await User.rememberMeTokens.all(user), 0)
      assert.lengthOf(await User.rememberMeTokens.all(other), 1)

      const response = await client.post('/login').withCsrfToken().redirects(0).form({
        email: user.email,
        password: 'new-password-123',
      })
      response.assertStatus(302)
      response.assertCookie('remember_web')
    })

  test('rejects an unknown account without creating a user', async ({ assert }) => {
    const command = await ace.create(UserResetPassword, ['missing@example.com'])
    await command.exec()

    command.assertFailed()
    command.assertLogMatches(/No user found/, 'stderr')
    assert.isNull(await User.findBy('email', 'missing@example.com'))
  })

  test('rejects invalid or unconfirmed passwords without changing credentials')
    .with([
      { password: '', confirmation: '' },
      { password: 'short', confirmation: 'short' },
      { password: 'x'.repeat(33), confirmation: 'x'.repeat(33) },
      { password: 'new-password-123', confirmation: 'different-password' },
    ])
    .run(async ({ assert }, { password, confirmation }) => {
      const user = await createUser()
      const originalHash = user.password
      await User.rememberMeTokens.create(user, '1 year')
      const command = await ace.create(UserResetPassword, [user.email])
      command.prompt.trap('New password').replyWith(password)
      command.prompt.trap('Confirm new password').replyWith(confirmation)

      await command.exec()

      command.assertFailed()
      await user.refresh()
      assert.equal(user.password, originalHash)
      assert.lengthOf(await User.rememberMeTokens.all(user), 1)
    })

  test('rolls back the password when remember-me revocation fails', async ({ assert }) => {
    const user = await createUser()
    const originalHash = user.password
    await User.rememberMeTokens.create(user, '1 year')
    await db.rawQuery(`
      CREATE TEMP TRIGGER fail_remember_token_delete
      BEFORE DELETE ON remember_me_tokens
      BEGIN SELECT RAISE(ABORT, 'simulated revocation failure'); END
    `)

    try {
      const command = await ace.create(UserResetPassword, [user.email])
      command.prompt.trap('New password').replyWith('new-password-123')
      command.prompt.trap('Confirm new password').replyWith('new-password-123')
      await command.exec()

      command.assertFailed()
      await user.refresh()
      assert.equal(user.password, originalHash)
      assert.lengthOf(await User.rememberMeTokens.all(user), 1)
    } finally {
      await db.rawQuery('DROP TRIGGER fail_remember_token_delete')
    }
  })
})
