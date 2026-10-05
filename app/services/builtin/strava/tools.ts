import { BuiltinToolError, type BuiltinTool } from '#services/builtin/definition'
import { builtinTool } from '#services/builtin/tool_input'
import { stravaGet, stravaRequest } from '#services/builtin/strava/api'
import {
  activitySummaries,
  compactStravaPayload,
  downsampleStreams,
} from '#services/builtin/strava/payload'
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
  STRAVA_LIMITS,
  STRAVA_STREAM_KEYS,
  stravaAthleteValidator,
  updateActivityValidator,
  updateAthleteWeightValidator,
  type StravaStreamKey,
} from '#validators/builtin_strava'
import { noArgumentsValidator } from '#validators/builtin_tools'

const UNITS =
  'Distances and elevations are in meters, durations in seconds, and speeds in meters per second.'

const ACTIVITY_SCOPES = ['activity:read', 'activity:read_all'] as const

const DEFAULT_STREAM_KEYS: readonly StravaStreamKey[] = [
  'time',
  'distance',
  'altitude',
  'velocity_smooth',
  'heartrate',
  'cadence',
  'watts',
]

const SPORT_TYPE_HINT =
  'Strava sport type in PascalCase, such as Run, TrailRun, Ride, GravelRide, MountainBikeRide, VirtualRide, Swim, Walk, Hike, WeightTraining, Workout, or Yoga.'

const DEFAULT_PAGE_SIZE = 30
const DEFAULT_STREAM_POINTS = 200

