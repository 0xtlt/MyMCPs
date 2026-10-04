import { BuiltinAuthorizationError, BuiltinToolError } from '#services/builtin/definition'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'

const STRAVA_API_URL = 'https://www.strava.com/api/v3'
const REQUEST_TIMEOUT_MS = 30_000

type QueryValue = string | number | boolean | undefined

type StravaFault = { resource?: unknown; field?: unknown; code?: unknown }

function faults(body: unknown): StravaFault[] {
  const errors = (body as { errors?: unknown } | null)?.errors
  return Array.isArray(errors) ? (errors as StravaFault[]) : []
}

/** Strava's own explanation, such as `Record Not Found (Activity not found)`. */
function faultSummary(body: unknown) {
  const message = (body as { message?: unknown } | null)?.message
  const details = faults(body)
    .map((fault) =>
      [fault.resource, fault.field, fault.code]
        .filter((part) => typeof part === 'string' && part)
        .join(' ')
    )
    .filter(Boolean)
    .join('; ')
  const summary = [typeof message === 'string' ? message : null, details ? `(${details})` : null]
    .filter(Boolean)
    .join(' ')
  return summary ? `: ${summary.slice(0, 200)}` : ''
}

/**
 * A missing scope is reported as HTTP 401 with a fault such as
 * `{"resource":"AccessToken","field":"activity:read_permission","code":"missing"}`.
 */
function missingPermission(body: unknown) {
  const fault = faults(body).find(
    (candidate) =>
      candidate.code === 'missing' &&
      typeof candidate.field === 'string' &&
      candidate.field.endsWith('_permission')
  )
  return fault ? String(fault.field).slice(0, -'_permission'.length) : null
}

/** Reads have their own, lower limit. Writes only count against the overall one. */
function rateLimitMessage(response: Response, isRead: boolean) {
  const header = (name: string) =>
    (isRead ? response.headers.get(`x-readratelimit-${name}`) : null) ??
    response.headers.get(`x-ratelimit-${name}`)
  const usage = header('usage')
  const limit = header('limit')
  const [shortUsage, dailyUsage] = usage?.split(',') ?? []
  const [shortLimit, dailyLimit] = limit?.split(',') ?? []
  const counters =
    shortUsage && dailyUsage && shortLimit && dailyLimit
      ? ` (${shortUsage.trim()} of ${shortLimit.trim()} requests in 15 minutes, ${dailyUsage.trim()} of ${dailyLimit.trim()} today)`
      : ''
  return `Strava rate limit reached${counters}. The 15-minute window resets on the quarter hour and the daily window at midnight UTC.`
}

async function stravaFailure(response: Response, isRead: boolean) {
  const body: unknown = await response.json().catch(() => null)

  if (response.status === 401) {
    const permission = missingPermission(body)
    return permission
      ? new BuiltinToolError(
          `Strava permission "${permission}" was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked.`
        )
      : new BuiltinAuthorizationError(
          'Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs.'
        )
  }
  if (response.status === 402) {
    return new BuiltinToolError('Strava only returns this data to athletes with a subscription.')
  }
  if (response.status === 403) {
    return new BuiltinToolError(`Strava denied access to this resource${faultSummary(body)}`)
  }
  if (response.status === 404) {
    return new BuiltinToolError(`Strava could not find this resource${faultSummary(body)}`)
  }
  if (response.status === 429) {
    return new BuiltinToolError(rateLimitMessage(response, isRead))
  }
  return new BuiltinToolError(`Strava API returned HTTP ${response.status}${faultSummary(body)}`)
}

type StravaRequest = {
  method?: 'GET' | 'POST' | 'PUT'
  query?: Record<string, QueryValue>
  /** Sent as `application/x-www-form-urlencoded`. */
  form?: Record<string, QueryValue>
  /** Sent as JSON. */
  json?: Record<string, unknown>
}

function withoutUndefined(values: Record<string, QueryValue>) {
  const params = new URLSearchParams()
  for (const [name, value] of Object.entries(values)) {
    if (value !== undefined) params.set(name, String(value))
  }
  return params
}

/** Call the Strava API v3 as the connected athlete. */
export async function stravaRequest(
  accessToken: string,
  path: string,
  { method = 'GET', query = {}, form, json }: StravaRequest = {}
): Promise<unknown> {
  const url = new URL(`${STRAVA_API_URL}${path}`)
  url.search = withoutUndefined(query).toString()

  const headers: Record<string, string> = {
    Accept: 'application/json',
    Authorization: `Bearer ${accessToken}`,
  }
  let body: string | undefined
  if (form) {
    headers['Content-Type'] = 'application/x-www-form-urlencoded'
    body = withoutUndefined(form).toString()
  } else if (json) {
    headers['Content-Type'] = 'application/json'
    body = JSON.stringify(json)
  }

  let response: Response
  try {
    response = await fetchWithSameOriginRedirects(
      url,
      { method, headers, body, signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) },
      'Strava API'
    )
  } catch (error) {
    if (error instanceof Error && error.name === 'TimeoutError') {
      // A write may have been applied even though its response never arrived.
      throw new BuiltinToolError(
        method === 'GET'
          ? 'Strava did not respond in time. Try again.'
          : 'Strava did not respond in time. The change may still have been applied, so check before retrying.',
        { cause: error }
      )
    }
    throw error
  }

  if (!response.ok) {
    throw await stravaFailure(response, method === 'GET')
  }
  return response.json()
}

export function stravaGet(
  accessToken: string,
  path: string,
  query: Record<string, QueryValue> = {}
) {
  return stravaRequest(accessToken, path, { query })
}
