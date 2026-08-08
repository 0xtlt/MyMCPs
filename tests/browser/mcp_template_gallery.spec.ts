import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

test.group('MCP template gallery', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('filters templates and prefills their setup form', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Add MCP' }).click()

    const gallery = page.getByRole('dialog', { name: 'Add an MCP' })
    assert.include(await gallery.innerText(), 'Notion')
    assert.include(await gallery.innerText(), 'Shopify Dev')
    assert.include(await gallery.innerText(), 'Atlassian Rovo')

    await gallery.getByRole('button', { name: 'Development' }).click()
    assert.include(await gallery.innerText(), 'GitHub')
    assert.include(await gallery.innerText(), 'Supabase')
    assert.include(await gallery.innerText(), 'Postman')
    assert.include(await gallery.innerText(), 'Microsoft Learn')
    assert.notInclude(await gallery.innerText(), 'Notion')

    await gallery.getByRole('textbox', { name: 'Search templates' }).fill('Shopify')
    assert.include(await gallery.innerText(), '1 template')
    assert.include(await gallery.innerText(), 'Shopify Dev')
    assert.notInclude(await gallery.innerText(), 'GitHub')

    await gallery.getByRole('button', { name: 'Set up Shopify Dev' }).click()

    const setup = page.getByRole('dialog', { name: 'Set up Shopify Dev' })
    assert.equal(await setup.getByLabel('Name').inputValue(), 'Shopify Dev')
    assert.equal(await setup.locator('input[name="npmPackage"]').inputValue(), '@shopify/dev-mcp')
    assert.equal(await setup.getByLabel('Version').inputValue(), 'latest')
    assert.equal(await setup.getByRole('radio', { name: /npm package/ }).isChecked(), true)

    await setup.getByRole('button', { name: 'Back to templates' }).click()
    const returnedGallery = page.getByRole('dialog', { name: 'Add an MCP' })
    assert.include(await returnedGallery.innerText(), 'Notion')
    assert.include(await returnedGallery.innerText(), 'Shopify Dev')
  })

  test('prefills newly added remote and npm templates', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Add MCP' }).click()
    let gallery = page.getByRole('dialog', { name: 'Add an MCP' })
    await gallery.getByRole('button', { name: 'All' }).click()
    assert.include(await gallery.innerText(), '20 templates')

    await gallery.getByRole('textbox', { name: 'Search templates' }).fill('Atlassian')
    await gallery.getByRole('button', { name: 'Set up Atlassian Rovo' }).click()
    let setup = page.getByRole('dialog', { name: 'Set up Atlassian Rovo' })
    assert.equal(
      await setup.getByLabel('HTTP URL').inputValue(),
      'https://mcp.atlassian.com/v1/mcp/authv2'
    )

    await setup.getByRole('button', { name: 'Back to templates' }).click()
    gallery = page.getByRole('dialog', { name: 'Add an MCP' })
    await gallery.getByRole('textbox', { name: 'Search templates' }).fill('MongoDB')
    await gallery.getByRole('button', { name: 'Set up MongoDB' }).click()
    setup = page.getByRole('dialog', { name: 'Set up MongoDB' })
    assert.equal(await setup.locator('input[name="npmPackage"]').inputValue(), 'mongodb-mcp-server')
    assert.equal(await setup.getByLabel('Extra args').inputValue(), '--readOnly')
    assert.equal(
      await setup.locator('input[name="npmEnv[0][name]"]').inputValue(),
      'MDB_MCP_CONNECTION_STRING'
    )
  })

  test('keeps every category available without horizontal overflow on mobile', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')
    await page.setViewportSize({ width: 390, height: 844 })

    await page.getByRole('button', { name: 'Add MCP' }).click()
    const gallery = page.getByRole('dialog', { name: 'Add an MCP' })

    await gallery.getByRole('button', { name: 'Categories' }).click()
    await page.getByRole('menuitemradio', { name: 'Infrastructure' }).click()

    assert.include(await gallery.innerText(), 'Vercel')
    assert.include(await gallery.innerText(), 'Cloudflare Docs')
    assert.include(await gallery.innerText(), 'Firebase')
    assert.include(await gallery.innerText(), 'MongoDB')
    assert.include(await gallery.innerText(), 'Neon')
    assert.equal(
      await gallery.evaluate((element) => {
        const dialog = element as unknown as { scrollWidth: number; clientWidth: number }
        return dialog.scrollWidth <= dialog.clientWidth
      }),
      true
    )
  })
})
