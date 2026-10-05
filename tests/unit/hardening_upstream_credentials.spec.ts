import { test } from '@japa/runner'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import { assignMcpFromPayload } from '#controllers/mcps_controller'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcpValidator } from '#validators/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

type FormValues = Partial<{
  transport: 'http' | 'npm'
  httpUrl: string
  npmPackage: string
  npmVersion: string
  npmArgs: string
  npmEnv: Array<{ name: string; value: string }>
  authType: 'auto' | 'bearer' | 'header'
  authBearer: string
  authHeaderName: string
  authHeaderValue: string
}>

function form(values: FormValues) {
  return createMcpValidator.validate({
    name: 'Repointed MCP',
    description: '',
    transport: 'http',
    httpUrl: '',
    npmPackage: '',
    npmVersion: '',
    npmArgs: '',
    npmEnv: [],
    authType: 'auto',
    authBearer: '',
    authHeaderName: '',
    authHeaderValue: '',
    enabled: true,
    ...values,
  })
}

/** Apply an edit to the row as saved, the way each request does. */
async function edit(id: number, values: FormValues) {
  const mcp = await Mcp.findOrFail(id)
  await assignMcpFromPayload(mcp, await form(values), { excludeId: id })
  return mcp
}

test.group('Saved credentials of a re-pointed HTTP MCP', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  async function bearerMcp() {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Repointed MCP',
      authType: 'bearer',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authBearer = McpSecretStore.encrypt('saved-bearer')
    await mcp.save()
    return mcp
  }

  async function headerMcp() {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Repointed MCP',
      authType: 'header',
      httpUrl: 'https://old.example/mcp',
    })
    mcp.authHeaderName = 'X-Api-Key'
    mcp.authHeaderValue = McpSecretStore.encrypt('saved-header-value')
    await mcp.save()
    return mcp
  }

  test('keeps a blank bearer token only while the origin stays the same', async ({ assert }) => {
    const { id } = await bearerMcp()

    for (const httpUrl of [
      'https://old.example/mcp',
      'https://old.example/v2/mcp?tenant=one',
      'https://user:pass@old.example/mcp',
    ]) {
      const kept = await edit(id, { httpUrl, authType: 'bearer' })
      assert.equal(McpSecretStore.decrypt(kept.authBearer), 'saved-bearer', httpUrl)
    }

    for (const httpUrl of [
      'https://attacker.example/mcp',
      'https://sub.old.example/mcp',
      'https://old.example:8443/mcp',
      'http://old.example/mcp',
    ]) {
      const dropped = await edit(id, { httpUrl, authType: 'bearer' })
      assert.isNull(dropped.authBearer, httpUrl)
    }
  })

  test('stores a bearer token typed again for the new origin', async ({ assert }) => {
    const { id } = await bearerMcp()

    const moved = await edit(id, {
      httpUrl: 'https://new.example/mcp',
      authType: 'bearer',
      authBearer: 'bearer-for-new-origin',
    })

    assert.equal(McpSecretStore.decrypt(moved.authBearer), 'bearer-for-new-origin')
  })

  test('applies the same rule to a custom header value', async ({ assert }) => {
    const { id } = await headerMcp()
    const header = { authType: 'header' as const, authHeaderName: 'X-Api-Key' }

    const kept = await edit(id, { httpUrl: 'https://old.example/other', ...header })
    assert.equal(McpSecretStore.decrypt(kept.authHeaderValue), 'saved-header-value')

    const dropped = await edit(id, { httpUrl: 'https://attacker.example/mcp', ...header })
    assert.isNull(dropped.authHeaderValue)
    assert.equal(dropped.authHeaderName, 'X-Api-Key')

    const retyped = await edit(id, {
      httpUrl: 'https://attacker.example/mcp',
      ...header,
      authHeaderValue: 'value-for-new-origin',
    })
    assert.equal(McpSecretStore.decrypt(retyped.authHeaderValue), 'value-for-new-origin')
  })

  test('drops them when the MCP becomes an npm package', async ({ assert }) => {
    const { id } = await bearerMcp()

    const npm = await edit(id, {
      transport: 'npm',
      npmPackage: '@example/reads-everything',
      authType: 'bearer',
    })

    assert.equal(npm.transport, 'npm')
    assert.isNull(npm.authBearer)
  })
})

test.group('Saved environment of a re-pointed npm MCP', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  async function npmMcp() {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      name: 'Repointed MCP',
      transport: 'npm',
      npmPackage: '@example/trusted-mcp',
      npmVersion: '1.0.0',
    })
    mcp.npmEnv = McpEnvironmentStore.merge(null, [
      { name: 'API_KEY', value: 'saved-api-key' },
      { name: 'REGION', value: 'eu-west-3' },
    ])
    await mcp.save()
    return mcp
  }

  const blank = [
    { name: 'API_KEY', value: '' },
    { name: 'REGION', value: '' },
  ]

  test('keeps blank values across a version or argument change', async ({ assert }) => {
    const { id } = await npmMcp()

    const upgraded = await edit(id, {
      transport: 'npm',
      npmPackage: '@example/trusted-mcp',
      npmVersion: '2.0.0',
      npmArgs: '--verbose',
      npmEnv: blank,
    })

    assert.deepEqual(upgraded.npmEnvironment, { API_KEY: 'saved-api-key', REGION: 'eu-west-3' })
  })

  test('asks for the values again when the package changes', async ({ assert }) => {
    const { id } = await npmMcp()

    let failure: unknown
    try {
      await edit(id, { transport: 'npm', npmPackage: '@attacker/exfiltrate', npmEnv: blank })
    } catch (error) {
      failure = error
    }

    assert.instanceOf(failure, errors.E_VALIDATION_ERROR)
    const [message] = (failure as InstanceType<typeof errors.E_VALIDATION_ERROR>).messages
    assert.equal(message.field, 'npmEnv.0.value')
    assert.include(message.message, 'Enter this value again')
    // The request is refused, so the saved row is untouched.
    const saved = await Mcp.findOrFail(id)
    assert.deepEqual(saved.npmEnvironment, { API_KEY: 'saved-api-key', REGION: 'eu-west-3' })
  })

  test('hands the new package only the values typed for it', async ({ assert }) => {
    const { id } = await npmMcp()

    const moved = await edit(id, {
      transport: 'npm',
      npmPackage: '@example/other-mcp',
      npmEnv: [{ name: 'API_KEY', value: 'key-for-other-package' }],
    })
    assert.deepEqual(moved.npmEnvironment, { API_KEY: 'key-for-other-package' })

    const emptied = await edit(id, { transport: 'npm', npmPackage: '@example/other-mcp' })
    assert.isNull(emptied.npmEnv)
  })
})
