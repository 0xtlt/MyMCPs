import { test } from '@japa/runner'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('hardening: browser history', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('asks the browser to encrypt the pages it keeps in its history', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()

    const response = await client.get('/tokens').withInertia().loginAs(admin)

    response.assertStatus(200)
    response.assertInertiaComponent('tokens/index')
    assert.isTrue(response.body().encryptHistory)
    assert.notProperty(response.body(), 'clearHistory')
  })

  test('drops the history key on the login page, where signing out leads', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()

    const logout = await client.post('/logout').loginAs(admin).withCsrfToken().redirects(0)
    logout.assertStatus(302)
    assertRedirectTo(assert, logout, '/login')
    const response = await client.get('/login').withInertia()

    response.assertStatus(200)
    response.assertInertiaComponent('auth/login')
    assert.isTrue(response.body().encryptHistory)
    assert.isTrue(response.body().clearHistory)
  })
})
