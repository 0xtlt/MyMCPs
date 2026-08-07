import { test } from '@japa/runner'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import { assignMcpFromPayload } from '#controllers/mcps_controller'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { sanitizeDiagnostic, sanitizeMcpDiagnostic } from '#services/security_redaction'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'
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

test.group('security boundaries', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('allows endpoint query configuration but rejects unsupported URL forms', async ({
    assert,
  }) => {
    const admin = await createAdmin()

    for (const httpUrl of [
      'file:///tmp/socket',
      'https://user:password@example.test/mcp',
      'https://example.test/mcp#secret',
    ]) {
      const mcp = new Mcp()
      mcp.status = 'draft'
      mcp.createdBy = admin.id
      await assert.rejects(
        async () => assignMcpFromPayload(mcp, await httpPayload({ httpUrl })),
        errors.E_VALIDATION_ERROR
      )
    }

    const mcp = new Mcp()
    mcp.status = 'draft'
    mcp.createdBy = admin.id
    const httpUrl = 'https://example.test/mcp?api_key=provider-required&code=fr&key=primary'

    await assignMcpFromPayload(mcp, await httpPayload({ httpUrl }))

    assert.equal(mcp.httpUrl, httpUrl)
  })

  test('allows production CORS only on the exact bearer gateway path', ({ assert }) => {
    const origin = 'https://agent.example'

    assert.equal(resolveCorsOrigin(origin, '/mcp', false), origin)
    assert.equal(resolveCorsOrigin(origin, '/mcp?session=one', false), origin)
    assert.deepEqual(resolveCorsOrigin(origin, '/mcps', false), [])
    assert.deepEqual(resolveCorsOrigin(origin, '/mcps/oauth/callback', false), [])
  })

  test('follows same-origin redirects but blocks cross-origin credential forwarding', async ({
    assert,
  }) => {
    const originalFetch = globalThis.fetch
    const calls: string[] = []
    globalThis.fetch = async (input, init) => {
      assert.equal(init?.redirect, 'manual')
      calls.push(input instanceof Request ? input.url : String(input))
      if (calls.length === 1) {
        return new Response(null, {
          status: 307,
          headers: { Location: '/canonical' },
        })
      }

      return new Response(null, { status: 200 })
    }

    try {
      const response = await fetchWithSameOriginRedirects(
        'https://trusted.example/mcp',
        {},
        'MCP endpoint'
      )
      assert.equal(response.status, 200)
      assert.deepEqual(calls, ['https://trusted.example/mcp', 'https://trusted.example/canonical'])
    } finally {
      globalThis.fetch = originalFetch
    }

    globalThis.fetch = async (_input, init) => {
      assert.equal(init?.redirect, 'manual')
      return new Response(null, {
        status: 307,
        headers: { Location: 'https://attacker.example/collect' },
      })
    }

    try {
      await assert.rejects(
        () => fetchWithSameOriginRedirects('https://trusted.example/mcp', {}, 'MCP endpoint'),
        'MCP endpoint redirected to a different origin'
      )
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
    const stringifiedCredential = JSON.stringify({ client_secret: 'stringified-secret' })
    const sanitized = sanitizeDiagnostic(
      `Authorization: Bearer abc123 {"client_secret":"oauth-secret","access_token":"access-secret"} https://example.test?X-Amz-Signature=aws-secret&code=useful-code session_id=session-secret Set-Cookie: jsessionid=session-cookie Stringified: ${stringifiedCredential} WWW-Authenticate: Bearer error="invalid_token"`
    )!
    assert.notInclude(sanitized, 'abc123')
    assert.notInclude(sanitized, 'oauth-secret')
    assert.notInclude(sanitized, 'access-secret')
    assert.notInclude(sanitized, 'aws-secret')
    assert.notInclude(sanitized, 'session-secret')
    assert.notInclude(sanitized, 'session-cookie')
    assert.notInclude(sanitized, 'stringified-secret')
    assert.include(sanitized, 'code=useful-code')
    assert.include(sanitized, 'Bearer error="invalid_token"')
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
    const formEncodedHeader = new URLSearchParams({ value: headerSecret })
      .toString()
      .slice('value='.length)
    const formEncodedEnvironment = new URLSearchParams({ value: environmentSecret })
      .toString()
      .slice('value='.length)

    const sanitized = sanitizeMcpDiagnostic(
      new Error(`${jsonDiagnostic} header=${formEncodedHeader} env=${formEncodedEnvironment}`),
      mcp
    )!
    assert.notInclude(sanitized, JSON.stringify(headerSecret).slice(1, -1))
    assert.notInclude(sanitized, JSON.stringify(environmentSecret).slice(1, -1))
    assert.notInclude(sanitized, headerSecret)
    assert.notInclude(sanitized, environmentSecret)
    assert.notInclude(sanitized, formEncodedHeader)
    assert.notInclude(sanitized, formEncodedEnvironment)
    assert.include(sanitized, '[REDACTED]')
  })
})
