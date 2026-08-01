import { test } from '@japa/runner'
import User from '#models/user'
import {
  beginTestTransaction,
  rollbackTestTransaction,
} from '#tests/helpers/database'

test.group('onboarding browser flow', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('creates an admin account and reaches the dashboard', async ({ visit }) => {
    const page = await visit('/')

    await page.assertPath('/onboarding')
    await page.assertText('h1', 'Set up MyMCPs')

    await page.getByLabel('Full name').fill('Browser Admin')
    await page.getByLabel('Email').fill('browser-admin@example.com')
    await page.getByLabel('Password', { exact: true }).fill('password123')
    await page.getByLabel('Confirm password').fill('password123')
    await page.getByRole('button', { name: 'Create admin' }).click()

    await page.waitForURL((url) => url.pathname === '/')
    await page.assertText('h1', 'Dashboard')
    await page.assertTextContains('body', 'Signed in as Browser Admin')

    const user = await User.findByOrFail('email', 'browser-admin@example.com')
    if (user.role !== 'admin') {
      throw new Error('The onboarding browser flow did not create an admin')
    }
  })
})
