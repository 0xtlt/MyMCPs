import { test } from '@japa/runner'
import type { VineValidator } from '@vinejs/vine'
import { DateTime } from 'luxon'
import { BuiltinToolError } from '#services/builtin/definition'
import { callBuiltinTool } from '#services/builtin/runtime'
import { toolInput } from '#services/builtin/tool_input'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { createStravaMcp, mockStrava, stravaJson } from '#tests/helpers/strava'
import { builtinTokenFailureValidator } from '#validators/builtin_oauth'
import {
  activityPageValidator,
  activityValidator,
  createActivityValidator,
  exploreSegmentsValidator,
  gearValidator,
  getActivityStreamsValidator,
  getActivityValidator,
  listActivitiesValidator,
  listSegmentEffortsValidator,
  pageValidator,
  routeValidator,
  segmentValidator,
  starSegmentValidator,
  STRAVA_STREAM_KEYS,
  stravaAthleteValidator,
  stravaFailureValidator,
  updateActivityValidator,
  updateAthleteWeightValidator,
} from '#validators/builtin_strava'

type Validator = VineValidator<any, any>
type Args = Record<string, unknown>

/** The sentence the agent reads when a tool refuses its arguments. */
async function refusal(validator: Validator, args: Args) {
  try {
    await toolInput(validator, args)
    return null
  } catch (error) {
    if (!(error instanceof BuiltinToolError)) throw error
    return error.message
  }
}

async function connectedStrava(options: Parameters<typeof createStravaMcp>[1] = {}) {
  const admin = await createAdmin()
  return createStravaMcp(admin.id, options)
}

function resultText(result: Awaited<ReturnType<typeof callBuiltinTool>>) {
  const [first] = result.content
  return first.type === 'text' ? first.text : ''
}

const box = { south_west_lat: 45.1, south_west_lng: 4.5, north_east_lat: 45.9, north_east_lng: 5.2 }
const activity = {
  name: 'Ride',
  sport_type: 'Ride',
  start_date_local: '2026-10-03T08:00:00',
  elapsed_time: 1800,
}

