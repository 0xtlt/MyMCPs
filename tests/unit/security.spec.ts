import { test } from '@japa/runner'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import { assignMcpFromPayload } from '#controllers/mcps_controller'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { sanitizeDiagnostic, sanitizeMcpDiagnostic } from '#services/security_redaction'
import { fetchWithoutRedirects } from '#services/upstream/safe_fetch'
import { connectHttpUpstream } from '#services/upstream/http_client'
import { createDenoStartupError } from '#services/upstream/deno_runner'
import { resolveCorsOrigin } from '#config/cors'
import { createMcpValidator } from '#validators/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

async function httpPayload(
  overrides: Partial<{
    httpUrl: string
    authType: 'auto' | 'bearer' | 'header'
    authBearer: string
    authHeaderName: string
    authHeaderValue: string
  }> = {}
) {
  return createMcpValidator.validate({
    name: 'Secure HTTP MCP',
    description: '',
    transport: 'http',
    httpUrl: overrides.httpUrl ?? 'https://old.example/mcp',
    npmPackage: '',
    npmVersion: '',
    npmArgs: '',
    authType: overrides.authType ?? 'auto',
    authBearer: overrides.authBearer ?? '',
    authHeaderName: overrides.authHeaderName ?? '',
    authHeaderValue: overrides.authHeaderValue ?? '',
    enabled: true,
  })
}

async function npmPayload(options: {
  npmPackage: string
  npmVersion?: string
  npmArgs?: string
  npmEnv: Array<{ name: string; value: string | null }>
}) {
  return createMcpValidator.validate({
    name: 'Secure npm MCP',
    description: '',
    transport: 'npm',
    httpUrl: '',
    npmPackage: options.npmPackage,
    npmVersion: options.npmVersion ?? '1.0.0',
    npmArgs: options.npmArgs ?? '',
    npmEnv: options.npmEnv,
    authType: 'auto',
    enabled: true,
  })
}

