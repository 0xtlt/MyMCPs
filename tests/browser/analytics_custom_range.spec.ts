import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcpCallLog, createStoredAccessToken } from '#tests/helpers/factories'

test.group('analytics custom range', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('applies and preserves a responsive custom date and time range', async ({
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
    assert.match(selectedUrl.searchParams.get('start')!, /Z$/)
    assert.match(selectedUrl.searchParams.get('end')!, /Z$/)
    assert.equal(await page.getByRole('slider').count(), 0)

    const sharedStart = '2026-10-25T00:30Z'
    const sharedEnd = '2026-10-25T01:30Z'
    await page.goto(
      `/analytics?range=custom&start=${encodeURIComponent(sharedStart)}&end=${encodeURIComponent(sharedEnd)}&timeZone=Europe%2FParis`
    )
    await page.getByRole('radio', { name: 'Custom', checked: true }).waitFor()
    await page.waitForTimeout(250)
    const sharedUrl = new URL(page.url())
    assert.equal(sharedUrl.searchParams.get('start'), sharedStart)
    assert.equal(sharedUrl.searchParams.get('end'), sharedEnd)
    assert.equal(sharedUrl.searchParams.get('timeZone'), 'Europe/Paris')

    await page.setViewportSize({ width: 430, height: 932 })
    await page.getByRole('radio', { name: 'Custom' }).click()
    const mobileDialog = page.getByRole('dialog', { name: 'Custom analytics range' })
    await mobileDialog.waitFor()
    await mobileDialog.getByRole('button', { name: /^Dates:/ }).click()
    const calendarDialog = page.getByRole('dialog', { name: 'Choose date range' })
    await calendarDialog.waitFor()
    await page.waitForTimeout(250)
    const hasHorizontalOverflow = await page.evaluate(() => {
      const browser = globalThis as unknown as {
        document: { documentElement: { scrollWidth: number } }
        innerWidth: number
      }
      return browser.document.documentElement.scrollWidth > browser.innerWidth
    })
    assert.isFalse(hasHorizontalOverflow)
    await mobileDialog.getByRole('button', { name: 'Close calendar' }).first().click()
    await mobileDialog.getByRole('button', { name: 'Apply range' }).click()
    await page.waitForURL((url) => url.searchParams.get('range') === 'custom')
  })
})
