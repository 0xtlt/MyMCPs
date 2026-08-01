import { test } from '@japa/runner'
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

  test('logs an authenticated user out', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    const response = await client.post('/logout').loginAs(admin).withCsrfToken().redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
  })
})
