import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import type { ApiClient, ApiResponse } from '@japa/api-client'
import type { Assert } from '@japa/assert'
import limiter from '@adonisjs/limiter/services/main'
import OauthAuthorizationCode from '#models/oauth_authorization_code'
import OauthClient from '#models/oauth_client'
import type User from '#models/user'
import AccessTokenService from '#services/access_token_service'
import { createAuthorizationCode } from '#services/gateway_oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

const resource = 'http://localhost:3333/mcp'
const loopbackRedirectUri = 'http://127.0.0.1:49152/callback'
const remoteRedirectUri = 'https://client.example/callback'
const codeVerifier = 'vine-oauth-code-verifier-for-mymcps-gateway-tests-123'
const codeChallenge = createHash('sha256').update(codeVerifier).digest('base64url')

const registration = {
  client_name: 'Vine client',
  redirect_uris: ['http://127.0.0.1/callback'],
  token_endpoint_auth_method: 'none',
  grant_types: ['authorization_code', 'refresh_token'],
  response_types: ['code'],
  scope: 'mcp:tools',
}

async function register(client: ApiClient, overrides: Record<string, unknown> = {}) {
  const response = await client.post('/register').json({ ...registration, ...overrides })
  response.assertStatus(201)
  return OauthClient.findByOrFail('client_id', response.body().client_id)
}

function authorization(clientId: string, redirectUri = loopbackRedirectUri) {
  return {
    client_id: clientId,
    redirect_uri: redirectUri,
    response_type: 'code',
    code_challenge: codeChallenge,
    code_challenge_method: 'S256',
    scope: 'mcp:tools',
    resource,
    state: 'state-from-client',
  }
}

type Changes = Record<string, string | string[] | null>

/**
 * A query string in which a parameter can be left out (null), sent empty or
 * sent several times (an array), which a plain object cannot express.
 */
function queryString(base: Record<string, string>, changes: Changes = {}) {
  const pairs = Object.entries(base).filter(([key]) => !(key in changes))
  for (const [key, value] of Object.entries(changes)) {
    if (Array.isArray(value)) pairs.push(...value.map((item): [string, string] => [key, item]))
    else if (value !== null) pairs.push([key, value])
  }
  return pairs.map(([key, value]) => `${key}=${encodeURIComponent(value)}`).join('&')
}

function assertOauthError(
  assert: Assert,
  response: ApiResponse,
  status: number,
  error: string,
  description: string
) {
  response.assertStatus(status)
  assert.deepEqual(response.body(), { error, error_description: description })
  assert.equal(response.header('cache-control'), 'no-store')
}

/** The error a loopback client is sent back with, as [error, description, state]. */
function redirectedError(assert: Assert, response: ApiResponse) {
  response.assertStatus(302)
  const callback = new URL(response.header('location')!)
  assert.equal(callback.origin + callback.pathname, loopbackRedirectUri)
  return [
    callback.searchParams.get('error'),
    callback.searchParams.get('error_description'),
    callback.searchParams.get('state'),
  ]
}

