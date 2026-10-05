import { mkdir, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { addressGuardRuntime, resetAddressGuardRuntime } from '#services/upstream/address_guard'
import {
  denoRuntime,
  removeMcpSandbox,
  resetDenoRuntime,
  sandboxRootFor,
} from '#services/upstream/deno_runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp, createMember } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

async function isFile(path: string) {
  const stats = await stat(path)
  return stats.isFile()
}

type SeenRequest = {
  method: string
  url: string
  authorization: string | null
  apiKey: string | null
}

/** Records what the gateway sends upstream and answers like an MCP without tools. */
function recordUpstream() {
  const original = globalThis.fetch
  const requests: SeenRequest[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    requests.push({
      method: request.method,
      url: request.url,
      authorization: request.headers.get('Authorization'),
      apiKey: request.headers.get('X-Api-Key'),
    })
    if (request.method !== 'POST') return new Response(null, { status: 405 })
    const message = (await request.json()) as {
      id?: number
      method: string
      params?: { protocolVersion?: string }
    }
    if (message.method === 'initialize') {
      return json({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          protocolVersion: message.params?.protocolVersion,
          capabilities: { tools: {} },
          serverInfo: { name: 'recorder', version: '1.0.0' },
        },
      })
    }
    if (message.method === 'tools/list') {
      return json({ jsonrpc: '2.0', id: message.id, result: { tools: [] } })
    }
    return new Response(null, { status: 202 })
  }
  return {
    requests,
    restore() {
      globalThis.fetch = original
    },
  }
}

const httpForm = {
  name: 'Repointed MCP',
  description: '',
  transport: 'http',
  npmPackage: '',
  npmVersion: '',
  npmArgs: '',
  enabled: 'on',
}

test.group('Re-pointing an MCP from the registry', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('does not send a saved bearer token to the new origin when probing it', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const member = await createMember()
    const mcp = await createMcp(admin.id, {
      name: 'Repointed MCP',
      authType: 'bearer',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authBearer = McpSecretStore.encrypt('saved-bearer')
    await mcp.save()
    const upstream = recordUpstream()

    try {
      const response = await client
        .put(`/mcps/${mcp.id}`)
        .loginAs(member)
        .withCsrfToken()
        .redirects(0)
        .form({
          ...httpForm,
          httpUrl: 'https://attacker.example/mcp',
          authType: 'bearer',
          authBearer: '',
        })

      response.assertStatus(302)
      assertRedirectTo(assert, response, '/mcps')
      assert.isAbove(upstream.requests.length, 0, 'the new origin is probed after saving')
      for (const request of upstream.requests) {
        assert.equal(new URL(request.url).origin, 'https://attacker.example')
        assert.isNull(request.authorization)
      }
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.authBearer)
    } finally {
      upstream.restore()
    }
  })

  test('still sends it after a path change within the same origin', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Repointed MCP',
      authType: 'header',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authHeaderName = 'X-Api-Key'
    mcp.authHeaderValue = McpSecretStore.encrypt('saved-header-value')
    await mcp.save()
    const upstream = recordUpstream()

    try {
      const response = await client
        .put(`/mcps/${mcp.id}`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form({
          ...httpForm,
          httpUrl: 'https://old.example/v2/mcp',
          authType: 'header',
          authHeaderName: 'X-Api-Key',
          authHeaderValue: '',
        })

      response.assertStatus(302)
      assert.isAbove(upstream.requests.length, 0)
      for (const request of upstream.requests) {
        assert.equal(request.url, 'https://old.example/v2/mcp')
        assert.equal(request.apiKey, 'saved-header-value')
      }
    } finally {
      upstream.restore()
    }
  })
})

