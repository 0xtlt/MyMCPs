import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('MCP OAuth pasted callback', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('sends a pasted loopback address to the OAuth callback', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await createMcp(admin.id, {
      name: 'Figma',
      authType: 'auto',
      oauthRequired: true,
      httpUrl: 'https://mcp.figma.com/mcp',
      status: 'draft',
    })
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Edit' }).click()
    const dialog = page.getByRole('dialog', { name: 'Edit Figma' })
    const callbackAddress = dialog.getByRole('textbox', { name: 'Callback address' })

    await callbackAddress.fill('http://localhost:45873/callback')
    await dialog.getByRole('button', { name: 'Finish connecting' }).click()
    await dialog.getByText('Paste the full localhost address').waitFor()
    assert.equal(new URL(page.url()).pathname, '/mcps')

    // No authorization was started in this session, so reaching the callback
    // with the pasted response is reported as an invalid callback.
    await callbackAddress.fill(
      'http://localhost:45873/callback?code=pasted-code&state=pasted-state'
    )
    await callbackAddress.press('Enter')
    await page.getByText('Invalid OAuth callback').waitFor()
    assert.equal(new URL(page.url()).pathname, '/mcps')
    assert.equal(new URL(page.url()).search, '')
  })

  test('keeps the direct redirect for providers that accept the instance callback', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await createMcp(admin.id, {
      name: 'Notion',
      authType: 'auto',
      oauthRequired: true,
      httpUrl: 'https://mcp.notion.com/mcp',
      status: 'draft',
    })
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Edit' }).click()
    const dialog = page.getByRole('dialog', { name: 'Edit Notion' })
    await dialog.getByRole('button', { name: 'Connect', exact: true }).waitFor()

    assert.equal(await dialog.getByRole('textbox', { name: 'Callback address' }).count(), 0)
  })
})
