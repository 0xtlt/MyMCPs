import { test } from '@japa/runner'
import InstanceSetting from '#models/instance_setting'
import { mcpNpmUpdateRuntime, resetMcpNpmUpdateRuntime } from '#services/mcp_npm_update_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp, createMember } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

test.group('npm MCP update endpoint', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => {
    mcpNpmUpdateRuntime.reload = async () => {}
    mcpNpmUpdateRuntime.probe = async (mcp) => {
      mcp.status = 'ready'
      mcp.lastError = null
      await mcp.save()
      return mcp
    }
  })
  group.each.teardown(async () => {
    resetMcpNpmUpdateRuntime()
    await rollbackTestTransaction()
  })

  test('requires authentication', async ({ client, assert }) => {
    await createAdmin()
    const response = await client.post('/mcps/1/update').withCsrfToken().redirects(0)
    response.assertStatus(302)
    assertRedirectTo(assert, response, '/login')
  })

  test('reloads a latest-tracking npm MCP', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      transport: 'npm',
      npmPackage: '@example/latest-mcp',
      npmVersion: 'latest',
    })
    const reloaded: number[] = []
    mcpNpmUpdateRuntime.reload = async (current) => {
      reloaded.push(current.id)
    }

    const response = await client
      .post(`/mcps/${mcp.id}/update`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/mcps')
    response.assertFlashMessage('success', 'MCP updated to latest')
    assert.deepEqual(reloaded, [mcp.id])

    await mcp.refresh()
    assert.equal(mcp.npmVersion, 'latest')
  })

  test('refuses HTTP MCPs', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'http' })
    let reloadCount = 0
    mcpNpmUpdateRuntime.reload = async () => {
      reloadCount += 1
    }

    const response = await client
      .post(`/mcps/${mcp.id}/update`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    response.assertFlashMessage('error', 'Only Deno npm MCPs can be updated')
    assert.equal(reloadCount, 0)
  })

  test('refuses pinned npm versions without changing them', async ({ client, assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, {
      transport: 'npm',
      npmPackage: '@example/pinned-mcp',
      npmVersion: '1.14.4',
    })
    let reloadCount = 0
    mcpNpmUpdateRuntime.reload = async () => {
      reloadCount += 1
    }

    const response = await client
      .post(`/mcps/${mcp.id}/update`)
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)

    response.assertStatus(302)
    response.assertFlashMessage('error', 'Pinned npm versions are not updated')
    assert.equal(reloadCount, 0)

    await mcp.refresh()
    assert.equal(mcp.npmVersion, '1.14.4')
  })
})

test.group('instance auto-update settings', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('persists auto-update enablement and cron for admins', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .patch('/settings/mcp-logging')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        gatewayToolMode: 'eager',
        mcpLogLevel: 'metadata',
        mcpLogRetentionDays: 14,
        mcpAutoUpdateEnabled: 'on',
        mcpAutoUpdateCron: '15 3 * * *',
      })

    response.assertStatus(302)
    assertRedirectTo(assert, response, '/settings')
    response.assertFlashMessage('success', 'Instance settings updated')

    const settings = await InstanceSetting.current()
    assert.isTrue(settings.mcpAutoUpdateEnabled)
    assert.equal(settings.mcpAutoUpdateCron, '15 3 * * *')
  })

  test('rejects an invalid auto-update cron expression', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .patch('/settings/mcp-logging')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        gatewayToolMode: 'eager',
        mcpLogLevel: 'metadata',
        mcpLogRetentionDays: 14,
        mcpAutoUpdateEnabled: 'on',
        mcpAutoUpdateCron: 'not a cron',
      })

    response.assertStatus(302)
    assert.property(response.flashMessage('inputErrorsBag'), 'mcpAutoUpdateCron')

    const settings = await InstanceSetting.current()
    assert.isFalse(settings.mcpAutoUpdateEnabled)
    assert.equal(settings.mcpAutoUpdateCron, '0 2 * * *')
  })

  test('restricts auto-update settings updates to admins', async ({ client, assert }) => {
    const member = await createMember()

    const response = await client
      .patch('/settings/mcp-logging')
      .loginAs(member)
      .withCsrfToken()
      .redirects(0)
      .form({
        gatewayToolMode: 'eager',
        mcpLogLevel: 'metadata',
        mcpLogRetentionDays: 14,
        mcpAutoUpdateEnabled: 'on',
        mcpAutoUpdateCron: '0 2 * * *',
      })

    assertRedirectTo(assert, response, '/')
  })
})
