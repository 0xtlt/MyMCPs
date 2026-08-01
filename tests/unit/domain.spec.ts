import { DateTime } from 'luxon'
import { test } from '@japa/runner'
import AccessTokenService from '#services/access_token_service'
import McpSecretStore from '#services/mcp_secret_store'
import { namespaceTool, parseNamespacedTool } from '#services/upstream/manager'
import Invite from '#models/invite'
import Mcp from '#models/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import {
  createAccessToken,
  createAdmin,
  createMcp,
  createStoredAccessToken,
} from '#tests/helpers/factories'

test.group('domain services and models', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('generates and hashes access tokens', async ({ assert }) => {
    const plaintext = AccessTokenService.generatePlaintext()

    assert.match(plaintext, /^mcp_[A-Za-z0-9_-]{43}$/)
    assert.equal(AccessTokenService.prefix(plaintext), plaintext.slice(0, 12))
    assert.equal(AccessTokenService.hash(plaintext).length, 64)
    assert.notEqual(AccessTokenService.hash(plaintext), plaintext)
  })

  test('creates tokens and resolves enabled MCPs by scope', async ({ assert }) => {
    const admin = await createAdmin()
    const enabledMcp = await createMcp(admin.id, { name: 'Enabled MCP', enabled: true })
    const disabledMcp = await createMcp(admin.id, { name: 'Disabled MCP', enabled: false })

    const allToken = await createAccessToken(admin.id, { scopeMode: 'all' })
    const selectedToken = await createAccessToken(admin.id, {
      scopeMode: 'selected',
      mcpIds: [enabledMcp.id, disabledMcp.id],
    })

    const allowedForAllToken = await AccessTokenService.resolveAllowedMcps(allToken.token)
    const allowedForSelectedToken = await AccessTokenService.resolveAllowedMcps(selectedToken.token)

    assert.deepEqual(
      allowedForAllToken.map((mcp) => mcp.id),
      [enabledMcp.id]
    )
    assert.deepEqual(
      allowedForSelectedToken.map((mcp) => mcp.id),
      [enabledMcp.id]
    )
    assert.equal(allToken.token.tokenHash, AccessTokenService.hash(allToken.plaintext))
    assert.equal(allToken.token.tokenPrefix, AccessTokenService.prefix(allToken.plaintext))
  })

  test('finds only usable access tokens and throttles last-used writes', async ({ assert }) => {
    const admin = await createAdmin()
    const created = await createAccessToken(admin.id)

    const found = await AccessTokenService.findUsableByPlaintext(created.plaintext)
    assert.equal(found?.id, created.token.id)

    created.token.revokedAt = DateTime.utc()
    await created.token.save()
    assert.isNull(await AccessTokenService.findUsableByPlaintext(created.plaintext))

    await createStoredAccessToken(admin.id, {
      tokenHash: AccessTokenService.hash('expired-token'),
      expiresAt: DateTime.utc().minus({ minutes: 1 }),
    })
    assert.isNull(await AccessTokenService.findUsableByPlaintext('expired-token'))

    const recent = await createStoredAccessToken(admin.id, {
      lastUsedAt: DateTime.utc(),
    })
    const lastUsedAt = recent.lastUsedAt!.toMillis()
    await AccessTokenService.touchLastUsed(recent)
    assert.equal(recent.lastUsedAt!.toMillis(), lastUsedAt)

    const staleLastUsedAt = DateTime.utc().minus({ minutes: 6 })
    recent.lastUsedAt = staleLastUsedAt
    await AccessTokenService.touchLastUsed(recent)
    assert.isAbove(recent.lastUsedAt!.toMillis(), staleLastUsedAt.toMillis())
  })

  test('encrypts MCP secrets and treats empty or corrupt values as missing', ({ assert }) => {
    const ciphertext = McpSecretStore.encrypt('top-secret')

    assert.isString(ciphertext)
    assert.notEqual(ciphertext, 'top-secret')
    assert.equal(McpSecretStore.decrypt(ciphertext), 'top-secret')
    assert.isNull(McpSecretStore.encrypt(''))
    assert.isNull(McpSecretStore.decrypt('not-valid-ciphertext'))
    assert.isFalse(McpSecretStore.hasSecret(null))
    assert.isTrue(McpSecretStore.hasSecret(ciphertext))
  })

  test('exposes model state and normalizes MCP names', async ({ assert }) => {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { name: 'Primary MCP' })
    const invite = await Invite.create({
      email: 'member@example.com',
      role: 'member',
      token: Invite.generateToken(),
      createdBy: admin.id,
      expiresAt: DateTime.utc().plus({ days: 1 }),
      acceptedAt: null,
    })
    const token = await createStoredAccessToken(admin.id)

    assert.isTrue(mcp.enabled)
    assert.isTrue(mcp.status === 'ready')
    assert.isTrue(invite.isUsable)
    assert.isFalse(invite.isAccepted)
    assert.isFalse(invite.isExpired)
    assert.isTrue(token.isUsable)
    assert.isFalse(token.isRevoked)
    assert.isFalse(token.isExpired)
    assert.equal(Mcp.slugify('  Hello, World!  '), 'hello-world')
    assert.equal(Mcp.slugify('---'), 'mcp')
    assert.equal(namespaceTool('weather', 'get_forecast'), 'weather__get_forecast')
    assert.deepEqual(parseNamespacedTool('weather__get_forecast'), {
      slug: 'weather',
      toolName: 'get_forecast',
    })
    assert.isNull(parseNamespacedTool('__missing-slug'))
    assert.isNull(parseNamespacedTool('missing-separator'))
  })
})
