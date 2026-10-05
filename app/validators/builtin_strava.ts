/**
 * Vine schemas for the built-in Strava MCP: the arguments of its tools, and
 * the JSON Strava answers with. A schema lists the arguments in the order
 * they are checked: of several wrong ones, the agent is told about the first.
 */
import vine from '@vinejs/vine'
import {
  boolean,
  choice,
  integer,
  isoDate,
  localTimestamp,
  number,
  pattern,
  text,
  toolVine,
  trimmedText,
  VineArgument,
} from '#validators/builtin_tools'

/** The bounds the tools advertise, and the schemas below enforce. */
export const STRAVA_LIMITS = {
  pageSize: 100,
  streamPoints: 1000,
  nameLength: 255,
  descriptionLength: 5000,
} as const

export const STRAVA_STREAM_KEYS = [
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

export type StravaStreamKey = (typeof STRAVA_STREAM_KEYS)[number]

const id = () => integer({ min: 1 })

const pagination = () => ({
  page: integer({ min: 1 }).optional(),
  per_page: integer({ min: 1, max: STRAVA_LIMITS.pageSize }).optional(),
})

const sportType = () => pattern(/^[A-Z][A-Za-z]{1,39}$/, 'a Strava sport type like Run')

const latitude = () => number({ min: -90, max: 90 })
const longitude = () => number({ min: -180, max: 180 })

/** One sentence whatever is wrong with the list, so one rule for all of it. */
const streamKeysRule = toolVine.createRule(
  (value, _options, field) => {
    const known: readonly unknown[] = STRAVA_STREAM_KEYS
    if (!Array.isArray(value) || value.length === 0 || !value.every((key) => known.includes(key))) {
      field.report('{{ field }} must be a non-empty array of: {{ keys }}', 'streamKeys', field, {
        keys: STRAVA_STREAM_KEYS.join(', '),
      })
      return
    }
    field.mutate([...new Set(value)], field)
  },
  {
    toJSONSchema: (schema) => {
      Object.assign(schema, {
        type: 'array',
        items: { type: 'string', enum: [...STRAVA_STREAM_KEYS] },
      })
    },
  }
)

const notBlankRule = toolVine.createRule((value, _options, field) => {
  const trimmed = (value as string).trim()
  if (trimmed === '') {
    field.report('{{ field }} must not be empty', 'notBlank', field)
    return
  }
  field.mutate(trimmed, field)
})

export const listActivitiesValidator = toolVine.create({
  after: isoDate().optional(),
  before: isoDate().optional(),
  ...pagination(),
})

export const getActivityValidator = toolVine.create({
  include_segment_efforts: boolean().optional(),
  activity_id: id(),
})

export const getActivityStreamsValidator = toolVine.create({
  max_points: integer({ min: 2, max: STRAVA_LIMITS.streamPoints }).optional(),
  activity_id: id(),
  keys: new VineArgument<StravaStreamKey[]>(streamKeysRule()).optional(),
})

export const activityValidator = toolVine.create({
  activity_id: id(),
})

/** One page of what belongs to an activity, such as its comments. */
export const activityPageValidator = toolVine.create({
  activity_id: id(),
  ...pagination(),
})

/** One page of what belongs to the athlete, such as their routes. */
export const pageValidator = toolVine.create(pagination())

export const segmentValidator = toolVine.create({
  segment_id: id(),
})

export const exploreSegmentsValidator = toolVine.create({
  south_west_lat: latitude(),
  south_west_lng: longitude(),
  north_east_lat: latitude(),
  north_east_lng: longitude(),
  activity_type: choice(['riding', 'running']).optional(),
  min_climb_category: integer({ min: 0, max: 5 }).optional(),
  max_climb_category: integer({ min: 0, max: 5 }).optional(),
})

export const listSegmentEffortsValidator = toolVine.create({
  segment_id: id(),
  start_date: isoDate().optional(),
  end_date: isoDate().optional(),
  // Only `per_page` is advertised and sent: Strava returns these efforts on
  // one page. A wrong `page` is refused all the same, as for the other lists.
  ...pagination(),
})

export const routeValidator = toolVine.create({
  route_id: pattern(/^\d{1,20}$/, 'a numeric route identifier'),
})

export const gearValidator = toolVine.create({
  gear_id: pattern(/^[bg]\d{1,20}$/, 'a gear identifier such as b1234567'),
})

export const createActivityValidator = toolVine.create({
  name: trimmedText(STRAVA_LIMITS.nameLength),
  sport_type: sportType(),
  start_date_local: localTimestamp(),
  elapsed_time: integer({ min: 1, max: 30 * 24 * 3600 }),
  distance: number({ min: 0, max: 10_000_000 }).optional(),
  description: text(STRAVA_LIMITS.descriptionLength).optional(),
  trainer: boolean().optional(),
  commute: boolean().optional(),
})

export const updateActivityValidator = toolVine.create({
  name: text(STRAVA_LIMITS.nameLength).use(notBlankRule()).optional(),
  description: text(STRAVA_LIMITS.descriptionLength).optional(),
  sport_type: sportType().optional(),
  gear_id: pattern(
    /^([bg]\d{1,20}|none)$/,
    'a gear identifier such as b1234567, or "none"'
  ).optional(),
  commute: boolean().optional(),
  trainer: boolean().optional(),
  hide_from_home: boolean().optional(),
  activity_id: id(),
})

export const updateAthleteWeightValidator = toolVine.create({
  weight: number({ min: 20, max: 400 }),
})

export const starSegmentValidator = toolVine.create({
  segment_id: id(),
  starred: boolean().optional(),
})

/** The athlete Strava answers with, of which only the identifier is read. */
export const stravaAthleteValidator = vine.create({
  id: vine.number({ strict: true }),
})

/** The reasons Strava gives for refusing a request, such as `{ resource: 'Activity', field: 'sport_type', code: 'invalid' }`. */
export const stravaFaults = () =>
  vine.array(
    vine.object({
      resource: vine.string().optional(),
      field: vine.string().optional(),
      code: vine.string().optional(),
    })
  )

/** The body of a response Strava sent with an error status. */
export const stravaFailureValidator = vine.create({
  message: vine.string().optional(),
  errors: stravaFaults().optional(),
})
