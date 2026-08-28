import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createMcpCallLog,
  createMcpDebugSession,
} from '#tests/helpers/factories'

test.group('debug call details', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('keeps payloads inside the dialog content inset on mobile', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const { token } = await createAccessToken(admin.id, { name: 'Debug token' })
    const debugSession = await createMcpDebugSession(token, admin.id, { status: 'paused' })
    const call = await createMcpCallLog(token, {
      requestedToolName: 'list_mcps',
      toolName: 'list_mcps',
      arguments: '{}',
      argumentsCaptured: true,
      argumentsBytes: 2,
      response: '{"mcps":[]}',
      responseCaptured: true,
      responseBytes: 11,
      durationMs: 680,
      debugSessionId: debugSession.id,
      debugSessionElapsedMs: 0,
      startedAt: debugSession.startedAt,
      createdAt: DateTime.utc(),
    })

    await browserContext.loginAs(admin)
    const page = await visit(
      `/debug?sessionId=${debugSession.id}&callId=${call.id}&timeZone=Europe%2FParis`
    )
    await page.setViewportSize({ width: 375, height: 812 })

    const dialog = page.getByRole('dialog', { name: 'list_mcps' })
    await dialog.waitFor()
    const metricCard = dialog
      .getByText('Duration', { exact: true })
      .locator('xpath=ancestor::*[contains(@class, "astryx-card")]')
    const inputHeading = dialog.getByRole('heading', { name: 'Input', exact: true })
    const outputHeading = dialog.getByRole('heading', { name: 'Output', exact: true })
    const metricBox = await metricCard.boundingBox()
    const inputBox = await inputHeading.boundingBox()
    const outputBox = await outputHeading.boundingBox()

    assert.isNotNull(metricBox)
    assert.isNotNull(inputBox)
    assert.isNotNull(outputBox)
    assert.approximately(inputBox!.x, metricBox!.x, 1)
    assert.approximately(outputBox!.x, metricBox!.x, 1)
  })
})
