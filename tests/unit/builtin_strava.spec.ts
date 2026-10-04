import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import Mcp from '#models/mcp'
import { builtinAuthorizationUrl, requestedBuiltinScopes } from '#services/builtin/oauth'
import { builtinMcp } from '#services/builtin/registry'
import { builtinWriteGranted, callBuiltinTool, listBuiltinTools } from '#services/builtin/runtime'
import { compactStravaPayload, downsampleStreams } from '#services/builtin/strava/payload'
import McpSecretStore from '#services/mcp_secret_store'
import { probeUpstream, testAndUpdateStatus } from '#services/upstream/manager'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { createStravaMcp, mockStrava, stravaFixtures, stravaJson } from '#tests/helpers/strava'

function resultText(result: Awaited<ReturnType<typeof callBuiltinTool>>) {
  const [first] = result.content
  return first.type === 'text' ? first.text : ''
}

async function connectedStrava(options: Parameters<typeof createStravaMcp>[1] = {}) {
  const admin = await createAdmin()
  return createStravaMcp(admin.id, options)
}

test.group('Built-in Strava MCP: payloads', () => {
  test('builds the Strava authorization URL with comma-separated scopes', ({ assert }) => {
    const strava = builtinMcp('strava')!
    const url = new URL(
      builtinAuthorizationUrl(strava, {
        clientId: '123456',
        redirectUri: 'https://mcp.example.com/mcps/oauth/callback',
        state: 'state-value',
        scopes: strava.oauth.scopes,
      })
    )

    assert.equal(url.origin + url.pathname, 'https://www.strava.com/oauth/authorize')
    assert.deepEqual(Object.fromEntries(url.searchParams), {
      client_id: '123456',
      redirect_uri: 'https://mcp.example.com/mcps/oauth/callback',
      response_type: 'code',
      scope: 'read,read_all,profile:read_all,activity:read_all',
      state: 'state-value',
      approval_prompt: 'force',
    })
  })

  test('only knows the registered built-in keys', ({ assert }) => {
    assert.equal(builtinMcp('strava')?.name, 'Strava')
    assert.isNull(builtinMcp('constructor'))
    assert.isNull(builtinMcp(null))
  })

  test('drops noise, nulls, and imprecise numeric route identifiers', ({ assert }) => {
    assert.deepEqual(
      compactStravaPayload({
        id: 2984453279043963000,
        id_str: '2984453279043962922',
        name: 'Col loop',
        resource_state: 3,
        description: null,
        map: { id: 'r1', summary_polyline: 'encoded' },
        athlete: { id: 4242, resource_state: 1, profile: 'https://images.example/a.jpg' },
        segments: [{ id: 7, resource_state: 2, map: {}, average_grade: 5.5 }],
      }),
      {
        id: '2984453279043962922',
        name: 'Col loop',
        athlete: { id: 4242 },
        segments: [{ id: 7, average_grade: 5.5 }],
      }
    )
  })

  test('downsamples every stream at the same evenly spaced positions', ({ assert }) => {
    const time = Array.from({ length: 1000 }, (_, index) => index)
    const result = downsampleStreams(
      {
        time: { data: time, original_size: 1000 },
        heartrate: { data: time.map((second) => 100 + (second % 60)), original_size: 1000 },
        ignored: { resolution: 'high' },
      },
      5
    )

    assert.equal(result.original_points, 1000)
    assert.equal(result.returned_points, 5)
    assert.deepEqual(result.streams.time, [0, 250, 500, 749, 999])
    assert.deepEqual(result.streams.heartrate, [100, 110, 120, 129, 139])
    assert.notProperty(result.streams, 'ignored')

    const short = downsampleStreams({ time: { data: [0, 1, 2] } }, 200)
    assert.deepEqual(short, {
      original_points: 3,
      returned_points: 3,
      streams: { time: [0, 1, 2] },
    })
  })
})

