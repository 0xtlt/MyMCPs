import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

test.group('MCP environment variable form', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('adds and removes multiple environment variable rows', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Add MCP' }).click()
    await page.getByRole('button', { name: 'Custom MCP' }).click()
    await page.getByRole('radio', { name: /npm package/ }).click()
    await page.getByRole('button', { name: 'Add variable' }).click()
    await page.getByRole('button', { name: 'Add variable' }).click()

    const names = page.locator('input[name^="npmEnv"][name$="[name]"]')
    const values = page.locator('input[name^="npmEnv"][name$="[value]"]')
    assert.equal(await names.count(), 2)
    assert.equal(await values.count(), 2)

    await names.nth(0).fill('API_KEY')
    await values.nth(0).fill('secret-one')
    await names.nth(1).fill('REGION')
    await values.nth(1).fill('eu-west-3')
    await page.getByRole('button', { name: 'Remove REGION' }).click()

    assert.equal(await names.count(), 1)
    assert.equal(await values.count(), 1)
    assert.notInclude(await page.locator('body').innerText(), 'secret-one')

    await page.locator('input[name="name"]').fill('Browser environment MCP')
    await page.locator('input[name="npmPackage"]').fill('@example/environment-mcp')
    await page.route('**/mcps', async (route) => {
      if (route.request().method() === 'POST') {
        await route.abort()
        return
      }
      await route.continue()
    })
    const submission = page.waitForRequest(
      (request) => request.method() === 'POST' && new URL(request.url()).pathname === '/mcps'
    )
    await page
      .getByRole('dialog', { name: 'Set up a custom MCP' })
      .getByRole('button', { name: 'Add MCP' })
      .click()

    const submittedRequest = await submission
    const postData = submittedRequest.postData() ?? ''
    assert.include(postData, 'API_KEY')
    assert.include(postData, 'secret-one')
  })
})
