import { BuiltinToolError, type BuiltinTool } from '#services/builtin/definition'
import {
  booleanInput,
  enumInput,
  integerInput,
  isoDateInput,
  numberInput,
  patternInput,
  required,
} from '#services/builtin/tool_input'
import { stravaGet } from '#services/builtin/strava/api'
import {
  activitySummaries,
  compactStravaPayload,
  downsampleStreams,
} from '#services/builtin/strava/payload'

type Args = Record<string, unknown>

const UNITS =
  'Distances and elevations are in meters, durations in seconds, and speeds in meters per second.'

const ACTIVITY_SCOPES = ['activity:read', 'activity:read_all'] as const

const STREAM_KEYS = [
  'time',
  'distance',
  'latlng',
  'altitude',
  'velocity_smooth',
  'heartrate',
  'cadence',
  'watts',
  'temp',
  'moving',
  'grade_smooth',
] as const

const DEFAULT_STREAM_KEYS: ReadonlyArray<(typeof STREAM_KEYS)[number]> = [
  'time',
  'distance',
  'altitude',
  'velocity_smooth',
  'heartrate',
  'cadence',
  'watts',
]

const DEFAULT_PAGE_SIZE = 30
const MAX_PAGE_SIZE = 100
const DEFAULT_STREAM_POINTS = 200
const MAX_STREAM_POINTS = 1000

const paginationProperties = {
  page: { type: 'integer', minimum: 1, default: 1, description: 'Page number, starting at 1.' },
  per_page: {
    type: 'integer',
    minimum: 1,
    maximum: MAX_PAGE_SIZE,
    default: DEFAULT_PAGE_SIZE,
    description: 'Number of items per page.',
  },
} as const

const activityIdProperty = {
  activity_id: {
    type: 'integer',
    description: 'Activity identifier, as returned by list_activities.',
  },
} as const

function pagination(args: Args) {
  return {
    page: integerInput(args, 'page', { min: 1 }) ?? 1,
    per_page: integerInput(args, 'per_page', { min: 1, max: MAX_PAGE_SIZE }) ?? DEFAULT_PAGE_SIZE,
  }
}

function isoTimestamp(args: Args, name: string) {
  return isoDateInput(args, name)?.toISO({ suppressMilliseconds: true }) ?? undefined
}

function idInput(args: Args, name: string) {
  return required(integerInput(args, name, { min: 1 }), name)
}

async function athleteId(accessToken: string) {
  const athlete = (await stravaGet(accessToken, '/athlete')) as { id?: unknown } | null
  if (typeof athlete?.id !== 'number') {
    throw new BuiltinToolError('Strava did not return the connected athlete')
  }
  return athlete.id
}

function streamKeysInput(args: Args) {
  const raw = args.keys
  if (raw === undefined || raw === null) return DEFAULT_STREAM_KEYS

  const allowed: readonly string[] = STREAM_KEYS
  if (
    !Array.isArray(raw) ||
    raw.length === 0 ||
    !raw.every((key) => typeof key === 'string' && allowed.includes(key))
  ) {
    throw new BuiltinToolError(`keys must be a non-empty array of: ${STREAM_KEYS.join(', ')}`)
  }
  return [...new Set(raw as string[])]
}