const paginationProperties = {
  page: { type: 'integer', minimum: 1, default: 1, description: 'Page number, starting at 1.' },
  per_page: {
    type: 'integer',
    minimum: 1,
    maximum: STRAVA_LIMITS.pageSize,
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

function pagination({ page = 1, per_page: perPage = DEFAULT_PAGE_SIZE }) {
  return { page, per_page: perPage }
}

/** The activity as get_activity returns it, without its segment efforts. */
function activityResult(activity: unknown) {
  const compacted = compactStravaPayload(activity) as Record<string, unknown>
  delete compacted.segment_efforts
  return compacted
}

async function athleteId(accessToken: string) {
  const [unexpected, athlete] = await stravaAthleteValidator.tryValidate(
    await stravaGet(accessToken, '/athlete')
  )
  if (unexpected) {
    throw new BuiltinToolError('Strava did not return the connected athlete')
  }
  return athlete.id
}

export const stravaTools: readonly BuiltinTool[] = [
  builtinTool({
    name: 'get_athlete',
    description:
      "Get the connected Strava athlete's profile: name, location, weight, FTP, measurement preference, and their bikes and shoes with the distance on each.",
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    input: noArgumentsValidator,
    run: async (_input, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete')),
  }),
  builtinTool({
    name: 'get_athlete_stats',
    description: `Get the connected athlete's ride, run, and swim totals for the last 4 weeks, the current year, and all time, plus their longest ride and biggest climb. Strava only counts activities visible to Everyone. ${UNITS}`,
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    input: noArgumentsValidator,
    run: async (_input, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/athletes/${await athleteId(accessToken)}/stats`)
      ),
  }),
  builtinTool({
    name: 'get_athlete_zones',
    description: "Get the connected athlete's heart rate and power zone boundaries.",
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    requiresAnyScope: ['profile:read_all'],
    input: noArgumentsValidator,
    run: async (_input, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete/zones')),
  }),
  builtinTool({
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
    input: listActivitiesValidator,
    run: async ({ after, before, ...page }, { accessToken }) =>
      activitySummaries(
        await stravaGet(accessToken, '/athlete/activities', {
          after: after?.toUnixInteger(),
          before: before?.toUnixInteger(),
          ...pagination(page),
        })
      ),
  }),
  builtinTool({
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
    input: getActivityValidator,
    run: async (
      { activity_id: activityId, include_segment_efforts: includeSegmentEfforts = false },
      { accessToken }
    ) => {
      const activity = await stravaGet(accessToken, `/activities/${activityId}`, {
        include_all_efforts: includeSegmentEfforts ? true : undefined,
      })
      return includeSegmentEfforts ? compactStravaPayload(activity) : activityResult(activity)
    },
  }),
  builtinTool({
    name: 'get_activity_streams',
    description: `Get the time series recorded during an activity, such as heart rate, power, cadence, speed, and altitude. Each stream is reduced to at most max_points evenly spaced samples that share the same positions, so index i of every stream is the same moment. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...activityIdProperty,
        keys: {
          type: 'array',
          items: { type: 'string', enum: [...STRAVA_STREAM_KEYS] },
          default: [...DEFAULT_STREAM_KEYS],
          description:
            'Streams to return. Strava omits streams the activity did not record. "latlng" contains GPS coordinates.',
        },
        max_points: {
          type: 'integer',
          minimum: 2,
          maximum: STRAVA_LIMITS.streamPoints,
          default: DEFAULT_STREAM_POINTS,
          description: 'Maximum number of samples per stream.',
        },
      },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    input: getActivityStreamsValidator,
    run: async (
      {
        activity_id: activityId,
        keys = DEFAULT_STREAM_KEYS,
        max_points: maxPoints = DEFAULT_STREAM_POINTS,
      },
      { accessToken }
    ) =>
      downsampleStreams(
        await stravaGet(accessToken, `/activities/${activityId}/streams`, {
          keys: keys.join(','),
          key_by_type: true,
        }),
        maxPoints
      ),
  }),
  builtinTool({
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
    input: activityValidator,
    run: async ({ activity_id: activityId }, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, `/activities/${activityId}/zones`)),
  }),
  builtinTool({
    name: 'list_activity_comments',
    description: 'List the comments on an activity.',
    inputSchema: {
      type: 'object',
      properties: { ...activityIdProperty, ...paginationProperties },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    input: activityPageValidator,
    run: async ({ activity_id: activityId, ...page }, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/activities/${activityId}/comments`, pagination(page))
      ),
  }),
  builtinTool({
    name: 'list_activity_kudos',
    description: 'List the athletes who gave kudos to an activity.',
    inputSchema: {
      type: 'object',
      properties: { ...activityIdProperty, ...paginationProperties },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ACTIVITY_SCOPES,
    input: activityPageValidator,
    run: async ({ activity_id: activityId, ...page }, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, `/activities/${activityId}/kudos`, pagination(page))
      ),
  }),
  builtinTool({
    name: 'list_starred_segments',
    description: `List the segments the connected athlete starred. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    input: pageValidator,
    run: async (page, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/segments/starred', pagination(page))),
  }),
  builtinTool({
    name: 'get_segment',
    description: `Get a segment: distance, grade, elevation, effort counts, and the connected athlete's personal record and effort count on it. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: { segment_id: { type: 'integer', description: 'Segment identifier.' } },
      required: ['segment_id'],
      additionalProperties: false,
    },
    input: segmentValidator,
    run: async ({ segment_id: segmentId }, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, `/segments/${segmentId}`)),
  }),
  builtinTool({
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
    input: exploreSegmentsValidator,
    run: async (input, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(accessToken, '/segments/explore', {
          bounds: [
            input.south_west_lat,
            input.south_west_lng,
            input.north_east_lat,
            input.north_east_lng,
          ].join(','),
          activity_type: input.activity_type,
          min_cat: input.min_climb_category,
          max_cat: input.max_climb_category,
        })
      ),
  }),
  builtinTool({
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
    input: listSegmentEffortsValidator,
    run: async (
      { segment_id: segmentId, start_date: startDate, end_date: endDate, ...page },
      { accessToken }
    ) =>
      compactStravaPayload(
        await stravaGet(accessToken, '/segment_efforts', {
          segment_id: segmentId,
          start_date_local: startDate?.toISO({ suppressMilliseconds: true }),
          end_date_local: endDate?.toISO({ suppressMilliseconds: true }),
          per_page: pagination(page).per_page,
        })
      ),
  }),
  builtinTool({
    name: 'list_routes',
    description: `List the routes the connected athlete created. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    input: pageValidator,
    run: async (page, { accessToken }) =>
      compactStravaPayload(
        await stravaGet(
          accessToken,
          `/athletes/${await athleteId(accessToken)}/routes`,
          pagination(page)
        )
      ),
  }),
  builtinTool({
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
    input: routeValidator,
    run: async ({ route_id: routeId }, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, `/routes/${routeId}`)),
  }),
  builtinTool({
    name: 'list_clubs',
    description: 'List the clubs the connected athlete is a member of.',
    inputSchema: {
      type: 'object',
      properties: paginationProperties,
      additionalProperties: false,
    },
    input: pageValidator,
    run: async (page, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, '/athlete/clubs', pagination(page))),
  }),
  builtinTool({
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
    input: gearValidator,
    run: async ({ gear_id: gearId }, { accessToken }) =>
      compactStravaPayload(await stravaGet(accessToken, `/gear/${gearId}`)),
  }),
  builtinTool({
    name: 'create_activity',
    description: `Create a manual activity on the connected athlete's Strava account, such as a workout recorded without a device. It appears in their feed like any other activity. Strava has no API to delete an activity, so confirm the details with the user first. ${UNITS}`,
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          maxLength: STRAVA_LIMITS.nameLength,
          description: 'Activity title.',
        },
        sport_type: { type: 'string', description: SPORT_TYPE_HINT },
        start_date_local: {
          type: 'string',
          description:
            "Start in the athlete's local time as an ISO 8601 date and time, such as 2026-01-31T18:00:00.",
        },
        elapsed_time: { type: 'integer', minimum: 1, description: 'Duration in seconds.' },
        distance: { type: 'number', minimum: 0, description: 'Distance in meters.' },
        description: { type: 'string', maxLength: STRAVA_LIMITS.descriptionLength },
        trainer: { type: 'boolean', description: 'Recorded on an indoor trainer or treadmill.' },
        commute: { type: 'boolean', description: 'Mark the activity as a commute.' },
      },
      required: ['name', 'sport_type', 'start_date_local', 'elapsed_time'],
      additionalProperties: false,
    },
    requiresAnyScope: ['activity:write'],
    write: true,
    input: createActivityValidator,
    run: async ({ trainer, commute, ...activity }, { accessToken }) => {
      // Strava reads these two flags from a form as 1 and 0.
      const flag = (value: boolean | undefined) => (value === undefined ? undefined : Number(value))
      return activityResult(
        await stravaRequest(accessToken, '/activities', {
          method: 'POST',
          form: { ...activity, trainer: flag(trainer), commute: flag(commute) },
        })
      )
    },
  }),
  builtinTool({
    name: 'update_activity',
    description:
      "Change an activity of the connected athlete: its title, description, sport type, gear, or its commute, trainer, and muted flags. Only the fields you pass are changed. Strava's API cannot change an activity's visibility, date, distance, or time.",
    inputSchema: {
      type: 'object',
      properties: {
        ...activityIdProperty,
        name: { type: 'string', maxLength: STRAVA_LIMITS.nameLength, description: 'New title.' },
        description: {
          type: 'string',
          maxLength: STRAVA_LIMITS.descriptionLength,
          description: 'New description. Pass an empty string to clear it.',
        },
        sport_type: { type: 'string', description: SPORT_TYPE_HINT },
        gear_id: {
          type: 'string',
          description:
            'Gear identifier from get_athlete, such as b1234567, or "none" to remove the gear.',
        },
        commute: { type: 'boolean' },
        trainer: { type: 'boolean' },
        hide_from_home: {
          type: 'boolean',
          description: "Mute the activity so it stays out of followers' home feeds.",
        },
      },
      required: ['activity_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ['activity:write'],
    write: true,
    input: updateActivityValidator,
    run: async ({ activity_id: activityId, ...changes }, { accessToken }) => {
      if (Object.keys(changes).length === 0) {
        throw new BuiltinToolError('Pass at least one field to change')
      }
      return activityResult(
        await stravaRequest(accessToken, `/activities/${activityId}`, {
          method: 'PUT',
          json: changes,
        })
      )
    },
  }),
  builtinTool({
    name: 'update_athlete_weight',
    description:
      "Set the connected athlete's weight on their Strava profile, in kilograms. Strava uses it to estimate power and calories.",
    inputSchema: {
      type: 'object',
      properties: {
        weight: { type: 'number', minimum: 20, maximum: 400, description: 'Weight in kilograms.' },
      },
      required: ['weight'],
      additionalProperties: false,
    },
    requiresAnyScope: ['profile:write'],
    write: true,
    input: updateAthleteWeightValidator,
    run: async ({ weight }, { accessToken }) =>
      compactStravaPayload(
        await stravaRequest(accessToken, '/athlete', { method: 'PUT', form: { weight } })
      ),
  }),
  builtinTool({
    name: 'star_segment',
    description: 'Star or unstar a segment for the connected athlete.',
    inputSchema: {
      type: 'object',
      properties: {
        segment_id: { type: 'integer', description: 'Segment identifier.' },
        starred: {
          type: 'boolean',
          default: true,
          description: 'True to star the segment, false to unstar it.',
        },
      },
      required: ['segment_id'],
      additionalProperties: false,
    },
    requiresAnyScope: ['profile:write'],
    write: true,
    input: starSegmentValidator,
    run: async ({ segment_id: segmentId, starred = true }, { accessToken }) =>
      compactStravaPayload(
        await stravaRequest(accessToken, `/segments/${segmentId}/starred`, {
          method: 'PUT',
          form: { starred },
        })
      ),
  }),
]