test.group('Built-in Strava MCP: tools', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('lists tools without calling Strava and hides tools whose scope was unchecked', async ({
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const full = await connectedStrava()
      const tools = await probeUpstream(full)
      const names = tools.map((tool) => tool.name)
      assert.includeMembers(names, [
        'get_athlete',
        'get_athlete_stats',
        'get_athlete_zones',
        'list_activities',
        'get_activity',
        'get_activity_streams',
        'list_starred_segments',
        'list_routes',
        'get_gear',
      ])
      assert.lengthOf(names, 17)
      for (const tool of listBuiltinTools(full)) {
        assert.equal(tool.inputSchema.type, 'object')
        assert.isAbove(tool.description!.length, 20)
      }

      const publicOnly = await connectedStrava({ name: 'Strava public', scopes: 'read' })
      const reduced = listBuiltinTools(publicOnly).map((tool) => tool.name)
      assert.include(reduced, 'get_athlete')
      assert.include(reduced, 'list_starred_segments')
      assert.notInclude(reduced, 'list_activities')
      assert.notInclude(reduced, 'get_athlete_zones')

      const unknownScopes = await connectedStrava({ name: 'Strava unknown', scopes: null })
      assert.lengthOf(listBuiltinTools(unknownScopes), 17)

      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('refuses to list or call tools before an account is connected', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava({ connected: false })

      assert.throws(() => listBuiltinTools(mcp), /Strava is not connected/)
      const result = await callBuiltinTool(mcp, 'get_athlete', {})
      assert.isTrue(result.isError)
      assert.include(resultText(result), 'Strava is not connected')
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('lists activities with ISO dates converted to epoch seconds and trimmed summaries', async ({
    assert,
  }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava()
      const result = await callBuiltinTool(mcp, 'list_activities', {
        after: '2026-09-01',
        before: '2026-10-01T12:00:00+02:00',
        per_page: '5',
      })

      assert.isUndefined(result.isError)
      const [request] = strava.apiRequests()
      assert.equal(request.url.pathname, '/api/v3/athlete/activities')
      assert.deepEqual(Object.fromEntries(request.url.searchParams), {
        after: String(DateTime.fromISO('2026-09-01T00:00:00Z').toUnixInteger()),
        before: String(DateTime.fromISO('2026-10-01T10:00:00Z').toUnixInteger()),
        page: '1',
        per_page: '5',
      })
      assert.equal(request.authorization, 'Bearer strava-access-token')

      const [activity] = JSON.parse(resultText(result))
      assert.equal(activity.id, 15000000001)
      assert.equal(activity.name, 'Morning Run')
      assert.equal(activity.average_heartrate, 152.3)
      assert.notProperty(activity, 'map')
      assert.notProperty(activity, 'external_id')
      assert.notProperty(activity, 'average_watts')
      assert.notInclude(resultText(result), 'encoded-polyline')
    } finally {
      strava.restore()
    }
  })

  test('returns one activity without segment efforts unless they are requested', async ({
    assert,
  }) => {
    const detail = {
      ...stravaFixtures.activity,
      resource_state: 3,
      description: 'Felt great',
      calories: 640,
      laps: [{ id: 1, resource_state: 2, lap_index: 1, distance: 1000 }],
      segment_efforts: [{ id: 99, name: 'Riverside sprint', resource_state: 2 }],
    }
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/activities/15000000001' ? stravaJson(detail) : undefined
    )
    try {
      const mcp = await connectedStrava()

      const compact = JSON.parse(
        resultText(await callBuiltinTool(mcp, 'get_activity', { activity_id: 15000000001 }))
      )
      assert.equal(compact.description, 'Felt great')
      assert.deepEqual(compact.laps, [{ id: 1, lap_index: 1, distance: 1000 }])
      assert.notProperty(compact, 'segment_efforts')
      assert.isFalse(strava.apiRequests()[0].url.searchParams.has('include_all_efforts'))

      const full = JSON.parse(
        resultText(
          await callBuiltinTool(mcp, 'get_activity', {
            activity_id: '15000000001',
            include_segment_efforts: true,
          })
        )
      )
      assert.deepEqual(full.segment_efforts, [{ id: 99, name: 'Riverside sprint' }])
      assert.equal(strava.apiRequests()[1].url.searchParams.get('include_all_efforts'), 'true')
    } finally {
      strava.restore()
    }
  })

  test('requests streams by type and returns downsampled series', async ({ assert }) => {
    const data = Array.from({ length: 600 }, (_, index) => index)
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/activities/7/streams'
        ? stravaJson({
            time: { data, original_size: 600, resolution: 'high', series_type: 'distance' },
            heartrate: { data: data.map(() => 150), original_size: 600 },
          })
        : undefined
    )
    try {
      const mcp = await connectedStrava()
      const result = JSON.parse(
        resultText(
          await callBuiltinTool(mcp, 'get_activity_streams', {
            activity_id: 7,
            keys: ['time', 'heartrate', 'time'],
            max_points: 3,
          })
        )
      )

      const [request] = strava.apiRequests()
      assert.equal(request.url.searchParams.get('keys'), 'time,heartrate')
      assert.equal(request.url.searchParams.get('key_by_type'), 'true')
      assert.deepEqual(result, {
        original_points: 600,
        returned_points: 3,
        streams: { time: [0, 300, 599], heartrate: [150, 150, 150] },
      })
    } finally {
      strava.restore()
    }
  })

  test('resolves the athlete before athlete-scoped endpoints', async ({ assert }) => {
    const strava = mockStrava(({ url }) => {
      if (url.pathname === '/api/v3/athletes/4242/stats') {
        return stravaJson({ all_run_totals: { count: 310, distance: 3100000 } })
      }
      if (url.pathname === '/api/v3/athletes/4242/routes') {
        return stravaJson([
          { id: 2984453279043963000, id_str: '2984453279043962922', name: 'Loop' },
        ])
      }
      return undefined
    })
    try {
      const mcp = await connectedStrava()

      const stats = JSON.parse(resultText(await callBuiltinTool(mcp, 'get_athlete_stats', {})))
      assert.deepEqual(stats, { all_run_totals: { count: 310, distance: 3100000 } })

      const routes = JSON.parse(resultText(await callBuiltinTool(mcp, 'list_routes', {})))
      assert.deepEqual(routes, [{ id: '2984453279043962922', name: 'Loop' }])
      assert.deepEqual(
        strava.apiRequests().map((request) => request.url.pathname),
        [
          '/api/v3/athlete',
          '/api/v3/athletes/4242/stats',
          '/api/v3/athlete',
          '/api/v3/athletes/4242/routes',
        ]
      )
    } finally {
      strava.restore()
    }
  })

  test('rejects invalid arguments before calling Strava', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava()
      const cases: Array<[string, Record<string, unknown>, string]> = [
        ['get_activity', {}, 'activity_id is required'],
        ['get_activity', { activity_id: '12/../../athlete' }, 'activity_id must be an integer'],
        ['get_activity', { activity_id: 0 }, 'activity_id must be an integer of at least 1'],
        ['get_gear', { gear_id: '../athlete' }, 'gear_id must be a gear identifier'],
        ['get_route', { route_id: '12?x=1' }, 'route_id must be a numeric route identifier'],
        ['list_activities', { after: 'last week' }, 'after must be an ISO 8601 date'],
        ['list_activities', { per_page: 500 }, 'per_page must be an integer between 1 and 100'],
        ['get_activity_streams', { activity_id: 1, keys: ['pace'] }, 'keys must be a non-empty'],
        ['explore_segments', { south_west_lat: 45 }, 'south_west_lng is required'],
        ['explore_segments', { south_west_lat: 120 }, 'south_west_lat must be a number between'],
        ['delete_everything', {}, 'Unknown Strava tool: delete_everything'],
      ]

      for (const [tool, args, message] of cases) {
        const result = await callBuiltinTool(mcp, tool, args)
        assert.isTrue(result.isError, `${tool} ${JSON.stringify(args)}`)
        assert.include(resultText(result), message)
      }
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('explains Strava failures to the agent without leaking credentials', async ({ assert }) => {
    const failures: Record<string, Response> = {
      '/api/v3/activities/401': stravaJson(
        {
          message: 'Authorization Error',
          errors: [{ resource: 'Athlete', field: 'access_token', code: 'invalid' }],
        },
        401
      ),
      '/api/v3/activities/4011': stravaJson(
        {
          message: 'Authorization Error',
          errors: [{ resource: 'AccessToken', field: 'activity:read_permission', code: 'missing' }],
        },
        401
      ),
      '/api/v3/activities/402/zones': stravaJson({ message: 'Payment Required', errors: [] }, 402),
      '/api/v3/activities/404': stravaJson(
        {
          message: 'Record Not Found',
          errors: [{ resource: 'Activity', field: '', code: 'not found' }],
        },
        404
      ),
      '/api/v3/activities/429': stravaJson({ message: 'Rate Limit Exceeded', errors: [] }, 429, {
        'X-ReadRateLimit-Limit': '100,1000',
        'X-ReadRateLimit-Usage': '101,420',
      }),
      '/api/v3/activities/500': new Response('<html>Bad gateway</html>', { status: 502 }),
    }
    const strava = mockStrava(({ url }) => failures[url.pathname]?.clone())
    try {
      const mcp = await connectedStrava()
      const call = async (tool: string, id: number) => {
        const result = await callBuiltinTool(mcp, tool, { activity_id: id })
        assert.isTrue(result.isError)
        assert.notInclude(resultText(result), 'strava-access-token')
        return resultText(result)
      }

      assert.equal(
        await call('get_activity', 401),
        'Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs.'
      )
      assert.include(
        await call('get_activity', 4011),
        'Strava permission "activity:read" was not granted'
      )
      assert.include(await call('get_activity_zones', 402), 'athletes with a subscription')
      assert.equal(
        await call('get_activity', 404),
        'Strava could not find this resource: Record Not Found (Activity not found)'
      )
      assert.include(
        await call('get_activity', 429),
        'Strava rate limit reached (101 of 100 requests in 15 minutes, 420 of 1000 today)'
      )
      assert.equal(await call('get_activity', 500), 'Strava API returned HTTP 502')
    } finally {
      strava.restore()
    }
  })

  test('refuses a tool whose permission was not granted', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava({ scopes: 'read' })
      const result = await callBuiltinTool(mcp, 'list_activities', {})

      assert.isTrue(result.isError)
      assert.include(
        resultText(result),
        'list_activities needs the Strava permission "activity:read" or "activity:read_all"'
      )
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })
})

