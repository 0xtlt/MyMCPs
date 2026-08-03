import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

test.group('MCP installation modal', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('builds copyable configurations for Codex, Claude, and Cursor', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    await page.getByRole('button', { name: 'Install MCP' }).click()
    await page.getByLabel('Access token').fill("token'quoted")

    const modal = page.getByRole('dialog', { name: 'Install MyMCPs' })
    assert.include(await modal.innerText(), '[mcp_servers.mymcps]')
    assert.include(await modal.innerText(), 'Authorization = "Bearer token\'quoted"')
    assert.include(await modal.innerText(), 'X-MyMCPs-Tool-Mode')

    await modal.getByRole('button', { name: 'Claude' }).click()
    assert.include(await modal.innerText(), 'claude mcp add --transport http --scope user mymcps')
    assert.include(await modal.innerText(), `'Authorization: Bearer token'"'"'quoted'`)

    await modal.getByRole('button', { name: 'Cursor' }).click()
    assert.include(await modal.innerText(), '"mcpServers"')
    assert.include(await modal.innerText(), '"Authorization": "Bearer token\'quoted"')
  })

  test('requires a token on manual opening', async ({ assert, browserContext, visit }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    await page.getByRole('button', { name: 'Install MCP' }).click()

    assert.equal(await page.getByLabel('Access token').inputValue(), '')
    await page.assertTextContains('body', 'Paste an access token to enable copying.')
  })
})
