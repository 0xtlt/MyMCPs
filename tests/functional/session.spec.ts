import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import User from '#models/user'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'
import limiter from '@adonisjs/limiter/services/main'

test.group('session authentication', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('renders the login page for guests', async ({ client }) => {
    const response = await client.get('/login')

    response.assertStatus(200)
    response.assertTextIncludes('MyMCPs')
    response.assertHeader('content-security-policy')
    response.assertTextIncludes('nonce=')
  })

  test('rate-limits repeated credential attempts by resolved client IP', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'limited@example.com' })

    for (let attempt = 0; attempt < 5; attempt++) {
      const response = await client
        .post('/login')
        .header('x-forwarded-for', '198.51.100.77')
        .withCsrfToken()
        .form({ email: 'limited@example.com', password: 'wrong-password' })
      assert.notEqual(response.status(), 429)
    }

    const limited = await client
      .post('/login')
      .header('x-forwarded-for', '198.51.100.77')
      .withCsrfToken()
      .form({ email: 'limited@example.com', password: 'wrong-password' })

    limited.assertStatus(429)
    limited.assertHeader('retry-after')
    limited.assertTextIncludes('Too many requests')
  })

  test('does not charge successful logins against the credential failure budget', async ({
    client,
  }) => {
    await createAdmin({ email: 'successful@example.com' })

    for (let attempt = 0; attempt < 6; attempt++) {
      const response = await client
        .post('/login')
        .header('x-forwarded-for', '198.51.100.78')
        .withCsrfToken()
        .redirects(0)
        .form({ email: 'successful@example.com', password: 'password123' })
      response.assertStatus(302)
    }
  })

  test('redirects guests away from authenticated routes', async ({ client, assert }) => {
    await createAdmin()

    const response = await client.get('/mcps').redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
  })

  test('allows an authenticated user to reach the dashboard', async ({ client }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    const response = await client.get('/').loginAs(admin)

    response.assertStatus(200)
    response.assertTextIncludes('data-page')
    response.assertTextIncludes('admin@example.com')
  })

  test('does not forward untrusted query values through redirects', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'redirects@example.com' })

    const guestOnly = await client
      .get('/login?token=attacker-controlled')
      .loginAs(admin)
      .redirects(0)
    guestOnly.assertStatus(302)
    assertRedirectTo(assert, guestOnly, '/')
    assert.equal(new URL(guestOnly.header('location')!, 'http://localhost').search, '')

    const login = await client
      .post('/login?code=attacker-controlled')
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'redirects@example.com', password: 'password123' })
    login.assertStatus(302)
    assertRedirectTo(assert, login, '/')
    assert.equal(new URL(login.header('location')!, 'http://localhost').search, '')
  })

  test('always remembers credential logins for one year', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const requestedAt = DateTime.utc()

    const response = await client.post('/login').withCsrfToken().redirects(0).form({
      email: 'admin@example.com',
      password: 'password123',
    })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/')
    response.assertCookie('remember_web')

    const tokens = await User.rememberMeTokens.all(admin)
    assert.lengthOf(tokens, 1)
    assert.approximately(
      tokens[0].expiresAt.getTime(),
      requestedAt.plus({ days: 365.25 }).toMillis(),
      2_000
    )
  })

  test('revokes the remember token on logout', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    const loginResponse = await client.post('/login').withCsrfToken().redirects(0).form({
      email: 'admin@example.com',
      password: 'password123',
    })
    const rememberCookie = loginResponse.cookie('remember_web')
    assert.exists(rememberCookie)

    const response = await client
      .post('/logout')
      .withSession(loginResponse.session())
      .encryptedCookie('remember_web', rememberCookie!.value)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
    assert.lengthOf(await User.rememberMeTokens.all(admin), 0)
  })

  test('logs an authenticated user out', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    const response = await client.post('/logout').loginAs(admin).withCsrfToken().redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
  })
})