test.group('Built-in Strava MCP: authorization lifecycle', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('renews an expired token once for concurrent calls and stores the rotated pair', async ({
    assert,
  }) => {
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/oauth/token'
        ? stravaJson({
            token_type: 'Bearer',
            access_token: 'rotated-access-token',
            refresh_token: 'rotated-refresh-token',
            expires_in: 21600,
          })
        : undefined
    )
    try {
      const mcp = await connectedStrava({ expiresAt: DateTime.utc().minus({ minutes: 1 }) })
      const callers = await Promise.all(Array.from({ length: 4 }, () => Mcp.findOrFail(mcp.id)))

      const results = await Promise.all(
        callers.map((caller) => callBuiltinTool(caller, 'get_athlete', {}))
      )

      for (const result of results) assert.isUndefined(result.isError)
      assert.lengthOf(strava.tokenRequests(), 1)
      assert.deepEqual(Object.fromEntries(strava.tokenRequests()[0].form!), {
        client_id: '123456',
        client_secret: 'strava-client-secret',
        grant_type: 'refresh_token',
        refresh_token: 'strava-refresh-token',
      })
      for (const request of strava.apiRequests()) {
        assert.equal(request.authorization, 'Bearer rotated-access-token')
      }

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthAccessToken), 'rotated-access-token')
      assert.equal(McpSecretStore.decrypt(saved.oauthRefreshToken), 'rotated-refresh-token')
      assert.equal(saved.oauthScopes, 'read read_all profile:read_all activity:read_all')
      assert.isAbove(
        saved.oauthTokenExpiresAt!.toMillis(),
        DateTime.utc().plus({ hours: 5 }).toMillis()
      )
    } finally {
      strava.restore()
    }
  })

  test('does not renew a token that is still valid', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava()
      await callBuiltinTool(mcp, 'get_athlete', {})

      assert.lengthOf(strava.tokenRequests(), 0)
    } finally {
      strava.restore()
    }
  })

  test('asks to re-authorize when Strava refuses to renew the authorization', async ({
    assert,
  }) => {
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/oauth/token'
        ? stravaJson(
            {
              message: 'Bad Request',
              errors: [{ resource: 'RefreshToken', field: 'refresh_token', code: 'invalid' }],
            },
            400
          )
        : undefined
    )
    try {
      const mcp = await connectedStrava({ expiresAt: DateTime.utc().minus({ minutes: 1 }) })

      const result = await callBuiltinTool(mcp, 'get_athlete', {})
      assert.isTrue(result.isError)
      assert.include(resultText(result), 'Strava refused to renew the saved authorization')
      assert.notInclude(resultText(result), 'strava-client-secret')
      assert.lengthOf(strava.apiRequests(), 0)

      await testAndUpdateStatus(mcp)
      assert.equal(mcp.status, 'error')
      assert.isTrue(mcp.oauthRequired)
      assert.include(mcp.lastError!, 'RefreshToken')
    } finally {
      strava.restore()
    }
  })

  test('reports connection health from one authenticated Strava request', async ({ assert }) => {
    let athleteStatus = 200
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/athlete' && athleteStatus !== 200
        ? stravaJson(
            {
              message: 'Authorization Error',
              errors: [{ resource: 'Athlete', field: 'access_token', code: 'invalid' }],
            },
            athleteStatus
          )
        : undefined
    )
    try {
      const pending = await connectedStrava({ name: 'Strava pending', connected: false })
      await testAndUpdateStatus(pending)
      assert.equal(pending.status, 'draft')
      assert.equal(pending.lastError, 'OAuth authorization required')
      assert.isTrue(pending.oauthRequired)
      assert.lengthOf(strava.requests, 0)

      const mcp = await connectedStrava()
      mcp.status = 'error'
      mcp.lastError = 'stale'
      await testAndUpdateStatus(mcp)
      assert.equal(mcp.status, 'ready')
      assert.isNull(mcp.lastError)
      assert.isFalse(mcp.oauthRequired)
      assert.deepEqual(
        strava.apiRequests().map((request) => request.url.pathname),
        ['/api/v3/athlete']
      )

      athleteStatus = 401
      await testAndUpdateStatus(mcp)
      assert.equal(mcp.status, 'error')
      assert.isTrue(mcp.oauthRequired)
      assert.equal(
        mcp.lastError,
        'Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs.'
      )

      athleteStatus = 503
      await testAndUpdateStatus(mcp)
      assert.equal(mcp.status, 'error')
      assert.isFalse(mcp.oauthRequired)
      assert.include(mcp.lastError!, 'Strava API returned HTTP 503')
    } finally {
      strava.restore()
    }
  })
})

