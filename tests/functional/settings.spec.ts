import { test } from '@japa/runner'
import User from '#models/user'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMember } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('settings', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('requires authentication for settings pages and updates', async ({ client, assert }) => {
    await createAdmin()

    const page = await client.get('/settings').redirects(0)
    const email = await client
      .patch('/settings/email')
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'new@example.com', currentPassword: 'password123' })
    const password = await client.patch('/settings/password').withCsrfToken().redirects(0).form({
      currentPassword: 'password123',
      newPassword: 'new-password123',
      passwordConfirmation: 'new-password123',
    })

    for (const response of [page, email, password]) {
      response.assertStatus(302)
      assertRedirectTo(assert, response, '/login')
    }
  })

  test('provides the role used to show instance settings only to admins', async ({ client }) => {
    const admin = await createAdmin()
    const member = await createMember()

    const adminPage = await client.get('/settings').withInertia().loginAs(admin)
    const memberPage = await client.get('/settings').withInertia().loginAs(member)

    adminPage.assertStatus(200)
    adminPage.assertInertiaComponent('settings/index')
    adminPage.assertInertiaPropsContains({ user: { id: admin.id, isAdmin: true } })
    memberPage.assertStatus(200)
    memberPage.assertInertiaComponent('settings/index')
    memberPage.assertInertiaPropsContains({ user: { id: member.id, isAdmin: false } })
  })

  test('updates the signed-in user email after password confirmation', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    const response = await client
      .patch('/settings/email')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'updated@example.com', currentPassword: 'password123' })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/settings')
    response.assertFlashMessage('success', 'Email updated')

    await admin.refresh()
    assert.equal(admin.email, 'updated@example.com')
    assert.isTrue(await admin.verifyPassword('password123'))
  })

  test('rejects duplicate emails and incorrect current passwords', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    await createMember({ email: 'member@example.com' })

    const duplicate = await client
      .patch('/settings/email')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'member@example.com', currentPassword: 'password123' })

    duplicate.assertStatus(302)
    assert.property(duplicate.flashMessage('inputErrorsBag'), 'email')

    const incorrectPassword = await client
      .patch('/settings/email')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'updated@example.com', currentPassword: 'incorrect-password' })

    incorrectPassword.assertStatus(302)
    assert.property(incorrectPassword.flashMessage('inputErrorsBag'), 'currentPassword')

    await admin.refresh()
    assert.equal(admin.email, 'admin@example.com')
  })

  test('updates the password and invalidates the old credential', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })

    const response = await client
      .patch('/settings/password')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        currentPassword: 'password123',
        newPassword: 'new-password123',
        passwordConfirmation: 'new-password123',
      })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/settings')
    response.assertFlashMessage('success', 'Password updated')

    const updatedUser = await User.findOrFail(admin.id)
    assert.isFalse(await updatedUser.verifyPassword('password123'))
    assert.isTrue(await updatedUser.verifyPassword('new-password123'))
  })

  test('rejects mismatched password confirmation', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .patch('/settings/password')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        currentPassword: 'password123',
        newPassword: 'new-password123',
        passwordConfirmation: 'different-password',
      })

    response.assertStatus(302)
    assert.property(response.flashMessage('inputErrorsBag'), 'passwordConfirmation')

    await admin.refresh()
    assert.isTrue(await admin.verifyPassword('password123'))
  })
})