test.group('Built-in Strava MCP: validators', () => {
  test('returns the arguments of each tool the way the tool uses them', async ({ assert }) => {
    const cases: Array<[Validator, Args, Args]> = [
      [pageValidator, {}, {}],
      [pageValidator, { page: '2', per_page: 100, other: 1 }, { page: 2, per_page: 100 }],
      [activityValidator, { activity_id: '15000000001' }, { activity_id: 15000000001 }],
      [activityPageValidator, { activity_id: 7, page: 3 }, { activity_id: 7, page: 3 }],
      [
        getActivityValidator,
        { activity_id: 7, include_segment_efforts: 'true' },
        { include_segment_efforts: true, activity_id: 7 },
      ],
      [
        getActivityStreamsValidator,
        { activity_id: 7, keys: ['time', 'heartrate', 'time'], max_points: '3' },
        { max_points: 3, activity_id: 7, keys: ['time', 'heartrate'] },
      ],
      [getActivityStreamsValidator, { activity_id: 7, keys: null }, { activity_id: 7 }],
      [segmentValidator, { segment_id: 229781 }, { segment_id: 229781 }],
      [
        exploreSegmentsValidator,
        { ...box, south_west_lat: '45.1', activity_type: 'running', min_climb_category: 0 },
        { ...box, activity_type: 'running', min_climb_category: 0 },
      ],
      [exploreSegmentsValidator, { ...box, activity_type: '' }, box],
      [routeValidator, { route_id: 2984453279 }, { route_id: '2984453279' }],
      [routeValidator, { route_id: ' 2984453279043962922 ' }, { route_id: '2984453279043962922' }],
      [gearValidator, { gear_id: 'b101' }, { gear_id: 'b101' }],
      [
        createActivityValidator,
        { ...activity, name: '  Evening yoga ', distance: '0', description: '', trainer: true },
        {
          ...activity,
          name: 'Evening yoga',
          start_date_local: '2026-10-03T08:00:00Z',
          distance: 0,
          description: '',
          trainer: true,
        },
      ],
      [
        updateActivityValidator,
        { activity_id: 7, name: ' Tempo run ', description: '', gear_id: 'none', commute: false },
        { name: 'Tempo run', description: '', gear_id: 'none', commute: false, activity_id: 7 },
      ],
      [
        updateActivityValidator,
        { activity_id: 7, sport_type: '', gear_id: '' },
        { activity_id: 7 },
      ],
      [updateAthleteWeightValidator, { weight: '68.4' }, { weight: 68.4 }],
      [starSegmentValidator, { segment_id: 229781 }, { segment_id: 229781 }],
      [
        starSegmentValidator,
        { segment_id: 229781, starred: 'false' },
        { segment_id: 229781, starred: false },
      ],
    ]

    for (const [validator, args, expected] of cases) {
      assert.deepEqual(await toolInput(validator, args), expected, JSON.stringify(args))
    }
  })

  test('reads the dates of a list as moments in time', async ({ assert }) => {
    const { after, before, ...page } = await toolInput(listActivitiesValidator, {
      after: '2026-09-01',
      before: '2026-10-01T12:00:00+02:00',
      per_page: '5',
    })

    assert.equal(after?.toUnixInteger(), DateTime.fromISO('2026-09-01T00:00:00Z').toUnixInteger())
    assert.equal(before?.toISO({ suppressMilliseconds: true }), '2026-10-01T10:00:00Z')
    assert.deepEqual(page, { per_page: 5 })
  })

  test('tells the agent which argument is wrong, and what it must be', async ({ assert }) => {
    const cases: Array<[Validator, Args, string]> = [
      [pageValidator, { page: 0 }, 'page must be an integer of at least 1'],
      [pageValidator, { per_page: 101 }, 'per_page must be an integer between 1 and 100'],
      [activityValidator, {}, 'activity_id is required'],
      [activityValidator, { activity_id: 0 }, 'activity_id must be an integer of at least 1'],
      [activityValidator, { activity_id: '7/..' }, 'activity_id must be an integer of at least 1'],
      [
        getActivityValidator,
        { activity_id: 7, include_segment_efforts: 'yes' },
        'include_segment_efforts must be true or false',
      ],
      [
        getActivityStreamsValidator,
        { activity_id: 7, max_points: 1 },
        'max_points must be an integer between 2 and 1000',
      ],
      [segmentValidator, { segment_id: 'abc' }, 'segment_id must be an integer of at least 1'],
      [exploreSegmentsValidator, { south_west_lat: 45 }, 'south_west_lng is required'],
      [
        exploreSegmentsValidator,
        { ...box, south_west_lat: 120 },
        'south_west_lat must be a number between -90 and 90',
      ],
      [
        exploreSegmentsValidator,
        { ...box, north_east_lng: 181 },
        'north_east_lng must be a number between -180 and 180',
      ],
      [
        exploreSegmentsValidator,
        { ...box, activity_type: 'walking' },
        'activity_type must be one of: riding, running',
      ],
      [
        exploreSegmentsValidator,
        { ...box, max_climb_category: 6 },
        'max_climb_category must be an integer between 0 and 5',
      ],
      [
        listSegmentEffortsValidator,
        { segment_id: 1, start_date: 'last week' },
        'start_date must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z',
      ],
      [routeValidator, { route_id: '12?x=1' }, 'route_id must be a numeric route identifier'],
      [routeValidator, {}, 'route_id is required'],
      [
        gearValidator,
        { gear_id: '../athlete' },
        'gear_id must be a gear identifier such as b1234567',
      ],
      [createActivityValidator, { ...activity, name: '   ' }, 'name is required'],
      [
        createActivityValidator,
        { ...activity, name: 'x'.repeat(256) },
        'name must be text of at most 255 characters',
      ],
      [
        createActivityValidator,
        { ...activity, sport_type: 'ride; DROP' },
        'sport_type must be a Strava sport type like Run',
      ],
      [
        createActivityValidator,
        { ...activity, start_date_local: 'tomorrow' },
        'start_date_local must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00',
      ],
      [
        createActivityValidator,
        { ...activity, elapsed_time: 30 * 24 * 3600 + 1 },
        'elapsed_time must be an integer between 1 and 2592000',
      ],
      [
        createActivityValidator,
        { ...activity, distance: -5 },
        'distance must be a number between 0 and 10000000',
      ],
      [
        createActivityValidator,
        { ...activity, description: 'x'.repeat(5001) },
        'description must be text of at most 5000 characters',
      ],
      [createActivityValidator, { ...activity, commute: 1 }, 'commute must be true or false'],
      [updateActivityValidator, { activity_id: 1, name: ' ' }, 'name must not be empty'],
      [
        updateActivityValidator,
        { activity_id: 1, gear_id: '../x' },
        'gear_id must be a gear identifier such as b1234567, or "none"',
      ],
      [updateActivityValidator, { name: 'No id' }, 'activity_id is required'],
      [updateAthleteWeightValidator, { weight: 4 }, 'weight must be a number between 20 and 400'],
      [updateAthleteWeightValidator, {}, 'weight is required'],
      [starSegmentValidator, { segment_id: 1, starred: 'no' }, 'starred must be true or false'],
    ]

    for (const [validator, args, sentence] of cases) {
      assert.equal(await refusal(validator, args), sentence, JSON.stringify(args))
    }
  })

  test('names the first wrong argument, in the order each schema lists them', async ({
    assert,
  }) => {
    assert.equal(
      await refusal(getActivityValidator, { include_segment_efforts: 'x' }),
      'include_segment_efforts must be true or false'
    )
    assert.equal(
      await refusal(getActivityStreamsValidator, { max_points: 1, keys: [] }),
      'max_points must be an integer between 2 and 1000'
    )
    assert.equal(
      await refusal(updateActivityValidator, { name: 5, description: 5 }),
      'name must be text of at most 255 characters'
    )
    assert.equal(
      await refusal(listSegmentEffortsValidator, { end_date: 'x', per_page: 0 }),
      'segment_id is required'
    )
  })

  test('accepts the streams Strava records, once each, and names them otherwise', async ({
    assert,
  }) => {
    const sentence = `keys must be a non-empty array of: ${STRAVA_STREAM_KEYS.join(', ')}`
    const keys = async (value: unknown) => {
      const input = await toolInput(getActivityStreamsValidator, { activity_id: 1, keys: value })
      return input.keys
    }

    assert.deepEqual(await keys([...STRAVA_STREAM_KEYS]), [...STRAVA_STREAM_KEYS])
    assert.deepEqual(await keys(['watts', 'watts', 'latlng']), ['watts', 'latlng'])
    assert.isUndefined(await keys(undefined))
    for (const value of [[], ['pace'], ['time', null], ['time', 1], 'time', '', {}, 5]) {
      assert.equal(
        await refusal(getActivityStreamsValidator, { activity_id: 1, keys: value }),
        sentence,
        JSON.stringify(value)
      )
    }
  })

  test('refuses a wrong page of segment efforts, although Strava returns a single one', async ({
    assert,
  }) => {
    assert.deepEqual(await toolInput(listSegmentEffortsValidator, { segment_id: 1, page: 3 }), {
      segment_id: 1,
      page: 3,
    })
    assert.equal(
      await refusal(listSegmentEffortsValidator, { segment_id: 1, page: 'x' }),
      'page must be an integer of at least 1'
    )
  })
})

