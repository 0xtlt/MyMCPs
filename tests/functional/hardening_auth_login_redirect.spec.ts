import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'

test.group('hardening: login redirect', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('returns to the pending authorization request without the login query string', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'admin@example.com' })
    const returnTo = '/authorize?client_id=abc&state=xyz'

    const response = await client
      .post('/login?state=forged&extra=1')
      .withCsrfToken()
      .withSession({ oauthReturnTo: returnTo })
      .redirects(0)
      .form({ email: 'admin@example.com', password: 'password123' })

    response.assertStatus(302)
    assert.equal(response.header('location'), returnTo)
  })
})