test.group('Sandbox of an npm MCP', (group) => {
  const sandboxes: number[] = []
  let starts = 0

  group.each.setup(async () => {
    await beginTestTransaction()
    starts = 0
    // Saving probes the MCP. These tests are about what happens before Deno starts.
    denoRuntime.binary = () => {
      starts++
      throw new Error('Deno is not started in this test')
    }
  })
  group.each.teardown(async () => {
    resetDenoRuntime()
    await Promise.all(sandboxes.splice(0).map((id) => removeMcpSandbox(id)))
    await rollbackTestTransaction()
  })

  async function npmMcpWithState() {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Sandboxed MCP',
      transport: 'npm',
      npmPackage: '@example/trusted-mcp',
      npmVersion: '1.0.0',
    })
    mcp.npmEnv = McpEnvironmentStore.merge(null, [{ name: 'API_KEY', value: 'saved-api-key' }])
    await mcp.save()
    sandboxes.push(mcp.id)
    const state = join(sandboxRootFor(mcp.id), '.config', 'session.json')
    await mkdir(join(sandboxRootFor(mcp.id), '.config'), { recursive: true })
    await writeFile(state, '{"token":"left by the previous package"}')
    return { admin, mcp, state }
  }

  /** The field names the MCP form submits for its environment rows. */
  function environmentFields(entries: Array<{ name: string; value: string }>) {
    return Object.fromEntries(
      entries.flatMap((entry, index) => [
        [`npmEnv[${index}][name]`, entry.name],
        [`npmEnv[${index}][value]`, entry.value],
      ])
    )
  }

  const npmForm = {
    name: 'Sandboxed MCP',
    description: '',
    transport: 'npm',
    httpUrl: '',
    npmArgs: '',
    authType: 'auto',
    enabled: 'on',
  }

  test('is deleted with the MCP', async ({ client, assert }) => {
    const { admin, mcp, state } = await npmMcpWithState()

    const response = await client
      .delete(`/mcps/${mcp.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    response.assertFlashMessage('success', 'MCP deleted')
    assert.isNull(await Mcp.find(mcp.id))
    await assert.rejects(() => stat(state))
    await assert.rejects(() => stat(sandboxRootFor(mcp.id)))
  })

  test('is emptied when the MCP runs another package', async ({ client, assert }) => {
    const { admin, mcp, state } = await npmMcpWithState()

    const response = await client
      .put(`/mcps/${mcp.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        ...npmForm,
        npmPackage: '@example/other-mcp',
        npmVersion: '',
        ...environmentFields([{ name: 'API_KEY', value: 'key-for-other-package' }]),
      })

    response.assertStatus(302)
    const saved = await Mcp.findOrFail(mcp.id)
    assert.equal(saved.npmPackage, '@example/other-mcp')
    await assert.rejects(() => stat(state))
  })

  test('is kept across a version change', async ({ client, assert }) => {
    const { admin, mcp, state } = await npmMcpWithState()

    const response = await client
      .put(`/mcps/${mcp.id}`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        ...npmForm,
        npmPackage: '@example/trusted-mcp',
        npmVersion: '2.0.0',
        ...environmentFields([{ name: 'API_KEY', value: '' }]),
      })

    response.assertStatus(302)
    const saved = await Mcp.findOrFail(mcp.id)
    assert.equal(saved.npmVersion, '2.0.0')
    assert.deepEqual(saved.npmEnvironment, { API_KEY: 'saved-api-key' })
    assert.isTrue(await isFile(state))
  })

  test('never starts another package with the saved environment values', async ({
    client,
    assert,
  }) => {
    const { mcp, state } = await npmMcpWithState()
    const member = await createMember()

    const response = await client
      .put(`/mcps/${mcp.id}`)
      .loginAs(member)
      .withCsrfToken()
      .redirects(0)
      .form({
        ...npmForm,
        npmPackage: '@attacker/exfiltrate',
        npmVersion: '',
        ...environmentFields([{ name: 'API_KEY', value: '' }]),
      })

    response.assertStatus(302)
    assert.include(JSON.stringify(response.flashMessages()), 'Enter this value again')
    assert.equal(starts, 0)
    const saved = await Mcp.findOrFail(mcp.id)
    assert.equal(saved.npmPackage, '@example/trusted-mcp')
    assert.deepEqual(saved.npmEnvironment, { API_KEY: 'saved-api-key' })
    assert.isTrue(await isFile(state))
  })

  test('refuses variables that would reconfigure the sandbox, with a reason', async ({
    client,
    assert,
  }) => {
    const { mcp } = await npmMcpWithState()
    const member = await createMember()

    const response = await client
      .put(`/mcps/${mcp.id}`)
      .loginAs(member)
      .withCsrfToken()
      .redirects(0)
      .form({
        ...npmForm,
        npmPackage: '@example/trusted-mcp',
        npmVersion: '1.0.0',
        ...environmentFields([
          { name: 'API_KEY', value: '' },
          { name: 'PATH', value: '/tmp/member-bin' },
          { name: 'LD_PRELOAD', value: '/tmp/member.so' },
        ]),
      })

    response.assertStatus(302)
    const errors = JSON.stringify(response.flashMessages())
    assert.include(errors, '\\"PATH\\" is set by MyMCPs for the sandbox and cannot be changed')
    assert.include(errors, '\\"LD_PRELOAD\\" changes how the sandbox process itself is loaded')
    assert.equal(starts, 0)
    const saved = await Mcp.findOrFail(mcp.id)
    assert.deepEqual(saved.npmEnvNames, ['API_KEY'])
  })
})

