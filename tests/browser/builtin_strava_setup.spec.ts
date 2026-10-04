import { test } from '@japa/runner'
import type { BrowserContext } from 'playwright'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { createStravaMcp, mockStrava } from '#tests/helpers/strava'

/**
 * Stand in for Strava's consent screen. Playwright does not intercept the
 * target of a redirect, so read where MyMCPs sends the browser and show a page
 * whose link returns to the callback the way Strava does on approval, granting
 * every scope that was requested.
 */
async function stubStravaConsent(browserContext: BrowserContext) {
  const authorizations: URL[] = []
  await browserContext.route('**/mcps/*/oauth/start', async (route) => {
    const redirect = await route.fetch({ maxRedirects: 0 })
    const authorization = new URL(redirect.headers()['location'])
    authorizations.push(authorization)
    // Like Strava, list the granted scopes with literal commas.
    const callback = [
      `${authorization.searchParams.get('redirect_uri')}?state=${authorization.searchParams.get('state')}`,
      'code=browser-strava-code',
      `scope=${authorization.searchParams.get('scope')}`,
    ].join('&amp;')
    await route.fulfill({
      contentType: 'text/html',
      body: `<a id="authorize" href="${callback}">Authorize</a>`,
    })
  })
  return authorizations
}

test.group('Built-in Strava setup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('guides the admin from the template to a connected Strava account', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const strava = mockStrava()
    const authorizations = await stubStravaConsent(browserContext)

    try {
      const admin = await createAdmin()
      await browserContext.loginAs(admin)
      const page = await visit('/mcps')

      await page.getByRole('button', { name: 'Add MCP' }).click()
      const gallery = page.getByRole('dialog', { name: 'Add an MCP' })
      await gallery.getByRole('button', { name: 'Health & fitness' }).click()
      assert.include(await gallery.innerText(), '1 template')
      assert.include(await gallery.innerText(), 'Health & fitness · Built-in')
      await gallery.getByRole('button', { name: 'Set up Strava' }).click()

      const setup = page.getByRole('dialog', { name: 'Set up Strava' })
      const guide = setup.getByRole('list', { name: 'Strava setup' })
      await guide.getByText('Authorization Callback Domain').waitFor()
      assert.include(await guide.innerText(), 'http://localhost:3333')
      assert.equal(
        await guide.getByRole('link', { name: /Strava API settings/ }).getAttribute('href'),
        'https://www.strava.com/settings/api'
      )
      assert.equal(await setup.getByRole('radio').count(), 0)
      assert.equal(await setup.locator('input[name="transport"]').inputValue(), 'builtin')
      assert.equal(await setup.locator('input[name="builtinKey"]').inputValue(), 'strava')
      assert.isFalse(await setup.getByRole('checkbox', { name: 'Allow write access' }).isChecked())

      await setup.getByRole('button', { name: 'Add MCP' }).click()
      await setup.getByText('Enter the Client ID of your Strava API application').waitFor()
      await setup.getByText('Enter the Client Secret of your Strava API application').waitFor()
      assert.equal(
        await setup.evaluate((element) => (element as unknown as { scrollTop: number }).scrollTop),
        0
      )

      await setup.getByRole('textbox', { name: 'Client ID' }).fill('123456')
      await setup.getByRole('textbox', { name: 'Client Secret' }).fill('strava-client-secret')
      await setup.getByRole('button', { name: 'Add MCP' }).click()

      const edit = page.getByRole('dialog', { name: 'Edit Strava' })
      await edit.getByText('Authorization required').waitFor()
      assert.equal(await edit.getByRole('textbox', { name: 'Client ID' }).inputValue(), '123456')
      assert.equal(
        await edit
          .getByRole('textbox', { name: 'Client Secret (leave blank to keep)' })
          .inputValue(),
        ''
      )
      assert.lengthOf(strava.requests, 0)

      await edit.getByRole('button', { name: 'Connect', exact: true }).click()
      await page.locator('#authorize').click()
      await page.getByText('OAuth connected').waitFor()

      assert.lengthOf(authorizations, 1)
      assert.equal(
        authorizations[0].origin + authorizations[0].pathname,
        'https://www.strava.com/oauth/authorize'
      )
      assert.equal(authorizations[0].searchParams.get('client_id'), '123456')
      assert.equal(
        authorizations[0].searchParams.get('scope'),
        'read,read_all,profile:read_all,activity:read_all'
      )
      assert.equal(new URL(page.url()).pathname, '/mcps')
      assert.equal(new URL(page.url()).search, '')

      const connected = page.getByRole('dialog', { name: 'Edit Strava' })
      await connected.getByText(/^Connected\./).waitFor()
      await connected.getByRole('button', { name: 'Re-authorize' }).waitFor()
      assert.equal(await connected.getByText('Authorization required').count(), 0)

      const saved = await Mcp.findByOrFail('slug', 'strava')
      assert.equal(saved.status, 'ready')
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'strava-access-token')
      assert.equal(saved.oauthScopes, 'read read_all profile:read_all activity:read_all')
      assert.isFalse(saved.builtinWriteEnabled)
      assert.equal(strava.tokenRequests()[0].form!.get('code'), 'browser-strava-code')
    } finally {
      strava.restore()
    }
  })

  test('asks to re-authorize after write access is allowed on a connected account', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const strava = mockStrava()
    const authorizations = await stubStravaConsent(browserContext)

    try {
      const admin = await createAdmin()
      const mcp = await createStravaMcp(admin.id)
      await browserContext.loginAs(admin)
      const page = await visit('/mcps')

      await page.getByRole('button', { name: 'Edit' }).click()
      const edit = page.getByRole('dialog', { name: 'Edit Strava' })
      assert.equal(await edit.getByText('Write access not granted yet').count(), 0)
      await edit.getByRole('checkbox', { name: 'Allow write access' }).check()
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await page.getByText('MCP updated').waitFor()

      // Saving closes the dialog. The account stays connected with its read-only grant.
      await page.getByRole('button', { name: 'Edit' }).click()
      const pending = page.getByRole('dialog', { name: 'Edit Strava' })
      await pending.getByText('Write access not granted yet').waitFor()
      assert.isTrue(await pending.getByRole('checkbox', { name: 'Allow write access' }).isChecked())

      await pending.getByRole('button', { name: 'Re-authorize' }).click()
      await page.locator('#authorize').click()
      await page.getByText('OAuth connected').waitFor()

      assert.equal(
        authorizations[0].searchParams.get('scope'),
        'read,read_all,profile:read_all,activity:read_all,activity:write,profile:write'
      )
      const granted = page.getByRole('dialog', { name: 'Edit Strava' })
      await granted.getByText(/^Connected\./).waitFor()
      assert.equal(await granted.getByText('Write access not granted yet').count(), 0)

      const saved = await Mcp.findOrFail(mcp.id)
      assert.isTrue(saved.builtinWriteEnabled)
      assert.include(saved.oauthScopes!, 'activity:write')
      assert.include(saved.oauthScopes!, 'profile:write')
    } finally {
      strava.restore()
    }
  })

  test('lists a connected Strava MCP as a built-in endpoint', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await createStravaMcp(admin.id)
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    const row = page.getByRole('row', { name: /Strava/ })
    assert.include(await row.innerText(), 'Built-in · Strava API')
    assert.include(await row.innerText(), 'oauth')
  })
})
