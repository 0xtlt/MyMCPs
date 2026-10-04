import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { createIcloudMailMcp, icloudMailSignIn, mockIcloudMail } from '#tests/helpers/icloud_mail'

test.group('Built-in iCloud Mail setup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('guides the admin from the template to a working MCP with chosen permissions', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const icloud = mockIcloudMail()

    try {
      const admin = await createAdmin()
      await browserContext.loginAs(admin)
      const page = await visit('/mcps')

      await page.getByRole('button', { name: 'Add MCP' }).click()
      const gallery = page.getByRole('dialog', { name: 'Add an MCP' })
      await gallery.getByRole('textbox', { name: 'Search templates' }).fill('mail')
      assert.include(await gallery.innerText(), '1 template')
      assert.include(await gallery.innerText(), 'Productivity · Built-in')
      await gallery.getByRole('button', { name: 'Set up iCloud Mail' }).click()

      const setup = page.getByRole('dialog', { name: 'Set up iCloud Mail' })
      const guide = setup.getByRole('list', { name: 'iCloud Mail setup' })
      await guide.getByText('Create an app-specific password').waitFor()
      assert.include(await setup.innerText(), 'Runs inside MyMCPs with an app-specific password')
      assert.equal(
        await guide.getByRole('link', { name: /your Apple Account/ }).getAttribute('href'),
        'https://account.apple.com/account/manage'
      )
      assert.equal(await setup.locator('input[name="transport"]').inputValue(), 'builtin')
      assert.equal(await setup.locator('input[name="builtinKey"]').inputValue(), 'icloud-mail')

      // Apple cannot scope the password, so the permissions are chosen here.
      // Only reading is allowed until the admin decides otherwise.
      assert.isTrue(await setup.getByRole('checkbox', { name: 'Read mail' }).isChecked())
      for (const name of ['Save drafts', 'Send mail', 'Organize mail']) {
        assert.isFalse(await setup.getByRole('checkbox', { name }).isChecked())
      }
      assert.equal(await setup.getByRole('checkbox', { name: 'Allow write access' }).count(), 0)

      await setup.getByRole('button', { name: 'Add MCP' }).click()
      await setup.getByText('Enter your iCloud Mail address, such as name@icloud.com').waitFor()
      await setup.getByText(/Your Apple Account password does not work here/).waitFor()

      await setup.getByRole('textbox', { name: 'iCloud Mail address' }).fill('thomas@icloud.com')
      await setup
        .getByRole('textbox', { name: 'App-specific password' })
        .fill(icloudMailSignIn.password)
      await setup
        .getByRole('textbox', { name: 'Other sender addresses' })
        .fill('hello@thomas.example, tt@icloud.com')
      await setup.getByRole('checkbox', { name: 'Save drafts' }).check()
      await setup.getByRole('button', { name: 'Add MCP' }).click()
      await page.getByText('MCP created').waitFor()

      const saved = await Mcp.findByOrFail('slug', 'icloud-mail')
      assert.equal(saved.status, 'ready')
      assert.equal(saved.builtinUsername, 'thomas@icloud.com')
      assert.equal(McpSecretStore.decrypt(saved.builtinPassword), icloudMailSignIn.password)
      assert.equal(saved.builtinPermissions, 'read draft')
      assert.equal(saved.builtinAliases, 'hello@thomas.example tt@icloud.com')
      assert.deepEqual(icloud.signIns, [icloudMailSignIn])

      const row = page.getByRole('row', { name: /iCloud Mail/ })
      assert.include(await row.innerText(), 'Built-in · iCloud Mail over IMAP and SMTP')
      assert.include(await row.innerText(), 'password')

      await row.getByRole('button', { name: 'Edit' }).click()
      const edit = page.getByRole('dialog', { name: 'Edit iCloud Mail' })
      await edit.getByText(/^Connected\./).waitFor()
      assert.equal(
        await edit.getByRole('textbox', { name: 'iCloud Mail address' }).inputValue(),
        'thomas@icloud.com'
      )
      assert.equal(
        await edit
          .getByRole('textbox', { name: 'App-specific password (leave blank to keep)' })
          .inputValue(),
        ''
      )
      assert.equal(
        await edit.getByRole('textbox', { name: 'Other sender addresses' }).inputValue(),
        'hello@thomas.example, tt@icloud.com'
      )
      assert.isTrue(await edit.getByRole('checkbox', { name: 'Save drafts' }).isChecked())
      assert.isFalse(await edit.getByRole('checkbox', { name: 'Send mail' }).isChecked())
      assert.equal(await edit.getByRole('button', { name: 'Re-authorize' }).count(), 0)
      assert.equal(await edit.getByRole('button', { name: 'Connect', exact: true }).count(), 0)

      await edit.getByRole('checkbox', { name: 'Read mail' }).uncheck()
      await edit.getByRole('checkbox', { name: 'Save drafts' }).uncheck()
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await edit.getByText('Allow at least one permission').waitFor()

      await edit.getByRole('checkbox', { name: 'Send mail' }).check()
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await page.getByText('MCP updated').waitFor()
      const updated = await Mcp.findOrFail(saved.id)
      assert.equal(updated.builtinPermissions, 'send')
      assert.equal(updated.builtinAliases, 'hello@thomas.example tt@icloud.com')
      assert.equal(McpSecretStore.decrypt(updated.builtinPassword), icloudMailSignIn.password)
    } finally {
      icloud.restore()
    }
  })

  test('reopens the dialog on a rejected password without offering to connect', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const icloud = mockIcloudMail({ rejectSignIn: true })

    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id)
      await browserContext.loginAs(admin)
      const page = await visit('/mcps')

      await page.getByRole('button', { name: 'Edit' }).click()
      const edit = page.getByRole('dialog', { name: 'Edit iCloud Mail' })
      await edit
        .getByRole('textbox', { name: 'App-specific password (leave blank to keep)' })
        .fill('zyxw-vuts-rqpo-nmlk')
      await edit.getByRole('button', { name: 'Save changes' }).click()

      const rejected = page.getByRole('dialog', { name: 'Edit iCloud Mail' })
      await rejected.getByText('Last connection error').waitFor()
      await rejected.getByText(/iCloud Mail rejected the sign-in/).waitFor()
      assert.equal(await rejected.getByText('Authorization required').count(), 0)
      assert.equal(await rejected.getByRole('button', { name: 'Connect', exact: true }).count(), 0)
      assert.equal(await rejected.getByText(/^Connected\./).count(), 0)

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(saved.status, 'error')
      assert.equal(icloud.signIns.at(-1)!.password, 'zyxw-vuts-rqpo-nmlk')
    } finally {
      icloud.restore()
    }
  })
})