test.group('vine: gateway OAuth client registration', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('answers with the first rule a registration breaks', async ({ client, assert }) => {
    await createAdmin()
    const wrong = {
      redirect_uris: ['http://example.com/callback'],
      token_endpoint_auth_method: 'private_key_jwt',
      grant_types: ['implicit'],
      response_types: ['token'],
      scope: 'other',
      client_name: 'x'.repeat(121),
    }
    const expected: Array<[keyof typeof wrong, string, string]> = [
      [
        'redirect_uris',
        'invalid_redirect_uri',
        'Redirect URIs must use HTTPS, HTTP on an exact loopback host, or an approved native-app callback',
      ],
      [
        'token_endpoint_auth_method',
        'invalid_client_metadata',
        'Unsupported token endpoint authentication method',
      ],
      ['grant_types', 'invalid_client_metadata', 'Unsupported OAuth grant type'],
      ['response_types', 'invalid_client_metadata', 'Only the code response type is supported'],
      ['scope', 'invalid_client_metadata', 'Unsupported OAuth scope'],
      ['client_name', 'invalid_client_metadata', 'Client name is too long'],
    ]

    // Everything from one field onwards is wrong: that field decides.
    for (const [index, [, error, description]] of expected.entries()) {
      const metadata = {
        ...registration,
        ...Object.fromEntries(expected.slice(index).map(([field]) => [field, wrong[field]])),
      }
      const response = await client.post('/register').json(metadata)
      assertOauthError(assert, response, 400, error, description)
    }
    assert.isNull(await OauthClient.first())
  })

  test('leaves the shape of the metadata to the MCP SDK schema', async ({ client, assert }) => {
    await createAdmin()

    for (const overrides of [
      { redirect_uris: 'https://client.example/callback' },
      { redirect_uris: ['not a url'] },
      { redirect_uris: [''] },
      { token_endpoint_auth_method: ['none'] },
      { grant_types: 'authorization_code' },
      { response_types: [1] },
      { scope: ['mcp:tools'] },
      { client_name: 5 },
    ]) {
      const response = await client.post('/register').json({ ...registration, ...overrides })
      assertOauthError(
        assert,
        response,
        400,
        'invalid_client_metadata',
        'Invalid OAuth client metadata'
      )
    }
  })

  test('accepts between one and ten redirect URIs of at most 2048 characters', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const uri = (length: number) =>
      `https://client.example/${'a'.repeat(length - 'https://client.example/'.length)}`
    const uris = (count: number) =>
      Array.from({ length: count }, (_, index) => `https://client.example/${index}`)

    for (const [redirectUris, status] of [
      [[], 400],
      [uris(10), 201],
      [uris(11), 400],
      [[uri(2048)], 201],
      [[uri(2049)], 400],
      [['https://client.example/callback', 'http://example.com/callback'], 400],
    ] as Array<[string[], number]>) {
      const response = await client
        .post('/register')
        .json({ ...registration, redirect_uris: redirectUris })
      response.assertStatus(status)
      if (status === 201) {
        assert.deepEqual(response.body().redirect_uris, redirectUris)
      } else {
        assert.equal(response.body().error, 'invalid_redirect_uri')
      }
    }
  })

  test('fills in the defaults and echoes the metadata it was sent', async ({ client, assert }) => {
    await createAdmin()

    const response = await client.post('/register').json({
      redirect_uris: [remoteRedirectUri],
      client_uri: 'https://client.example',
      contacts: ['ops@client.example'],
      software_id: 'vine-client',
      software_version: '1.2.3',
      not_in_the_registry: 'dropped',
    })

    response.assertStatus(201)
    const body = response.body()
    assert.deepEqual(
      { ...body, client_id: null, client_secret: null, client_id_issued_at: null },
      {
        redirect_uris: [remoteRedirectUri],
        client_uri: 'https://client.example',
        contacts: ['ops@client.example'],
        software_id: 'vine-client',
        software_version: '1.2.3',
        client_name: 'MCP client',
        token_endpoint_auth_method: 'client_secret_basic',
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        scope: 'mcp:tools',
        client_id: null,
        client_id_issued_at: null,
        client_secret: null,
        client_secret_expires_at: body.client_secret_expires_at,
      }
    )
    assert.match(body.client_secret, /^mcp_secret_[A-Za-z0-9_-]{43}$/)

    const padded = await client.post('/register').json({
      ...registration,
      client_name: `  ${'x'.repeat(120)}  `,
      scope: '  mcp:tools  ',
      grant_types: ['refresh_token', 'authorization_code', 'refresh_token'],
    })
    padded.assertStatus(201)
    assert.equal(padded.body().client_name, 'x'.repeat(120))
    assert.equal(padded.body().scope, 'mcp:tools')
    assert.deepEqual(padded.body().grant_types, ['refresh_token', 'authorization_code'])
  })
})

