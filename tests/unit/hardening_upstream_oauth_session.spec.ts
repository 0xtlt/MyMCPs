import type { HttpContext } from '@adonisjs/core/http'
import { test } from '@japa/runner'
import type Mcp from '#models/mcp'
import { MAX_PENDING_OAUTH_START_BYTES, startOauthSession } from '#services/upstream/oauth'

function fakeSession() {
  const values = new Map<string, unknown>()
  const session = {
    all: () => Object.fromEntries(values),
    get: (key: string) => values.get(key),
    put: (key: string, value: unknown) => values.set(key, value),
    forget: (key: string) => values.delete(key),
  } as unknown as HttpContext['session']
  return { session, values }
}

function start(session: HttpContext['session'], state: string, clientId = 'client') {
  return startOauthSession(session, { id: 1 } as Mcp, {
    redirectUri: 'http://localhost:3333/mcps/oauth/callback',
    authorizationServerUrl: 'https://auth.example.com',
    resource: 'https://mcp.example.com/mcp',
    clientId,
    codeVerifier: 'v'.repeat(43),
    state,
  })
}

function pendingBytes(values: Map<string, unknown>) {
  return [...values]
    .filter(([key]) => key.startsWith('mcp_oauth:'))
    .reduce(
      (total, [key, value]) =>
        total + Buffer.byteLength(key) + Buffer.byteLength(JSON.stringify(value)),
      0
    )
}

test.group('hardening: pending OAuth starts in the session', () => {
  test('older starts make room so the pending ones fit in a cookie-store session', ({ assert }) => {
    const { session, values } = fakeSession()
    values.set('auth_web', 42)

    // Long client identifiers: five of these would not fit in the session cookie.
    for (let index = 0; index < 5; index++) {
      start(session, `state-${index}`, 'c'.repeat(400))
    }

    const pending = [...values.keys()].filter((key) => key.startsWith('mcp_oauth:'))
    assert.isBelow(pending.length, 5)
    assert.isAtMost(pendingBytes(values), MAX_PENDING_OAUTH_START_BYTES)
    assert.equal(pending.at(-1), 'mcp_oauth:state-4')
    assert.equal(values.get('auth_web'), 42)
  })

  test('the newest start is kept even when it alone is over the size', ({ assert }) => {
    const { session, values } = fakeSession()
    start(session, 'older')
    start(session, 'newest', 'c'.repeat(MAX_PENDING_OAUTH_START_BYTES))

    assert.deepEqual(
      [...values.keys()].filter((key) => key.startsWith('mcp_oauth:')),
      ['mcp_oauth:newest']
    )
  })

  test('short starts are still limited by their number', ({ assert }) => {
    const { session, values } = fakeSession()
    for (let index = 0; index < 8; index++) {
      start(session, `s${index}`, 'c')
    }

    const pending = [...values.keys()].filter((key) => key.startsWith('mcp_oauth:'))
    assert.isAtMost(pending.length, 5)
    assert.equal(pending.at(-1), 'mcp_oauth:s7')
  })
})
