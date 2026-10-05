import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import {
  addressGuardRuntime,
  discoveredEndpointGuard,
  isRestrictedAddress,
  resetAddressGuardRuntime,
  resolvesToRestrictedAddress,
  RestrictedEndpointError,
} from '#services/upstream/address_guard'
import { refreshOauthAccessToken, startOauthFlow } from '#services/upstream/oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

/** Name resolution without the network: only the names a test declares resolve. */
function resolveNames(names: Record<string, string[]>) {
  const lookups: string[] = []
  addressGuardRuntime.lookup = async (hostname) => {
    lookups.push(hostname)
    if (!Object.hasOwn(names, hostname)) {
      throw Object.assign(new Error(`getaddrinfo ENOTFOUND ${hostname}`), { code: 'ENOTFOUND' })
    }
    return names[hostname]
  }
  return lookups
}

function fakeSession() {
  const values = new Map<string, unknown>()
  return {
    all: () => Object.fromEntries(values),
    get: (key: string) => values.get(key),
    put: (key: string, value: unknown) => values.set(key, value),
    forget: (key: string) => values.delete(key),
  } as unknown as HttpContext['session']
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

/** Serves JSON documents by exact URL, 404 for the rest, and records every request. */
function mockDocuments(documents: Record<string, unknown>) {
  const original = globalThis.fetch
  const calls: Array<{ method: string; url: string; body: string }> = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    calls.push({ method: request.method, url: request.url, body: await request.clone().text() })
    return Object.hasOwn(documents, request.url)
      ? json(documents[request.url])
      : new Response('not found', { status: 404 })
  }
  return {
    calls,
    hosts: () => calls.map((call) => new URL(call.url).host),
    restore() {
      globalThis.fetch = original
    },
  }
}

function authorizationServer(issuer: string, overrides: Record<string, unknown> = {}) {
  return {
    issuer,
    authorization_endpoint: `${issuer}/authorize`,
    token_endpoint: `${issuer}/token`,
    registration_endpoint: `${issuer}/register`,
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code', 'refresh_token'],
    token_endpoint_auth_methods_supported: ['none'],
    code_challenge_methods_supported: ['S256'],
    ...overrides,
  }
}

const REGISTERED_CLIENT = {
  client_id: 'registered-client',
  redirect_uris: ['http://localhost:3333/mcps/oauth/callback'],
  grant_types: ['authorization_code', 'refresh_token'],
  response_types: ['code'],
  token_endpoint_auth_method: 'none',
}

