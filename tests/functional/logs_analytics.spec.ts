import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import InstanceSetting from '#models/instance_setting'
import McpCallLog from '#models/mcp_call_log'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAdmin,
  createMcp,
  createMcpCallLog,
  createMember,
  createStoredAccessToken,
} from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('MCP logs and analytics administration', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('restricts logs, analytics, and logging updates to admins', async ({ client, assert }) => {
    const admin = await createAdmin()
    const member = await createMember()

    const anonymousLogs = await client.get('/logs').redirects(0)
    const memberLogs = await client.get('/logs').loginAs(member).redirects(0)
    const memberAnalytics = await client.get('/analytics').loginAs(member).redirects(0)
    const memberUpdate = await client
      .patch('/settings/mcp-logging')
      .loginAs(member)
      .withCsrfToken()
      .redirects(0)
      .form({ mcpLogLevel: 'off', mcpLogRetentionDays: 14 })

    assertRedirectTo(assert, anonymousLogs, '/login')
    for (const response of [memberLogs, memberAnalytics, memberUpdate]) {
      assertRedirectTo(assert, response, '/')
    }

    const adminLogs = await client.get('/logs').withInertia().loginAs(admin)
    const adminAnalytics = await client.get('/analytics').withInertia().loginAs(admin)
    adminLogs.assertStatus(200)
    adminLogs.assertInertiaComponent('logs/index')
    adminAnalytics.assertStatus(200)
    adminAnalytics.assertInertiaComponent('analytics/index')
  })

  test('shows defaults and updates logging level and retention', async ({ client, assert }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id)
    await createMcpCallLog(token, {
      createdAt: DateTime.utc().minus({ days: 2 }),
    })

    const settingsPage = await client.get('/settings').withInertia().loginAs(admin)
    settingsPage.assertInertiaPropsContains({
      mcpLogging: { level: 'metadata', retentionDays: 14 },
    })

    const response = await client
      .patch('/settings/mcp-logging')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ mcpLogLevel: 'responses', mcpLogRetentionDays: 1 })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/settings')
    response.assertFlashMessage('success', 'MCP logging settings updated')

    const settings = await InstanceSetting.findOrFail(1)
    assert.equal(settings.mcpLogLevel, 'responses')
    assert.equal(settings.mcpLogRetentionDays, 1)
    assert.equal(settings.updatedBy, admin.id)
    assert.equal(
      Number(
        await McpCallLog.query()
          .count('* as total')
          .then((rows) => rows[0].$extras.total)
      ),
      0
    )
  })

  test('rejects invalid logging settings', async ({ client, assert }) => {
    const admin = await createAdmin()

    for (const payload of [
      { mcpLogLevel: 'everything', mcpLogRetentionDays: 14 },
      { mcpLogLevel: 'metadata', mcpLogRetentionDays: 0 },
      { mcpLogLevel: 'metadata', mcpLogRetentionDays: 366 },
      { mcpLogLevel: 'metadata', mcpLogRetentionDays: 1.5 },
    ]) {
      const response = await client
        .patch('/settings/mcp-logging')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form(payload)
      response.assertStatus(302)
      assert.isObject(response.flashMessage('inputErrorsBag'))
    }
  })

  test('filters logs and returns the selected detail', async ({ client }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id, {
      name: 'Agent token',
      tokenPrefix: 'mcp_agent',
    })
    const mcp = await createMcp(admin.id, { name: 'Search MCP', slug: 'search' })
    await createMcpCallLog(token, { mcp, outcome: 'success', requestedToolName: 'search__ok' })
    const failed = await createMcpCallLog(token, {
      mcp,
      outcome: 'error',
      requestedToolName: 'search__fail',
      toolName: 'fail',
      errorCategory: 'tool_error',
      errorSummary: 'Upstream tool returned an error',
      callerIp: '192.0.2.12',
      argumentsCaptured: true,
      arguments: JSON.stringify({ query: 'private value' }),
      responseCaptured: true,
      response: JSON.stringify({ content: [{ type: 'text', text: 'private result' }] }),
    })

    const page = await client
      .get(`/logs?range=all&outcome=error&mcp=search&token=mcp_agent&logId=${failed.id}`)
      .withInertia()
      .loginAs(admin)

    page.assertStatus(200)
    page.assertInertiaPropsContains({
      filters: { range: 'all', outcome: 'error', mcp: 'search', token: 'mcp_agent' },
      pagination: { pageSize: 25, total: 1 },
      logs: [{ id: failed.id, requestedToolName: 'search__fail', outcome: 'error' }],
      selectedLog: {
        id: failed.id,
        callerIp: '192.0.2.12',
        argumentsCaptured: true,
        arguments: JSON.stringify({ query: 'private value' }),
        responseCaptured: true,
        response: JSON.stringify({ content: [{ type: 'text', text: 'private result' }] }),
      },
    })
  })

  test('validates and normalizes log query parameters with Vine', async ({ client, assert }) => {
    const admin = await createAdmin()

    const blankFilters = await client.get('/logs?outcome=&mcp=&token=').withInertia().loginAs(admin)
    blankFilters.assertStatus(200)
    blankFilters.assertInertiaPropsContains({
      filters: { range: '24h', outcome: '', mcp: '', token: '' },
      pagination: { pageSize: 25 },
    })

    const selectedPageSize = await client
      .get('/logs?pageSize=10&timeZone=Europe/Paris')
      .withInertia()
      .loginAs(admin)
    selectedPageSize.assertStatus(200)
    selectedPageSize.assertInertiaPropsContains({
      filters: { timeZone: 'Europe/Paris' },
      pagination: { pageSize: 10 },
    })

    const invalidRange = await client.get('/logs?range=invalid').loginAs(admin).redirects(0)
    invalidRange.assertStatus(302)
    assert.property(invalidRange.flashMessage('inputErrorsBag'), 'range')

    const invalidPageSize = await client.get('/logs?pageSize=30').loginAs(admin).redirects(0)
    invalidPageSize.assertStatus(302)
    assert.property(invalidPageSize.flashMessage('inputErrorsBag'), 'pageSize')
  })

  test('calculates analytics metrics, buckets, and breakdowns', async ({ client, assert }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id, { name: 'Analytics token' })
    const mcp = await createMcp(admin.id, { name: 'Analytics MCP' })
    await createMcpCallLog(token, { mcp, durationMs: 100 })
    await createMcpCallLog(token, {
      mcp,
      outcome: 'error',
      errorCategory: 'tool_error',
      durationMs: 300,
    })

    const page = await client
      .get('/analytics?range=7d&timeZone=Europe/Paris')
      .withInertia()
      .loginAs(admin)

    page.assertStatus(200)
    page.assertInertiaPropsContains({
      range: '7d',
      timeZone: 'Europe/Paris',
      metrics: {
        total: 2,
        successes: 1,
        errors: 1,
        successRate: 50,
        errorRate: 50,
        averageDurationMs: 200,
      },
      topMcps: [{ label: 'Analytics MCP', total: 2, errors: 1 }],
      topTokens: [{ label: 'Analytics token', total: 2, errors: 1 }],
    })

    const currentLocalBucket = page.inertiaProps.timeline.at(-1)
    const expectedLocalBucket = DateTime.now().setZone('Europe/Paris').startOf('day')
    assert.equal(currentLocalBucket.bucket, expectedLocalBucket.toUTC().toISO())
    assert.equal(currentLocalBucket.label, expectedLocalBucket.toFormat('LLL d'))
    assert.equal(currentLocalBucket.total, 2)
    assert.equal(currentLocalBucket.errors, 1)
  })

  test('filters analytics by an exact custom time range', async ({ client, assert }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id, { name: 'Custom range token' })
    const customStart = DateTime.now()
      .setZone('Europe/Paris')
      .startOf('hour')
      .minus({ hours: 6 })
      .plus({ minutes: 30 })
    const customEnd = customStart.plus({ hours: 4 })

    await createMcpCallLog(token, {
      durationMs: 50,
      createdAt: customStart.minus({ minutes: 1 }).toUTC(),
    })
    await createMcpCallLog(token, {
      durationMs: 100,
      createdAt: customStart.toUTC(),
    })
    await createMcpCallLog(token, {
      durationMs: 300,
      outcome: 'error',
      createdAt: customEnd.minus({ minutes: 1 }).toUTC(),
    })
    await createMcpCallLog(token, {
      durationMs: 500,
      createdAt: customEnd.toUTC(),
    })

    const start = customStart.toUTC().toISO({ suppressSeconds: true, suppressMilliseconds: true })!
    const end = customEnd.toUTC().toISO({ suppressSeconds: true, suppressMilliseconds: true })!
    const page = await client
      .get(
        `/analytics?range=custom&start=${encodeURIComponent(start)}&end=${encodeURIComponent(end)}&timeZone=Europe%2FParis`
      )
      .withInertia()
      .loginAs(admin)

    page.assertStatus(200)
    page.assertInertiaPropsContains({
      range: 'custom',
      timeZone: 'Europe/Paris',
      metrics: {
        total: 2,
        successes: 1,
        errors: 1,
        successRate: 50,
        errorRate: 50,
        averageDurationMs: 200,
      },
    })

    assert.lengthOf(page.inertiaProps.timeline, 4)
    assert.deepEqual(
      page.inertiaProps.timeline.map((point: { total: number }) => point.total),
      [1, 0, 0, 1]
    )
    assert.equal(
      DateTime.fromISO(page.inertiaProps.start, { setZone: true }).toUTC().toMillis(),
      customStart.toUTC().toMillis()
    )
    assert.equal(
      DateTime.fromISO(page.inertiaProps.end, { setZone: true }).toUTC().toMillis(),
      customEnd.toUTC().toMillis()
    )
    assert.equal(page.inertiaProps.timeline[0].bucket, customStart.toUTC().toISO())
    assert.equal(
      page.inertiaProps.timeline.at(-1).bucket,
      customEnd.minus({ hours: 1 }).toUTC().toISO()
    )
  })

  test('keeps custom instants exact across daylight-saving transitions', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const transitions = [
      {
        start: '2026-03-29T00:30Z',
        end: '2026-03-29T01:30Z',
        localStart: '2026-03-29T01:30+01:00',
        localEnd: '2026-03-29T03:30+02:00',
      },
      {
        start: '2026-10-25T00:30Z',
        end: '2026-10-25T01:30Z',
        localStart: '2026-10-25T02:30+02:00',
        localEnd: '2026-10-25T02:30+01:00',
      },
    ]

    for (const transition of transitions) {
      const page = await client
        .get(
          `/analytics?range=custom&start=${encodeURIComponent(transition.start)}&end=${encodeURIComponent(transition.end)}&timeZone=Europe%2FParis`
        )
        .withInertia()
        .loginAs(admin)

      page.assertStatus(200)
      page.assertInertiaPropsContains({
        range: 'custom',
        start: transition.localStart,
        end: transition.localEnd,
        timeZone: 'Europe/Paris',
      })
      assert.lengthOf(page.inertiaProps.timeline, 1)
    }

    const offsetFreeGap = await client
      .get(
        '/analytics?range=custom&start=2026-03-29T02%3A30&end=2026-03-29T03%3A30&timeZone=Europe%2FParis'
      )
      .withInertia()
      .loginAs(admin)
    offsetFreeGap.assertStatus(200)
    offsetFreeGap.assertInertiaPropsContains({ range: '7d', timeZone: 'Europe/Paris' })

    const fractionalBoundary = await client
      .get(
        '/analytics?range=custom&start=2026-03-29T00%3A30%3A00.500Z&end=2026-03-29T01%3A30%3A00.500Z&timeZone=Europe%2FParis'
      )
      .withInertia()
      .loginAs(admin)
    fractionalBoundary.assertStatus(200)
    fractionalBoundary.assertInertiaPropsContains({ range: '7d', timeZone: 'Europe/Paris' })
  })
})
