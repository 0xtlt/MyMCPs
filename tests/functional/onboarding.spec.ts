import { test } from '@japa/runner'
import User from '#models/user'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'
import limiter from '@adonisjs/limiter/services/main'

test.group('onboarding', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('redirects the first visit to onboarding', async ({ client, assert }) => {
    const response = await client.get('/').redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/onboarding')
  })

  test('creates the first user as an admin', async ({ client, assert }) => {
    const response = await client.post('/onboarding').withCsrfToken().redirects(0).form({
      fullName: 'First Admin',
      email: 'admin@example.com',
      password: 'password123',
      passwordConfirmation: 'password123',
    })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/')

    const user = await User.findByOrFail('email', 'admin@example.com')
    assert.equal(user.fullName, 'First Admin')
    assert.equal(user.role, 'admin')
    response.assertCookie('remember_web')
    assert.lengthOf(await User.rememberMeTokens.all(user), 1)
  })

  test('rate-limits repeated onboarding attempts', async ({ client, assert }) => {
    for (let attempt = 0; attempt < 5; attempt++) {
      const response = await client
        .post('/onboarding')
        .withCsrfToken()
        .form({ fullName: '', email: 'invalid', password: '' })
      assert.notEqual(response.status(), 429)
    }

    const limited = await client
      .post('/onboarding')
      .withCsrfToken()
      .form({ fullName: '', email: 'invalid', password: '' })

    limited.assertStatus(429)
    limited.assertHeader('retry-after')
    limited.assertTextIncludes('Too many requests')
  })

  test('does not allow onboarding after setup is complete', async ({ client, assert }) => {
    await User.create({
      fullName: 'Existing Admin',
      email: 'existing@example.com',
      password: 'password123',
      role: 'admin',
    })

    const response = await client.get('/onboarding').redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
    const [row] = await User.query().count('* as total')
    assert.equal(row.$extras.total, 1)
  })
})