test.group('Restricted addresses', (group) => {
  group.each.teardown(resetAddressGuardRuntime)

  test('recognizes loopback, private, link-local, CGNAT, unique-local and unspecified addresses', ({
    assert,
  }) => {
    const restricted = [
      '127.0.0.1',
      '127.255.255.254',
      '10.1.2.3',
      '172.16.0.1',
      '172.31.255.255',
      '192.168.1.1',
      '169.254.169.254',
      '100.64.0.1',
      '100.127.255.255',
      '0.0.0.0',
      '0.1.2.3',
      '192.0.0.8',
      '198.18.0.1',
      '224.0.0.1',
      '255.255.255.255',
      '::',
      '::1',
      'fe80::1',
      'fe80::1%en0',
      'fc00::1',
      'fd12:3456:789a::1',
      'fec0::1',
      'ff02::1',
      // IPv6 spellings of restricted IPv4 addresses.
      '::ffff:127.0.0.1',
      '::ffff:a00:1',
      '::a00:1',
      '64:ff9b::a9fe:a9fe',
      '64:ff9b:1::1',
      '2002:7f00:1::1',
      'not-an-address',
    ]
    for (const address of restricted) {
      assert.isTrue(isRestrictedAddress(address), `${address} should be restricted`)
    }

    const reachable = [
      '8.8.8.8',
      '1.1.1.1',
      '172.15.255.255',
      '172.32.0.1',
      '100.63.255.255',
      '100.128.0.1',
      '169.253.255.255',
      '192.0.2.10',
      '198.51.100.7',
      '203.0.113.10',
      '2606:4700:4700::1111',
      '2001:4860:4860::8888',
      '::ffff:8.8.8.8',
      '64:ff9b::808:808',
      '2002:808:808::1',
    ]
    for (const address of reachable) {
      assert.isFalse(isRestrictedAddress(address), `${address} should be reachable`)
    }
  })

  test('sees through every IP notation a URL accepts, without a lookup', async ({ assert }) => {
    const lookups = resolveNames({})
    const urls = [
      'http://127.0.0.1/',
      'http://2130706433/',
      'http://0x7f000001/',
      'http://0x7f.1/',
      'http://017700000001/',
      'http://127.1/',
      'http://127.0.0.1./',
      'http://0/',
      'http://0xA9FEA9FE/latest/meta-data/',
      'http://169.254.169.254/latest/meta-data/',
      'http://[::1]/',
      'http://[0:0:0:0:0:0:0:1]/',
      'http://[::]/',
      'http://[::ffff:127.0.0.1]/',
      'http://[::ffff:7f00:1]/',
      'http://[fd00::1]/',
    ]
    for (const url of urls) {
      assert.isTrue(await resolvesToRestrictedAddress(new URL(url).hostname), url)
    }
    assert.isFalse(await resolvesToRestrictedAddress(new URL('http://203.0.113.10/').hostname))
    assert.isFalse(await resolvesToRestrictedAddress(new URL('http://[2606:4700::1]/').hostname))
    assert.deepEqual(lookups, [])
  })

  test('judges a hostname by every address it resolves to', async ({ assert }) => {
    resolveNames({
      'public.example': ['203.0.113.10', '2606:4700::1'],
      'internal.example': ['10.0.0.5'],
      'mixed.example': ['203.0.113.10', '127.0.0.1'],
      'six.example': ['fd00::5'],
    })

    assert.isFalse(await resolvesToRestrictedAddress('public.example'))
    assert.isTrue(await resolvesToRestrictedAddress('internal.example'))
    assert.isTrue(await resolvesToRestrictedAddress('mixed.example'))
    assert.isTrue(await resolvesToRestrictedAddress('six.example'))
    // No address, no connection: the request fails by itself.
    assert.isFalse(await resolvesToRestrictedAddress('unresolvable.example'))
  })
})

test.group('Discovered endpoint guard', (group) => {
  group.each.teardown(resetAddressGuardRuntime)

  test('keeps a public MCP out of restricted networks', async ({ assert }) => {
    const lookups = resolveNames({
      'mcp.example': ['203.0.113.10'],
      'auth.example': ['203.0.113.11'],
      'internal.example': ['192.168.10.4'],
    })
    const assertAllowed = discoveredEndpointGuard(new URL('https://mcp.example/mcp'))

    await assertAllowed(
      'https://mcp.example/.well-known/oauth-protected-resource',
      'OAuth endpoint'
    )
    assert.deepEqual(lookups, [], 'the MCP host itself needs no check')

    await assertAllowed('https://auth.example/token', 'OAuth token endpoint')
    await assertAllowed(new URL('https://auth.example/register'), 'OAuth registration endpoint')
    assert.deepEqual(lookups, ['mcp.example', 'auth.example'], 'each name is resolved once')

    for (const url of [
      'http://169.254.169.254/latest/meta-data/',
      'http://127.0.0.1:8080/token',
      'http://[::1]:8080/token',
      'https://internal.example/token',
    ]) {
      let failure: unknown
      try {
        await assertAllowed(url, 'OAuth token endpoint')
      } catch (error) {
        failure = error
      }
      assert.instanceOf(failure, RestrictedEndpointError, url)
      assert.include(
        (failure as Error).message,
        `OAuth token endpoint host "${new URL(url).hostname}"`
      )
    }
  })

  test('leaves an MCP on a private address free to use its own network', async ({ assert }) => {
    resolveNames({ 'mcp.lan.example': ['192.168.1.5'] })

    for (const mcpUrl of [
      'http://127.0.0.1:9999/mcp',
      'http://192.168.1.20/mcp',
      'http://[::1]/mcp',
    ]) {
      const assertAllowed = discoveredEndpointGuard(new URL(mcpUrl))
      await assert.doesNotReject(() =>
        assertAllowed('http://10.0.0.5/token', 'OAuth token endpoint')
      )
    }

    const byName = discoveredEndpointGuard(new URL('http://mcp.lan.example/mcp'))
    await assert.doesNotReject(() =>
      byName('http://192.168.1.6:8080/token', 'OAuth token endpoint')
    )
  })
})

