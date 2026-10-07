import { test } from '@japa/runner'
import ApprovalRequest from '#models/approval_request'
import Mcp from '#models/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'
import { gatewayRpc, resultText } from '#tests/helpers/gateway'
import { createGoogleAdsMcp, mockGoogleAds } from '#tests/helpers/google_ads'

test.group('Tool approvals', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('takes a person from the link an agent was given to an approved call', async ({
    assert,
    client,
    visit,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin({ email: 'owner@example.com', fullName: 'Olga Owner' })
      const mcp = await createGoogleAdsMcp(admin.id, { writeEnabled: true })
      const { plaintext } = await createAccessToken(admin.id, { name: 'Claude' })
      const call = {
        name: 'google-ads__update_campaign_budget',
        arguments: { customer_id: '123-456-7890', campaign_id: '111', daily_budget: 250 },
      }

      const held = resultText(await gatewayRpc(client, plaintext, 'tools/call', call))
      const [link] = held.match(/http:\/\/localhost:3333\/approvals\/[\w-]{32}/)!

      // The link grants nothing: whoever follows it signs in first, then lands on the request.
      const page = await visit(link)
      await page.getByRole('heading', { name: 'Sign in' }).waitFor()
      await page.getByRole('textbox', { name: 'Email' }).fill('owner@example.com')
      await page.getByRole('textbox', { name: 'Password' }).fill('password123')
      await page.getByRole('button', { name: 'Sign in' }).click()

      await page
        .getByRole('heading', {
          name: 'Change the daily budget of the campaign "Spring sale" from €2.50 to €250.00',
        })
        .waitFor()
      const request = await page.locator('body').innerText()
      assert.include(request, 'An agent using the access token “Claude” wants to do this')
      assert.include(request, 'The new budget is 100 times the current one.')
      assert.include(request, '€250.00 a day')
      assert.include(request, 'Now: €2.50 a day')
      assert.include(request, 'Acme Shoes (123-456-7890)')

      await page.getByRole('button', { name: 'Approve' }).click()
      await page.getByText('Approved by Olga Owner').waitFor()
      assert.equal(await page.getByRole('button', { name: 'Approve' }).count(), 0)
      assert.lengthOf(google.mutations(), 0)

      const approved = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.include(resultText(approved), '"daily_budget":250')
      assert.lengthOf(google.mutations(), 1)

      await page.getByRole('link', { name: 'Approvals' }).first().click()
      await page.getByRole('heading', { name: 'Decided or expired' }).waitFor()
      assert.include(await page.locator('body').innerText(), 'Approved and run')
      const saved = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      assert.equal(saved.state, 'used')
    } finally {
      google.restore()
    }
  })

  test('lets the admin choose which tools ask, from the MCP dialog', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const mcp = await createGoogleAdsMcp(admin.id, { writeEnabled: true })
      await browserContext.loginAs(admin)
      const page = await visit(`/mcps/${mcp.id}`)

      const dialog = page.getByRole('dialog', { name: 'Edit Google Ads' })
      await dialog.getByRole('link', { name: 'Tool approvals' }).click()
      await page.getByRole('heading', { name: 'Tool approvals' }).waitFor()
      await page.getByText('4 of 28 tools ask for approval').waitFor()

      await page.getByRole('textbox', { name: 'Find a tool' }).fill('keyword')
      const addKeywords = page.getByRole('radiogroup', { name: 'When an agent calls add_keywords' })
      assert.isTrue(await addKeywords.getByRole('radio', { name: 'Runs' }).isChecked())
      await addKeywords.getByRole('radio', { name: 'Asks' }).click()
      await page.getByText('1 unsaved change').waitFor()

      await page.getByRole('button', { name: 'Save' }).click()
      await page.getByText('Tool approvals saved').waitFor()
      await page.getByText('5 of 28 tools ask for approval').waitFor()
      const saved = await Mcp.findOrFail(mcp.id)
      assert.deepEqual(JSON.parse(saved.toolApprovals!), { add_keywords: 'ask' })
    } finally {
      google.restore()
    }
  })

  test('offers Google Ads among the templates and walks through its setup', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Add MCP' }).click()
    const gallery = page.getByRole('dialog', { name: 'Add an MCP' })
    await gallery.getByRole('button', { name: 'Marketing' }).click()
    assert.include(await gallery.innerText(), 'Marketing · Built-in')
    await gallery.getByRole('button', { name: 'Set up Google Ads' }).click()

    const setup = page.getByRole('dialog', { name: 'Set up Google Ads' })
    const guide = setup.getByRole('list', { name: 'Google Ads setup' })
    await guide.getByText('Authorized redirect URI').waitFor()
    assert.include(await guide.innerText(), 'http://localhost:3333/mcps/oauth/callback')
    assert.equal(await setup.getByRole('textbox', { name: /Developer token/ }).count(), 0)
    await setup.getByRole('textbox', { name: 'Manager account ID' }).fill('12345')

    await setup.getByRole('button', { name: 'Add MCP' }).click()
    await setup.getByText('Enter the ID of the manager account, such as 123-456-7890').waitFor()
    await setup.getByText('Enter the Client ID of your Google Ads API application').waitFor()
    assert.lengthOf(await Mcp.all(), 0)
  })
})
