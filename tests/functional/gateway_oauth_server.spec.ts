import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import limiter from '@adonisjs/limiter/services/main'
import AccessToken from '#models/access_token'
import AccessTokenService from '#services/access_token_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

const resource = 'http://localhost:3333/mcp'
const registeredRedirectUri = 'http://127.0.0.1/callback'
const runtimeRedirectUri = 'http://127.0.0.1:49152/callback'
const codeVerifier = 'oauth-code-verifier-for-mymcps-tests-1234567890'
const codeChallenge = createHash('sha256').update(codeVerifier).digest('base64url')

type RegisteredClient = {
  client_id: string
  client_name: string
  token_endpoint_auth_method: string
}

async function registerPublicClient(client: ApiClient, clientName = 'Codex test client') {
  const response = await client.post('/register').json({
    client_name: clientName,
    redirect_uris: [registeredRedirectUri],
    token_endpoint_auth_method: 'none',
    grant_types: ['authorization_code', 'refresh_token'],
    response_types: ['code'],
    scope: 'mcp:tools',
  })
  response.assertStatus(201)
  return response.body() as RegisteredClient
}

function authorizationPayload(clientId: string) {
  return {
    client_id: clientId,
    redirect_uri: runtimeRedirectUri,
    response_type: 'code',
    code_challenge: codeChallenge,
    code_challenge_method: 'S256',
    scope: 'mcp:tools',
    resource,
    state: 'state-from-client',
  }
}

function authorizationPath(clientId: string) {
  return `/authorize?${new URLSearchParams(authorizationPayload(clientId)).toString()}`
}

