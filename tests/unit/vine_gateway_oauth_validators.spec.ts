import { test } from '@japa/runner'
import type { Assert } from '@japa/assert'
import {
  authorizationClientIdValidator,
  authorizationCodeGrantValidator,
  authorizationRedirectUriValidator,
  authorizationResponseTypeValidator,
  authorizationStateLengthValidator,
  authorizationStateValidator,
  clientAuthMethodValidator,
  clientGrantTypesValidator,
  clientNameValidator,
  clientRedirectUrisValidator,
  clientResponseTypesValidator,
  consentApprovalValidator,
  gatewayResourceValidator,
  pkceChallengeValidator,
  pkceVerifierValidator,
  postedClientCredentialsValidator,
  refreshScopeValidator,
  refreshTokenGrantValidator,
  requestedScopeValidator,
  revocationRequestValidator,
  tokenRequestValidator,
} from '#validators/gateway_oauth'

type Validator = {
  tryValidate(data: unknown, options?: any): Promise<[{ messages: any[] } | null, unknown]>
}

/** Values that stand where a string is expected without being one. */
const notStrings = [undefined, null, 0, 1, true, ['a'], ['a', 'b'], [], {}, { a: 'b' }]

async function accepted(assert: Assert, validator: Validator, value: unknown, options?: object) {
  const [error, data] = await validator.tryValidate(value, options)
  assert.isNull(error, `expected ${JSON.stringify(value)} to be accepted`)
  return data
}

async function refused(assert: Assert, validator: Validator, value: unknown, options?: object) {
  const [error] = await validator.tryValidate(value, options)
  assert.isNotNull(error, `expected ${JSON.stringify(value)} to be refused`)
  return error!.messages
}