export const stravaTools: readonly BuiltinTool[] = [
  {
    name: 'get_athlete',
    description:
      "Get the connected Strava athlete's profile: name, location, weight, FTP, measurement preference, and their bikes and shoes with the distance on each.",
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    run: async (_args, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete')),
  },
  {
    name: 'get_athlete_stats',
    description: `Get the connected athlete's ride, run, and swim totals for the last 4 weeks, the current year, and all time, plus their longest ride and biggest climb. Strava only counts activities visible to Everyone. ${UNITS}`,
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    run: async (_args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/athletes/${await athleteId(accessToken)}/stats`)
      ),
  },
  {
    name: 'get_athlete_zones',
    description: "Get the connected athlete's heart rate and power zone boundaries.",
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    requiresAnyScope: ['profile:read_all'],
    run: async (_args, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete/zones')),
  },
  {
    name: 'list_activities',
    description: `List the connected athlete's activities with summary metrics: distance, time, elevation, speed, heart rate, and power. Results are newest first, or oldest first when "after" is set. Call get_activity for the full detail of one activity. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        after: {
          type: 'string',
          description:
            'Only activities that started after this ISO 8601 date or datetime, such as 2026-01-31.',
        },
        before: {
          type: 'string',
          description: 'Only activities that started before this ISO 8601 date or datetime.',
        },
        ...paginationProperties,
      },
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) =>
      activitySummaries(
        await stravaGet(accessToken, '/athlete/activities', {
          after: isoDateInput(args, 'after')?.toUnixInteger(),
          before: isoDateInput(args, 'before')?.toUnixInteger(),
          ...pagination(args),
        })
      ),
  },
  {
    name: 'get_activity',
    description: `Get one activity in full: description, calories, device, gear, splits, laps, and best efforts. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...activityIdProperty,
        include_segment_efforts: {
          type: 'boolean',
          default: false,
          description:
            'Also return every segment effort of the activity. This makes the result much larger.',
        },
      },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) => {
      const includeSegmentEfforts = booleanInput(args, 'include_segment_efforts') ?? false
      const activity = compactStravaPayload(
        await stravaGet(accessToken, `/activities/${idInput(args, 'activity_id')}`, {
          include_all_efforts: includeSegmentEfforts ? true : undefined,
        })
      ) as Record<string, unknown>
      if (!includeSegmentEfforts) delete activity.segment_efforts
      return activity
    },
  },
  {
    name: 'get_activity_streams',
    description: `Get the time series recorded during an activity, such as heart rate, power, cadence, speed, and altitude. Each stream is reduced to at most max_points evenly spaced samples that share the same positions, so index i of every stream is the same moment. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...activityIdProperty,
        keys: {
          type: 'array',
          items: { type: 'string', enum: [...STREAM_KEYS] },
          default: [...DEFAULT_STREAM_KEYS],
          description:
            'Streams to return. Strava omits streams the activity did not record. "latlng" contains GPS coordinates.',
        },
        max_points: {
          type: 'integer',
          minimum: 2,
          maximum: MAX_STREAM_POINTS,
          default: DEFAULT_STREAM_POINTS,
          description: 'Maximum number of samples per stream.',
        },
      },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) => {
      const maxPoints =
        integerInput(args, 'max_points', { min: 2, max: MAX_STREAM_POINTS }) ??
        DEFAULT_STREAM_POINTS
      return downsampleStreams(
        await stravaGet(accessToken, `/activities/${idInput(args, 'activity_id')}/streams`, {
          keys: streamKeysInput(args).join(','),
          key_by_type: true,
        }),
        maxPoints
      )
    },
  },
  {
    name: 'get_activity_zones',
    description:
      'Get the time spent in each heart rate and power zone during an activity, in seconds. Strava requires a subscription for this data.',
    inputSchema: {
      type: 'object',
      properties: activityIdProperty,
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/activities/${idInput(args, 'activity_id')}/zones`)
      ),
  },
  {
    name: 'list_activity_comments',
    description: 'List the comments on an activity.',
    inputSchema: {
      type: 'object',
      properties: { ...activityIdProperty, ...paginationProperties },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(
          accessToken,
          `/activities/${idInput(args, 'activity_id')}/comments`,
          pagination(args)
        )
      ),
  },
  {
    name: 'list_activity_kudos',
    description: 'List the athletes who gave kudos to an activity.',
    inputSchema: {
      type: 'object',
      properties: { ...activityIdProperty, ...paginationProperties },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(
          accessToken,
          `/activities/${idInput(args, 'activity_id')}/kudos`,
          pagination(args)
        )
      ),
  },
  {
    name: 'list_starred_segments',
    description: `List the segments the connected athlete starred. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    run: async (args, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/segments/starred', pagination(args))),
  },
  {
    name: 'get_segment',
    description: `Get a segment: distance, grade, elevation, effort counts, and the connected athlete's personal record and effort count on it. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: { segment_id: { type: 'integer', description: 'Segment identifier.' } },
      required: ['segment_id'],
      additionalProperties: false,
    },
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/segments/${idInput(args, 'segment_id')}`)
      ),
  },
  {
    name: 'explore_segments',
    description: `Find up to 10 popular segments inside a latitude/longitude bounding box. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        south_west_lat: { type: 'number', minimum: -90, maximum: 90 },
        south_west_lng: { type: 'number', minimum: -180, maximum: 180 },
        north_east_lat: { type: 'number', minimum: -90, maximum: 90 },
        north_east_lng: { type: 'number', minimum: -180, maximum: 180 },
        activity_type: { type: 'string', enum: ['riding', 'running'], default: 'riding' },
        min_climb_category: {
          type: 'integer',
          minimum: 0,
          maximum: 5,
          description: 'Lowest climb category to include, from 0 (uncategorized) to 5 (hardest).',
        },
        max_climb_category: {
          type: 'integer',
          minimum: 0,
          maximum: 5,
          description: 'Highest climb category to include, from 0 (uncategorized) to 5 (hardest).',
        },
      },
      required: ['south_west_lat', 'south_west_lng', 'north_east_lat', 'north_east_lng'],
      additionalProperties: false,
    },
    run: async (args, { accessToken }) => {
      const latitude = { min: -90, max: 90 }
      const longitude = { min: -180, max: 180 }
      const bounds = [
        required(numberInput(args, 'south_west_lat', latitude), 'south_west_lat'),
        required(numberInput(args, 'south_west_lng', longitude), 'south_west_lng'),
        required(numberInput(args, 'north_east_lat', latitude), 'north_east_lat'),
        required(numberInput(args, 'north_east_lng', longitude), 'north_east_lng'),
      ]
      return compactStravaPayload(
        await stravaGet(accessToken, '/segments/explore', {
          bounds: bounds.join(','),
          activity_type: enumInput(args, 'activity_type', ['riding', 'running']),
          min_cat: integerInput(args, 'min_climb_category', { min: 0, max: 5 }),
          max_cat: integerInput(args, 'max_climb_category', { min: 0, max: 5 }),
        })
      )
    },
  },
  {
    name: 'list_segment_efforts',
    description: `List the connected athlete's efforts on one segment, optionally within a date range. Strava requires a subscription for this data. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        segment_id: { type: 'integer', description: 'Segment identifier.' },
        start_date: {
          type: 'string',
          description: 'Only efforts on or after this ISO 8601 date or datetime.',
        },
        end_date: {
          type: 'string',
          description: 'Only efforts on or before this ISO 8601 date or datetime.',
        },
        per_page: paginationProperties.per_page,
      },
      required: ['segment_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, '/segment_efforts', {
          segment_id: idInput(args, 'segment_id'),
          start_date_local: isoTimestamp(args, 'start_date'),
          end_date_local: isoTimestamp(args, 'end_date'),
          per_page: pagination(args).per_page,
        })
      ),
  },
  {
    name: 'list_routes',
    description: `List the routes the connected athlete created. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    run: async (args, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(
          accessToken,
          `/athletes/${await athleteId(accessToken)}/routes`,
          pagination(args)
        )
      ),
  },
  {
    name: 'get_route',
    description: `Get a route: distance, elevation gain, estimated moving time, and the segments along it. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        route_id: {
          type: 'string',
          description: 'Route identifier, as returned by list_routes. Pass it as a string.',
        },
      },
      required: ['route_id'],
      additionalProperties: false,
    },
    run: async (args, { accessToken }) => {
      const routeId = required(
        patternInput(args, 'route_id', /^\d{1,20}$/, 'a numeric route identifier'),
        'route_id'
      )
      return compactStravaPayload(await stravaGet(accessToken, `/routes/${routeId}`))
    },
  },
  {
    name: 'list_clubs',
    description: 'List the clubs the connected athlete is a member of.',
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    run: async (args, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete/clubs', pagination(args))),
  },
  {
    name: 'get_gear',
    description:
      'Get a bike or pair of shoes: brand, model, and total distance in meters. Gear identifiers come from get_athlete and from the gear_id of an activity.',
    inputSchema: {
      type: 'object',
      properties: {
        gear_id: { type: 'string', description: 'Gear identifier, such as b1234567 or g1234567.' },
      },
      required: ['gear_id'],
      additionalProperties: false,
    },
    run: async (args, { accessToken }) => {
      const gearId = required(
        patternInput(args, 'gear_id', /^[bg]\d{1,20}$/, 'a gear identifier such as b1234567'),
        'gear_id'
      )
      return compactStravaPayload(await stravaGet(accessToken, `/gear/${gearId}`))
    },
  },
]
