import { test } from '@japa/runner'
import env from '#start/env'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createInvite, createMcp } from '#tests/helpers/factories'

test.group('missing public app URL browser state', (group) => {
  let originalAppUrl: string | undefined

  group.each.setup(async () => {
    await beginTestTransaction()
    originalAppUrl = env.get('APP_URL')
    env.set('APP_URL', undefined)
    delete process.env.APP_URL
  })

  group.each.teardown(async () => {
    env.set('APP_URL', originalAppUrl)
    if (originalAppUrl === undefined) delete process.env.APP_URL
    await rollbackTestTransaction()
  })

  test('shows the warning and disables public URL actions', async ({ visit, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    await createInvite(admin.id)
    await createMcp(admin.id, {
      name: 'OAuth MCP',
      authType: 'auto',
      oauthRequired: true,
      httpUrl: 'https://mcp.example/mcp',
      status: 'draft',
    })

    const page = await visit('/login')
    await page.getByLabel('Email').fill('admin@example.com')
    await page.getByLabel('Password').fill('password123')
    await page.getByRole('button', { name: 'Sign in' }).click()
    await page.waitForURL((url) => url.pathname === '/')
    await page.assertTextContains('body', 'Set APP_URL to enable public links')

    await page.getByRole('link', { name: 'Tokens', exact: true }).click()
    await page.waitForURL((url) => url.pathname === '/tokens')
    assert.equal(
      await page.getByRole('button', { name: 'Copy URL' }).getAttribute('aria-disabled'),
      'true'
    )

    await page.getByRole('link', { name: 'Invites', exact: true }).click()
    await page.waitForURL((url) => url.pathname === '/invites')
    await page.getByRole('radio', { name: /^Invites/ }).click()
    assert.equal(
      await page.getByRole('button', { name: 'Copy link' }).getAttribute('aria-disabled'),
      'true'
    )

    await page.getByRole('link', { name: 'MCPs', exact: true }).click()
    await page.waitForURL((url) => url.pathname === '/mcps')
    await page.getByRole('button', { name: 'Edit' }).click()
    assert.equal(
      await page
        .getByRole('button', { name: 'Connect', exact: true })
        .getAttribute('aria-disabled'),
      'true'
    )
  })
})
