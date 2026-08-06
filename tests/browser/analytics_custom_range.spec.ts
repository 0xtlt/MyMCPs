import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcpCallLog, createStoredAccessToken } from '#tests/helpers/factories'

test.group('analytics custom range', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('applies a custom date and time range without adding chart controls', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id, { name: 'Browser analytics token' })
    await createMcpCallLog(token, { createdAt: DateTime.utc().minus({ hours: 4 }) })
    await createMcpCallLog(token, { createdAt: DateTime.utc().minus({ hours: 2 }) })

    await browserContext.loginAs(admin)
    const page = await visit('/analytics?range=24h&timeZone=UTC')
    await page.getByRole('heading', { name: 'Calls and errors over time' }).waitFor()

    await page.getByRole('radio', { name: 'Custom' }).click()
    const dialog = page.getByRole('dialog', { name: 'Custom analytics range' })
    await dialog.waitFor()
    assert.equal(await dialog.locator('.astryx-date-range-input').count(), 1)
    assert.equal(await dialog.getByLabel('Start time').count(), 1)
    assert.equal(await dialog.getByLabel('End time').count(), 1)
    assert.isFalse(await dialog.getByRole('button', { name: 'Apply range' }).isDisabled())
    await dialog.getByRole('button', { name: 'Apply range' }).click()
    await page.waitForURL((url) => url.searchParams.get('range') === 'custom')

    const selectedUrl = new URL(page.url())
    assert.equal(selectedUrl.searchParams.get('range'), 'custom')
    assert.isNotNull(selectedUrl.searchParams.get('start'))
    assert.isNotNull(selectedUrl.searchParams.get('end'))
    assert.equal(await page.getByRole('slider').count(), 0)

    await page.getByRole('radio', { name: 'Custom' }).click()
    const reopenedDialog = page.getByRole('dialog', { name: 'Custom analytics range' })
    await reopenedDialog.waitFor()
    await reopenedDialog.getByRole('button', { name: 'Cancel' }).click()
    await reopenedDialog.waitFor({ state: 'hidden' })

    await page.setViewportSize({ width: 430, height: 932 })
    await page.waitForTimeout(500)
    const hasHorizontalOverflow = await page.evaluate(() => {
      const browser = globalThis as unknown as {
        document: { documentElement: { scrollWidth: number } }
        innerWidth: number
      }
      return browser.document.documentElement.scrollWidth > browser.innerWidth
    })
    assert.isFalse(hasHorizontalOverflow)
  })
})
