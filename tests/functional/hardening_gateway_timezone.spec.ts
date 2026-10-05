import { readFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import app from '@adonisjs/core/services/app'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import OauthAuthorizationCode from '#models/oauth_authorization_code'
import OauthClient from '#models/oauth_client'
import AccessTokenService from '#services/access_token_service'
import { createAuthorizationCode } from '#services/gateway_oauth'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

const resource = 'http://localhost:3333/mcp'

/**
 * Run this file with a hostile zone to see what it guards against:
 * `TZ=Europe/Paris NODE_ENV=test node ace test functional --files=hardening_gateway_timezone`
 */
test.group('hardening: process time zone', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('pins every entrypoint to UTC before the application loads', async ({ assert }) => {
    for (const entrypoint of ['bin/server.ts', 'bin/console.ts', 'bin/test.ts']) {
      const source = await readFile(app.makePath(entrypoint), 'utf8')
      const pin = source.indexOf("process.env.TZ = 'UTC'")

      assert.isAbove(pin, -1, `${entrypoint} does not pin the time zone`)
      assert.isBelow(pin, source.indexOf('Ignitor'), `${entrypoint} pins the time zone too late`)
    }

    assert.equal(process.env.TZ, 'UTC')
    assert.equal(new Date().getTimezoneOffset(), 0)
    assert.equal(DateTime.local().offset, 0)
  })

  test('reads token and code expiries back from the database unchanged', async ({ assert }) => {
    const admin = await createAdmin()
    const client = await OauthClient.create({
      clientId: 'mcp_client_timezone',
      clientName: 'Time zone client',
      redirectUris: JSON.stringify(['http://127.0.0.1/callback']),
      tokenEndpointAuthMethod: 'none',
      grantTypes: JSON.stringify(['authorization_code', 'refresh_token']),
      responseTypes: JSON.stringify(['code']),
      scope: 'mcp:tools',
    })

    const issuedAt = DateTime.utc()
    const created = await AccessTokenService.createOauthGrant({
      name: client.clientName,
      clientId: client.id,
      clientSupportsRefresh: true,
      scopes: 'mcp:tools',
      resource,
      createdBy: admin.id,
    })

    const token = await AccessToken.findOrFail(created.token.id)
    assert.isTrue(token.isUsable)
    assert.isNotNull(await AccessTokenService.findUsableByPlaintext(created.plaintext))
    assert.closeTo(token.expiresAt!.diff(issuedAt).as('minutes'), 60, 1)
    assert.closeTo(token.oauthRefreshExpiresAt!.diff(issuedAt).as('days'), 30, 0.01)
    assert.closeTo(token.createdAt.diff(issuedAt).as('minutes'), 0, 1)

    const plaintext = await createAuthorizationCode(
      {
        client,
        redirectUri: 'http://127.0.0.1/callback',
        state: null,
        codeChallenge: 'c'.repeat(43),
        scopes: 'mcp:tools',
        resource,
      },
      admin.id
    )
    const code = await OauthAuthorizationCode.findByOrFail(
      'code_hash',
      AccessTokenService.hash(plaintext)
    )
    assert.closeTo(code.expiresAt.diff(issuedAt).as('minutes'), 5, 1)
  })
})