test.group('Built-in Strava MCP: write tools', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  const writeTools = ['create_activity', 'update_activity', 'update_athlete_weight', 'star_segment']

  test('only requests write scopes once write access is allowed', async ({ assert }) => {
    const strava = builtinMcp('strava')!
    const readOnly = await connectedStrava({ name: 'Strava read' })
    const writable = await connectedStrava({ name: 'Strava write', writeEnabled: true })

    assert.deepEqual(requestedBuiltinScopes(strava, readOnly), [
      'read',
      'read_all',
      'profile:read_all',
      'activity:read_all',
    ])
    assert.deepEqual(requestedBuiltinScopes(strava, writable), [
      'read',
      'read_all',
      'profile:read_all',
      'activity:read_all',
      'activity:write',
      'profile:write',
    ])
  })

  test('exposes write tools only when allowed here and granted on Strava', async ({ assert }) => {
    const names = (mcp: Mcp) => listBuiltinTools(mcp).map((tool) => tool.name)

    const readOnly = await connectedStrava({ name: 'Strava read' })
    assert.lengthOf(names(readOnly), 17)
    assert.notIncludeMembers(names(readOnly), writeTools)

    const writable = await connectedStrava({ name: 'Strava write', writeEnabled: true })
    assert.lengthOf(names(writable), 21)
    assert.includeMembers(names(writable), writeTools)

    // Allowed in MyMCPs after connecting: the saved authorization is still read-only.
    const awaiting = await connectedStrava({
      name: 'Strava awaiting',
      writeEnabled: true,
      scopes: 'read read_all profile:read_all activity:read_all',
    })
    assert.notIncludeMembers(names(awaiting), writeTools)
    assert.isFalse(builtinWriteGranted(awaiting))
    assert.isTrue(builtinWriteGranted(writable))

    // The athlete unchecked the profile permission on Strava.
    const partial = await connectedStrava({
      name: 'Strava partial',
      writeEnabled: true,
      scopes: 'read activity:read_all activity:write',
    })
    assert.includeMembers(names(partial), ['create_activity', 'update_activity'])
    assert.notIncludeMembers(names(partial), ['update_athlete_weight', 'star_segment'])

    // Turned off in MyMCPs while the Strava authorization still carries the scopes.
    const turnedOff = await connectedStrava({
      name: 'Strava off',
      scopes: 'read activity:read_all activity:write profile:write',
    })
    assert.notIncludeMembers(names(turnedOff), writeTools)
  })

  test('refuses write tools while write access is turned off', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava({
        scopes: 'read activity:read_all activity:write profile:write',
      })

      for (const tool of writeTools) {
        const result = await callBuiltinTool(mcp, tool, { activity_id: 1, name: 'x' })
        assert.isTrue(result.isError)
        assert.include(resultText(result), 'write access is turned off for this MCP')
      }
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('creates a manual activity with the local start time kept as written', async ({
    assert,
  }) => {
    const strava = mockStrava(({ method, url }) =>
      method === 'POST' && url.pathname === '/api/v3/activities'
        ? stravaJson(
            {
              ...stravaFixtures.activity,
              id: 15000000099,
              name: 'Evening yoga',
              segment_efforts: [],
            },
            201
          )
        : undefined
    )
    try {
      const mcp = await connectedStrava({ writeEnabled: true })
      const result = await callBuiltinTool(mcp, 'create_activity', {
        name: '  Evening yoga ',
        sport_type: 'Yoga',
        start_date_local: '2026-10-03T19:30:00+02:00',
        elapsed_time: 3600,
        distance: 0,
        description: 'Hip mobility',
        trainer: true,
      })

      assert.isUndefined(result.isError)
      const [request] = strava.apiRequests()
      assert.equal(request.method, 'POST')
      assert.equal(request.authorization, 'Bearer strava-access-token')
      assert.deepEqual(Object.fromEntries(request.form!), {
        name: 'Evening yoga',
        sport_type: 'Yoga',
        start_date_local: '2026-10-03T19:30:00Z',
        elapsed_time: '3600',
        distance: '0',
        description: 'Hip mobility',
        trainer: '1',
      })

      const created = JSON.parse(resultText(result))
      assert.equal(created.id, 15000000099)
      assert.notProperty(created, 'map')
      assert.notProperty(created, 'segment_efforts')
    } finally {
      strava.restore()
    }
  })

  test('updates only the activity fields that were passed', async ({ assert }) => {
    const strava = mockStrava(({ method, url }) =>
      method === 'PUT' && url.pathname === '/api/v3/activities/15000000001'
        ? stravaJson({ ...stravaFixtures.activity, name: 'Tempo run', description: '' })
        : undefined
    )
    try {
      const mcp = await connectedStrava({ writeEnabled: true })
      const result = await callBuiltinTool(mcp, 'update_activity', {
        activity_id: 15000000001,
        name: 'Tempo run',
        description: '',
        gear_id: 'none',
        commute: false,
      })

      assert.isUndefined(result.isError)
      const [request] = strava.apiRequests()
      assert.equal(request.method, 'PUT')
      assert.isNull(request.form)
      assert.deepEqual(request.json, {
        name: 'Tempo run',
        description: '',
        gear_id: 'none',
        commute: false,
      })
      assert.equal(JSON.parse(resultText(result)).name, 'Tempo run')
    } finally {
      strava.restore()
    }
  })

  test('updates the athlete weight and stars or unstars a segment', async ({ assert }) => {
    const strava = mockStrava(({ method, url }) => {
      if (method !== 'PUT') return undefined
      if (url.pathname === '/api/v3/athlete') {
        return stravaJson({ ...stravaFixtures.athlete, weight: 68.4 })
      }
      if (url.pathname === '/api/v3/segments/229781/starred') {
        return stravaJson({ id: 229781, name: 'Hawk Hill', starred: false, resource_state: 3 })
      }
      return undefined
    })
    try {
      const mcp = await connectedStrava({ writeEnabled: true })

      const athlete = await callBuiltinTool(mcp, 'update_athlete_weight', { weight: 68.4 })
      assert.equal(JSON.parse(resultText(athlete)).weight, 68.4)

      const segment = await callBuiltinTool(mcp, 'star_segment', {
        segment_id: 229781,
        starred: false,
      })
      assert.deepEqual(JSON.parse(resultText(segment)), {
        id: 229781,
        name: 'Hawk Hill',
        starred: false,
      })

      assert.deepEqual(
        strava
          .apiRequests()
          .map((request) => [
            request.method,
            request.url.pathname,
            Object.fromEntries(request.form!),
          ]),
        [
          ['PUT', '/api/v3/athlete', { weight: '68.4' }],
          ['PUT', '/api/v3/segments/229781/starred', { starred: 'false' }],
        ]
      )
    } finally {
      strava.restore()
    }
  })

  test('rejects invalid write arguments before calling Strava', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava({ writeEnabled: true })
      const activity = {
        name: 'Ride',
        sport_type: 'Ride',
        start_date_local: '2026-10-03T08:00:00',
        elapsed_time: 1800,
      }
      const cases: Array<[string, Record<string, unknown>, string]> = [
        ['create_activity', { ...activity, name: '   ' }, 'name is required'],
        ['create_activity', { ...activity, sport_type: 'ride; DROP' }, 'sport_type must be'],
        ['create_activity', { ...activity, start_date_local: 'tomorrow' }, 'start_date_local must'],
        ['create_activity', { ...activity, elapsed_time: 0 }, 'elapsed_time must be an integer'],
        ['create_activity', { ...activity, distance: -5 }, 'distance must be a number'],
        ['update_activity', { activity_id: 1 }, 'Pass at least one field to change'],
        ['update_activity', { activity_id: 1, name: ' ' }, 'name must not be empty'],
        ['update_activity', { activity_id: 1, gear_id: '../x' }, 'gear_id must be'],
        ['update_activity', { name: 'No id' }, 'activity_id is required'],
        ['update_athlete_weight', { weight: 4 }, 'weight must be a number between 20 and 400'],
        ['star_segment', { segment_id: 'abc' }, 'segment_id must be an integer'],
      ]

      for (const [tool, args, message] of cases) {
        const result = await callBuiltinTool(mcp, tool, args)
        assert.isTrue(result.isError, `${tool} ${JSON.stringify(args)}`)
        assert.include(resultText(result), message)
      }
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })

  test('explains write failures reported by Strava', async ({ assert }) => {
    const responses: Response[] = [
      stravaJson(
        {
          message: 'Authorization Error',
          errors: [
            { resource: 'AccessToken', field: 'activity:write_permission', code: 'missing' },
          ],
        },
        401
      ),
      stravaJson(
        {
          message: 'Bad Request',
          errors: [{ resource: 'Activity', field: 'sport_type', code: 'invalid' }],
        },
        400
      ),
      stravaJson({ message: 'Rate Limit Exceeded', errors: [] }, 429, {
        'X-RateLimit-Limit': '200,2000',
        'X-RateLimit-Usage': '201,640',
        'X-ReadRateLimit-Limit': '100,1000',
        'X-ReadRateLimit-Usage': '12,340',
      }),
    ]
    const strava = mockStrava(({ method }) => (method === 'PUT' ? responses.shift() : undefined))
    try {
      const mcp = await connectedStrava({ writeEnabled: true })
      const rename = async () =>
        resultText(await callBuiltinTool(mcp, 'update_activity', { activity_id: 7, name: 'New' }))

      assert.include(await rename(), 'Strava permission "activity:write" was not granted')
      assert.equal(
        await rename(),
        'Strava API returned HTTP 400: Bad Request (Activity sport_type invalid)'
      )
      assert.include(
        await rename(),
        'Strava rate limit reached (201 of 200 requests in 15 minutes, 640 of 2000 today)'
      )
    } finally {
      strava.restore()
    }
  })
})
