import { test } from '@japa/runner'
import User from '#models/user'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('onboarding', (group) => {
  group.each.setup(beginTestTransaction)
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
