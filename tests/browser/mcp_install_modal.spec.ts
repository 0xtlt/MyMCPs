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

    const modal = page.getByRole('dialog', { name: 'Install MyMCPs' })
    await modal.getByRole('button', { name: 'Access token' }).click()
    const tokenInput = modal.getByPlaceholder('Paste your MyMCPs access token')
    assert.equal(await tokenInput.getAttribute('type'), 'password')
    await tokenInput.fill("token'quoted")
    const visibilityToggle = modal.getByRole('button', { name: 'Show access token' })
    assert.equal(await visibilityToggle.getAttribute('aria-pressed'), 'false')
    await visibilityToggle.click()
    assert.equal(await tokenInput.getAttribute('type'), 'text')
    assert.equal(
      await modal.getByRole('button', { name: 'Hide access token' }).getAttribute('aria-pressed'),
      'true'
    )

    assert.include(await modal.innerText(), '[mcp_servers.mymcps]')
    assert.include(await modal.innerText(), 'Authorization = "Bearer token\'quoted"')
    assert.notInclude(await modal.innerText(), '"X-MyMCPs-Tool-Mode" = "lazy"')

    const lazyModeToggle = modal.getByRole('switch', { name: 'Enable lazy tool mode' })
    assert.equal(await lazyModeToggle.isChecked(), false)
    await lazyModeToggle.click()
    assert.equal(await lazyModeToggle.isChecked(), true)
    assert.include(await modal.innerText(), '"X-MyMCPs-Tool-Mode" = "lazy"')

    await modal.getByRole('button', { name: 'Claude' }).click()
    assert.include(await modal.innerText(), 'claude mcp add --transport http --scope user mymcps')
    assert.include(await modal.innerText(), `'Authorization: Bearer token'"'"'quoted'`)
    assert.include(await modal.innerText(), `'X-MyMCPs-Tool-Mode: lazy'`)

    await modal.getByRole('button', { name: 'Cursor' }).click()
    assert.include(await modal.innerText(), '"mcpServers"')
    assert.include(await modal.innerText(), '"Authorization": "Bearer token\'quoted"')
    assert.include(await modal.innerText(), '"X-MyMCPs-Tool-Mode": "lazy"')
  })

  test('opens in OAuth mode without requiring a token', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    await page.getByRole('button', { name: 'Install MCP' }).click()

    const modal = page.getByRole('dialog', { name: 'Install MyMCPs' })
    assert.include(await modal.innerText(), 'No token to copy')
    assert.notInclude(await modal.innerText(), 'Authorization =')
    assert.notInclude(await modal.innerText(), '<YOUR_ACCESS_TOKEN>')

    await modal.getByRole('button', { name: 'Claude' }).click()
    assert.include(await modal.innerText(), 'claude mcp add --transport http --scope user mymcps')
    assert.notInclude(await modal.innerText(), '--header')

    await modal.getByRole('button', { name: 'Cursor' }).click()
    assert.include(await modal.innerText(), '"mcpServers"')
    assert.notInclude(await modal.innerText(), '"headers"')
  })

  test('opens with the newly created access token pre-filled and visible', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    await page.getByRole('button', { name: 'Create token' }).click()
    const createDialog = page.getByRole('dialog', { name: 'Create token' })
    await createDialog.getByLabel('Name').fill('Fresh install token')
    await createDialog.getByRole('button', { name: 'Create token' }).click()

    const installDialog = page.getByRole('dialog', { name: 'Install MyMCPs' })
    await installDialog.waitFor()
    const tokenInput = installDialog.getByPlaceholder('Paste your MyMCPs access token')
    assert.match(await tokenInput.inputValue(), /^mcp_[A-Za-z0-9_-]{43}$/)
    assert.equal(await tokenInput.getAttribute('type'), 'text')
    const visibilityToggle = installDialog.getByRole('button', { name: 'Hide access token' })
    assert.equal(await visibilityToggle.getAttribute('aria-pressed'), 'true')
    await visibilityToggle.click()
    assert.equal(await tokenInput.getAttribute('type'), 'password')
  })
})
