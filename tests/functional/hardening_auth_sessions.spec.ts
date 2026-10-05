import { test } from '@japa/runner'
import ace from '@adonisjs/core/services/ace'
import db from '@adonisjs/lucid/services/db'
import limiter from '@adonisjs/limiter/services/main'
import type { ApiClient } from '@japa/api-client'
import type { Assert } from '@japa/assert'
import User from '#models/user'
import { SESSION_STAMP_KEY, createSessionStamp } from '#services/session_stamp'
import type { SessionStamp } from '#services/session_stamp'
import { createAdmin, createInvite } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'
import UserResetPassword from '../../commands/user_reset_password.js'

const MINUTE = 60 * 1000
const HOUR = 60 * MINUTE

type SessionValues = Record<string, any>

function signIn(client: ApiClient, email: string, password = 'password123') {
  return client.post('/login').withCsrfToken().redirects(0).form({ email, password })
}

function sessionFor(user: User, stamp: Partial<SessionStamp> = {}): SessionValues {
  return { auth_web: user.id, [SESSION_STAMP_KEY]: { ...createSessionStamp(user), ...stamp } }
}

/**
 * The cookie store keeps the whole session in the cookie, so presenting the
 * contents of a session again is what replaying a copied cookie amounts to.
 */
function replay(client: ApiClient, session: SessionValues, rememberCookie?: string) {
  const request = client.get('/').withSession(session).redirects(0)
  return rememberCookie ? request.withEncryptedCookie('remember_web', rememberCookie) : request
}

async function assertAccepted(client: ApiClient, session: SessionValues, rememberCookie?: string) {
  const response = await replay(client, session, rememberCookie)
  response.assertStatus(200)
  return response
}

async function assertRejected(
  client: ApiClient,
  assert: Assert,
  session: SessionValues,
  rememberCookie?: string
) {
  const response = await replay(client, session, rememberCookie)
  response.assertStatus(302)
  assertRedirectTo(assert, response, '/login')
  return response
}

