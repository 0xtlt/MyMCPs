import { test } from '@japa/runner'
import { isValidFiveFieldCron } from '#services/mcp_auto_update_cron'
import {
  isLatestNpmVersion,
  isTrackingLatest,
  mcpNpmUpdateRuntime,
  resetMcpNpmUpdateRuntime,
  updateLatestTrackingMcps,
  updateMcpToLatest,
} from '#services/mcp_npm_update_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('latest npm version helpers', () => {
  test('treats empty, null, and latest as tracking latest', ({ assert }) => {
    assert.isTrue(isLatestNpmVersion(null))
    assert.isTrue(isLatestNpmVersion(''))
    assert.isTrue(isLatestNpmVersion('  '))
    assert.isTrue(isLatestNpmVersion('latest'))
    assert.isTrue(isLatestNpmVersion('Latest'))
    assert.isFalse(isLatestNpmVersion('1.2.3'))
    assert.isFalse(isLatestNpmVersion('1.14.4'))
  })

  test('only npm MCPs without a pin track latest', ({ assert }) => {
    assert.isTrue(isTrackingLatest({ transport: 'npm', npmVersion: null }))
    assert.isTrue(isTrackingLatest({ transport: 'npm', npmVersion: 'latest' }))
    assert.isFalse(isTrackingLatest({ transport: 'npm', npmVersion: '1.2.3' }))
    assert.isFalse(isTrackingLatest({ transport: 'http', npmVersion: null }))
    assert.isFalse(isTrackingLatest({ transport: 'http', npmVersion: 'latest' }))
  })
})

test.group('five-field cron validation', () => {
  test('accepts the default daily 02:00 UTC expression', ({ assert }) => {
    assert.isTrue(isValidFiveFieldCron('0 2 * * *'))
    assert.isTrue(isValidFiveFieldCron('  30 14 * * 1  '))
  })

  test('rejects empty, 6-field, and unparsable expressions', ({ assert }) => {
    assert.isFalse(isValidFiveFieldCron(''))
    assert.isFalse(isValidFiveFieldCron('* * * *'))
    assert.isFalse(isValidFiveFieldCron('0 2 * * * *'))
    assert.isFalse(isValidFiveFieldCron('not a cron'))
    assert.isFalse(isValidFiveFieldCron('99 2 * * *'))
  })
})

test.group('npm MCP update service', (group) => {
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

  test('reloads a latest-tracking npm MCP without rewriting npmVersion', async ({ assert }) => {
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

    await updateMcpToLatest(mcp)
    await mcp.refresh()

    assert.deepEqual(reloaded, [mcp.id])
    assert.equal(mcp.npmVersion, 'latest')
    assert.equal(mcp.status, 'ready')
  })

  test('refuses HTTP MCPs and pinned npm versions without calling reload', async ({ assert }) => {
    const admin = await createAdmin()
    const httpMcp = await createMcp(admin.id, { transport: 'http' })
    const pinned = await createMcp(admin.id, {
      transport: 'npm',
      npmPackage: '@example/pinned-mcp',
      npmVersion: '1.14.4',
    })
    let reloadCount = 0
    mcpNpmUpdateRuntime.reload = async () => {
      reloadCount += 1
    }

    await assert.rejects(() => updateMcpToLatest(httpMcp), 'Only Deno npm MCPs can be updated')
    await assert.rejects(() => updateMcpToLatest(pinned), 'Pinned npm versions are not updated')

    await pinned.refresh()
    assert.equal(pinned.npmVersion, '1.14.4')
    assert.equal(reloadCount, 0)
  })

  test('batch updates only latest-tracking npm MCPs', async ({ assert }) => {
    const admin = await createAdmin()
    const latest = await createMcp(admin.id, {
      name: 'Latest MCP',
      transport: 'npm',
      npmPackage: '@example/latest-mcp',
      npmVersion: null,
    })
    const pinned = await createMcp(admin.id, {
      name: 'Pinned MCP',
      transport: 'npm',
      npmPackage: '@example/pinned-mcp',
      npmVersion: '2.0.0',
    })
    await createMcp(admin.id, { name: 'HTTP MCP', transport: 'http' })

    const reloaded: number[] = []
    mcpNpmUpdateRuntime.reload = async (mcp) => {
      reloaded.push(mcp.id)
    }

    const result = await updateLatestTrackingMcps()

    assert.deepEqual(reloaded, [latest.id])
    assert.equal(result.updated, 1)
    assert.equal(result.skipped, 1)
    assert.lengthOf(result.failed, 0)

    await pinned.refresh()
    await latest.refresh()
    assert.equal(pinned.npmVersion, '2.0.0')
    assert.isNull(latest.npmVersion)
  })
})
