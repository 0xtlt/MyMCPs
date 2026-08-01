import { test } from '@japa/runner'
import { applyNpmEnv } from '#controllers/mcps_controller'
import Mcp from '#models/mcp'
import { buildDenoEnv } from '#services/upstream/deno_runner'
import { createMcpValidator } from '#validators/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('MCP package environment variables', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('validates multiple environment variables and rejects duplicate names', async ({
    assert,
  }) => {
    const payload = await createMcpValidator.validate({
      name: 'Package MCP',
      transport: 'npm',
      npmPackage: '@example/server',
      npmEnv: [
        { name: 'API_KEY', value: 'secret' },
        { name: 'REGION', value: 'us-east-1' },
      ],
      authType: 'none',
      enabled: true,
    })

    assert.deepEqual(payload.npmEnv, [
      { name: 'API_KEY', value: 'secret' },
      { name: 'REGION', value: 'us-east-1' },
    ])

    const [error] = await createMcpValidator.tryValidate({
      name: 'Package MCP',
      transport: 'npm',
      npmPackage: '@example/server',
      npmEnv: [
        { name: 'API_KEY', value: 'secret' },
        { name: 'API_KEY', value: 'other-secret' },
      ],
      authType: 'none',
      enabled: true,
    })

    assert.isNotNull(error)
  })

  test('encrypts the environment map and exposes only metadata', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/server'
    mcp.npmEnvMap = {
      API_KEY: 'top-secret',
      REGION: 'us-east-1',
    }
    await mcp.save()

    const saved = await Mcp.findOrFail(mcp.id)
    assert.notInclude(saved.npmEnv ?? '', 'top-secret')
    assert.deepEqual(saved.npmEnvMap, {
      API_KEY: 'top-secret',
      REGION: 'us-east-1',
    })
    assert.deepEqual(saved.npmEnvEntries, [
      { name: 'API_KEY', hasValue: true },
      { name: 'REGION', hasValue: true },
    ])
  })

  test('preserves existing values and removes deleted rows', async ({ assert }) => {
    const mcp = new Mcp()
    mcp.npmEnvMap = {
      KEEP: 'old-value',
      REMOVE: 'removed-value',
    }

    applyNpmEnv(mcp, {
      transport: 'npm',
      npmEnv: [
        { name: 'KEEP', value: null },
        { name: 'NEW', value: 'new-value' },
      ],
    })

    assert.deepEqual(mcp.npmEnvMap, {
      KEEP: 'old-value',
      NEW: 'new-value',
    })

    applyNpmEnv(mcp, { transport: 'http', npmEnv: [] })
    assert.deepEqual(mcp.npmEnvMap, {})
  })

  test('merges package variables into the sandbox environment', ({ assert }) => {
    const mcp = new Mcp()
    mcp.npmEnvMap = { API_KEY: 'top-secret', REGION: 'us-east-1' }

    assert.deepEqual(buildDenoEnv(mcp, '/tmp/mcp-sandbox'), {
      API_KEY: 'top-secret',
      REGION: 'us-east-1',
      HOME: '/tmp/mcp-sandbox',
      TMPDIR: '/tmp/mcp-sandbox',
      NO_COLOR: '1',
    })
  })
})
