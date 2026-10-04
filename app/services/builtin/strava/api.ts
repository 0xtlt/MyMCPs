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

function rateLimitMessage(response: Response) {
  const usage =
    response.headers.get('x-readratelimit-usage') ?? response.headers.get('x-ratelimit-usage')
  const limit =
    response.headers.get('x-readratelimit-limit') ?? response.headers.get('x-ratelimit-limit')
  const [shortUsage, dailyUsage] = usage?.split(',') ?? []
  const [shortLimit, dailyLimit] = limit?.split(',') ?? []
  const counters =
    shortUsage && dailyUsage && shortLimit && dailyLimit
      ? ` (${shortUsage.trim()} of ${shortLimit.trim()} requests in 15 minutes, ${dailyUsage.trim()} of ${dailyLimit.trim()} today)`
      : ''
  return `Strava rate limit reached${counters}. The 15-minute window resets on the quarter hour and the daily window at midnight UTC.`
}

async function stravaFailure(response: Response) {
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
    return new BuiltinToolError(rateLimitMessage(response))
  }
  return new BuiltinToolError(`Strava API returned HTTP ${response.status}${faultSummary(body)}`)
}

/** GET a Strava API v3 resource as the connected athlete. */
export async function stravaGet(
  accessToken: string,
  path: string,
  query: Record<string, QueryValue> = {}
): Promise<unknown> {
  const url = new URL(`${STRAVA_API_URL}${path}`)
  for (const [name, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(name, String(value))
  }

  let response: Response
  try {
    response = await fetchWithSameOriginRedirects(
      url,
      {
        headers: { Accept: 'application/json', Authorization: `Bearer ${accessToken}` },
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      },
      'Strava API'
    )
  } catch (error) {
    if (error instanceof Error && error.name === 'TimeoutError') {
      throw new BuiltinToolError('Strava did not respond in time. Try again.', { cause: error })
    }
    throw error
  }

  if (!response.ok) {
    throw await stravaFailure(response)
  }
  return response.json()
}
