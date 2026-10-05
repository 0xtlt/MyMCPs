import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import logger from '@adonisjs/core/services/logger'
import limiter from '@adonisjs/limiter/services/main'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import OauthAuthorizationCode from '#models/oauth_authorization_code'
import OauthClient from '#models/oauth_client'
import AccessTokenService from '#services/access_token_service'
import {
  MAX_OAUTH_CLIENTS,
  UNUSED_CLIENT_RETENTION_DAYS,
  pruneUnusedOauthClients,
} from '#services/gateway_oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

const resource = 'http://localhost:3333/mcp'
const loopbackRedirectUri = 'http://127.0.0.1:49152/callback'
const remoteRedirectUri = 'https://client.example/callback'
const codeChallenge = createHash('sha256')
  .update('hardening-oauth-code-verifier-for-mymcps-tests-123')
  .digest('base64url')

async function registerClient(client: ApiClient, redirectUri: string) {
  const response = await client.post('/register').json({
    client_name: 'Hardening client',
    redirect_uris: [redirectUri],
    token_endpoint_auth_method: 'none',
    grant_types: ['authorization_code', 'refresh_token'],
    response_types: ['code'],
  })
  response.assertStatus(201)
  return response.body().client_id as string
}

function authorizationPayload(clientId: string, redirectUri: string) {
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

function authorizationPath(payload: Record<string, string>) {
  return `/authorize?${new URLSearchParams(payload)}`
}

async function storedClient(
  name: string,
  createdAt: DateTime = DateTime.utc().minus({ days: UNUSED_CLIENT_RETENTION_DAYS + 1 })
) {
  return OauthClient.create({
    clientId: `mcp_client_${name}`,
    clientName: name,
    redirectUris: JSON.stringify([loopbackRedirectUri]),
    tokenEndpointAuthMethod: 'none',
    grantTypes: JSON.stringify(['authorization_code', 'refresh_token']),
    responseTypes: JSON.stringify(['code']),
    scope: 'mcp:tools',
    createdAt,
  })
}

async function storedGrant(oauthClient: OauthClient, userId: number) {
  const { token } = await AccessTokenService.createOauthGrant({
    name: oauthClient.clientName,
    clientId: oauthClient.id,
    clientSupportsRefresh: true,
    scopes: 'mcp:tools',
    resource,
    createdBy: userId,
  })
  return token
}

/** Age a grant so that both its tokens have expired and it was last touched at `lastActive`. */
async function expireGrant(token: AccessToken, lastActive: DateTime) {
  await db
    .from('access_tokens')
    .where('id', token.id)
    .update({
      expires_at: lastActive.toSQL({ includeOffset: false }),
      oauth_refresh_expires_at: lastActive.toSQL({ includeOffset: false }),
      updated_at: lastActive.toSQL({ includeOffset: false }),
    })
}

async function clientCount() {
  const [{ total }] = await db.from('oauth_clients').count('* as total')
  return Number(total)
}

test.group('hardening: OAuth authorization endpoint', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('answers HEAD /authorize with 405 and never issues a code', async ({ client, assert }) => {
    const admin = await createAdmin()
    const clientId = await registerClient(client, remoteRedirectUri)

    const response = await client
      .head(
        authorizationPath({
          ...authorizationPayload(clientId, remoteRedirectUri),
          decision: 'approve',
        })
      )
      .loginAs(admin)
      .redirects(0)

    response.assertStatus(405)
    assert.equal(response.header('allow'), 'GET, POST')
    assert.isUndefined(response.header('location'))
    assert.isNull(await OauthAuthorizationCode.first())
  })

  test('reads the decision from the form body, not from the query string', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const clientId = await registerClient(client, loopbackRedirectUri)
    const authorization = authorizationPayload(clientId, loopbackRedirectUri)

    const response = await client
      .post('/authorize?decision=approve')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form(authorization)

    response.assertStatus(302)
    const callback = new URL(response.header('location')!)
    assert.equal(callback.searchParams.get('error'), 'access_denied')
    assert.isNull(callback.searchParams.get('code'))
    assert.isNull(await OauthAuthorizationCode.first())

    const queryOnly = await client
      .post(authorizationPath({ ...authorization, decision: 'approve' }))
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({})

    queryOnly.assertStatus(400)
    assert.equal(queryOnly.body().error, 'invalid_request')
    assert.isNull(await OauthAuthorizationCode.first())
  })

  test('shows a rejected request here instead of redirecting to a remote client', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const clientId = await registerClient(client, remoteRedirectUri)

    const response = await client
      .get(
        authorizationPath({
          ...authorizationPayload(clientId, remoteRedirectUri),
          response_type: 'token',
        })
      )
      .redirects(0)

    response.assertStatus(400)
    assert.isUndefined(response.header('location'))
    assert.equal(response.body().error, 'unsupported_response_type')
    assert.equal(response.header('cache-control'), 'no-store')
  })

  test('returns a rejected request to a loopback client with its state intact', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const clientId = await registerClient(client, loopbackRedirectUri)

    const response = await client
      .get(
        authorizationPath({
          ...authorizationPayload(clientId, loopbackRedirectUri),
          response_type: 'token',
        })
      )
      .redirects(0)

    response.assertStatus(302)
    const callback = new URL(response.header('location')!)
    assert.equal(callback.origin, 'http://127.0.0.1:49152')
    assert.deepEqual([...callback.searchParams.keys()], ['error', 'error_description', 'state'])
    assert.equal(callback.searchParams.get('error'), 'unsupported_response_type')
    assert.equal(callback.searchParams.get('state'), 'state-from-client')
  })

  test('sends signed-out users to the login page without the authorization query', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const clientId = await registerClient(client, remoteRedirectUri)

    const response = await client
      .get(authorizationPath(authorizationPayload(clientId, remoteRedirectUri)))
      .redirects(0)

    response.assertStatus(302)
    assert.equal(response.header('location'), '/login')
  })

  test('still returns the operator decision to a remote client', async ({ client, assert }) => {
    const admin = await createAdmin()
    const clientId = await registerClient(client, remoteRedirectUri)
    const authorization = authorizationPayload(clientId, remoteRedirectUri)

    const denial = await client
      .post('/authorize')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ ...authorization, decision: 'deny' })
    denial.assertStatus(302)
    const denied = new URL(denial.header('location')!)
    assert.equal(denied.origin + denied.pathname, remoteRedirectUri)
    assert.equal(denied.searchParams.get('error'), 'access_denied')
    assert.equal(denied.searchParams.get('state'), 'state-from-client')

    const approval = await client
      .post('/authorize')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ ...authorization, decision: 'approve' })
    approval.assertStatus(302)
    const approved = new URL(approval.header('location')!)
    assert.equal(approved.origin + approved.pathname, remoteRedirectUri)
    assert.match(approved.searchParams.get('code') ?? '', /^[A-Za-z0-9_-]{43}$/)
    assert.equal(approved.searchParams.get('state'), 'state-from-client')
  })

  test('logs an unexpected OAuth failure without the raw error', async ({ client, assert }) => {
    await createAdmin()
    const logged: unknown[][] = []
    const originalChild = logger.child
    const originalCreate = OauthClient.create
    logger.child = function (this: typeof logger, ...args: Parameters<typeof logger.child>) {
      const child = originalChild.apply(this, args)
      child.error = ((...entry: unknown[]) => {
        logged.push(entry)
      }) as typeof child.error
      return child
    } as typeof logger.child
    OauthClient.create = (() => {
      throw new Error(
        "insert into `oauth_clients` (`client_secret_hash`) values ('Bearer raw-secret-value') - SQLITE_BUSY"
      )
    }) as typeof OauthClient.create

    try {
      const response = await client.post('/register').json({
        client_name: 'Failing client',
        redirect_uris: [loopbackRedirectUri],
        token_endpoint_auth_method: 'none',
      })

      response.assertStatus(500)
      assert.equal(response.body().error, 'server_error')
    } finally {
      logger.child = originalChild
      OauthClient.create = originalCreate
    }

    assert.lengthOf(logged, 1)
    const [fields, message] = logged[0] as [Record<string, unknown>, string]
    assert.equal(message, 'OAuth request failed')
    assert.notProperty(fields, 'err')
    assert.include(fields.error as string, 'Bearer [REDACTED]')
    assert.notInclude(JSON.stringify(fields), 'raw-secret-value')
  })
})

