import type { Infer } from '@vinejs/vine/types'
import {
  BuiltinAuthorizationError,
  BuiltinToolError,
  type BuiltinToolContext,
} from '#services/builtin/definition'
import { customerLabel, customerNumber } from '#services/builtin/google_ads/format'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'
import {
  googleAdsFailureValidator,
  googleAdsMutationValidator,
  googleAdsRowsValidator,
} from '#validators/builtin_google_ads'

/**
 * Google retires a version about a year after its release, and announces it
 * months ahead: https://developers.google.com/google-ads/api/docs/sunset-dates
 * A retired version answers every request with an HTML 404. Every field and
 * enum of a version is in its discovery document, which is what to check the
 * tools against when moving to another one:
 * https://googleads.googleapis.com/$discovery/rest?version=v25
 */
export const GOOGLE_ADS_API_VERSION = 'v25'
const GOOGLE_ADS_API_URL = `https://googleads.googleapis.com/${GOOGLE_ADS_API_VERSION}`

/** A report over a large account takes Google a while. */
const REQUEST_TIMEOUT_MS = 60_000
const MAX_REPORTED_FAULTS = 5
const MAX_FAILURE_CHARS = 900

type GoogleAdsFailure = Infer<typeof googleAdsFailureValidator>
type Fault = NonNullable<
  NonNullable<GoogleAdsFailure['error']['details']>[number]['errors']
>[number]

/** What to do about the errors that the setup, not the call, has to fix. */
const SETUP_HINTS: Record<string, string> = {
  CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION:
    'The Google Cloud project of the OAuth client only has test access, which reaches test accounts only. Apply for Explorer access on the Google Ads API Overview page of that project.',
  PROJECT_DISABLED:
    'The Google Ads API is not enabled in the Google Cloud project of the OAuth client. Enable it in the Google Cloud console.',
  USER_PERMISSION_DENIED:
    'The connected Google sign-in cannot open this account directly. If it is managed through a manager account, an administrator must set that manager as Manager account ID from the MCPs page in MyMCPs.',
  CUSTOMER_NOT_ENABLED:
    'This Google Ads account is not active: it was closed, suspended, or never finished its setup.',
  NOT_ADS_USER: 'The connected Google sign-in has no Google Ads account.',
  TWO_STEP_VERIFICATION_NOT_ENROLLED:
    'Google requires 2-Step Verification on the connected Google account before it can use the Google Ads API.',
  INVALID_LOGIN_CUSTOMER_ID:
    'The Manager account ID saved for this MCP is not one the connected Google sign-in can use. An administrator must correct it from the MCPs page in MyMCPs.',
  CANNOT_BE_EXECUTED_BY_MANAGER_ACCOUNT:
    'This is a manager account, which holds no campaigns. Use the ID of one of its client accounts, as list_accounts returns them.',
  EU_POLITICAL_ADVERTISING_DECLARATION_REQUIRED:
    'A campaign of this account has not declared whether it carries EU political advertising, and Google blocks every change until it has. Declare it in Google Ads.',
  RESOURCE_EXHAUSTED:
    'The Google Cloud project has used up its Google Ads API operations for today. Try again later, or apply for a higher access level.',
  RESOURCE_TEMPORARILY_EXHAUSTED:
    'Google Ads is rate limiting these requests. Try again in a minute.',
}

/** The errors that say the sign-in itself is no longer good, whatever the HTTP status. */
const REJECTED_SIGN_IN = new Set([
  'OAUTH_TOKEN_EXPIRED',
  'OAUTH_TOKEN_INVALID',
  'OAUTH_TOKEN_REVOKED',
  'OAUTH_TOKEN_DISABLED',
  'GOOGLE_ACCOUNT_COOKIE_INVALID',
])

/** `campaign.name`, `operations[0].create.keyword.text`: where in the request the mistake is. */
function faultLocation(fault: Fault) {
  return (fault.location?.fieldPathElements ?? [])
    .map(({ fieldName, index }) => `${fieldName ?? ''}${index === undefined ? '' : `[${index}]`}`)
    .filter(Boolean)
    .join('.')
}

function faultCode(fault: Fault) {
  return Object.values(fault.errorCode ?? {})[0]
}

function describeFault(fault: Fault) {
  const code = faultCode(fault)
  const location = faultLocation(fault)
  return [
    fault.message ?? code ?? 'Unknown error',
    code && fault.message ? `(${code})` : null,
    location ? `at ${location}` : null,
  ]
    .filter(Boolean)
    .join(' ')
}

async function failureOf(response: Response) {
  const body: unknown = await response.json().catch(() => null)
  const [, failure] = await googleAdsFailureValidator.tryValidate(body)
  return failure?.error ?? {}
}

async function googleAdsFailure(response: Response) {
  const failure = await failureOf(response)
  const faults = (failure.details ?? []).flatMap((detail) => detail.errors ?? [])
  const codes = faults.map(faultCode)

  // A token Google no longer knows gets a bare 401, without any of its own errors.
  if (
    codes.some((code) => code && REJECTED_SIGN_IN.has(code)) ||
    (response.status === 401 && faults.length === 0)
  ) {
    return new BuiltinAuthorizationError(
      'Google rejected the saved authorization. Re-authorize this MCP in MyMCPs.'
    )
  }

  const reasons =
    faults.length > 0
      ? faults.slice(0, MAX_REPORTED_FAULTS).map(describeFault).join('; ')
      : (failure.message ?? `HTTP ${response.status}`)
  const more =
    faults.length > MAX_REPORTED_FAULTS ? ` (and ${faults.length - MAX_REPORTED_FAULTS} more)` : ''
  const hint = codes.map((code) => (code ? SETUP_HINTS[code] : undefined)).find(Boolean)

  return new BuiltinToolError(
    `Google Ads refused the request: ${`${reasons}${more}`.slice(0, MAX_FAILURE_CHARS)}${hint ? ` ${hint}` : ''}`
  )
}