test.group('vine: gateway OAuth authorization request', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('tells a missing client id from one that names no client', async ({ client, assert }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)

    for (const clientId of [null, '', [oauthClient.clientId, oauthClient.clientId]]) {
      const response = await client
        .get(`/authorize?${queryString(base, { client_id: clientId })}`)
        .redirects(0)
      assertOauthError(assert, response, 400, 'invalid_request', 'client_id is required')
    }

    // A blank value is a client id, and names no client.
    for (const clientId of [' ', ` ${oauthClient.clientId}`, 'mcp_client_unknown']) {
      const response = await client
        .get(`/authorize?${queryString(base, { client_id: clientId })}`)
        .redirects(0)
      assertOauthError(assert, response, 400, 'invalid_client', 'Unknown OAuth client')
      assert.equal(response.header('www-authenticate'), 'Basic realm="MyMCPs OAuth"')
    }
  })

  test('never redirects to a URI the client did not register', async ({ client, assert }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)

    for (const redirectUri of [
      null,
      '',
      ' ',
      [loopbackRedirectUri, loopbackRedirectUri],
      'http://127.0.0.1:49152/other',
      'http://localhost:49152/callback',
      'https://127.0.0.1:49152/callback',
      'http://127.0.0.1:49152/callback?x=1',
      remoteRedirectUri,
    ]) {
      const response = await client
        .get(
          `/authorize?${queryString(base, { redirect_uri: redirectUri, response_type: 'token' })}`
        )
        .redirects(0)
      assertOauthError(assert, response, 400, 'invalid_request', 'Unregistered redirect_uri')
      assert.isUndefined(response.header('location'))
    }

    const remote = await register(client, { redirect_uris: [remoteRedirectUri] })
    const otherPort = await client
      .get(
        `/authorize?${queryString(authorization(remote.clientId, 'https://client.example:8443/callback'))}`
      )
      .redirects(0)
    assertOauthError(assert, otherPort, 400, 'invalid_request', 'Unregistered redirect_uri')
  })

  test('answers with the first parameter an authorization request gets wrong', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)
    const wrong = {
      response_type: 'token',
      code_challenge: 'too-short',
      scope: 'other',
      resource: 'https://other.example/mcp',
      state: 's'.repeat(2049),
    }
    const expected: Array<[keyof typeof wrong, string, string]> = [
      ['response_type', 'unsupported_response_type', 'Only the code response type is supported'],
      ['code_challenge', 'invalid_request', 'PKCE with the S256 method is required'],
      ['scope', 'invalid_scope', 'Unsupported OAuth scope'],
      ['resource', 'invalid_target', 'The OAuth resource must be the MyMCPs gateway'],
      ['state', 'invalid_request', 'OAuth state is too long'],
    ]

    // Everything from one parameter onwards is wrong: that parameter decides.
    for (const [index, [, error, description]] of expected.entries()) {
      const changes = Object.fromEntries(
        expected.slice(index).map(([field]) => [field, wrong[field]])
      )
      const response = await client.get(`/authorize?${queryString(base, changes)}`).redirects(0)
      assert.deepEqual(redirectedError(assert, response), [error, description, wrong.state])
    }
  })

  test('requires PKCE with a well-formed S256 challenge', async ({ client, assert }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)

    for (const changes of [
      { code_challenge: null },
      { code_challenge: '' },
      { code_challenge: 'a'.repeat(42) },
      { code_challenge: 'a'.repeat(129) },
      { code_challenge: `${'a'.repeat(42)}.` },
      { code_challenge: [codeChallenge, codeChallenge] },
      { code_challenge_method: null },
      { code_challenge_method: 'plain' },
      { code_challenge_method: 's256' },
      { code_challenge_method: ['S256', 'S256'] },
    ] as Changes[]) {
      const response = await client.get(`/authorize?${queryString(base, changes)}`).redirects(0)
      assert.deepEqual(redirectedError(assert, response), [
        'invalid_request',
        'PKCE with the S256 method is required',
        'state-from-client',
      ])
    }

    for (const challenge of ['a'.repeat(43), 'a'.repeat(128)]) {
      const response = await client
        .get(`/authorize?${queryString(base, { code_challenge: challenge })}`)
        .redirects(0)
      response.assertStatus(302)
      assert.equal(response.header('location'), '/login')
    }
  })

  test('accepts the gateway scope and resource in the spellings clients use', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)

    for (const changes of [
      { scope: null },
      { scope: ' mcp:tools ' },
      // Sent twice, the parameter is not a scope string and counts as left out.
      { scope: ['other', 'another'] },
      { resource: 'http://LOCALHOST:3333/mcp' },
      { resource: 'HTTP://localhost:3333/a/../mcp' },
    ] as Changes[]) {
      const response = await client.get(`/authorize?${queryString(base, changes)}`).redirects(0)
      response.assertStatus(302)
      assert.equal(response.header('location'), '/login')
    }

    for (const scope of ['', ' ', 'mcp:tools mcp:tools', 'mcp:tools other', 'MCP:TOOLS']) {
      const response = await client.get(`/authorize?${queryString(base, { scope })}`).redirects(0)
      assert.deepEqual(redirectedError(assert, response).slice(0, 2), [
        'invalid_scope',
        'Unsupported OAuth scope',
      ])
    }

    for (const target of [
      null,
      '',
      'http://localhost:3333/mcp/',
      'http://localhost:3333/mcp?x=1',
      'http://localhost:3333',
      'not a url',
      [resource, resource],
    ]) {
      const response = await client
        .get(`/authorize?${queryString(base, { resource: target })}`)
        .redirects(0)
      assert.deepEqual(redirectedError(assert, response).slice(0, 2), [
        'invalid_target',
        'The OAuth resource must be the MyMCPs gateway',
      ])
    }
  })

  test('echoes the state exactly as the client sent it', async ({ client, assert }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const base = { ...authorization(oauthClient.clientId), response_type: 'token' }

    for (const [state, echoed] of [
      [null, null],
      ['', ''],
      [' ', ' '],
      ['a b&c=d#e%20', 'a b&c=d#e%20'],
      ['s'.repeat(2049), 's'.repeat(2049)],
      // Sent twice, the parameter is not a state.
      [['one', 'two'], null],
    ] as Array<[string | string[] | null, string | null]>) {
      const response = await client.get(`/authorize?${queryString(base, { state })}`).redirects(0)
      assert.deepEqual(redirectedError(assert, response), [
        'unsupported_response_type',
        'Only the code response type is supported',
        echoed,
      ])
    }
  })

  test('grants access on an explicit approval only', async ({ client, assert }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client)
    const base = authorization(oauthClient.clientId)
    const decide = (decision: Record<string, unknown>) =>
      client
        .post('/authorize')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form({ ...base, ...decision })

    for (const decision of [
      {},
      { decision: '' },
      { decision: 'deny' },
      { decision: 'APPROVE' },
      { decision: 'true' },
      { decision: ['approve', 'approve'] },
      { decision: { value: 'approve' } },
    ]) {
      const response = await decide(decision)
      assert.deepEqual(redirectedError(assert, response), [
        'access_denied',
        'The user denied the authorization request',
        'state-from-client',
      ])
    }
    assert.isNull(await OauthAuthorizationCode.first())

    const approval = await decide({ decision: 'approve' })
    approval.assertStatus(302)
    const callback = new URL(approval.header('location')!)
    assert.match(callback.searchParams.get('code') ?? '', /^[A-Za-z0-9_-]{43}$/)
    assert.equal(callback.searchParams.get('state'), 'state-from-client')
  })
})

