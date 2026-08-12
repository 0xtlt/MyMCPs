import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'

function getBackgroundColor(element: unknown) {
  const browser = globalThis as unknown as {
    getComputedStyle: (target: unknown) => { backgroundColor: string }
  }
  return browser.getComputedStyle(element).backgroundColor
}

async function createCleanupTokens() {
  const admin = await createAdmin()
  await createAccessToken(admin.id, { name: 'Active token' })
  await createAccessToken(admin.id, {
    name: 'Expired token',
    expiresAt: DateTime.utc().minus({ minutes: 1 }),
  })
  const revoked = await createAccessToken(admin.id, { name: 'Revoked token' })
  revoked.token.revokedAt = DateTime.utc()
  await revoked.token.save()
  return admin
}

test.group('access token cleanup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('confirms single, selected, and bulk token deletion at 1440px', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createCleanupTokens()

    await browserContext.loginAs(admin)
    const page = await visit('/tokens')
    await page.setViewportSize({ width: 1440, height: 900 })

    const expiredRow = page.getByRole('row').filter({ hasText: 'Expired token' })
    const rowDeleteButton = expiredRow.getByRole('button', { name: 'Delete' })
    const neutralTriggerBackground = await rowDeleteButton.evaluate(getBackgroundColor)
    await rowDeleteButton.click()
    const singleDialog = page.getByRole('alertdialog', { name: 'Delete tokens?' })
    await singleDialog.getByText('Permanently delete 1 token?').waitFor()
    const destructiveConfirmationBackground = await singleDialog
      .getByRole('button', { name: 'Delete tokens' })
      .evaluate(getBackgroundColor)
    assert.notEqual(neutralTriggerBackground, destructiveConfirmationBackground)
    await singleDialog.getByRole('button', { name: 'Cancel' }).click()

    await page.getByRole('checkbox', { name: 'Select Expired token' }).click()
    const deleteSelectedButton = page.getByRole('button', { name: 'Delete selected' })
    assert.equal(await deleteSelectedButton.evaluate(getBackgroundColor), neutralTriggerBackground)
    assert.equal(
      await page.getByRole('button', { name: 'Delete all' }).evaluate(getBackgroundColor),
      neutralTriggerBackground
    )
    await deleteSelectedButton.click()
    const selectedDialog = page.getByRole('alertdialog', { name: 'Delete tokens?' })
    await selectedDialog.getByRole('button', { name: 'Delete tokens' }).click()
    await selectedDialog.waitFor({ state: 'hidden' })
    await page.getByLabel('Notifications').getByText('1 token deleted').waitFor()

    await page.getByRole('button', { name: 'Delete all' }).click()
    const clearDialog = page.getByRole('alertdialog', {
      name: 'Delete all expired and revoked tokens?',
    })
    await clearDialog.getByRole('button', { name: 'Delete all' }).click()
    await clearDialog.waitFor({ state: 'hidden' })

    await page.getByText('Active token').waitFor()
    await page.getByText('Expired token').waitFor({ state: 'detached' })
    await page.getByText('Revoked token').waitFor({ state: 'detached' })
  })

  test('keeps mobile cleanup controls and confirmation usable at 375px', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createCleanupTokens()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')
    await page.setViewportSize({ width: 375, height: 812 })

    await page.getByRole('heading', { name: 'Expired & revoked' }).waitFor()
    await page.getByRole('heading', { name: 'Active', exact: true }).waitFor()
    assert.equal(await page.getByRole('toolbar', { name: 'Token cleanup' }).count(), 0)
    assert.isTrue(await page.getByRole('button', { name: 'Actions for Expired token' }).isVisible())
    await page.getByRole('checkbox', { name: 'Select Expired token for deletion' }).click()
    await page.getByRole('button', { name: 'Delete 1' }).click()
    const dialog = page.getByRole('alertdialog', { name: 'Delete tokens?' })
    await dialog.getByText('Permanently delete 1 token?').waitFor()
    assert.isTrue(await dialog.getByRole('button', { name: 'Delete tokens' }).isVisible())
    assert.isTrue(
      await page.evaluate(() => {
        const browser = globalThis as unknown as {
          document: { documentElement: { scrollWidth: number } }
          innerWidth: number
        }
        return browser.document.documentElement.scrollWidth <= browser.innerWidth
      })
    )
    await dialog.getByRole('button', { name: 'Cancel' }).click()
  })

  test('keeps bulk cleanup and confirmation usable at 768px', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createCleanupTokens()
    await browserContext.loginAs(admin)
    const page = await visit('/tokens')
    await page.setViewportSize({ width: 768, height: 1024 })

    await page.getByRole('button', { name: 'Delete all' }).click()
    const dialog = page.getByRole('alertdialog', {
      name: 'Delete all expired and revoked tokens?',
    })
    await dialog.getByText('Permanently delete 2 tokens?').waitFor()
    assert.isTrue(await dialog.getByRole('button', { name: 'Delete all' }).isVisible())
    assert.isTrue(
      await page.evaluate(() => {
        const browser = globalThis as unknown as {
          document: { documentElement: { scrollWidth: number } }
          innerWidth: number
        }
        return browser.document.documentElement.scrollWidth <= browser.innerWidth
      })
    )
    await dialog.getByRole('button', { name: 'Cancel' }).click()
  })
})