function mockOAuthProvider() {
  const original = globalThis.fetch
  const requests: string[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    requests.push(`${request.method} ${request.url}`)
    if (request.url.includes('/.well-known/oauth-protected-resource')) {
      return json({
        resource: 'https://mcp.example/mcp',
        authorization_servers: ['https://auth.example'],
        scopes_supported: ['read'],
      })
    }
    if (request.url === 'https://auth.example/.well-known/oauth-authorization-server') {
      return json({
        issuer: 'https://auth.example',
        authorization_endpoint: 'https://auth.example/authorize',
        token_endpoint: 'https://auth.example/token',
        registration_endpoint: 'https://auth.example/register',
        response_types_supported: ['code'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
        token_endpoint_auth_methods_supported: ['none'],
        code_challenge_methods_supported: ['S256'],
      })
    }
    if (request.url === 'https://auth.example/register') {
      return json({
        client_id: 'registered-client',
        redirect_uris: ['http://localhost:3333/mcps/oauth/callback'],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        token_endpoint_auth_method: 'none',
      })
    }
    return new Response('not found', { status: 404 })
  }
  return {
    requests,
    restore() {
      globalThis.fetch = original
    },
  }
}

test.group('Starting an upstream OAuth authorization', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.setup(() => {
    addressGuardRuntime.lookup = async () => ['203.0.113.10']
  })
  group.each.teardown(resetAddressGuardRuntime)

  async function oauthMcp(name = 'OAuth MCP') {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name,
      authType: 'auto',
      oauthRequired: true,
      httpUrl: 'https://mcp.example/mcp',
      status: 'draft',
    })
    return { admin, mcp }
  }

  test('has no effect for a HEAD request', async ({ client, assert }) => {
    const { admin, mcp } = await oauthMcp()
    const provider = mockOAuthProvider()

    try {
      const response = await client.head(`/mcps/${mcp.id}/oauth/start`).loginAs(admin).redirects(0)

      response.assertStatus(405)
      assert.equal(response.header('allow'), 'GET')
      assert.deepEqual(provider.requests, [])
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.oauthClientId)
    } finally {
      provider.restore()
    }
  })

  test('cannot be triggered from another site', async ({ client, assert }) => {
    const { admin, mcp } = await oauthMcp()
    const provider = mockOAuthProvider()

    try {
      for (const site of ['cross-site', 'same-site']) {
        const response = await client
          .get(`/mcps/${mcp.id}/oauth/start?from=elsewhere`)
          .header('Sec-Fetch-Site', site)
          .loginAs(admin)
          .redirects(0)

        response.assertStatus(302)
        assert.equal(response.header('location'), '/mcps')
        response.assertFlashMessage('error', 'Start the OAuth connection from the MCPs page')
      }
      assert.deepEqual(provider.requests, [])
      const saved = await Mcp.findOrFail(mcp.id)
      assert.isNull(saved.oauthClientId)
      assert.isNull(saved.oauthIssuer)
    } finally {
      provider.restore()
    }
  })

  test('starts for the app itself, a typed address, and clients that are not browsers', async ({
    client,
    assert,
  }) => {
    const provider = mockOAuthProvider()

    try {
      for (const site of ['same-origin', 'none', undefined]) {
        const { admin, mcp } = await oauthMcp(`OAuth MCP ${site}`)
        const request = client.get(`/mcps/${mcp.id}/oauth/start`).loginAs(admin).redirects(0)
        if (site) request.header('Sec-Fetch-Site', site)
        const response = await request

        response.assertStatus(302)
        assert.equal(new URL(response.header('location')!).origin, 'https://auth.example')
        const saved = await Mcp.findOrFail(mcp.id)
        assert.equal(saved.oauthClientId, 'registered-client')
      }
    } finally {
      provider.restore()
    }
  })

  test('does not forward its own query string to the provider', async ({ client, assert }) => {
    const { admin, mcp } = await oauthMcp()
    const provider = mockOAuthProvider()

    try {
      const response = await client
        .get(
          `/mcps/${mcp.id}/oauth/start?scope=admin&prompt=none&redirect_uri=https%3A%2F%2Fattacker.example%2Fcb`
        )
        .loginAs(admin)
        .redirects(0)

      response.assertStatus(302)
      const location = response.header('location')!
      const authorization = new URL(location)
      assert.equal(authorization.origin, 'https://auth.example')
      assert.deepEqual(authorization.searchParams.getAll('scope'), ['read'])
      assert.deepEqual(authorization.searchParams.getAll('redirect_uri'), [
        'http://localhost:3333/mcps/oauth/callback',
      ])
      assert.isFalse(authorization.searchParams.has('prompt'))
      assert.equal(location.split('?').length, 2)
      assert.notInclude(location, 'attacker.example')
      assert.notInclude(location, 'admin')
    } finally {
      provider.restore()
    }
  })
})