test.group('vine: gateway OAuth token and revocation endpoints', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  function issueCode(oauthClient: OauthClient, admin: User) {
    return createAuthorizationCode(
      {
        client: oauthClient,
        redirectUri: loopbackRedirectUri,
        state: null,
        codeChallenge,
        scopes: 'mcp:tools',
        resource,
      },
      admin.id
    )
  }

  async function issueGrant(oauthClient: OauthClient, admin: User) {
    const created = await AccessTokenService.createOauthGrant({
      name: oauthClient.clientName,
      clientId: oauthClient.id,
      clientSupportsRefresh: true,
      scopes: 'mcp:tools',
      resource,
      createdBy: admin.id,
    })
    return { accessToken: created.plaintext, refreshToken: created.refreshToken! }
  }

  test('authenticates the client before it reads the grant', async ({ client, assert }) => {
    await createAdmin()
    const oauthClient = await register(client)

    for (const credentials of [
      '',
      'client_id=',
      queryString({}, { client_id: [oauthClient.clientId, oauthClient.clientId] }),
    ]) {
      const response = await client.post('/token').form(credentials)
      assertOauthError(
        assert,
        response,
        401,
        'invalid_client',
        'OAuth client authentication is required'
      )
      assert.equal(response.header('www-authenticate'), 'Basic realm="MyMCPs OAuth"')
    }

    const unknown = await client.post('/token').form({ client_id: 'mcp_client_unknown' })
    assertOauthError(assert, unknown, 401, 'invalid_client', 'Invalid OAuth client credentials')

    // A public client that sends a secret is not the client that registered.
    const withSecret = await client
      .post('/token')
      .form({ client_id: oauthClient.clientId, client_secret: 'surplus' })
    assertOauthError(assert, withSecret, 401, 'invalid_client', 'Invalid OAuth client credentials')

    // A secret sent twice is not a secret, and the client is a public one.
    const secretTwice = await client
      .post('/token')
      .form(
        `${queryString({ client_id: oauthClient.clientId })}&client_secret=surplus&client_secret=surplus`
      )
    assertOauthError(assert, secretTwice, 400, 'invalid_request', 'grant_type is required')
  })

  test('accepts a client secret in the body or in a Basic header', async ({ client, assert }) => {
    await createAdmin()
    const posted = await client
      .post('/register')
      .json({ ...registration, token_endpoint_auth_method: 'client_secret_post' })
    const basic = await client
      .post('/register')
      .json({ ...registration, token_endpoint_auth_method: 'client_secret_basic' })
    const header = (id: string, secret: string) =>
      `Basic ${Buffer.from(`${encodeURIComponent(id)}:${encodeURIComponent(secret)}`).toString('base64')}`

    const viaBody = await client.post('/token').form({
      client_id: posted.body().client_id,
      client_secret: posted.body().client_secret,
    })
    assertOauthError(assert, viaBody, 400, 'invalid_request', 'grant_type is required')

    const viaHeader = await client
      .post('/token')
      .header('authorization', header(basic.body().client_id, basic.body().client_secret))
      .form({})
    assertOauthError(assert, viaHeader, 400, 'invalid_request', 'grant_type is required')

    for (const response of [
      await client.post('/token').form({ client_id: posted.body().client_id }),
      await client
        .post('/token')
        .form({ client_id: posted.body().client_id, client_secret: 'mcp_secret_wrong' }),
      await client
        .post('/token')
        .header('authorization', header(basic.body().client_id, 'mcp_secret_wrong'))
        .form({}),
      // The method a client registered is the only one it may use.
      await client
        .post('/token')
        .header('authorization', header(posted.body().client_id, posted.body().client_secret))
        .form({}),
      await client.post('/token').form({
        client_id: basic.body().client_id,
        client_secret: basic.body().client_secret,
      }),
    ]) {
      assertOauthError(assert, response, 401, 'invalid_client', 'Invalid OAuth client credentials')
    }
  })

  test('names the grant type it misses and refuses those it does not have', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const oauthClient = await register(client)
    const credentials = { client_id: oauthClient.clientId }

    for (const grantType of [null, '', ['refresh_token', 'refresh_token']]) {
      const response = await client
        .post('/token')
        .form(queryString(credentials, { grant_type: grantType }))
      assertOauthError(assert, response, 400, 'invalid_request', 'grant_type is required')
    }

    for (const grantType of ['client_credentials', 'password', 'AUTHORIZATION_CODE']) {
      const response = await client.post('/token').form({ ...credentials, grant_type: grantType })
      assertOauthError(
        assert,
        response,
        400,
        'unsupported_grant_type',
        'Only authorization_code and refresh_token grants are supported'
      )
    }
  })

  test('names the first parameter an authorization code grant misses', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client)
    const grant = {
      grant_type: 'authorization_code',
      client_id: oauthClient.clientId,
      code: await issueCode(oauthClient, admin),
      code_verifier: codeVerifier,
      redirect_uri: loopbackRedirectUri,
      resource,
    }
    const parameters = ['code', 'code_verifier', 'redirect_uri', 'resource'] as const

    for (const [index, parameter] of parameters.entries()) {
      // Everything from this parameter onwards is missing, empty or sent twice.
      for (const value of [null, '', ['one', 'two']]) {
        const changes = Object.fromEntries(parameters.slice(index).map((name) => [name, value]))
        const response = await client.post('/token').form(queryString(grant, changes))
        assertOauthError(assert, response, 400, 'invalid_request', `${parameter} is required`)
      }
    }

    // None of these requests used the code up.
    const exchange = await client.post('/token').form(grant)
    exchange.assertStatus(200)
    assert.match(exchange.body().access_token, /^mcp_[A-Za-z0-9_-]{43}$/)
  })

  test('answers a malformed verifier or a foreign resource like a code that does not match', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client)
    const grant = {
      grant_type: 'authorization_code',
      client_id: oauthClient.clientId,
      code: await issueCode(oauthClient, admin),
      code_verifier: codeVerifier,
      redirect_uri: loopbackRedirectUri,
      resource,
    }

    for (const changes of [
      { code_verifier: 'a'.repeat(42) },
      { code_verifier: 'a'.repeat(129) },
      { code_verifier: `${codeVerifier}!` },
      { code_verifier: 'another-verifier-that-is-long-enough-to-be-well-formed' },
      { resource: 'https://other.example/mcp' },
      { resource: 'http://localhost:3333/mcp/' },
      { resource: 'not a url' },
      { redirect_uri: 'http://127.0.0.1/callback' },
      { code: 'unknown-code' },
    ]) {
      const response = await client.post('/token').form({ ...grant, ...changes })
      assertOauthError(
        assert,
        response,
        400,
        'invalid_grant',
        'Invalid or expired authorization code'
      )
    }

    const exchange = await client
      .post('/token')
      .form({ ...grant, resource: 'http://LOCALHOST:3333/mcp' })
    exchange.assertStatus(200)
  })

  test('checks a refresh request in the order client, scope, resource, token', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client)
    const { refreshToken } = await issueGrant(oauthClient, admin)
    const grant = {
      grant_type: 'refresh_token',
      client_id: oauthClient.clientId,
      refresh_token: refreshToken,
      resource,
    }

    for (const [changes, description] of [
      [{ refresh_token: null, resource: null }, 'refresh_token is required'],
      [{ refresh_token: ['one', 'two'] }, 'refresh_token is required'],
      [{ resource: null, scope: 'other' }, 'resource is required'],
      [{ resource: '' }, 'resource is required'],
    ] as Array<[Changes, string]>) {
      const response = await client.post('/token').form(queryString(grant, changes))
      assertOauthError(assert, response, 400, 'invalid_request', description)
    }

    for (const scope of ['other', 'mcp:tools mcp:tools', 'MCP:TOOLS']) {
      const response = await client
        .post('/token')
        .form({ ...grant, scope, resource: 'https://other.example/mcp' })
      assertOauthError(assert, response, 400, 'invalid_scope', 'Unsupported OAuth scope')
    }
    // An empty scope in the query string is a scope, and names none.
    const emptyScope = await client.post('/token?scope=').form(grant)
    assertOauthError(assert, emptyScope, 400, 'invalid_scope', 'Unsupported OAuth scope')

    for (const target of ['https://other.example/mcp', 'http://localhost:3333/mcp/', 'not a url']) {
      const response = await client
        .post('/token')
        .form({ ...grant, resource: target, refresh_token: 'mcp_refresh_unknown' })
      assertOauthError(
        assert,
        response,
        400,
        'invalid_target',
        'The OAuth resource must be the MyMCPs gateway'
      )
    }

    const unknown = await client
      .post('/token')
      .form({ ...grant, refresh_token: 'mcp_refresh_unknown' })
    assertOauthError(
      assert,
      unknown,
      400,
      'invalid_grant',
      'Invalid, expired, or revoked refresh token'
    )

    // Sent twice, the scope is not a scope string and counts as left out.
    const refreshed = await client
      .post('/token')
      .form(
        queryString(grant, { scope: ['other', 'another'], resource: 'http://LOCALHOST:3333/mcp' })
      )
    refreshed.assertStatus(200)
    assert.match(refreshed.body().refresh_token, /^mcp_refresh_[A-Za-z0-9_-]{43}$/)

    const again = await client
      .post('/token')
      .form({ ...grant, refresh_token: refreshed.body().refresh_token, scope: 'mcp:tools' })
    again.assertStatus(200)
  })

  test('refuses a refresh to a client registered without that grant, whatever else is wrong', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client, { grant_types: ['authorization_code'] })
    const { refreshToken } = await issueGrant(oauthClient, admin)

    const response = await client.post('/token').form({
      grant_type: 'refresh_token',
      client_id: oauthClient.clientId,
      refresh_token: refreshToken,
      scope: 'other',
      resource: 'https://other.example/mcp',
    })

    assertOauthError(
      assert,
      response,
      400,
      'unauthorized_client',
      'This OAuth client cannot refresh tokens'
    )
  })

  test('requires a token to revoke and says nothing about tokens it does not know', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const oauthClient = await register(client)
    const { accessToken } = await issueGrant(oauthClient, admin)
    const credentials = { client_id: oauthClient.clientId }

    for (const token of [null, '', ['one', 'two']]) {
      const response = await client.post('/revoke').form(queryString(credentials, { token }))
      assertOauthError(assert, response, 400, 'invalid_request', 'token is required')
    }
    const anonymous = await client.post('/revoke').form({ token: accessToken })
    assertOauthError(
      assert,
      anonymous,
      401,
      'invalid_client',
      'OAuth client authentication is required'
    )
    assert.isNotNull(await AccessTokenService.findUsableByPlaintext(accessToken))

    // A blank token in the query string is a token, and one nobody holds.
    for (const response of [
      await client.post('/revoke').form({ ...credentials, token: 'mcp_unknown' }),
      await client.post('/revoke?token=%20').form(credentials),
    ]) {
      response.assertStatus(200)
      assert.deepEqual(response.body(), {})
    }
    assert.isNotNull(await AccessTokenService.findUsableByPlaintext(accessToken))

    const revoked = await client.post('/revoke').form({ ...credentials, token: accessToken })
    revoked.assertStatus(200)
    assert.isNull(await AccessTokenService.findUsableByPlaintext(accessToken))
  })
})
