import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createMcpCallLog,
  createMcpDebugSession,
} from '#tests/helpers/factories'

type TimelinePaint = {
  backgroundColor: string
  borderColor: string
}

function readTimelinePaint(element: unknown): TimelinePaint {
  const browser = globalThis as unknown as {
    getComputedStyle(target: unknown): TimelinePaint
  }
  const style = browser.getComputedStyle(element)
  return {
    backgroundColor: style.backgroundColor,
    borderColor: style.borderColor,
  }
}

test.group('debug timeline', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('uses solid, distinct fills for successful and failed calls', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const { token } = await createAccessToken(admin.id, { name: 'Debug token' })
    const startedAt = DateTime.utc().minus({ seconds: 12 })
    const debugSession = await createMcpDebugSession(token, admin.id, {
      status: 'paused',
      startedAt,
      pausedAt: DateTime.utc(),
    })
    const successfulCall = await createMcpCallLog(token, {
      requestedToolName: 'successful_call',
      toolName: 'successful_call',
      outcome: 'success',
      durationMs: 680,
      debugSessionId: debugSession.id,
      debugSessionElapsedMs: 0,
      startedAt,
      createdAt: startedAt,
    })
    await createMcpCallLog(token, {
      requestedToolName: 'failed_call',
      toolName: 'failed_call',
      outcome: 'error',
      errorCategory: 'tool_error',
      durationMs: 2_200,
      debugSessionId: debugSession.id,
      debugSessionElapsedMs: 8_000,
      startedAt: startedAt.plus({ seconds: 8 }),
      createdAt: startedAt.plus({ seconds: 8 }),
    })

    await browserContext.loginAs(admin)
    const page = await visit(`/debug?sessionId=${debugSession.id}&timeZone=Europe%2FParis`)
    await page.setViewportSize({ width: 1440, height: 900 })

    const successButton = page.getByRole('button', { name: 'Inspect successful_call, 680 ms' })
    const errorButton = page.getByRole('button', { name: 'Inspect failed_call, 2.20 s' })
    const track = successButton.locator('xpath=..')
    const successPaint = await successButton
      .locator('.debug-timeline-bar')
      .evaluate(readTimelinePaint)
    const errorPaint = await errorButton.locator('.debug-timeline-bar').evaluate(readTimelinePaint)
    const trackPaint = await track.evaluate(readTimelinePaint)
    const duration = page.getByText('680 ms', { exact: true })
    const trackBox = await track.boundingBox()
    const durationBox = await duration.boundingBox()

    assert.notEqual(successPaint.backgroundColor, trackPaint.backgroundColor)
    assert.notEqual(errorPaint.backgroundColor, trackPaint.backgroundColor)
    assert.notEqual(successPaint.backgroundColor, errorPaint.backgroundColor)
    assert.equal(successPaint.backgroundColor, successPaint.borderColor)
    assert.equal(errorPaint.backgroundColor, errorPaint.borderColor)
    assert.isNotNull(trackBox)
    assert.isNotNull(durationBox)
    assert.isAbove(durationBox!.y, trackBox!.y + trackBox!.height)
    assert.isBelow(durationBox!.height, 20)

    await successButton.click()
    await page.getByRole('dialog', { name: 'successful_call' }).waitFor()
    assert.include(page.url(), `callId=${successfulCall.id}`)
    assert.equal(await successButton.getAttribute('data-selected'), 'true')
  })
})