test.group('OAuth endpoints from remote documents', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    resolveNames({
      'mcp.example': ['203.0.113.10'],
      'auth.example': ['203.0.113.11'],
      'tokens.internal.example': ['192.168.10.4'],
      'mcp.lan.example': ['192.168.1.5'],
    })
  })
  group.each.teardown(resetAddressGuardRuntime)

  async function remoteMcp(httpUrl = 'https://mcp.example/mcp') {
    const admin = await createAdmin()
    return createMcp(admin.id, { authType: 'auto', httpUrl, status: 'draft', oauthRequired: true })
  }

  async function connectedMcp() {
    const mcp = await remoteMcp()
    mcp.oauthIssuer = 'https://auth.example'
    mcp.oauthAuthorizeUrl = 'https://auth.example/authorize'
    mcp.oauthTokenUrl = 'https://auth.example/token'
    mcp.oauthClientId = 'registered-client'
    mcp.oauthClientAuthMethod = 'none'
    mcp.oauthAccessToken = McpSecretStore.encrypt('access-old')
    mcp.oauthRefreshToken = McpSecretStore.encrypt('refresh-old')
    mcp.oauthTokenExpiresAt = DateTime.utc().minus({ minutes: 1 })
    await mcp.save()
    return mcp
  }

  const protectedResource = (provider: string, resource = 'https://mcp.example/mcp') => ({
    'https://mcp.example/.well-known/oauth-protected-resource/mcp': {
      resource,
      authorization_servers: [provider],
    },
  })

  test('refuses an authorization server on the metadata address before requesting it', async ({
    assert,
  }) => {
    const mcp = await remoteMcp()
    const mock = mockDocuments(protectedResource('http://169.254.169.254/latest'))

    try {
      await assert.rejects(
        () => startOauthFlow(fakeSession(), mcp),
        /OAuth endpoint host "169\.254\.169\.254" is a loopback, private or link-local address/
      )
      assert.notInclude(mock.hosts(), '169.254.169.254')
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.oauthIssuer)
    } finally {
      mock.restore()
    }
  })

  test('refuses registration, token and authorization endpoints inside the network', async ({
    assert,
  }) => {
    const cases: Array<[Record<string, unknown>, RegExp, string]> = [
      [
        { registration_endpoint: 'http://10.0.0.5:8080/register' },
        /OAuth registration endpoint host "10\.0\.0\.5"/,
        '10.0.0.5:8080',
      ],
      [
        { token_endpoint: 'https://tokens.internal.example/token' },
        /OAuth token endpoint host "tokens\.internal\.example"/,
        'tokens.internal.example',
      ],
      [
        { authorization_endpoint: 'http://[::1]:9000/authorize' },
        /OAuth authorization endpoint host "\[::1\]"/,
        '[::1]:9000',
      ],
      [
        { token_endpoint: 'http://2130706433/token' },
        /OAuth token endpoint host "127\.0\.0\.1"/,
        '127.0.0.1',
      ],
    ]

    for (const [overrides, message, host] of cases) {
      const mcp = await remoteMcp()
      const mock = mockDocuments({
        ...protectedResource('https://auth.example'),
        'https://auth.example/.well-known/oauth-authorization-server': authorizationServer(
          'https://auth.example',
          overrides
        ),
        'https://auth.example/register': REGISTERED_CLIENT,
        'http://10.0.0.5:8080/register': REGISTERED_CLIENT,
      })

      try {
        await assert.rejects(() => startOauthFlow(fakeSession(), mcp), message)
        assert.notInclude(mock.hosts(), host)
        // Nothing was registered or saved for a provider that was refused.
        assert.isFalse(mock.calls.some((call) => call.method === 'POST'))
        const saved = await Mcp.findOrFail(mcp.id)
        assert.isNull(saved.oauthClientId)
      } finally {
        mock.restore()
      }
    }
  })

  test('checks the token endpoint again on every refresh', async ({ assert }) => {
    // The provider's metadata is fetched again for each refresh.
    const moved = await connectedMcp()
    const movedMock = mockDocuments({
      'https://auth.example/.well-known/oauth-authorization-server': authorizationServer(
        'https://auth.example',
        { token_endpoint: 'http://127.0.0.1:8080/token' }
      ),
    })
    try {
      await assert.rejects(
        () => refreshOauthAccessToken(moved),
        /OAuth token endpoint host "127\.0\.0\.1"/
      )
      assert.isFalse(movedMock.calls.some((call) => call.method === 'POST'))
    } finally {
      movedMock.restore()
    }

    // Without discovery the endpoints saved on the row are used.
    const saved = await connectedMcp()
    saved.oauthTokenUrl = 'http://10.0.0.9/token'
    await saved.save()
    const savedMock = mockDocuments({})
    try {
      await assert.rejects(
        () => refreshOauthAccessToken(saved),
        /OAuth token endpoint host "10\.0\.0\.9"/
      )
      assert.isFalse(savedMock.calls.some((call) => call.method === 'POST'))
      const row = await Mcp.findOrFail(saved.id)
      assert.equal(McpSecretStore.decrypt(row.oauthRefreshToken), 'refresh-old')
    } finally {
      savedMock.restore()
    }
  })

  test('lets a self-hosted MCP use a provider on its own network', async ({ assert }) => {
    for (const [mcpUrl, provider] of [
      ['http://127.0.0.1:9999/mcp', 'http://127.0.0.1:9998'],
      ['http://mcp.lan.example/mcp', 'http://192.168.1.6:8080'],
    ]) {
      const mcp = await remoteMcp(mcpUrl)
      const mock = mockDocuments({
        [`${new URL(mcpUrl).origin}/.well-known/oauth-protected-resource/mcp`]: {
          resource: mcpUrl,
          authorization_servers: [provider],
        },
        [`${provider}/.well-known/oauth-authorization-server`]: authorizationServer(provider),
        [`${provider}/register`]: REGISTERED_CLIENT,
      })

      try {
        const redirect = new URL(await startOauthFlow(fakeSession(), mcp))
        assert.equal(redirect.origin, provider)
        assert.equal(mcp.oauthClientId, 'registered-client')
      } finally {
        mock.restore()
      }
    }
  })

  test('requires the resource indicator to be the MCP URL or a parent of it', async ({
    assert,
  }) => {
    const provider = {
      'https://auth.example/.well-known/oauth-authorization-server':
        authorizationServer('https://auth.example'),
      'https://auth.example/register': REGISTERED_CLIENT,
    }

    for (const resource of ['https://api.other.example/', 'https://mcp.example/other']) {
      const mcp = await remoteMcp()
      const mock = mockDocuments({
        ...protectedResource('https://auth.example', resource),
        ...provider,
      })
      try {
        await assert.rejects(
          () => startOauthFlow(fakeSession(), mcp),
          'OAuth protected resource does not match the MCP URL'
        )
        assert.isFalse(mock.calls.some((call) => call.method === 'POST'))
      } finally {
        mock.restore()
      }
    }

    const mcp = await remoteMcp()
    const mock = mockDocuments({
      ...protectedResource('https://auth.example', 'https://mcp.example'),
      ...provider,
    })
    try {
      const redirect = new URL(await startOauthFlow(fakeSession(), mcp))
      assert.equal(new URL(redirect.searchParams.get('resource')!).href, 'https://mcp.example/')
      assert.equal(mcp.oauthResource, 'https://mcp.example')
    } finally {
      mock.restore()
    }
  })

  test('does not refresh tokens for a saved resource that is not the MCP', async ({ assert }) => {
    const mcp = await connectedMcp()
    mcp.oauthResource = 'https://api.other.example/'
    await mcp.save()
    const mock = mockDocuments({
      'https://auth.example/.well-known/oauth-authorization-server':
        authorizationServer('https://auth.example'),
    })

    try {
      await assert.rejects(
        () => refreshOauthAccessToken(mcp),
        'OAuth protected resource does not match the MCP URL'
      )
      assert.isFalse(mock.calls.some((call) => call.method === 'POST'))
    } finally {
      mock.restore()
    }
  })
})