type GoogleAdsRequest = {
  method?: 'GET' | 'POST'
  /** Sent as JSON. */
  body?: Record<string, unknown>
  /** Whether a timeout may have left a change behind. */
  changes?: boolean
}

/** Call the Google Ads API as the connected Google sign-in. */
export async function googleAdsRequest(
  { accessToken, settings }: BuiltinToolContext,
  path: string,
  { method = 'POST', body, changes = false }: GoogleAdsRequest = {}
): Promise<unknown> {
  // No developer token: Google retired them in September 2026, and the access
  // level now belongs to the Google Cloud project of the OAuth client.
  const headers: Record<string, string> = {
    Accept: 'application/json',
    Authorization: `Bearer ${accessToken}`,
  }
  // Names the manager account the sign-in acts through to reach its client accounts.
  if (settings.loginCustomerId) headers['login-customer-id'] = settings.loginCustomerId
  if (body) headers['Content-Type'] = 'application/json'

  let response: Response
  try {
    response = await fetchWithSameOriginRedirects(
      `${GOOGLE_ADS_API_URL}${path}`,
      {
        method,
        headers,
        body: body ? JSON.stringify(body) : undefined,
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      },
      'Google Ads API'
    )
  } catch (error) {
    if (error instanceof Error && error.name === 'TimeoutError') {
      throw new BuiltinToolError(
        changes
          ? 'Google Ads did not respond in time. The change may still have been applied, so check before retrying.'
          : 'Google Ads did not respond in time. Try again, with a shorter period or a lower limit.',
        { cause: error }
      )
    }
    throw error
  }

  if (!response.ok) {
    throw await googleAdsFailure(response)
  }
  return response.json()
}

/**
 * The accounts the MCP may act on: every one the sign-in reaches, unless the
 * administrator listed some. `null` when all are allowed.
 */
export function allowedCustomers({ settings }: BuiltinToolContext) {
  return settings.customerIds ? settings.customerIds.split(' ') : null
}

/** The account a tool was asked to act on, once it is known to be one the MCP may use. */
export function customerOf(context: BuiltinToolContext, customerId: string) {
  const customer = customerNumber(customerId)
  const allowed = allowedCustomers(context)
  if (allowed && !allowed.includes(customer)) {
    throw new BuiltinToolError(
      `This MCP may not use the Google Ads account ${customerLabel(customer)}. It is limited to: ${allowed.map(customerLabel).join(', ')}. An administrator can change that from the MCPs page in MyMCPs.`
    )
  }
  return customer
}

/** A row of a report: one object for each resource the query selects from. */
export type GoogleAdsRow = Record<string, Record<string, any> | undefined>

/**
 * Run a Google Ads Query Language query and return at most `limit` rows.
 * `truncated` says whether Google had more, which a query that limits itself
 * only shows when it asks for one row more than `limit`.
 */
export async function searchGoogleAds(
  context: BuiltinToolContext,
  customer: string,
  query: string,
  limit: number
) {
  const rows: GoogleAdsRow[] = []
  let pageToken: string | undefined
  let hasMore = false

  do {
    const [unexpected, page] = await googleAdsRowsValidator.tryValidate(
      await googleAdsRequest(context, `/customers/${customer}/googleAds:search`, {
        body: { query, ...(pageToken ? { pageToken } : {}) },
      })
    )
    if (unexpected) {
      throw new BuiltinToolError('Google Ads did not return the rows of a report')
    }

    rows.push(...((page.results ?? []) as GoogleAdsRow[]))
    pageToken = page.nextPageToken || undefined
    hasMore = Boolean(pageToken) || rows.length > limit
  } while (pageToken && rows.length < limit)

  return { rows: rows.slice(0, limit), truncated: hasMore }
}

/** One change to one resource, such as `{ campaignOperation: { update, updateMask } }`. */
export type GoogleAdsOperation = Record<string, Record<string, unknown>>

/**
 * Apply changes to an account together: either all of them are made, or none.
 * With `validateOnly`, Google checks them and makes none. Returns the
 * resource name each change produced, in the order of the operations.
 */
export async function mutateGoogleAds(
  context: BuiltinToolContext,
  customer: string,
  operations: GoogleAdsOperation[],
  { validateOnly = false }: { validateOnly?: boolean } = {}
) {
  const [unexpected, result] = await googleAdsMutationValidator.tryValidate(
    await googleAdsRequest(context, `/customers/${customer}/googleAds:mutate`, {
      body: { mutateOperations: operations, ...(validateOnly ? { validateOnly: true } : {}) },
      changes: !validateOnly,
    })
  )
  if (unexpected) {
    throw new BuiltinToolError(
      'Google Ads did not confirm the change. It may still have been applied, so check before retrying.'
    )
  }

  return (result.mutateOperationResponses ?? []).map(
    (response) => Object.values(response)[0]?.resourceName ?? null
  )
}

/** The last number of a resource name: `456` in `customers/123/campaigns/456`, `789` in `…/adGroupAds/456~789`. */
export function resourceId(resourceName: string | null | undefined) {
  return resourceName?.match(/(\d+)$/)?.[1] ?? null
}
