import { test } from '@japa/runner'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import { assignMcpFromPayload } from '#controllers/mcps_controller'
import McpEnvironmentStore from '#services/mcp_environment_store'
import { buildDenoArgs, buildDenoEnvironment, resolveDenoDir } from '#services/upstream/deno_runner'
import McpTransformer from '#transformers/mcp_transformer'
import { createMcpValidator } from '#validators/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

async function npmPayload(
  overrides: Partial<{
    name: string
    transport: 'http' | 'npm'
    httpUrl: string
    npmEnv: Array<{ name: string; value: string }>
  }> = {}
) {
  const transport = overrides.transport ?? 'npm'
  return createMcpValidator.validate({
    name: overrides.name ?? 'Environment MCP',
    description: '',
    transport,
    httpUrl: transport === 'http' ? (overrides.httpUrl ?? 'http://127.0.0.1:9999/mcp') : '',
    npmPackage: transport === 'npm' ? '@example/environment-mcp' : '',
    npmVersion: '',
    npmArgs: '',
    npmEnv: overrides.npmEnv ?? [],
    authType: 'auto',
    enabled: true,
  })
}

test.group('npm MCP environment variables', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('encrypts values and only exposes variable names through the transformer', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = new Mcp()
    mcp.status = 'draft'
    mcp.createdBy = admin.id

    await assignMcpFromPayload(
      mcp,
      await npmPayload({
        npmEnv: [
          { name: 'API_KEY', value: 'top-secret' },
          { name: 'REGION', value: 'eu-west-3' },
        ],
      })
    )
    await mcp.save()

    assert.notInclude(mcp.npmEnv!, 'top-secret')
    assert.notInclude(mcp.npmEnv!, 'eu-west-3')
    assert.deepEqual(mcp.npmEnvironment, { API_KEY: 'top-secret', REGION: 'eu-west-3' })

    const transformed = new McpTransformer(mcp).toObject()
    assert.deepEqual(transformed.npmEnv, [
      { name: 'API_KEY', hasValue: true },
      { name: 'REGION', hasValue: true },
    ])
    assert.notInclude(JSON.stringify(transformed), 'top-secret')
  })

  test('preserves blank existing values, replaces values, and removes omitted rows', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { name: 'Editable environment', transport: 'npm' })
    mcp.npmPackage = '@example/environment-mcp'
    mcp.npmEnv = McpEnvironmentStore.merge(null, [
      { name: 'KEEP_ME', value: 'original' },
      { name: 'REMOVE_ME', value: 'delete-this' },
    ])
    await mcp.save()

    await assignMcpFromPayload(
      mcp,
      await npmPayload({
        name: mcp.name,
        npmEnv: [
          { name: 'KEEP_ME', value: '' },
          { name: 'NEW_VALUE', value: 'replacement' },
        ],
      }),
      { excludeId: mcp.id }
    )

    assert.deepEqual(mcp.npmEnvironment, {
      KEEP_ME: 'original',
      NEW_VALUE: 'replacement',
    })
  })

  test('rejects blank new values, duplicate names, reserved names, and oversized totals', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/environment-mcp'

    const blankPayload = await npmPayload({ npmEnv: [{ name: 'NEW_SECRET', value: '' }] })
    await assert.rejects(
      () => assignMcpFromPayload(mcp, blankPayload, { excludeId: mcp.id }),
      errors.E_VALIDATION_ERROR
    )

    const duplicatePayload = await npmPayload({
      npmEnv: [
        { name: 'DUPLICATE', value: 'one' },
        { name: 'DUPLICATE', value: 'two' },
      ],
    })
    await assert.rejects(
      () => assignMcpFromPayload(mcp, duplicatePayload, { excludeId: mcp.id }),
      errors.E_VALIDATION_ERROR
    )

    await assert.rejects(
      () => npmPayload({ npmEnv: [{ name: 'HOME', value: 'elsewhere' }] }),
      errors.E_VALIDATION_ERROR
    )

    const oversizedPayload = await npmPayload({
      npmEnv: Array.from({ length: 9 }, (_, index) => ({
        name: `VALUE_${index}`,
        value: 'x'.repeat(8192),
      })),
    })
    await assert.rejects(
      () => assignMcpFromPayload(mcp, oversizedPayload, { excludeId: mcp.id }),
      errors.E_VALIDATION_ERROR
    )
  })

  test('clears variables for HTTP and protects the Deno runtime environment', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@example/environment-mcp'
    mcp.npmEnv = McpEnvironmentStore.merge(null, [
      { name: 'API_KEY', value: 'secret' },
      { name: 'HOME', value: '/unsafe' },
    ])

    assert.deepEqual(buildDenoEnvironment(mcp, '/safe/sandbox'), {
      API_KEY: 'secret',
      HOME: '/safe/sandbox',
      TMPDIR: '/safe/sandbox',
      NO_COLOR: '1',
    })

    await assignMcpFromPayload(mcp, await npmPayload({ transport: 'http' }), { excludeId: mcp.id })
    assert.isNull(mcp.npmEnv)
  })

  test('grants Deno sandbox permissions including os.homedir for Node npm MCPs', async ({
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm' })
    mcp.npmPackage = '@shopify/dev-mcp'
    mcp.npmVersion = '1.14.4'
    mcp.npmArgs = JSON.stringify(['--flag'])

    assert.deepEqual(buildDenoArgs(mcp, '/safe/sandbox'), [
      'run',
      '--quiet',
      '--node-modules-dir=none',
      `--allow-read=/safe/sandbox,${resolveDenoDir()}`,
      '--allow-write=/safe/sandbox',
      '--allow-net',
      '--allow-env',
      '--allow-sys=homedir',
      '--no-prompt',
      'npm:@shopify/dev-mcp@1.14.4',
      '--flag',
    ])
  })

  test('fails closed when stored JSON or ciphertext is corrupted', ({ assert }) => {
    assert.throws(
      () => McpEnvironmentStore.decrypt('{not-json'),
      'Environment variable configuration is corrupted'
    )
    assert.throws(
      () => McpEnvironmentStore.decrypt(JSON.stringify({ API_KEY: 'not-ciphertext' })),
      'Environment variable "API_KEY" could not be decrypted'
    )
  })
})