test.group('Built-in Strava MCP: what Strava answers', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('reads the identifier of the athlete, and nothing that is not a number', async ({
    assert,
  }) => {
    assert.deepEqual(
      await stravaAthleteValidator.validate({ id: 4242, firstname: 'Test', ftp: null }),
      { id: 4242 }
    )
    for (const body of [{ id: '4242' }, { id: null }, {}, [], null, 'athlete', 4242]) {
      const [unexpected] = await stravaAthleteValidator.tryValidate(body)
      assert.isNotNull(unexpected, JSON.stringify(body))
    }
  })

  test('reads what Strava says about a failure, and nothing from another body', async ({
    assert,
  }) => {
    const failure = {
      message: 'Bad Request',
      errors: [{ resource: 'Activity', field: 'sport_type', code: 'invalid', extra: 1 }],
      documentation_url: 'https://developers.strava.com',
    }
    assert.deepEqual(await stravaFailureValidator.validate(failure), {
      message: 'Bad Request',
      errors: [{ resource: 'Activity', field: 'sport_type', code: 'invalid' }],
    })
    assert.deepEqual(await stravaFailureValidator.validate({ message: null, errors: null }), {})

    for (const body of [null, [], '<html>', { message: 5 }, { errors: 'none' }, { errors: [5] }]) {
      const [unexpected] = await stravaFailureValidator.tryValidate(body)
      assert.isNotNull(unexpected, JSON.stringify(body))
    }
  })

  test('reads why a token request was refused, in either of the two shapes', async ({ assert }) => {
    assert.deepEqual(
      await builtinTokenFailureValidator.validate({
        error: 'invalid_grant',
        error_description: 'The refresh token was revoked',
        error_uri: 'https://example.com',
      }),
      { error: 'invalid_grant', error_description: 'The refresh token was revoked' }
    )
    assert.deepEqual(
      await builtinTokenFailureValidator.validate({
        message: 'Bad Request',
        errors: [{ resource: 'RefreshToken', field: 'refresh_token', code: 'invalid' }],
      }),
      {
        message: 'Bad Request',
        errors: [{ resource: 'RefreshToken', field: 'refresh_token', code: 'invalid' }],
      }
    )
    const [unexpected] = await builtinTokenFailureValidator.tryValidate({ error: { code: 400 } })
    assert.isNotNull(unexpected)
  })

  test('refuses to guess the athlete when Strava does not name one', async ({ assert }) => {
    for (const [index, athlete] of [{ id: '4242' }, {}, null, []].entries()) {
      const strava = mockStrava(({ url }) =>
        url.pathname === '/api/v3/athlete' ? stravaJson(athlete) : undefined
      )
      try {
        const mcp = await connectedStrava({ name: `Strava ${index}` })
        const result = await callBuiltinTool(mcp, 'get_athlete_stats', {})

        assert.isTrue(result.isError)
        assert.equal(resultText(result), 'Strava did not return the connected athlete')
        assert.lengthOf(strava.apiRequests(), 1)
      } finally {
        strava.restore()
      }
    }
  })

  test('explains a failure from the status alone when its body is not the documented one', async ({
    assert,
  }) => {
    const bodies: unknown[] = [
      { message: 'Bad Request', errors: [null] },
      { message: { text: 'Bad Request' } },
      { errors: [{ resource: 'Activity', code: 400 }] },
      ['Bad Request'],
    ]
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/activities/7' ? stravaJson(bodies.shift(), 400) : undefined
    )
    try {
      const mcp = await connectedStrava()

      while (bodies.length > 0) {
        const result = await callBuiltinTool(mcp, 'get_activity', { activity_id: 7 })
        assert.isTrue(result.isError)
        assert.equal(resultText(result), 'Strava API returned HTTP 400')
      }
    } finally {
      strava.restore()
    }
  })

  test('explains a refused renewal from an RFC 6749 error', async ({ assert }) => {
    const strava = mockStrava(({ url }) =>
      url.pathname === '/api/v3/oauth/token'
        ? stravaJson({ error: 'invalid_grant', error_description: 'Token was revoked' }, 400)
        : undefined
    )
    try {
      const mcp = await connectedStrava({ expiresAt: DateTime.utc().minus({ minutes: 1 }) })
      const result = await callBuiltinTool(mcp, 'get_athlete', {})

      assert.isTrue(result.isError)
      assert.include(
        resultText(result),
        'Strava refused to renew the saved authorization (Token was revoked)'
      )
    } finally {
      strava.restore()
    }
  })

  test('refuses wrong arguments before asking Strava for anything', async ({ assert }) => {
    const strava = mockStrava()
    try {
      const mcp = await connectedStrava({ writeEnabled: true })
      const cases: Array<[string, Args, string]> = [
        ['list_routes', { per_page: 500 }, 'per_page must be an integer between 1 and 100'],
        [
          'list_segment_efforts',
          { segment_id: 1, page: 0 },
          'page must be an integer of at least 1',
        ],
        ['update_activity', { activity_id: 1 }, 'Pass at least one field to change'],
        [
          'update_activity',
          { activity_id: 1, sport_type: '' },
          'Pass at least one field to change',
        ],
      ]

      for (const [tool, args, sentence] of cases) {
        const result = await callBuiltinTool(mcp, tool, args)
        assert.isTrue(result.isError, tool)
        assert.equal(resultText(result), sentence)
      }
      assert.lengthOf(strava.requests, 0)
    } finally {
      strava.restore()
    }
  })
})