test.group('gateway OAuth server', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('publishes MCP protected-resource and authorization-server metadata', async ({
    client,
    assert,
  }) => {
    await createAdmin()

    const resourceResponse = await client.get('/.well-known/oauth-protected-resource/mcp')
    resourceResponse.assertStatus(200)
    assert.deepEqual(resourceResponse.body(), {
      resource,
      authorization_servers: ['http://localhost:3333'],
      scopes_supported: ['mcp:tools'],
      bearer_methods_supported: ['header'],
      resource_name: 'MyMCPs gateway',
    })

    const metadataResponse = await client.get('/.well-known/oauth-authorization-server')
    metadataResponse.assertStatus(200)
    assert.equal(metadataResponse.body().authorization_endpoint, 'http://localhost:3333/authorize')
    assert.equal(metadataResponse.body().token_endpoint, 'http://localhost:3333/token')
    assert.equal(metadataResponse.body().registration_endpoint, 'http://localhost:3333/register')
    assert.deepEqual(metadataResponse.body().code_challenge_methods_supported, ['S256'])
  })

  test('challenges unauthenticated MCP calls with OAuth discovery details', async ({
    client,
    assert,
  }) => {
    await createAdmin()

    const response = await client.get('/mcp')

    response.assertStatus(401)
    const challenge = response.header('www-authenticate')
    assert.include(challenge, 'Bearer')
    assert.include(
      challenge,
      'resource_metadata="http://localhost:3333/.well-known/oauth-protected-resource/mcp"'
    )
    assert.include(challenge, 'scope="mcp:tools"')
  })

  test('registers public clients and rejects unsafe redirect URIs', async ({ client, assert }) => {
    await createAdmin()

    const registered = await registerPublicClient(client)
    assert.match(registered.client_id, /^mcp_client_[A-Za-z0-9_-]{32}$/)
    assert.equal(registered.token_endpoint_auth_method, 'none')

    const unsafe = await client.post('/register').json({
      client_name: 'Unsafe client',
      redirect_uris: ['http://example.com/callback'],
      token_endpoint_auth_method: 'none',
      grant_types: ['authorization_code'],
      response_types: ['code'],
    })
    unsafe.assertStatus(400)
    assert.equal(unsafe.body().error, 'invalid_redirect_uri')
  })

  test('returns users to the authorization request after credential login', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'oauth-user@example.com' })
    const registered = await registerPublicClient(client)

    const start = await client.get(authorizationPath(registered.client_id)).redirects(0)
    start.assertStatus(302)
    assertRedirectTo(assert, start, '/login')

    const login = await client
      .post('/login')
      .withSession(start.session())
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'oauth-user@example.com', password: 'password123' })

    login.assertStatus(302)
    assert.equal(new URL(login.header('location')!, 'http://localhost').pathname, '/authorize')

    const consent = await client
      .get(login.header('location')!)
      .withInertia()
      .withSession(login.session())
    consent.assertStatus(200)
    consent.assertInertiaComponent('oauth/authorize')
    consent.assertInertiaPropsContains({
      clientName: 'Codex test client',
      userEmail: 'oauth-user@example.com',
      redirectHost: '127.0.0.1:49152',
    })
  })

  test('issues, refreshes, lists, and revokes an OAuth connection', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'owner@example.com' })
    const registered = await registerPublicClient(client, 'Claude Desktop')
    const authorization = authorizationPayload(registered.client_id)

    const consent = await client
      .get(authorizationPath(registered.client_id))
      .withInertia()
      .loginAs(admin)
    consent.assertStatus(200)
    consent.assertInertiaComponent('oauth/authorize')
    consent.assertInertiaPropsContains({
      clientName: 'Claude Desktop',
      redirectHost: '127.0.0.1:49152',
      isLoopbackRedirect: true,
      scope: 'mcp:tools',
    })

    const approval = await client
      .post('/authorize')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ ...authorization, decision: 'approve' })
    approval.assertStatus(302)
    const callback = new URL(approval.header('location')!)
    assert.equal(callback.origin, 'http://127.0.0.1:49152')
    assert.equal(callback.searchParams.get('state'), 'state-from-client')
    const code = callback.searchParams.get('code')
    assert.isNotNull(code)

    const exchange = await client.post('/token').form({
      grant_type: 'authorization_code',
      client_id: registered.client_id,
      code,
      code_verifier: codeVerifier,
      redirect_uri: runtimeRedirectUri,
      resource,
    })
    exchange.assertStatus(200)
    assert.equal(exchange.body().token_type, 'Bearer')
    assert.equal(exchange.body().expires_in, 3600)
    assert.equal(exchange.body().scope, 'mcp:tools')
    assert.match(exchange.body().access_token, /^mcp_[A-Za-z0-9_-]{43}$/)
    assert.match(exchange.body().refresh_token, /^mcp_refresh_[A-Za-z0-9_-]{43}$/)

    const oauthToken = await AccessToken.findByOrFail('name', 'Claude Desktop')
    assert.equal(oauthToken.source, 'oauth')
    assert.equal(oauthToken.createdBy, admin.id)
    assert.notEqual(oauthToken.tokenHash, exchange.body().access_token)
    assert.notEqual(oauthToken.oauthRefreshTokenHash, exchange.body().refresh_token)
    assert.isTrue(oauthToken.isActive)

    const tokenList = await client.get('/tokens').withInertia().loginAs(admin)
    tokenList.assertStatus(200)
    tokenList.assertInertiaComponent('tokens/index')
    tokenList.assertInertiaPropsContains({
      tokens: [
        {
          id: oauthToken.id,
          name: 'Claude Desktop',
          source: 'oauth',
          oauthClientName: 'Claude Desktop',
          canRevoke: true,
        },
      ],
    })

    const oldAccessToken = exchange.body().access_token as string
    const oldRefreshToken = exchange.body().refresh_token as string
    const refresh = await client.post('/token').form({
      grant_type: 'refresh_token',
      client_id: registered.client_id,
      refresh_token: oldRefreshToken,
      resource,
    })
    refresh.assertStatus(200)
    assert.notEqual(refresh.body().access_token, oldAccessToken)
    assert.notEqual(refresh.body().refresh_token, oldRefreshToken)
    assert.isNull(await AccessTokenService.findUsableByPlaintext(oldAccessToken))
    assert.isNotNull(await AccessTokenService.findUsableByPlaintext(refresh.body().access_token))

    const replay = await client.post('/token').form({
      grant_type: 'refresh_token',
      client_id: registered.client_id,
      refresh_token: oldRefreshToken,
      resource,
    })
    replay.assertStatus(400)
    assert.equal(replay.body().error, 'invalid_grant')

    const revoke = await client
      .post(`/tokens/${oauthToken.id}/revoke`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
    revoke.assertStatus(302)

    const revoked = await AccessToken.findOrFail(oauthToken.id)
    assert.isTrue(revoked.isRevoked)
    assert.isNull(await AccessTokenService.findUsableByPlaintext(refresh.body().access_token))

    const refreshAfterRevoke = await client.post('/token').form({
      grant_type: 'refresh_token',
      client_id: registered.client_id,
      refresh_token: refresh.body().refresh_token,
      resource,
    })
    refreshAfterRevoke.assertStatus(400)
    assert.equal(refreshAfterRevoke.body().error, 'invalid_grant')
  })

  test('rejects authorization-code replay and an incorrect PKCE verifier', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const registered = await registerPublicClient(client)
    const authorization = authorizationPayload(registered.client_id)
    const approval = await client
      .post('/authorize')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ ...authorization, decision: 'approve' })
    const code = new URL(approval.header('location')!).searchParams.get('code')!

    const invalidVerifier = await client.post('/token').form({
      grant_type: 'authorization_code',
      client_id: registered.client_id,
      code,
      code_verifier: 'incorrect-verifier-that-is-still-long-enough-123456789',
      redirect_uri: runtimeRedirectUri,
      resource,
    })
    invalidVerifier.assertStatus(400)
    assert.equal(invalidVerifier.body().error, 'invalid_grant')

    const valid = await client.post('/token').form({
      grant_type: 'authorization_code',
      client_id: registered.client_id,
      code,
      code_verifier: codeVerifier,
      redirect_uri: runtimeRedirectUri,
      resource,
    })
    valid.assertStatus(200)

    const replay = await client.post('/token').form({
      grant_type: 'authorization_code',
      client_id: registered.client_id,
      code,
      code_verifier: codeVerifier,
      redirect_uri: runtimeRedirectUri,
      resource,
    })
    replay.assertStatus(400)
    assert.equal(replay.body().error, 'invalid_grant')
  })
})
