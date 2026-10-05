import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import type { ApiClient } from '@japa/api-client'
import type User from '#models/user'
import { createAdmin, createMember } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { assertRedirectTo } from '#tests/helpers/http'

function changeEmail(client: ApiClient, user: User, currentPassword: string) {
  return client
    .patch('/settings/email')
    .loginAs(user)
    .withCsrfToken()
    .redirects(0)
    .form({ email: 'updated@example.com', currentPassword })
}

function changePassword(client: ApiClient, user: User, currentPassword: string) {
  return client.patch('/settings/password').loginAs(user).withCsrfToken().redirects(0).form({
    currentPassword,
    newPassword: 'new-password123',
    passwordConfirmation: 'new-password123',
  })
}

test.group('hardening: current-password checks', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('limits wrong current passwords across the email and password forms', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    for (let guess = 0; guess < 5; guess++) {
      const change = guess % 2 === 0 ? changeEmail : changePassword
      const wrong = await change(client, admin, `wrong-password-${guess}`)
      wrong.assertStatus(302)
      assert.property(wrong.flashMessage('inputErrorsBag'), 'currentPassword')
    }

    const email = await changeEmail(client, admin, 'password123')
    email.assertStatus(429)
    email.assertHeader('retry-after')
    email.assertTextIncludes('Too many requests')

    const password = await changePassword(client, admin, 'password123')
    password.assertStatus(429)

    await admin.refresh()
    assert.equal(admin.email, 'admin@example.com')
    assert.isTrue(await admin.verifyPassword('password123'))
  })

  test('caps a parallel burst of wrong current passwords', async ({ client, assert }) => {
    const admin = await createAdmin()

    const responses = await Promise.all(
      Array.from({ length: 12 }, (_, guess) => changeEmail(client, admin, `wrong-${guess}`))
    )

    assert.lengthOf(
      responses.filter((response) => response.status() !== 429),
      5
    )
  })

  test('counts each user separately', async ({ client, assert }) => {
    const admin = await createAdmin()
    const member = await createMember()

    for (let guess = 0; guess < 5; guess++) {
      await changeEmail(client, admin, 'wrong-password')
    }

    const response = await changeEmail(client, member, 'password123')
    response.assertStatus(302)
    assertRedirectTo(assert, response, '/settings')
    response.assertFlashMessage('success', 'Email updated')
  })

  test('a correct current password clears the count', async ({ client, assert }) => {
    const admin = await createAdmin()

    for (let round = 0; round < 2; round++) {
      for (let guess = 0; guess < 4; guess++) {
        await changeEmail(client, admin, 'wrong-password')
      }
      const response = await changeEmail(client, admin, 'password123')
      response.assertStatus(302)
      assertRedirectTo(assert, response, '/settings')
    }
  })
})
