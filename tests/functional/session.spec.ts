import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import User from '#models/user'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('session authentication', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('renders the login page for guests', async ({ client }) => {
    const response = await client.get('/login')

    response.assertStatus(200)
    response.assertTextIncludes('MyMCPs')
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