test.group('vine: gateway OAuth client registration', () => {
  test('accepts HTTPS, loopback HTTP and the approved native callback as redirect URIs', async ({
    assert,
  }) => {
    for (const uris of [
      ['https://client.example/callback'],
      ['https://client.example:8443/callback?tenant=1'],
      ['HTTPS://CLIENT.example/callback'],
      ['http://localhost/callback'],
      ['http://localhost:8080/callback'],
      ['http://127.0.0.1:49152/callback'],
      ['http://[::1]/callback'],
      ['cursor://anysphere.cursor-mcp/oauth/callback'],
      ['https://client.example/a', 'http://127.0.0.1/b'],
      Array.from({ length: 10 }, (_, index) => `https://client.example/${index}`),
      [`https://client.example/${'a'.repeat(2048 - 'https://client.example/'.length)}`],
    ]) {
      assert.deepEqual(await accepted(assert, clientRedirectUrisValidator, uris), uris)
    }
  })

  test('refuses unsafe, malformed, oversized and miscounted redirect URIs', async ({ assert }) => {
    for (const uris of [
      [],
      Array.from({ length: 11 }, (_, index) => `https://client.example/${index}`),
      ['http://example.com/callback'],
      ['http://127.0.0.2/callback'],
      ['http://localhost./callback'],
      ['http://sub.localhost/callback'],
      ['https://client.example/callback#fragment'],
      ['https://user:password@client.example/callback'],
      ['https://user@client.example/callback'],
      ['cursor://anysphere.cursor-mcp/oauth/callback/'],
      ['cursor://anysphere.cursor-mcp/oauth/callback?x=1'],
      ['CURSOR://anysphere.cursor-mcp/oauth/callback'],
      ['cursor://attacker.example/oauth/callback'],
      ['vscode://vendor.extension/callback'],
      ['ftp://client.example/callback'],
      ['javascript:alert(1)'],
      ['client.example/callback'],
      ['/callback'],
      [''],
      [' '],
      [`https://client.example/${'a'.repeat(2049 - 'https://client.example/'.length)}`],
      ['https://client.example/callback', 'http://example.com/callback'],
      [1],
      [null],
      [['https://client.example/callback']],
      'https://client.example/callback',
      { 0: 'https://client.example/callback' },
      undefined,
      null,
    ]) {
      await refused(assert, clientRedirectUrisValidator, uris)
    }
  })

  test('accepts the three client authentication methods and defaults to client_secret_basic', async ({
    assert,
  }) => {
    for (const method of ['none', 'client_secret_post', 'client_secret_basic']) {
      assert.equal(await accepted(assert, clientAuthMethodValidator, method), method)
    }
    assert.equal(
      await accepted(assert, clientAuthMethodValidator, undefined),
      'client_secret_basic'
    )

    for (const method of [
      '',
      ' ',
      ' none',
      'none ',
      'NONE',
      'private_key_jwt',
      1,
      true,
      ['none'],
    ]) {
      await refused(assert, clientAuthMethodValidator, method)
    }
  })

  test('reduces grant types to their distinct values and requires the authorization code', async ({
    assert,
  }) => {
    assert.deepEqual(await accepted(assert, clientGrantTypesValidator, undefined), [
      'authorization_code',
      'refresh_token',
    ])
    assert.deepEqual(await accepted(assert, clientGrantTypesValidator, ['authorization_code']), [
      'authorization_code',
    ])
    assert.deepEqual(
      await accepted(assert, clientGrantTypesValidator, [
        'refresh_token',
        'authorization_code',
        'refresh_token',
        'authorization_code',
      ]),
      ['refresh_token', 'authorization_code']
    )
    assert.deepEqual(
      await accepted(assert, clientGrantTypesValidator, [
        ...Array.from({ length: 5000 }, () => 'authorization_code'),
        ...Array.from({ length: 5000 }, () => 'refresh_token'),
      ]),
      ['authorization_code', 'refresh_token']
    )

    for (const grantTypes of [
      [],
      ['refresh_token'],
      ['refresh_token', 'refresh_token'],
      ['authorization_code', 'implicit'],
      ['authorization_code', ''],
      ['Authorization_code'],
      ['client_credentials'],
      [1],
      [null],
      'authorization_code',
      {},
    ]) {
      await refused(assert, clientGrantTypesValidator, grantTypes)
    }
  })

  test('bounds type lists before looking at their members', async ({ assert }) => {
    const unknown = Array.from({ length: 5000 }, (_, index) => `type-${index}`)

    assert.lengthOf(await refused(assert, clientGrantTypesValidator, unknown), 1)
    assert.lengthOf(await refused(assert, clientResponseTypesValidator, unknown), 1)
  })

  test('accepts the code response type only, however often it is repeated', async ({ assert }) => {
    assert.deepEqual(await accepted(assert, clientResponseTypesValidator, undefined), ['code'])
    assert.deepEqual(await accepted(assert, clientResponseTypesValidator, ['code']), ['code'])
    assert.deepEqual(await accepted(assert, clientResponseTypesValidator, ['code', 'code']), [
      'code',
    ])

    for (const responseTypes of [
      [],
      ['token'],
      ['code', 'token'],
      ['token', 'code'],
      ['Code'],
      ['code '],
      [''],
      [1],
      'code',
      {},
    ]) {
      await refused(assert, clientResponseTypesValidator, responseTypes)
    }
  })

  test('accepts a scope list that names the gateway scope and nothing else', async ({ assert }) => {
    for (const scope of ['mcp:tools', ' mcp:tools', 'mcp:tools ', '   mcp:tools   ']) {
      await accepted(assert, requestedScopeValidator, scope)
    }
    // Left out, or sent in a shape that is not a scope string at all.
    for (const scope of notStrings) {
      await accepted(assert, requestedScopeValidator, scope)
    }

    for (const scope of [
      '',
      ' ',
      '   ',
      'mcp:tools mcp:tools',
      'mcp:tools other',
      'other mcp:tools',
      'other',
      'MCP:TOOLS',
      'mcp:toolsx',
      'mcp: tools',
      'mcp:tools\t',
      '\tmcp:tools',
      'mcp:tools\n',
      ' mcp:tools',
    ]) {
      await refused(assert, requestedScopeValidator, scope)
    }
  })

  test('trims a client name and limits what is left to 120 characters', async ({ assert }) => {
    assert.equal(
      await accepted(assert, clientNameValidator, '  Claude Desktop  '),
      'Claude Desktop'
    )
    assert.equal(await accepted(assert, clientNameValidator, 'x'.repeat(120)), 'x'.repeat(120))
    assert.equal(
      await accepted(assert, clientNameValidator, ` ${'x'.repeat(120)} `),
      'x'.repeat(120)
    )
    // Nothing is left of these: the caller falls back to its default name.
    assert.equal(await accepted(assert, clientNameValidator, ''), '')
    assert.equal(await accepted(assert, clientNameValidator, ' \t\n'), '')
    assert.isUndefined(await accepted(assert, clientNameValidator, undefined))

    for (const name of ['x'.repeat(121), ` ${'x'.repeat(121)} `, 1, true, ['name'], {}]) {
      await refused(assert, clientNameValidator, name)
    }
  })
})