test.group('hardening: session lifetime and revocation', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('stamps the session of a credential login', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const requestedAt = Date.now()

    const response = await signIn(client, 'admin@example.com')

    response.assertStatus(302)
    const stamp: SessionStamp = response.session()[SESSION_STAMP_KEY]
    assert.equal(stamp.version, admin.sessionVersion)
    assert.closeTo(stamp.authenticatedAt, requestedAt, 5_000)
    assert.equal(stamp.lastSeenAt, stamp.authenticatedAt)
    await assertAccepted(client, response.session())
  })

  test('stamps the session created by onboarding', async ({ client }) => {
    const response = await client.post('/onboarding').withCsrfToken().redirects(0).form({
      fullName: 'First Admin',
      email: 'admin@example.com',
      password: 'password123',
      passwordConfirmation: 'password123',
    })

    response.assertStatus(302)
    response.assertSession(SESSION_STAMP_KEY)
    await assertAccepted(client, response.session())
  })

  test('stamps the session created by accepting an invite', async ({ client }) => {
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
    response.assertSession(SESSION_STAMP_KEY)
    await assertAccepted(client, response.session())
  })

  test('rejects a session that carries no valid stamp: {$self}')
    .with(['missing', 'malformed'] as const)
    .run(async ({ client, assert }, stamp) => {
      const admin = await createAdmin()
      const session: SessionValues = { auth_web: admin.id }
      if (stamp === 'malformed') {
        session[SESSION_STAMP_KEY] = { version: admin.sessionVersion, authenticatedAt: 'now' }
      }

      await assertRejected(client, assert, session)
    })

  test('rejects a session stamped under an earlier session version', async ({ client, assert }) => {
    const admin = await createAdmin()
    const session = sessionFor(admin)
    await assertAccepted(client, session)

    await admin.invalidateSessions()

    const response = await assertRejected(client, assert, session)
    response.assertSessionMissing('auth_web')
    response.assertSessionMissing(SESSION_STAMP_KEY)
  })

  test('signing out retires a session captured beforehand', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const signedIn = await signIn(client, 'admin@example.com')
    const captured = signedIn.session()
    await assertAccepted(client, captured)

    const logout = await client.post('/logout').withSession(captured).withCsrfToken().redirects(0)
    logout.assertStatus(302)
    assertRedirectTo(assert, logout, '/login')

    await assertRejected(client, assert, captured)
    await admin.refresh()
    assert.equal(admin.sessionVersion, captured[SESSION_STAMP_KEY].version + 1)
  })

  test('changing the password retires captured sessions but not the browser that did it', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'admin@example.com' })
    const signedIn = await signIn(client, 'admin@example.com')
    const captured = signedIn.session()

    const changed = await client
      .patch('/settings/password')
      .withSession(captured)
      .withCsrfToken()
      .redirects(0)
      .form({
        currentPassword: 'password123',
        newPassword: 'new-password123',
        passwordConfirmation: 'new-password123',
      })
    changed.assertStatus(302)
    assertRedirectTo(assert, changed, '/settings')

    await assertRejected(client, assert, captured)
    await assertAccepted(client, changed.session())
  })

  test('user:reset-password retires captured sessions', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const signedIn = await signIn(client, 'admin@example.com')

    ace.ui.switchMode('raw')
    try {
      const command = await ace.create(UserResetPassword, [admin.email])
      command.prompt.trap('New password').replyWith('new-password-123')
      command.prompt.trap('Confirm new password').replyWith('new-password-123')
      await command.exec()
      command.assertSucceeded()
    } finally {
      ace.ui.switchMode('normal')
    }

    await assertRejected(client, assert, signedIn.session())
  })

  test('does not bump the session version when the password change is rolled back', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    await User.rememberMeTokens.create(admin, '1 year')
    await db.rawQuery(`
      CREATE TEMP TRIGGER fail_remember_token_delete
      BEFORE DELETE ON remember_me_tokens
      BEGIN SELECT RAISE(ABORT, 'simulated revocation failure'); END
    `)

    try {
      await assert.rejects(() => admin.changePassword('new-password123'))

      const stored = await User.findOrFail(admin.id)
      assert.equal(stored.sessionVersion, 1)
      assert.isTrue(await stored.verifyPassword('password123'))
    } finally {
      await db.rawQuery('DROP TRIGGER fail_remember_token_delete')
    }
  })

  test('accepts a session until it sat idle for the session age')
    .with([
      { idleFor: 2 * HOUR - MINUTE, status: 200 },
      { idleFor: 2 * HOUR + MINUTE, status: 302 },
    ])
    .run(async ({ client }, { idleFor, status }) => {
      const admin = await createAdmin()
      const now = Date.now()

      const response = await replay(
        client,
        sessionFor(admin, { authenticatedAt: now - 3 * HOUR, lastSeenAt: now - idleFor })
      )

      response.assertStatus(status)
    })

  test('accepts an active session until its absolute lifetime ran out')
    .with([
      { age: 24 * HOUR - MINUTE, status: 200 },
      { age: 24 * HOUR + MINUTE, status: 302 },
    ])
    .run(async ({ client }, { age, status }) => {
      const admin = await createAdmin()
      const now = Date.now()

      const response = await replay(
        client,
        sessionFor(admin, { authenticatedAt: now - age, lastSeenAt: now })
      )

      response.assertStatus(status)
    })

  test('refreshes last-seen while a session is in use', async ({ client, assert }) => {
    const admin = await createAdmin()
    const now = Date.now()
    const authenticatedAt = now - 3 * HOUR

    const response = await assertAccepted(
      client,
      sessionFor(admin, { authenticatedAt, lastSeenAt: now - 90 * MINUTE })
    )

    const stamp: SessionStamp = response.session()[SESSION_STAMP_KEY]
    assert.equal(stamp.authenticatedAt, authenticatedAt)
    assert.closeTo(stamp.lastSeenAt, now, 5_000)
  })

  test('the remember-me cookie replaces a session that is {$self}')
    .with(['absent', 'unstamped', 'idle', 'past its lifetime', 'retired'] as const)
    .run(async ({ client, assert }, state) => {
      const admin = await createAdmin({ email: 'admin@example.com' })
      const signedIn = await signIn(client, 'admin@example.com')
      const rememberCookie = signedIn.cookie('remember_web')!.value
      const now = Date.now()

      const sessions: Record<typeof state, SessionValues> = {
        'absent': {},
        'unstamped': { auth_web: admin.id },
        'idle': sessionFor(admin, { authenticatedAt: now - 4 * HOUR, lastSeenAt: now - 3 * HOUR }),
        'past its lifetime': sessionFor(admin, { authenticatedAt: now - 25 * HOUR }),
        'retired': sessionFor(admin),
      }
      if (state === 'retired') {
        await admin.invalidateSessions()
      }

      const response = await assertAccepted(client, sessions[state], rememberCookie)

      response.assertSession('auth_web', admin.id)
      const stamp: SessionStamp = response.session()[SESSION_STAMP_KEY]
      assert.equal(stamp.version, admin.sessionVersion)
      assert.closeTo(stamp.authenticatedAt, now, 5_000)
      await assertAccepted(client, response.session())
    })

  test('other browsers of the user carry on after one of them signs out', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'admin@example.com' })
    const first = await signIn(client, 'admin@example.com')
    const second = await signIn(client, 'admin@example.com')
    const firstRememberCookie = first.cookie('remember_web')!.value
    const secondRememberCookie = second.cookie('remember_web')!.value

    await client
      .post('/logout')
      .withSession(first.session())
      .withEncryptedCookie('remember_web', firstRememberCookie)
      .withCsrfToken()
      .redirects(0)

    await assertRejected(client, assert, first.session(), firstRememberCookie)
    await assertRejected(client, assert, second.session())
    await assertAccepted(client, second.session(), secondRememberCookie)
  })

  test('a remember-me cookie revoked by a password change does not rescue a retired session', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const signedIn = await signIn(client, 'admin@example.com')

    await admin.changePassword('new-password123')

    await assertRejected(client, assert, signedIn.session(), signedIn.cookie('remember_web')!.value)
  })
})