test.group('security boundaries', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('rejects non-HTTP, credential-bearing, and fragmented MCP URLs', async ({ assert }) => {
    const admin = await createAdmin()

    for (const httpUrl of [
      'file:///tmp/socket',
      'https://user:password@example.test/mcp',
      'https://example.test/mcp#secret',
      'https://example.test/mcp?api_key=plaintext-secret',
      'https://example.test/mcp?access-token=plaintext-secret',
      'https://example.test/mcp?key=plaintext-secret',
      'https://example.test/mcp?Ocp-Apim-Subscription-Key=plaintext-secret',
      'https://example.test/mcp?X-Amz-Credential=plaintext-secret',
      'https://example.test/mcp?X-Amz-Signature=plaintext-secret',
      'https://example.test/mcp?AWSAccessKeyId=plaintext-secret',
      'https://example.test/mcp?X-Goog-Credential=plaintext-secret',
      'https://example.test/mcp?private_token=plaintext-secret',
      'https://example.test/mcp?private_key=plaintext-secret',
      'https://example.test/mcp?session-key=plaintext-secret',
      'https://example.test/mcp?session_id=plaintext-secret',
      'https://example.test/mcp?jsessionid=plaintext-secret',
      'https://example.test/mcp?sid=plaintext-secret',
      'https://example.test/mcp?csrf_token=plaintext-secret',
      'https://example.test/mcp?XSRF-TOKEN=plaintext-secret',
      'https://example.test/mcp?aws_secret_access_key=plaintext-secret',
      'https://example.test/mcp?google_access_id=plaintext-secret',
      'https://example.test/mcp?oauth_token=plaintext-secret',
      'https://example.test/mcp?safe=value;auth=plaintext-secret',
    ]) {
      const mcp = new Mcp()
      mcp.status = 'draft'
      mcp.createdBy = admin.id
      await assert.rejects(
        async () => assignMcpFromPayload(mcp, await httpPayload({ httpUrl })),
        errors.E_VALIDATION_ERROR
      )
    }
  })

  test('allows production CORS only on the exact bearer gateway path', ({ assert }) => {
    const origin = 'https://agent.example'

    assert.equal(resolveCorsOrigin(origin, '/mcp', false), origin)
    assert.equal(resolveCorsOrigin(origin, '/mcp?session=one', false), origin)
    assert.deepEqual(resolveCorsOrigin(origin, '/mcps', false), [])
    assert.deepEqual(resolveCorsOrigin(origin, '/mcps/oauth/callback', false), [])
  })

  test('requires bearer re-entry before changing its destination', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      authType: 'bearer',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authBearer = McpSecretStore.encrypt('old-secret')
    await mcp.save()

    await assert.rejects(
      async () =>
        assignMcpFromPayload(
          mcp,
          await httpPayload({ authType: 'bearer', httpUrl: 'https://new.example/mcp' }),
          { excludeId: mcp.id }
        ),
      errors.E_VALIDATION_ERROR
    )
    assert.equal(mcp.httpUrl, 'https://old.example/mcp')
    assert.equal(McpSecretStore.decrypt(mcp.authBearer), 'old-secret')

    await assignMcpFromPayload(
      mcp,
      await httpPayload({
        authType: 'bearer',
        authBearer: 'new-secret',
        httpUrl: 'https://new.example/mcp',
      }),
      { excludeId: mcp.id }
    )
    assert.equal(mcp.httpUrl, 'https://new.example/mcp')
    assert.equal(McpSecretStore.decrypt(mcp.authBearer), 'new-secret')
  })

  test('requires environment re-entry before changing an npm package', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/old-mcp'
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'API_KEY', value: 'old-secret' }])
    await mcp.save()

    await assert.rejects(
      async () =>
        assignMcpFromPayload(
          mcp,
          await npmPayload({
            npmPackage: '@example/new-mcp',
            npmEnv: [{ name: 'API_KEY', value: null }],
          }),
          { excludeId: mcp.id }
        ),
      errors.E_VALIDATION_ERROR
    )

    await mcp.refresh()
    await assignMcpFromPayload(
      mcp,
      await npmPayload({
        npmPackage: '@example/new-mcp',
        npmEnv: [{ name: 'API_KEY', value: 'new-secret' }],
      }),
      { excludeId: mcp.id }
    )
    assert.deepEqual(mcp.npmEnvironment, { API_KEY: 'new-secret' })
  })

  test('requires environment re-entry before changing an npm package version', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/versioned-mcp'
    mcp.npmVersion = '1.0.0'
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'CUSTOM_VALUE', value: 'old-secret' }])
    await mcp.save()

    await assert.rejects(
      async () =>
        assignMcpFromPayload(
          mcp,
          await npmPayload({
            npmPackage: '@example/versioned-mcp',
            npmVersion: '2.0.0',
            npmEnv: [{ name: 'CUSTOM_VALUE', value: null }],
          }),
          { excludeId: mcp.id }
        ),
      errors.E_VALIDATION_ERROR
    )

    await mcp.refresh()
    await assignMcpFromPayload(
      mcp,
      await npmPayload({
        npmPackage: '@example/versioned-mcp',
        npmVersion: '2.0.0',
        npmEnv: [{ name: 'CUSTOM_VALUE', value: 'new-secret' }],
      }),
      { excludeId: mcp.id }
    )
    assert.equal(mcp.npmVersion, '2.0.0')
    assert.deepEqual(mcp.npmEnvironment, { CUSTOM_VALUE: 'new-secret' })
  })

  test('requires environment re-entry before changing npm process arguments', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/argument-mcp'
    mcp.npmVersion = '1.0.0'
    mcp.npmArgsList = ['--tenant', 'old']
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'CUSTOM_VALUE', value: 'old-secret' }])
    await mcp.save()

    await assert.rejects(
      async () =>
        assignMcpFromPayload(
          mcp,
          await npmPayload({
            npmPackage: '@example/argument-mcp',
            npmArgs: '--tenant new',
            npmEnv: [{ name: 'CUSTOM_VALUE', value: null }],
          }),
          { excludeId: mcp.id }
        ),
      errors.E_VALIDATION_ERROR
    )
  })

  test('blocks redirects before authenticated requests can be forwarded', async ({ assert }) => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = async (_input, init) => {
      assert.equal(init?.redirect, 'manual')
      return new Response(null, {
        status: 307,
        headers: { Location: 'https://attacker.example/collect' },
      })
    }

    try {
      await assert.rejects(
        () => fetchWithoutRedirects('https://trusted.example/mcp', {}, 'MCP endpoint'),
        'MCP endpoint redirected the request. Configure the final URL directly.'
      )
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('blocks persisted legacy credential URLs at the runtime boundary', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      httpUrl: 'https://legacy.example/mcp?private_token=plaintext-secret',
    })
    const originalFetch = globalThis.fetch
    let fetched = false
    globalThis.fetch = async () => {
      fetched = true
      return new Response(null, { status: 200 })
    }

    try {
      await assert.rejects(
        () => connectHttpUpstream(mcp),
        'MCP URL must not include credential query parameters'
      )
      assert.isFalse(fetched)
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('redacts long npm environment secrets before truncating startup errors', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    const secret = `opaque-${'x'.repeat(400)}-tail`
    mcp.npmPackage = '@example/failing-mcp'
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'OPAQUE_VALUE', value: secret }])

    const error = createDenoStartupError(mcp, new Error(`startup echoed ${secret}`))

    assert.include(error.message, '[REDACTED]')
    assert.notInclude(error.message, secret)
    assert.notInclude(error.message, secret.slice(0, 300))
  })

  test('redacts structured and header-shaped credentials from diagnostics', ({ assert }) => {
    const stringifiedCredential = JSON.stringify(
      JSON.stringify({ client_secret: 'stringified-secret' })
    )
    const sanitized = sanitizeDiagnostic(
      `Authorization: Bearer abc123 {"client_secret":"oauth-secret","access_token":"access-secret","subscription-key":"azure-secret","secret_key":"part\\\"rest","private_key":"private-secret"} https://example.test?X-Amz-Signature=aws-secret private_token=form-secret session-key=session-secret google_access_id=google-secret Response: {"client_secret":"nested-json"} error: access_token=nested-form Cookie: session_token=nested-cookie Set-Cookie: sessionid=session-cookie; sid=short-session; csrf_token=csrf-secret Stringified: ${stringifiedCredential} access_token=adjacent-one session_token=adjacent-two`
    )!
    assert.notInclude(sanitized, 'abc123')
    assert.notInclude(sanitized, 'oauth-secret')
    assert.notInclude(sanitized, 'access-secret')
    assert.notInclude(sanitized, 'azure-secret')
    assert.notInclude(sanitized, 'aws-secret')
    assert.notInclude(sanitized, 'form-secret')
    assert.notInclude(sanitized, 'part')
    assert.notInclude(sanitized, 'rest')
    assert.notInclude(sanitized, 'private-secret')
    assert.notInclude(sanitized, 'session-secret')
    assert.notInclude(sanitized, 'google-secret')
    assert.notInclude(sanitized, 'nested-json')
    assert.notInclude(sanitized, 'nested-form')
    assert.notInclude(sanitized, 'nested-cookie')
    assert.notInclude(sanitized, 'session-cookie')
    assert.notInclude(sanitized, 'short-session')
    assert.notInclude(sanitized, 'csrf-secret')
    assert.notInclude(sanitized, 'stringified-secret')
    assert.notInclude(sanitized, 'adjacent-one')
    assert.notInclude(sanitized, 'adjacent-two')
    assert.include(sanitized, '[REDACTED]')
  })

  test('redacts exact custom header and npm environment values', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    const headerSecret = 'p"ass word'
    const environmentSecret = 'line\\break\nnext value'
    mcp.authHeaderValue = McpSecretStore.encrypt(headerSecret)
    mcp.npmEnv = McpEnvironmentStore.merge(null, [
      { name: 'CUSTOM_CREDENTIAL', value: environmentSecret },
    ])

    const jsonDiagnostic = JSON.stringify({ arbitrary: headerSecret, another: environmentSecret })
    const doublyStringifiedDiagnostic = JSON.stringify(
      JSON.stringify({ arbitrary: headerSecret, another: environmentSecret })
    )
    const formEncodedHeader = new URLSearchParams({ value: headerSecret })
      .toString()
      .slice('value='.length)
    const formEncodedEnvironment = new URLSearchParams({ value: environmentSecret })
      .toString()
      .slice('value='.length)
    let escapeIndex = 0
    const mixedCaseEnvironment = formEncodedEnvironment.replaceAll(/%[0-9a-f]{2}/gi, (escape) =>
      ++escapeIndex % 2 === 0 ? escape.toUpperCase() : escape.toLowerCase()
    )

    const sanitized = sanitizeMcpDiagnostic(
      new Error(
        `${jsonDiagnostic} nested=${doublyStringifiedDiagnostic} arbitrary=${formEncodedHeader.toLowerCase()} mixed=${mixedCaseEnvironment}`
      ),
      mcp
    )!
    assert.notInclude(sanitized, JSON.stringify(headerSecret).slice(1, -1))
    assert.notInclude(sanitized, JSON.stringify(environmentSecret).slice(1, -1))
    assert.notInclude(sanitized, headerSecret)
    assert.notInclude(sanitized, environmentSecret)
    assert.notInclude(sanitized, formEncodedHeader.toLowerCase())
    assert.notInclude(sanitized, mixedCaseEnvironment)
    assert.include(sanitized, '[REDACTED]')
  })
})
