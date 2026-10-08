import { readFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import Mcp from '#models/mcp'
import User from '#models/user'
import {
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
  BACKUP_PASSWORD,
  backupLeftovers,
  openBackupFile,
  resetBackupTests,
  scratchPath,
  untilNoBackupLeftovers,
} from '#tests/helpers/backup'
import { prepareTestDatabase } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('backup browser flow', (group) => {
  group.each.setup(resetBackupTests)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(resetBackupTests)

  test('exports a backup from Settings and imports it on a new instance', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin({
      email: ADMIN_EMAIL,
      password: ADMIN_PASSWORD,
      fullName: 'Backup Admin',
    })
    await createMcp(admin.id, { name: 'Kept MCP' })
    await browserContext.loginAs(admin)
    const settings = await visit('/settings')

    await settings.getByRole('heading', { name: 'Backup' }).waitFor()
    await settings
      .getByText('Users, MCPs with their credentials, access tokens, call logs and settings.')
      .waitFor()
    const open = settings.getByRole('button', { name: 'Export backup' })
    const dialog = settings.getByRole('dialog', { name: 'Export backup' })
    // The label of a required field says so after its name.
    const password = dialog.getByLabel(/^Backup password/)
    const confirmation = dialog.getByLabel(/^Confirm backup password/)
    const currentPassword = dialog.getByLabel(/^Current password/)
    const submit = dialog.getByRole('button', { name: 'Export backup' })

    // A wrong current password: the browser comes back to the page, which says so.
    await open.click()
    await dialog
      .getByText(
        'The file is encrypted with the password you choose. It cannot be opened without it.'
      )
      .waitFor()
    await dialog.getByText('Your account password, to confirm it is you.').waitFor()
    await password.fill(BACKUP_PASSWORD)
    await confirmation.fill(BACKUP_PASSWORD)
    await currentPassword.fill('not-my-password')
    await submit.click()
    await dialog.getByText('The current password is incorrect').waitFor()
    await settings.assertPath('/settings')
    assert.equal(await password.inputValue(), '')
    assert.equal(await currentPassword.inputValue(), '')
    assert.deepEqual(await backupLeftovers(), [])

    // The right one: the browser saves the file and stays on the page.
    await password.fill(BACKUP_PASSWORD)
    await confirmation.fill(BACKUP_PASSWORD)
    await currentPassword.fill(ADMIN_PASSWORD)
    const downloading = settings.waitForEvent('download')
    await submit.click()
    const download = await downloading
    assert.match(download.suggestedFilename(), /^mymcps-backup-\d{8}-\d{6}\.mymcps$/)
    const path = await scratchPath(download.suggestedFilename())
    await download.saveAs(path)

    // The dialog has closed, and nothing of what was typed is left in it.
    await dialog.waitFor({ state: 'hidden' })
    await settings.assertPath('/settings')
    await open.click()
    for (const field of [password, confirmation, currentPassword]) {
      assert.equal(await field.inputValue(), '')
    }
    assert.equal(await dialog.getByText('The current password is incorrect').count(), 0)
    await dialog.getByRole('button', { name: 'Cancel' }).click()
    await dialog.waitFor({ state: 'hidden' })

    const backup = await openBackupFile(BACKUP_PASSWORD, await readFile(path))
    assert.equal(backup.database.subarray(0, 15).toString('latin1'), 'SQLite format 3')
    await untilNoBackupLeftovers()
    await settings.close()

    /** A new instance: no account yet, and a browser that never saw it. */
    await prepareTestDatabase()
    await browserContext.clearCookies()
    const page = await visit('/')

    await page.assertPath('/onboarding')
    await page.getByRole('link', { name: 'Import a backup' }).click()
    await page.waitForURL((url) => url.pathname === '/onboarding/import')
    await page.assertText('h1', 'Import a backup')
    await page
      .getByText(
        'Restore the users, MCPs, access tokens, call logs and settings of another MyMCPs instance.'
      )
      .waitFor()
    await page.getByRole('link', { name: 'Create a new instance instead' }).waitFor()

    // Nothing chosen yet.
    const importing = page.getByRole('button', { name: 'Import backup' })
    await importing.click()
    await page.getByText('Choose a backup file').first().waitFor()
    await page.getByText('Enter the password of the backup').first().waitFor()

    // The wrong password, then the right one, without choosing the file again.
    await page.locator('input[type="file"]').setInputFiles(path)
    // The name is also announced to screen readers, in a region of its own.
    await page.getByText(download.suggestedFilename(), { exact: true }).waitFor()
    await page.getByLabel('Backup password').fill('not-the-password')
    await importing.click()
    await page
      .getByText('The password is incorrect, or the backup file is damaged')
      .first()
      .waitFor()
    assert.lengthOf(await User.all(), 0)
    assert.deepEqual(await backupLeftovers(), [])

    await page.getByLabel('Backup password').fill(BACKUP_PASSWORD)
    await importing.click()
    await page.waitForURL((url) => url.pathname === '/login')
    await page
      .locator('[data-flash-toast]')
      .getByText('Backup imported. Sign in with an account of the imported instance.')
      .waitFor()
    assert.deepEqual(await backupLeftovers(), [])

    // The instance is the one that was exported: its administrator signs in.
    await page.getByLabel('Email').fill(ADMIN_EMAIL)
    await page.getByLabel('Password').fill(ADMIN_PASSWORD)
    await page.getByRole('button', { name: 'Sign in' }).click()
    await page.waitForURL((url) => url.pathname === '/')
    await page.assertTextContains('body', 'Signed in as Backup Admin')
    const mcps = await Mcp.all()
    assert.deepEqual(
      mcps.map((mcp) => mcp.name),
      ['Kept MCP']
    )
  })
})