test.group('vine: gateway OAuth authorization request', () => {
  test('takes any non-empty string as a client id', async ({ assert }) => {
    for (const clientId of ['mcp_client_abc', ' ', ' padded ', '0']) {
      assert.equal(await accepted(assert, authorizationClientIdValidator, clientId), clientId)
    }
    for (const clientId of ['', ...notStrings]) {
      await refused(assert, authorizationClientIdValidator, clientId)
    }
  })

  test('matches a redirect URI exactly, apart from the port of a loopback URI', async ({
    assert,
  }) => {
    const loopback = { meta: { registeredRedirectUris: ['http://127.0.0.1/callback?x=1'] } }
    for (const uri of [
      'http://127.0.0.1/callback?x=1',
      'http://127.0.0.1:49152/callback?x=1',
      'http://127.0.0.1:1/callback?x=1',
      'HTTP://127.0.0.1:49152/callback?x=1',
    ]) {
      assert.equal(await accepted(assert, authorizationRedirectUriValidator, uri, loopback), uri)
    }
    for (const uri of [
      'http://127.0.0.1:49152/callback',
      'http://127.0.0.1:49152/callback?x=2',
      'http://127.0.0.1:49152/callback/?x=1',
      'http://127.0.0.1:49152/callback?x=1#fragment',
      'https://127.0.0.1:49152/callback?x=1',
      'http://localhost:49152/callback?x=1',
      'http://[::1]:49152/callback?x=1',
      'not a url',
      '',
      ' ',
      ...notStrings,
    ]) {
      await refused(assert, authorizationRedirectUriValidator, uri, loopback)
    }

    const remote = {
      meta: {
        registeredRedirectUris: [
          'https://client.example/callback',
          'cursor://anysphere.cursor-mcp/oauth/callback',
        ],
      },
    }
    await accepted(
      assert,
      authorizationRedirectUriValidator,
      'https://client.example/callback',
      remote
    )
    await accepted(
      assert,
      authorizationRedirectUriValidator,
      'cursor://anysphere.cursor-mcp/oauth/callback',
      remote
    )
    for (const uri of [
      'https://client.example:8443/callback',
      'https://client.example:443/callback',
      'https://CLIENT.example/callback',
      'https://client.example/callback/',
      'https://client.example/callback?x=1',
      'http://client.example/callback',
      'cursor://anysphere.cursor-mcp/oauth/callback/',
    ]) {
      await refused(assert, authorizationRedirectUriValidator, uri, remote)
    }
  })

  test('never matches an empty redirect URI, even against a stored empty one', async ({
    assert,
  }) => {
    await refused(assert, authorizationRedirectUriValidator, '', {
      meta: { registeredRedirectUris: [''] },
    })
    await refused(assert, authorizationRedirectUriValidator, 'https://client.example/callback', {
      meta: { registeredRedirectUris: [] },
    })
  })

  test('reads a state as sent and judges its length separately', async ({ assert }) => {
    for (const state of ['state-from-client', '', ' ', 's'.repeat(5000)]) {
      assert.equal(await accepted(assert, authorizationStateValidator, state), state)
    }
    assert.isUndefined(await accepted(assert, authorizationStateValidator, undefined))
    assert.isUndefined(await accepted(assert, authorizationStateValidator, null))
    for (const state of [0, 1, true, ['a'], {}]) {
      await refused(assert, authorizationStateValidator, state)
    }

    for (const state of [null, '', ' ', 'state', 's'.repeat(2048)]) {
      await accepted(assert, authorizationStateLengthValidator, state)
    }
    await refused(assert, authorizationStateLengthValidator, 's'.repeat(2049))
  })

  test('accepts the code response type alone', async ({ assert }) => {
    await accepted(assert, authorizationResponseTypeValidator, 'code')
    for (const responseType of [
      '',
      'token',
      'CODE',
      'code ',
      ' code',
      'code token',
      ...notStrings,
    ]) {
      await refused(assert, authorizationResponseTypeValidator, responseType)
    }
  })

  test('requires a base64url S256 challenge of 43 to 128 characters', async ({ assert }) => {
    const pkce = (challenge: unknown, method: unknown) => ({
      code_challenge: challenge,
      code_challenge_method: method,
    })
    for (const challenge of ['a'.repeat(43), 'a'.repeat(128), `${'A-z_09'.repeat(8)}`]) {
      assert.deepEqual(await accepted(assert, pkceChallengeValidator, pkce(challenge, 'S256')), {
        code_challenge: challenge,
        code_challenge_method: 'S256',
      })
    }
    for (const challenge of [
      'a'.repeat(42),
      'a'.repeat(129),
      `${'a'.repeat(42)}.`,
      `${'a'.repeat(42)}~`,
      `${'a'.repeat(43)}=`,
      `${'a'.repeat(43)}\n`,
      `${'a'.repeat(21)} ${'a'.repeat(21)}`,
      '',
      ...notStrings,
    ]) {
      await refused(assert, pkceChallengeValidator, pkce(challenge, 'S256'))
    }
    for (const method of ['plain', 's256', 'S256 ', ' S256', '', ...notStrings]) {
      await refused(assert, pkceChallengeValidator, pkce('a'.repeat(43), method))
    }
    await refused(assert, pkceChallengeValidator, {})
  })

  test('accepts the gateway resource in any equivalent spelling of its URL', async ({ assert }) => {
    const gateway = { meta: { gatewayResource: 'https://mcp.example.com/mcp' } }
    for (const resource of [
      'https://mcp.example.com/mcp',
      'https://MCP.example.com/mcp',
      'HTTPS://mcp.example.com/mcp',
      'https://mcp.example.com:443/mcp',
      'https://mcp.example.com/a/../mcp',
      ' https://mcp.example.com/mcp ',
    ]) {
      assert.equal(await accepted(assert, gatewayResourceValidator, resource, gateway), resource)
    }
    for (const resource of [
      'https://mcp.example.com/mcp/',
      'https://mcp.example.com/mcp?',
      'https://mcp.example.com/mcp?x=1',
      'https://mcp.example.com/mcp#',
      'https://mcp.example.com/mcp#fragment',
      'https://mcp.example.com/%6dcp',
      'https://mcp.example.com',
      'https://mcp.example.com:8443/mcp',
      'http://mcp.example.com/mcp',
      'https://user@mcp.example.com/mcp',
      'https://other.example.com/mcp',
      '/mcp',
      'not a url',
      '',
      ' ',
      ...notStrings,
    ]) {
      await refused(assert, gatewayResourceValidator, resource, gateway)
    }
  })

  test('takes nothing but an explicit approval as consent', async ({ assert }) => {
    await accepted(assert, consentApprovalValidator, 'approve')
    for (const decision of ['deny', '', 'APPROVE', ' approve', 'approve ', 'true', ...notStrings]) {
      await refused(assert, consentApprovalValidator, decision)
    }
    await refused(assert, consentApprovalValidator, ['approve'])
  })
})

