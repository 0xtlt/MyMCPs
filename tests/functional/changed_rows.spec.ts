import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import limiter from '@adonisjs/limiter/services/main'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import McpCallLog from '#models/mcp_call_log'
import AccessTokenService from '#services/access_token_service'
import { changedRows } from '#services/changed_rows'
import McpCallLogService from '#services/mcp_call_log_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcpCallLog, createStoredAccessToken } from '#tests/helpers/factories'

const resource = 'http://localhost:3333/mcp'
const redirectUri = 'http://127.0.0.1:49152/callback'
const codeVerifier = 'changed-rows-code-verifier-for-mymcps-tests-123456'
const codeChallenge = createHash('sha256').update(codeVerifier).digest('base64url')

/** Registration is closed until the instance has its first admin. */
async function registerClient(client: ApiClient, clientName: string) {
  await createAdmin()
  const response = await client.post('/register').json({
    client_name: clientName,
    redirect_uris: ['http://127.0.0.1/callback'],
    token_endpoint_auth_method: 'none',
    grant_types: ['authorization_code', 'refresh_token'],
    response_types: ['code'],
    scope: 'mcp:tools',
  })
  response.assertStatus(201)
  return response.body().client_id as string
}

async function authorizationCode(client: ApiClient, clientId: string) {
  const admin = await createAdmin()
  const approval = await client
    .post('/authorize')
    .loginAs(admin)
    .withCsrfToken()
    .redirects(0)
    .form({
      client_id: clientId,
      redirect_uri: redirectUri,
      response_type: 'code',
      code_challenge: codeChallenge,
      code_challenge_method: 'S256',
      scope: 'mcp:tools',
      resource,
      state: 'state-from-client',
      decision: 'approve',
    })
  return new URL(approval.header('location')!).searchParams.get('code')!
}

function exchangeCode(client: ApiClient, clientId: string, code: string) {
  return client.post('/token').form({
    grant_type: 'authorization_code',
    client_id: clientId,
    code,
    code_verifier: codeVerifier,
    redirect_uri: redirectUri,
    resource,
  })
}

function refresh(client: ApiClient, clientId: string, refreshToken: string) {
  return client.post('/token').form({
    grant_type: 'refresh_token',
    client_id: clientId,
    refresh_token: refreshToken,
    resource,
  })
}

async function connect(client: ApiClient, clientName: string) {
  const clientId = await registerClient(client, clientName)
  const exchange = await exchangeCode(client, clientId, await authorizationCode(client, clientId))
  exchange.assertStatus(200)
  return { clientId, refreshToken: exchange.body().refresh_token as string }
}

function refreshHistory(grant: AccessToken) {
  return db.from('oauth_refresh_token_history').where('access_token_id', grant.id)
}

/**
 * Runs `action` just before the next transaction opens. The token endpoint
 * reads a grant or a code, then writes it inside a transaction: this is where
 * a concurrent request gets its turn. Returns a function that undoes the hook.
 */
function beforeNextTransaction(action: () => Promise<unknown>) {
  const transaction = db.transaction
  const restore = () => {
    db.transaction = transaction
  }

  db.transaction = (async (...args: unknown[]) => {
    restore()
    await action()
    return Reflect.apply(transaction, db, args)
  }) as typeof db.transaction

  return restore
}

test.group('changed rows', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('counts the rows a write changed with either query builder', async ({ assert }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id)
    const [first, second] = [await createMcpCallLog(token), await createMcpCallLog(token)]
    await createMcpCallLog(token)
    await createMcpCallLog(token)
    const logs = () => McpCallLog.query().where('access_token_id', token.id)
    const rawLogs = () => db.from('mcp_call_logs').where('access_token_id', token.id)

    assert.equal(changedRows(await logs().update({ durationMs: 1 })), 4)
    assert.equal(changedRows(await logs().where('id', -1).update({ durationMs: 2 })), 0)
    assert.equal(changedRows(await rawLogs().update({ duration_ms: 3 })), 4)
    assert.equal(changedRows(await rawLogs().where('id', -1).update({ duration_ms: 4 })), 0)

    assert.equal(changedRows(await logs().whereIn('id', [first.id, second.id]).delete()), 2)
    assert.equal(changedRows(await logs().whereIn('id', [first.id, second.id]).delete()), 0)
    assert.equal(changedRows(await rawLogs().delete()), 2)
    assert.equal(changedRows(await rawLogs().delete()), 0)
  })

  test('treats a refresh that loses a concurrent rotation as a replay', async ({
    client,
    assert,
    cleanup,
  }) => {
    const { clientId, refreshToken } = await connect(client, 'Racing refresh client')
    let winner!: Awaited<ReturnType<typeof refresh>>
    cleanup(
      beforeNextTransaction(async () => {
        winner = await refresh(client, clientId, refreshToken)
      })
    )

    const loser = await refresh(client, clientId, refreshToken)

    winner.assertStatus(200)
    loser.assertStatus(400)
    assert.equal(loser.body().error, 'invalid_grant')

    const grant = await AccessToken.findByOrFail('name', 'Racing refresh client')
    assert.isTrue(grant.isRevoked)
    assert.lengthOf(await refreshHistory(grant), 1)
  })

  test('issues no tokens when the grant is revoked while its refresh token rotates', async ({
    client,
    assert,
    cleanup,
  }) => {
    const { clientId, refreshToken } = await connect(client, 'Revoked refresh client')
    const before = await AccessToken.findByOrFail('name', 'Revoked refresh client')
    cleanup(beforeNextTransaction(() => AccessTokenService.revoke(before)))

    const response = await refresh(client, clientId, refreshToken)

    response.assertStatus(400)
    assert.equal(response.body().error, 'invalid_grant')
    assert.notProperty(response.body(), 'access_token')

    const grant = await AccessToken.findOrFail(before.id)
    assert.equal(grant.tokenHash, before.tokenHash)
    assert.equal(grant.oauthRefreshTokenHash, AccessTokenService.hash(refreshToken))
    assert.lengthOf(await refreshHistory(grant), 0)
  })

  test('issues one grant when an authorization code is exchanged twice at once', async ({
    client,
    assert,
    cleanup,
  }) => {
    const clientId = await registerClient(client, 'Racing code client')
    const code = await authorizationCode(client, clientId)
    let winner!: Awaited<ReturnType<typeof exchangeCode>>
    cleanup(
      beforeNextTransaction(async () => {
        winner = await exchangeCode(client, clientId, code)
      })
    )

    const loser = await exchangeCode(client, clientId, code)

    winner.assertStatus(200)
    loser.assertStatus(400)
    assert.equal(loser.body().error, 'invalid_grant')
    assert.notProperty(loser.body(), 'access_token')
    assert.lengthOf(await AccessToken.query().where('name', 'Racing code client'), 1)
  })

  test('reports how many expired call logs were pruned', async ({ assert }) => {
    const admin = await createAdmin()
    const token = await createStoredAccessToken(admin.id)
    const expired = DateTime.utc().minus({ days: 60 })
    await createMcpCallLog(token, { createdAt: expired })
    await createMcpCallLog(token, { createdAt: expired })
    await createMcpCallLog(token, { createdAt: expired })
    await createMcpCallLog(token)

    assert.equal(await McpCallLogService.pruneExpired({ force: true }), 3)
    assert.equal(await McpCallLogService.pruneExpired({ force: true }), 0)
    assert.lengthOf(await McpCallLog.all(), 1)
  })
})
