import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import McpDebugSession from '#models/mcp_debug_session'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMember } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('MCP debug sessions', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('restricts the debugger and lifecycle actions to admins', async ({ client, assert }) => {
    const member = await createMember()

    const page = await client.get('/debug').loginAs(member).redirects(0)
    const start = await client
      .post('/debug-sessions')
      .loginAs(member)
      .withCsrfToken()
      .redirects(0)
      .form({ accessTokenId: 1 })

    assertRedirectTo(assert, page, '/')
    assertRedirectTo(assert, start, '/')
  })

  test('starts one session per access token and renders it in the debugger', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const { token } = await createAccessToken(admin.id, { name: 'Codex session' })

    const start = await client
      .post('/debug-sessions')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ accessTokenId: token.id })

    start.assertStatus(302)
    start.assertFlashMessage('success', 'Debug session started')
    const debugSession = await McpDebugSession.firstOrFail()
    const location = new URL(start.header('location')!, 'http://localhost')
    assert.equal(location.pathname, '/debug')
    assert.equal(location.searchParams.get('sessionId'), String(debugSession.id))
    assert.equal(debugSession.accessTokenName, 'Codex session')
    assert.equal(debugSession.status, 'active')
    assert.equal(debugSession.pausedDurationMs, 0)

    const duplicate = await client
      .post('/debug-sessions')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ accessTokenId: token.id })
    duplicate.assertFlashMessage('error', 'This access token already has an open debug session')
    const debugSessions = await McpDebugSession.all()
    assert.equal(debugSessions.length, 1)

    const page = await client
      .get(`/debug?sessionId=${debugSession.id}`)
      .withInertia()
      .loginAs(admin)
    page.assertStatus(200)
    page.assertInertiaComponent('debug/index')
    page.assertInertiaPropsContains({
      selectedSession: { id: debugSession.id, status: 'active' },
      tokens: [{ id: token.id, name: 'Codex session' }],
    })
  })

  test('pauses, continues, and stops a debug session', async ({ client, assert }) => {
    const admin = await createAdmin()
    const { token } = await createAccessToken(admin.id)
    const debugSession = await McpDebugSession.create({
      accessTokenId: token.id,
      accessTokenName: token.name,
      accessTokenPrefix: token.tokenPrefix,
      status: 'active',
      createdBy: admin.id,
      startedAt: token.createdAt,
      pausedAt: null,
      endedAt: null,
    })

    for (const action of ['pause', 'resume', 'stop'] as const) {
      if (action === 'resume') {
        debugSession.pausedAt = DateTime.utc().minus({ seconds: 2 })
        await debugSession.save()
      }
      const response = await client
        .patch(`/debug-sessions/${debugSession.id}`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form({ action })
      response.assertStatus(302)
      await debugSession.refresh()
    }

    assert.equal(debugSession.status, 'stopped')
    assert.isNotNull(debugSession.endedAt)
    assert.isAtLeast(debugSession.pausedDurationMs, 1_900)

    const unavailable = await client
      .patch(`/debug-sessions/${debugSession.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ action: 'resume' })
    unavailable.assertFlashMessage('error', 'That action is not available for this debug session')
  })
})
