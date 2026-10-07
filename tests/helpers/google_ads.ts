import { DateTime } from 'luxon'
import Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcp } from '#tests/helpers/factories'

export type GoogleAdsTestRequest = {
  method: string
  url: URL
  headers: Headers
  form: URLSearchParams | null
  json: any
}

type GoogleAdsResponder = (
  request: GoogleAdsTestRequest
) => Response | undefined | Promise<Response | undefined>

export const GOOGLE_ADS_SCOPE = 'https://www.googleapis.com/auth/adwords'
export const GOOGLE_ADS_CUSTOMER = '1234567890'

export function googleJson(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

/** A refusal as the Google Ads API words it: one entry per mistake, each with its kind and code. */
export function googleAdsFailure(
  errors: Array<{ errorCode: Record<string, string>; message: string; fields?: string[] }>,
  status = 400
) {
  return googleJson(
    {
      error: {
        code: status,
        message: 'Request contains an invalid argument.',
        status: 'INVALID_ARGUMENT',
        details: [
          {
            '@type': 'type.googleapis.com/google.ads.googleads.v25.errors.GoogleAdsFailure',
            'errors': errors.map(({ errorCode, message, fields }) => ({
              errorCode,
              message,
              ...(fields
                ? { location: { fieldPathElements: fields.map((fieldName) => ({ fieldName })) } }
                : {}),
            })),
            'requestId': 'test-request-id',
          },
        ],
      },
    },
    status
  )
}

const account = {
  resourceName: `customers/${GOOGLE_ADS_CUSTOMER}`,
  id: GOOGLE_ADS_CUSTOMER,
  descriptiveName: 'Acme Shoes',
  currencyCode: 'EUR',
  timeZone: 'Europe/Paris',
  manager: false,
  testAccount: false,
  status: 'ENABLED',
}

/** Rows shaped like real Google Ads API v25 answers: camelCase keys, 64-bit numbers as text. */
export const googleAdsFixtures = {
  account,
  campaign: {
    campaign: {
      resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/campaigns/111`,
      id: '111',
      name: 'Spring sale',
      status: 'ENABLED',
      primaryStatus: 'ELIGIBLE',
      advertisingChannelType: 'SEARCH',
      biddingStrategyType: 'TARGET_SPEND',
      startDateTime: '2026-03-01 00:00:00',
      networkSettings: {
        targetGoogleSearch: true,
        targetSearchNetwork: false,
        targetContentNetwork: false,
      },
      campaignBudget: `customers/${GOOGLE_ADS_CUSTOMER}/campaignBudgets/222`,
    },
    campaignBudget: {
      resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/campaignBudgets/222`,
      id: '222',
      amountMicros: '2500000',
      referenceCount: '1',
    },
    customer: account,
    metrics: {
      impressions: '12000',
      clicks: '300',
      costMicros: '150000000',
      ctr: 0.025,
      conversions: 12,
      conversionsValue: 960,
    },
  },
  adGroup: {
    adGroup: {
      resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/adGroups/333`,
      id: '333',
      name: 'Running shoes',
      status: 'ENABLED',
      type: 'SEARCH_STANDARD',
      cpcBidMicros: '1200000',
    },
    campaign: {
      resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/campaigns/111`,
      id: '111',
      name: 'Spring sale',
      status: 'ENABLED',
      advertisingChannelType: 'SEARCH',
    },
    customer: account,
  },
  tokens: {
    access_token: 'google-access-token',
    expires_in: 3599,
    refresh_token: 'google-refresh-token',
    scope: GOOGLE_ADS_SCOPE,
    token_type: 'Bearer',
  },
}

/** What a mutate answers for one operation: the resource it made or changed. */
function mutateResult(operation: Record<string, any>, index: number) {
  const [[kind, change]] = Object.entries(operation)
  const collection = `${kind.replace(/Operation$/, '')}s`.replace(/ys$/, 'ies')
  const named = change.remove ?? change.update?.resourceName
  return {
    [kind.replace(/Operation$/, 'Result')]: {
      resourceName: named ?? `customers/${GOOGLE_ADS_CUSTOMER}/${collection}/${9000 + index}`,
    },
  }
}

/**
 * Replace `fetch` with a fake Google: its token endpoint and the Google Ads
 * API. `respond` handles the cases a test cares about and returns
 * `undefined` to fall back to the defaults below.
 */
export function mockGoogleAds(respond: GoogleAdsResponder = () => undefined) {
  const originalFetch = globalThis.fetch
  const requests: GoogleAdsTestRequest[] = []

  globalThis.fetch = async (input, init) => {
    const raw = new Request(input, init)
    const url = new URL(raw.url)
    if (
      url.origin !== 'https://googleads.googleapis.com' &&
      url.origin !== 'https://oauth2.googleapis.com'
    ) {
      return originalFetch(input, init)
    }

    const body = raw.method === 'GET' ? '' : await raw.text()
    const isJson = raw.headers.get('Content-Type')?.includes('application/json') ?? false
    const request: GoogleAdsTestRequest = {
      method: raw.method,
      url,
      headers: raw.headers,
      form: body && !isJson ? new URLSearchParams(body) : null,
      json: body && isJson ? JSON.parse(body) : null,
    }
    requests.push(request)

    const custom = await respond(request)
    if (custom) return custom

    if (url.origin === 'https://oauth2.googleapis.com') {
      return googleJson(googleAdsFixtures.tokens)
    }
    if (url.pathname === '/v25/customers:listAccessibleCustomers') {
      return googleJson({ resourceNames: [`customers/${GOOGLE_ADS_CUSTOMER}`] })
    }
    if (url.pathname.endsWith('/googleAds:mutate')) {
      return googleJson(
        request.json.validateOnly
          ? {}
          : { mutateOperationResponses: request.json.mutateOperations.map(mutateResult) }
      )
    }
    if (url.pathname.endsWith('/googleAds:search')) {
      const query: string = request.json.query
      if (/ FROM customer /.test(query)) return googleJson({ results: [{ customer: account }] })
      if (/ FROM campaign /.test(query)) {
        return googleJson({ results: [googleAdsFixtures.campaign] })
      }
      if (/ FROM ad_group /.test(query)) return googleJson({ results: [googleAdsFixtures.adGroup] })
      return googleJson({ results: [] })
    }
    return googleJson({ error: { code: 404, message: 'Not found', status: 'NOT_FOUND' } }, 404)
  }

  const to = (suffix: string) => requests.filter(({ url }) => url.pathname.endsWith(suffix))
  return {
    requests,
    tokenRequests: () =>
      requests.filter(({ url }) => url.origin === 'https://oauth2.googleapis.com'),
    /** The GAQL queries sent, in order. */
    queries: () => to('/googleAds:search').map(({ json }) => json.query as string),
    /** The changes sent to be made, without the ones only sent to be checked. */
    mutations: () =>
      to('/googleAds:mutate')
        .filter(({ json }) => !json.validateOnly)
        .map(({ json }) => json.mutateOperations),
    /** The changes sent to be checked only. */
    validations: () =>
      to('/googleAds:mutate')
        .filter(({ json }) => json.validateOnly)
        .map(({ json }) => json.mutateOperations),
    restore: () => {
      globalThis.fetch = originalFetch
    },
  }
}

/** A built-in Google Ads MCP as the setup form leaves it, optionally already authorized. */
export async function createGoogleAdsMcp(
  createdBy: number,
  options: {
    connected?: boolean
    writeEnabled?: boolean
    /** The manager account the sign-in acts through. */
    loginCustomerId?: string
    /** The accounts agents may use, as digits. */
    customerIds?: string[]
  } = {}
) {
  const connected = options.connected ?? true
  const mcp = await createMcp(createdBy, {
    name: 'Google Ads',
    transport: 'builtin',
    builtinKey: 'google-ads',
    status: connected ? 'ready' : 'draft',
    oauthRequired: !connected,
  })
  mcp.builtinWriteEnabled = options.writeEnabled ?? false
  mcp.oauthClientId = '1234567890-abc.apps.googleusercontent.com'
  mcp.oauthClientSecret = McpSecretStore.encrypt('google-client-secret')
  mcp.builtinSettings = McpEnvironmentStore.merge(null, [
    ...(options.loginCustomerId
      ? [{ name: 'loginCustomerId', value: options.loginCustomerId }]
      : []),
    ...(options.customerIds ? [{ name: 'customerIds', value: options.customerIds.join(' ') }] : []),
  ])
  if (connected) {
    mcp.oauthAccessToken = McpSecretStore.encrypt('google-access-token')
    mcp.oauthRefreshToken = McpSecretStore.encrypt('google-refresh-token')
    mcp.oauthTokenType = 'Bearer'
    mcp.oauthTokenExpiresAt = DateTime.utc().plus({ minutes: 50 })
    mcp.oauthScopes = GOOGLE_ADS_SCOPE
  }
  await mcp.save()
  return Mcp.findOrFail(mcp.id)
}
