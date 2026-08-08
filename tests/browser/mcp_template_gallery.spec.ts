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

    await gallery.getByRole('button', { name: 'Development' }).click()
    assert.include(await gallery.innerText(), 'GitHub')
    assert.include(await gallery.innerText(), 'Supabase')
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
    assert.equal(
      await gallery.evaluate((element) => {
        const dialog = element as unknown as { scrollWidth: number; clientWidth: number }
        return dialog.scrollWidth <= dialog.clientWidth
      }),
      true
    )
  })
})
