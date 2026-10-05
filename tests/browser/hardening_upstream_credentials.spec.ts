import { test } from '@japa/runner'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('MCP form: saved credentials of a re-pointed MCP', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('stops offering to keep a saved bearer token once the URL has another origin', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Bearer MCP',
      authType: 'bearer',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authBearer = McpSecretStore.encrypt('saved-bearer')
    await mcp.save()

    await browserContext.loginAs(admin)
    const page = await visit('/mcps')
    await page.getByRole('button', { name: 'Edit' }).click()
    const dialog = page.getByRole('dialog', { name: 'Edit Bearer MCP' })
    const url = dialog.locator('input[name="httpUrl"]')
    const keepsToken = dialog.getByLabel('Bearer token (leave blank to keep)')
    const warning = dialog.getByText('The saved token is not sent to a different server')

    await keepsToken.waitFor()

    // The same server at another path still receives the saved token.
    await url.fill('https://old.example/v2/mcp')
    assert.equal(await keepsToken.count(), 1)
    assert.equal(await warning.count(), 0)

    await url.fill('https://attacker.example/mcp')
    await warning.waitFor()
    assert.equal(await keepsToken.count(), 0)
    assert.equal(await dialog.locator('input[name="authBearer"]').count(), 1)

    await url.fill('https://old.example/mcp')
    await keepsToken.waitFor()
    assert.equal(await warning.count(), 0)
  })

  test('asks for environment values again once the package changes', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Package MCP',
      transport: 'npm',
      npmPackage: '@example/trusted-mcp',
      npmVersion: '1.0.0',
    })
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'API_KEY', value: 'saved-api-key' }])
    await mcp.save()

    await browserContext.loginAs(admin)
    const page = await visit('/mcps')
    await page.getByRole('button', { name: 'Edit' }).click()
    const dialog = page.getByRole('dialog', { name: 'Edit Package MCP' })
    const keepsValue = dialog.getByText('Leave blank to keep the saved value')
    const asksAgain = dialog.getByText('Enter the value again for the new package')

    await keepsValue.waitFor()

    await dialog.locator('input[name="npmVersion"]').fill('2.0.0')
    assert.equal(await keepsValue.count(), 1)

    await dialog.locator('input[name="npmPackage"]').fill('@example/other-mcp')
    await asksAgain.waitFor()
    assert.equal(await keepsValue.count(), 0)
    assert.notInclude(await page.locator('body').innerText(), 'saved-api-key')
  })
})
