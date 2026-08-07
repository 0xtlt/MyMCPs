import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createInvite,
  createMcpCallLog,
  createMember,
  createStoredAccessToken,
} from '#tests/helpers/factories'

test.group('day-first date formatting', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('renders dates consistently across desktop, mobile, and selected timezones', async ({
    browserContext,
    visit,
  }) => {
    const displayedAt = DateTime.fromISO('2035-09-06T11:13:52Z')
    const admin = await createAdmin()
    const member = await createMember({ fullName: 'Date Member' })
    member.createdAt = displayedAt
    await member.save()

    await createAccessToken(admin.id, {
      name: 'Day-first token',
      expiresAt: displayedAt,
    })
    await createInvite(admin.id, {
      email: 'day-first@example.com',
      expiresAt: displayedAt,
    })

    const logToken = await createStoredAccessToken(admin.id, { name: 'Date log token' })
    await createMcpCallLog(logToken, {
      requestedToolName: 'date__format',
      createdAt: DateTime.fromISO('2035-09-06T23:30:00Z'),
    })

    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    const tokenRow = page.getByRole('row').filter({ hasText: 'Day-first token' })
    await tokenRow.getByText(/^06\/09\/2035,/).waitFor()

    await page.setViewportSize({ width: 430, height: 932 })
    await page.goto('/tokens')
    await page.getByText('Expires 06/09/2035').waitFor()

    await page.setViewportSize({ width: 1280, height: 800 })
    await page.goto('/invites')
    const memberRow = page.getByRole('row').filter({ hasText: 'Date Member' })
    await memberRow.getByText(/^06\/09\/2035,/).waitFor()
    await page.getByRole('radio', { name: /Invites/ }).click()
    const inviteRow = page.getByRole('row').filter({ hasText: 'day-first@example.com' })
    await inviteRow.getByText(/^06\/09\/2035,/).waitFor()

    await page.setViewportSize({ width: 430, height: 932 })
    await page.goto('/invites')
    await page.getByRole('radio', { name: /Invites/ }).click()
    await page.getByText('Expires 06/09/2035').waitFor()

    const browserSession = await browserContext.newCDPSession(page)
    await browserSession.send('Emulation.setTimezoneOverride', { timezoneId: 'Europe/Paris' })
    await page.setViewportSize({ width: 1280, height: 800 })
    await page.goto('/logs?range=all&timeZone=Europe%2FParis')
    await page.getByText('07/09/2035, 01:30:00').first().waitFor()
  })
})
