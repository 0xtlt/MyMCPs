/**
 * Strava responses are shaped for apps: avatar URLs, encoded map polylines,
 * and client flags. Agents pay for every token, so drop what they cannot use.
 */
const NOISE_KEYS = new Set([
  'available_zones',
  'badge_type_id',
  'cover_photo',
  'cover_photo_small',
  'display_hide_heartrate_option',
  'embed_token',
  'external_id',
  'from_accepted_tag',
  'has_kudoed',
  'heartrate_opt_out',
  'map',
  'map_urls',
  'photos',
  'profile',
  'profile_medium',
  'resource_state',
  'stats_visibility',
  'upload_id',
  'upload_id_str',
])

/** Summary fields that matter when scanning a training log. */
const ACTIVITY_SUMMARY_KEYS = [
  'id',
  'name',
  'sport_type',
  'start_date_local',
  'timezone',
  'distance',
  'moving_time',
  'elapsed_time',
  'total_elevation_gain',
  'elev_high',
  'elev_low',
  'average_speed',
  'max_speed',
  'average_heartrate',
  'max_heartrate',
  'average_watts',
  'weighted_average_watts',
  'device_watts',
  'kilojoules',
  'average_cadence',
  'average_temp',
  'suffer_score',
  'pr_count',
  'achievement_count',
  'kudos_count',
  'comment_count',
  'athlete_count',
  'workout_type',
  'gear_id',
  'trainer',
  'commute',
  'manual',
  'private',
] as const

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** Remove noise keys and nulls at every depth. */
export function compactStravaPayload(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(compactStravaPayload)
  }
  if (!isRecord(value)) {
    return value
  }

  const compacted: Record<string, unknown> = {}
  for (const [key, entry] of Object.entries(value)) {
    if (entry === null || NOISE_KEYS.has(key) || key === 'id_str') continue
    compacted[key] = compactStravaPayload(entry)
  }
  // Route identifiers exceed 2^53, so the parsed numeric `id` is already wrong.
  if (typeof value.id_str === 'string') {
    compacted.id = value.id_str
  }
  return compacted
}

export function activitySummaries(value: unknown) {
  if (!Array.isArray(value)) return []

  return value.filter(isRecord).map((activity) => {
    const summary: Record<string, unknown> = {}
    for (const key of ACTIVITY_SUMMARY_KEYS) {
      if (activity[key] !== null && activity[key] !== undefined) summary[key] = activity[key]
    }
    return summary
  })
}

function sampleIndexes(length: number, maxPoints: number) {
  if (length <= maxPoints) {
    return Array.from({ length }, (_, index) => index)
  }
  return Array.from({ length: maxPoints }, (_, index) =>
    Math.round((index * (length - 1)) / (maxPoints - 1))
  )
}

/**
 * Reduce `key_by_type` streams to evenly spaced samples. A one-hour activity
 * recorded every second is 3,600 values per stream.
 */
export function downsampleStreams(value: unknown, maxPoints: number) {
  const streams: Record<string, unknown[]> = {}
  let originalPoints = 0
  let returnedPoints = 0

  if (isRecord(value)) {
    for (const [type, stream] of Object.entries(value)) {
      const data = isRecord(stream) && Array.isArray(stream.data) ? stream.data : null
      if (!data) continue
      const sampled = sampleIndexes(data.length, maxPoints).map((index) => data[index])
      streams[type] = sampled
      originalPoints = Math.max(originalPoints, data.length)
      returnedPoints = Math.max(returnedPoints, sampled.length)
    }
  }

  return { original_points: originalPoints, returned_points: returnedPoints, streams }
}
