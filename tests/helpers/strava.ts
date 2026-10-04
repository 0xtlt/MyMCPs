import { DateTime } from 'luxon'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcp } from '#tests/helpers/factories'

export type StravaTestRequest = {
  method: string
  url: URL
  authorization: string | null
  form: URLSearchParams | null
}

type StravaResponder = (
  request: StravaTestRequest
) => Response | undefined | Promise<Response | undefined>

export function stravaJson(body: unknown, status = 200, headers: Record<string, string> = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  })
}

/** Shapes follow real Strava API v3 responses, including the fields agents never need. */
export const stravaFixtures = {
  athlete: {
    id: 4242,
    resource_state: 3,
    firstname: 'Test',
    lastname: 'Athlete',
    city: 'Lyon',
    country: 'France',
    weight: 70.5,
    ftp: null,
    profile: 'https://images.example/large.jpg',
    profile_medium: 'https://images.example/medium.jpg',
    badge_type_id: 1,
    measurement_preference: 'meters',
    bikes: [{ id: 'b101', name: 'Road bike', distance: 1250000, resource_state: 2 }],
    shoes: [],
  },
  activity: {
    resource_state: 2,
    athlete: { id: 4242, resource_state: 1 },
    id: 15000000001,
    name: 'Morning Run',
    sport_type: 'Run',
    type: 'Run',
    start_date: '2026-09-30T05:30:00Z',
    start_date_local: '2026-09-30T07:30:00Z',
    timezone: '(GMT+01:00) Europe/Paris',
    distance: 10012.4,
    moving_time: 2890,
    elapsed_time: 2950,
    total_elevation_gain: 84.2,
    average_speed: 3.464,
    max_speed: 5.1,
    average_heartrate: 152.3,
    max_heartrate: 178,
    average_watts: null,
    kudos_count: 7,
    comment_count: 1,
    achievement_count: 2,
    pr_count: 1,
    trainer: false,
    commute: false,
    manual: false,
    private: false,
    gear_id: 'g202',
    upload_id: 16000000001,
    upload_id_str: '16000000001',
    external_id: 'garmin_ping_123',
    has_kudoed: false,
    map: { id: 'a15000000001', summary_polyline: 'encoded-polyline', resource_state: 2 },
  },
  tokens: {
    token_type: 'Bearer',
    expires_at: 1791127000,
    expires_in: 21600,
    refresh_token: 'strava-refresh-token',
    access_token: 'strava-access-token',
    athlete: { id: 4242 },
  },
}

/**
 * Replace `fetch` with a fake Strava. `respond` handles the cases a test cares
 * about and returns `undefined` to fall back to the defaults below.
 */
export function mockStrava(respond: StravaResponder = () => undefined) {
  const originalFetch = globalThis.fetch
  const requests: StravaTestRequest[] = []

  globalThis.fetch = async (input, init) => {
    const raw = new Request(input, init)
    const url = new URL(raw.url)
    if (url.origin !== 'https://www.strava.com') {
      return originalFetch(input, init)
    }

    const body = raw.method === 'GET' ? '' : await raw.text()
    const request: StravaTestRequest = {
      method: raw.method,
      url,
      authorization: raw.headers.get('Authorization'),
      form: body ? new URLSearchParams(body) : null,
    }
    requests.push(request)

    const custom = await respond(request)
    if (custom) return custom

    if (url.pathname === '/api/v3/oauth/token') {
      return stravaJson(stravaFixtures.tokens)
    }
    if (url.pathname === '/api/v3/athlete') {
      return stravaJson(stravaFixtures.athlete)
    }
    if (url.pathname === '/api/v3/athlete/activities') {
      return stravaJson([stravaFixtures.activity])
    }
    return stravaJson(
      {
        message: 'Record Not Found',
        errors: [{ resource: 'Resource', field: '', code: 'not found' }],
      },
      404
    )
  }

  return {
    requests,
    apiRequests: () => requests.filter((request) => request.url.pathname !== '/api/v3/oauth/token'),
    tokenRequests: () =>
      requests.filter((request) => request.url.pathname === '/api/v3/oauth/token'),
    restore: () => {
      globalThis.fetch = originalFetch
    },
  }
}

/** A built-in Strava MCP as the setup form leaves it, optionally already authorized. */
export async function createStravaMcp(
  createdBy: number,
  options: {
    name?: string
    connected?: boolean
    scopes?: string | null
    expiresAt?: DateTime | null
  } = {}
) {
  const connected = options.connected ?? true
  const mcp = await createMcp(createdBy, {
    name: options.name ?? 'Strava',
    transport: 'builtin',
    builtinKey: 'strava',
    status: connected ? 'ready' : 'draft',
    oauthRequired: !connected,
  })
  mcp.oauthClientId = '123456'
  mcp.oauthClientSecret = McpSecretStore.encrypt('strava-client-secret')
  if (connected) {
    mcp.oauthAccessToken = McpSecretStore.encrypt('strava-access-token')
    mcp.oauthRefreshToken = McpSecretStore.encrypt('strava-refresh-token')
    mcp.oauthTokenType = 'Bearer'
    mcp.oauthTokenExpiresAt =
      options.expiresAt === undefined ? DateTime.utc().plus({ hours: 5 }) : options.expiresAt
    mcp.oauthScopes =
      options.scopes === undefined
        ? 'read read_all profile:read_all activity:read_all'
        : options.scopes
  }
  await mcp.save()
  return Mcp.findOrFail(mcp.id)
}