test.group('hardening: OAuth client registration', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('stores each grant and response type once', async ({ client, assert }) => {
    await createAdmin()

    const response = await client.post('/register').json({
      client_name: 'Repeating client',
      redirect_uris: [loopbackRedirectUri],
      token_endpoint_auth_method: 'none',
      grant_types: [
        ...Array.from({ length: 5000 }, () => 'authorization_code'),
        ...Array.from({ length: 5000 }, () => 'refresh_token'),
      ],
      response_types: ['code', 'code', 'code'],
    })

    response.assertStatus(201)
    assert.deepEqual(response.body().grant_types, ['authorization_code', 'refresh_token'])
    assert.deepEqual(response.body().response_types, ['code'])

    const stored = await OauthClient.findByOrFail('client_id', response.body().client_id)
    assert.equal(stored.grantTypes, '["authorization_code","refresh_token"]')
    assert.equal(stored.responseTypes, '["code"]')
  })

  test('still rejects grant and response types outside the allowed set', async ({
    client,
    assert,
  }) => {
    await createAdmin()
    const metadata = {
      client_name: 'Unsupported client',
      redirect_uris: [loopbackRedirectUri],
      token_endpoint_auth_method: 'none',
    }

    for (const overrides of [
      { grant_types: ['authorization_code', 'client_credentials'] },
      { grant_types: ['refresh_token', 'refresh_token'] },
      { response_types: ['code', 'token'] },
    ]) {
      const response = await client.post('/register').json({ ...metadata, ...overrides })
      response.assertStatus(400)
      assert.equal(response.body().error, 'invalid_client_metadata')
    }
  })

  test('ignores an oversized type list stored by an earlier version', async ({ assert }) => {
    const legacy = await storedClient('legacy')
    legacy.grantTypes = JSON.stringify(Array.from({ length: 5000 }, () => 'refresh_token'))
    await legacy.save()
    const current = await storedClient('current')

    assert.deepEqual(legacy.grantTypeList, [])
    assert.deepEqual(current.grantTypeList, ['authorization_code', 'refresh_token'])
  })

  test('removes clients left unused for the retention period', async ({ assert }) => {
    const admin = await createAdmin()
    const longAgo = DateTime.utc().minus({ days: UNUSED_CLIENT_RETENTION_DAYS + 10 })

    await storedClient('never-used')
    await storedClient('registered-recently', DateTime.utc().minus({ days: 1 }))

    await storedGrant(await storedClient('live-grant'), admin.id)

    const pending = await storedClient('pending-code')
    await OauthAuthorizationCode.create({
      codeHash: AccessTokenService.hash('pending-code'),
      oauthClientId: pending.id,
      userId: admin.id,
      redirectUri: loopbackRedirectUri,
      codeChallenge,
      scopes: 'mcp:tools',
      resource,
      expiresAt: DateTime.utc().plus({ minutes: 5 }),
    })

    const idleGrant = await storedGrant(await storedClient('grant-expired-recently'), admin.id)
    await expireGrant(idleGrant, DateTime.utc().minus({ days: 10 }))

    const deadGrant = await storedGrant(await storedClient('grant-expired-long-ago'), admin.id)
    await expireGrant(deadGrant, longAgo)

    await pruneUnusedOauthClients({ force: true })

    const remaining = await OauthClient.query().orderBy('client_name', 'asc')
    assert.deepEqual(
      remaining.map((oauthClient) => oauthClient.clientName),
      ['grant-expired-recently', 'live-grant', 'pending-code', 'registered-recently']
    )

    // The expired connection stays in the token list without its client.
    const orphaned = await AccessToken.findOrFail(deadGrant.id)
    assert.isNull(orphaned.oauthClientId)
  })

  test('evicts the oldest unused client when the client limit is reached', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const inUse = await storedClient('in-use', DateTime.utc())
    await storedGrant(inUse, admin.id)
    const oldestUnused = await storedClient('oldest-unused', DateTime.utc())

    const createdAt = DateTime.utc().toSQL({ includeOffset: false })
    const fillers = Array.from({ length: MAX_OAUTH_CLIENTS - 2 }, (_, index) => ({
      client_id: `mcp_client_filler_${index}`,
      client_name: 'Filler',
      redirect_uris: '[]',
      token_endpoint_auth_method: 'none',
      grant_types: '["authorization_code"]',
      response_types: '["code"]',
      scope: 'mcp:tools',
      created_at: createdAt,
    }))
    for (let offset = 0; offset < fillers.length; offset += 100) {
      await db.table('oauth_clients').multiInsert(fillers.slice(offset, offset + 100))
    }
    assert.equal(await clientCount(), MAX_OAUTH_CLIENTS)

    await registerClient(client, loopbackRedirectUri)

    assert.equal(await clientCount(), MAX_OAUTH_CLIENTS)
    assert.isNotNull(await OauthClient.find(inUse.id))
    assert.isNull(await OauthClient.find(oldestUnused.id))

    // Once every client is in use there is nothing left to evict.
    await db.rawQuery(
      `insert into oauth_authorization_codes
        (code_hash, oauth_client_id, user_id, redirect_uri, code_challenge, scopes, resource, expires_at, created_at)
       select 'pending-' || id, id, ?, ?, ?, 'mcp:tools', ?, ?, ? from oauth_clients`,
      [
        admin.id,
        loopbackRedirectUri,
        codeChallenge,
        resource,
        DateTime.utc().plus({ minutes: 5 }).toSQL({ includeOffset: false }),
        createdAt,
      ]
    )

    const refused = await client.post('/register').json({
      client_name: 'One too many',
      redirect_uris: [loopbackRedirectUri],
      token_endpoint_auth_method: 'none',
    })
    refused.assertStatus(503)
    assert.equal(refused.body().error, 'temporarily_unavailable')
    assert.equal(await clientCount(), MAX_OAUTH_CLIENTS)
  })
})