test.group('vine: gateway OAuth token and revocation requests', () => {
  test('reads posted client credentials and ignores a secret that is not a string', async ({
    assert,
  }) => {
    assert.deepEqual(
      await accepted(assert, postedClientCredentialsValidator, {
        client_id: 'mcp_client_abc',
        client_secret: 'mcp_secret_abc',
        grant_type: 'refresh_token',
      }),
      { client_id: 'mcp_client_abc', client_secret: 'mcp_secret_abc' }
    )
    assert.deepEqual(
      await accepted(assert, postedClientCredentialsValidator, { client_id: 'mcp_client_abc' }),
      { client_id: 'mcp_client_abc' }
    )
    // An empty secret is kept as sent; the caller treats it as no secret.
    assert.deepEqual(
      await accepted(assert, postedClientCredentialsValidator, {
        client_id: ' ',
        client_secret: '',
      }),
      { client_id: ' ', client_secret: '' }
    )
    for (const secret of notStrings) {
      assert.deepEqual(
        await accepted(assert, postedClientCredentialsValidator, {
          client_id: 'mcp_client_abc',
          client_secret: secret,
        }),
        { client_id: 'mcp_client_abc' }
      )
    }

    for (const clientId of ['', ...notStrings]) {
      await refused(assert, postedClientCredentialsValidator, {
        client_id: clientId,
        client_secret: 'mcp_secret_abc',
      })
    }
  })

  test('names the first missing parameter of a token or revocation request', async ({ assert }) => {
    const firstMissing = async (validator: Validator, input: Record<string, unknown>) => {
      const [first] = await refused(assert, validator, input)
      return first.field
    }

    assert.equal(await firstMissing(tokenRequestValidator, {}), 'grant_type')
    assert.equal(await firstMissing(revocationRequestValidator, {}), 'token')

    const code = { code: 'c', code_verifier: 'v', redirect_uri: 'r', resource: 'x' }
    assert.deepEqual(await accepted(assert, authorizationCodeGrantValidator, code), code)
    assert.equal(await firstMissing(authorizationCodeGrantValidator, {}), 'code')
    for (const field of ['code', 'code_verifier', 'redirect_uri', 'resource'] as const) {
      for (const value of ['', ...notStrings]) {
        assert.equal(
          await firstMissing(authorizationCodeGrantValidator, { ...code, [field]: value }),
          field
        )
      }
    }
    assert.equal(
      await firstMissing(authorizationCodeGrantValidator, {
        ...code,
        code_verifier: '',
        resource: [],
      }),
      'code_verifier'
    )

    const refresh = { refresh_token: 't', resource: 'x' }
    assert.deepEqual(await accepted(assert, refreshTokenGrantValidator, refresh), refresh)
    assert.equal(await firstMissing(refreshTokenGrantValidator, {}), 'refresh_token')
    assert.equal(await firstMissing(refreshTokenGrantValidator, { refresh_token: 't' }), 'resource')
    assert.equal(
      await firstMissing(refreshTokenGrantValidator, { resource: 'x', refresh_token: ['t'] }),
      'refresh_token'
    )
  })

  test('accepts blank parameters, which are for the grant itself to refuse', async ({ assert }) => {
    const blank = { code: ' ', code_verifier: ' ', redirect_uri: ' ', resource: ' ' }
    assert.deepEqual(await accepted(assert, authorizationCodeGrantValidator, blank), blank)
    assert.deepEqual(await accepted(assert, revocationRequestValidator, { token: ' ' }), {
      token: ' ',
    })
  })

  test('keeps the scope of a refresh request only when it is a string', async ({ assert }) => {
    const refresh = { refresh_token: 't', resource: 'x' }
    for (const scope of ['mcp:tools', 'other', '', ' ']) {
      assert.deepEqual(await accepted(assert, refreshTokenGrantValidator, { ...refresh, scope }), {
        ...refresh,
        scope,
      })
    }
    for (const scope of notStrings) {
      assert.deepEqual(
        await accepted(assert, refreshTokenGrantValidator, { ...refresh, scope }),
        refresh
      )
    }
  })

  test('lets a refresh request repeat the gateway scope and ask for no other', async ({
    assert,
  }) => {
    await accepted(assert, refreshScopeValidator, null)
    await accepted(assert, refreshScopeValidator, 'mcp:tools')
    for (const scope of ['', ' ', ' mcp:tools', 'mcp:tools ', 'mcp:tools mcp:tools', 'other']) {
      await refused(assert, refreshScopeValidator, scope)
    }
  })

  test('requires a code verifier of 43 to 128 unreserved characters', async ({ assert }) => {
    for (const verifier of ['a'.repeat(43), 'a'.repeat(128), `${'aZ09-._~'.repeat(6)}`]) {
      assert.equal(await accepted(assert, pkceVerifierValidator, verifier), verifier)
    }
    for (const verifier of [
      'a'.repeat(42),
      'a'.repeat(129),
      `${'a'.repeat(43)}!`,
      `${'a'.repeat(43)} `,
      ` ${'a'.repeat(43)}`,
      `${'a'.repeat(43)}\n`,
      `${'a'.repeat(42)}é`,
      '',
      ...notStrings,
    ]) {
      await refused(assert, pkceVerifierValidator, verifier)
    }
  })
})
