import { randomBytes, randomUUID } from 'node:crypto'
import { readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { test } from '@japa/runner'
import type { ApiClient, ApiResponse } from '@japa/api-client'
import type { Assert } from '@japa/assert'
import { Encryption } from '@adonisjs/core/encryption'
import { AES256GCM } from '@adonisjs/core/encryption/drivers/aes_256_gcm'
import app from '@adonisjs/core/services/app'
import encryption from '@adonisjs/core/services/encryption'
import limiter from '@adonisjs/limiter/services/main'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import ApprovalRequest from '#models/approval_request'
import InstanceSetting from '#models/instance_setting'
import Invite from '#models/invite'
import Mcp from '#models/mcp'
import McpCallLog from '#models/mcp_call_log'
import User from '#models/user'
import AccessTokenService from '#services/access_token_service'
import { BACKUP_CHUNK_BYTES, BACKUP_HEADER_BYTES } from '#services/backup/container'
import { BackupImportError, beginBackupImport, importBackup } from '#services/backup/import'
import { backupRuntime } from '#services/backup/runtime'
import { openSqlite } from '#services/backup/sqlite'
import { backupWorkspaceRoot, createBackupWorkspace } from '#services/backup/workspace'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import env from '#start/env'
import {
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
  BACKUP_PASSWORD,
  backupLeftoverCount,
  backupLeftovers,
  backupMetadata,
  createBackupDatabase,
  createBackupFile,
  openBackupFile,
  receiveBytes,
  resetBackupTests,
  sealBackupFile,
  testBackupParams,
} from '#tests/helpers/backup'
import {
  createAccessToken,
  createAdmin,
  createInvite,
  createMcp,
  createMcpCallLog,
  createMember,
} from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

const MESSAGES = {
  notABackup: 'This file is not a MyMCPs backup',
  newerVersion:
    'This backup was made by a newer version of MyMCPs. Update this instance, then import it again.',
  wrongPassword: 'The password is incorrect, or the backup file is damaged',
  damaged: 'The backup file is damaged or incomplete',
  noAdmin: 'This backup holds no administrator account',
  tooLarge: 'The backup file is larger than 4 GB',
  busy: 'Another import is in progress. Try again in a moment.',
  noFile: 'Choose a backup file',
  noPassword: 'Enter the password of the backup',
  imported: 'Backup imported. Sign in with an account of the imported instance.',
}

function importBackupFile(client: ApiClient, file: Buffer | null, password: string | null) {
  const request = client.post('/onboarding/import').withCsrfToken().redirects(0)
  if (file) request.file('backup', file, { filename: 'backup.mymcps' })
  // A form without a file is still a form: the field makes the request multipart.
  request.fields(password === null ? { other: 'field' } : { password })
  return request
}

async function assertUntouched(assert: Assert) {
  const [users] = await User.query().count('* as total')
  const [mcps] = await Mcp.query().count('* as total')
  assert.equal(Number(users.$extras.total), 0, 'a user was imported')
  assert.equal(Number(mcps.$extras.total), 0, 'an MCP was imported')
  assert.deepEqual(await backupLeftovers(), [], 'the files of the import were left behind')
}

/** The import went back to its form with this to say, and changed nothing. */
async function assertRefused(
  assert: Assert,
  response: ApiResponse,
  errors: Partial<Record<'backup' | 'password', string>>
) {
  response.assertStatus(302)
  assertRedirectTo(assert, response, '/onboarding/import')
  assert.deepEqual(
    response.flashMessage('inputErrorsBag'),
    Object.fromEntries(Object.entries(errors).map(([field, message]) => [field, [message]]))
  )
  // Neither the password nor the file travels back in the session.
  assert.deepEqual(Object.keys(response.flashMessages()).sort(), ['errorsBag', 'inputErrorsBag'])
  await assertUntouched(assert)
}

async function assertImported(assert: Assert, response: ApiResponse) {
  response.assertStatus(302)
  assertRedirectTo(assert, response, '/login')
  response.assertFlashMessage('success', MESSAGES.imported)
  // Nobody is signed in by an import.
  response.assertSessionMissing('auth_web')
  assert.deepEqual(await backupLeftovers(), [])
}

async function signIn(client: ApiClient, email = ADMIN_EMAIL, password = ADMIN_PASSWORD) {
  return client.post('/login').withCsrfToken().redirects(0).form({ email, password })
}

async function userEmails() {
  const users = await User.all()
  return users.map((user) => user.email)
}

async function migrationNames() {
  const files = await readdir(app.migrationsPath())
  return files.map((file) => `database/migrations/${file.replace(/\.ts$/, '')}`).sort()
}

async function ledger() {
  const rows = await db.from('adonis_schema').select('name').orderBy('name')
  return rows.map((row) => row.name as string)
}

const serverUrl = (path: string) => `http://${env.get('HOST')}:${env.get('PORT')}${path}`

/**
 * What a browser holds once it has opened the import page: its cookies, and
 * the token it sends back in a header. For requests the test client cannot
 * make, such as a body that arrives slowly.
 */
async function openImportPage() {
  const response = await fetch(serverUrl('/onboarding/import'))
  const cookies = response.headers.getSetCookie().map((cookie) => cookie.split(';')[0])
  const token = cookies.find((cookie) => cookie.startsWith('XSRF-TOKEN='))!
  return {
    'cookie': cookies.join('; '),
    'x-xsrf-token': decodeURIComponent(token.slice('XSRF-TOKEN='.length)),
  }
}

/** What the import page tells the person who is sent back to it. */
async function shownErrors(session: Record<string, string>) {
  const response = await fetch(serverUrl('/onboarding/import'), {
    headers: { cookie: session.cookie },
  })
  const html = await response.text()
  const [, page] = html.match(/<script data-page="app"[^>]*>(.*?)<\/script>/s)!
  return JSON.parse(page).props.errors as Record<string, string>
}

/**
 * The import form as a body that is sent piece by piece: the password, then
 * the start of the file. `send` adds to the file, `finish` ends the form.
 */
function slowForm(password: string) {
  const boundary = `----backup-test-${randomUUID()}`
  let controller: ReadableStreamDefaultController<Uint8Array>
  const body = new ReadableStream<Uint8Array>({
    start(started) {
      controller = started
      controller.enqueue(
        Buffer.from(
          `--${boundary}\r\nContent-Disposition: form-data; name="password"\r\n\r\n${password}\r\n` +
            `--${boundary}\r\nContent-Disposition: form-data; name="backup"; filename="backup.mymcps"\r\n` +
            'Content-Type: application/octet-stream\r\n\r\n'
        )
      )
    },
  })

  return {
    send: (bytes: Buffer) => controller.enqueue(bytes),
    finish: () => {
      controller.enqueue(Buffer.from(`\r\n--${boundary}--\r\n`))
      controller.close()
    },
    post: (session: Record<string, string>, signal = AbortSignal.timeout(20_000)) =>
      fetch(serverUrl('/onboarding/import'), {
        method: 'POST',
        body,
        redirect: 'manual',
        headers: { ...session, 'content-type': `multipart/form-data; boundary=${boundary}` },
        signal,
        duplex: 'half',
      } as RequestInit),
  }
}

async function until(condition: () => boolean | Promise<boolean>) {
  while (!(await condition())) {
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
}

test.group('backup import', (group) => {
  group.each.setup(resetBackupTests)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(resetBackupTests)

  test('shows the import form on a new instance only', async ({ client, assert }) => {
    const form = await client.get('/onboarding/import').withInertia()
    form.assertStatus(200)
    form.assertInertiaComponent('onboarding/import')

    const admin = await createAdmin()

    const guest = await client.get('/onboarding/import').redirects(0)
    guest.assertStatus(302)
    assertRedirectTo(assert, guest, '/login')

    const signedIn = await client.get('/onboarding/import').loginAs(admin).redirects(0)
    signedIn.assertStatus(302)
    assertRedirectTo(assert, signedIn, '/')
  })

  test('takes no backup once the instance is set up', async ({ client, assert }) => {
    const file = await createBackupFile()
    const admin = await createAdmin({ email: 'first@example.com' })

    const guest = await importBackupFile(client, file, BACKUP_PASSWORD)
    guest.assertStatus(302)
    assertRedirectTo(assert, guest, '/login')

    const signedIn = await importBackupFile(client, file, BACKUP_PASSWORD).loginAs(admin)
    signedIn.assertStatus(302)
    assertRedirectTo(assert, signedIn, '/')

    // The file was not even read.
    assert.deepEqual(await backupLeftovers(), [])
    assert.deepEqual(await userEmails(), ['first@example.com'])
  })

  test('refuses an import without a valid CSRF token, and writes no file', async ({
    client,
    assert,
  }) => {
    const file = await createBackupFile()
    let created = false
    const watching = setInterval(async () => {
      created ||= (await backupLeftoverCount()) > 0
    }, 1)

    const response = await client
      .post('/onboarding/import')
      .redirects(0)
      .file('backup', file, { filename: 'backup.mymcps' })
      .fields({ password: BACKUP_PASSWORD })
    clearInterval(watching)

    response.assertStatus(302)
    response.assertFlashMessage('error', 'Invalid or expired CSRF token')
    assert.isFalse(created)
    await assertUntouched(assert)
  })

  test('makes a new instance with another APP_KEY the one that was exported', async ({
    client,
    assert,
  }) => {
    /**
     * The exporting instance, played by this one: its rows are encrypted
     * with a key of its own, which it writes in its backups.
     */
    const sourceKey = `base64:${randomBytes(32).toString('base64')}`
    const source = new Encryption({
      driver: (key) => new AES256GCM({ id: 'gcm', key }),
      keys: [sourceKey],
    })
    backupRuntime.appKey = () => sourceKey

    const secrets = {
      authBearer: 'bearer of the source instance',
      authHeaderValue: 'header value of the source instance',
      oauthClientSecret: 'client secret of the source instance',
      oauthAccessToken: 'access token of the source instance',
      oauthRefreshToken: 'refresh token of the source instance',
      builtinPassword: 'app password of the source instance',
    }
    // Not in alphabetical order: the order is part of what is kept.
    const environment = { ZONE: 'eu-west-3', API_TOKEN: 'token of the npm MCP', DEBUG: '1' }
    const settings = { customerId: '123-456-7890', accountLabel: 'Été 2026 ☀' }
    const encryptedMap = (map: Record<string, string>) =>
      JSON.stringify(
        Object.fromEntries(
          Object.entries(map).map(([name, value]) => [name, source.encrypt(value)])
        )
      )

    const admin = await createAdmin({
      email: ADMIN_EMAIL,
      password: ADMIN_PASSWORD,
      fullName: 'Source Admin',
    })
    const member = await createMember({ email: 'member@example.com', password: 'member-pass-1' })
    await User.rememberMeTokens.create(admin, '1 year')

    const mcp = await createMcp(admin.id, { name: 'Every secret', authType: 'bearer' })
    await mcp
      .merge({
        ...Object.fromEntries(
          Object.entries(secrets).map(([column, value]) => [column, source.encrypt(value)])
        ),
        npmEnv: encryptedMap(environment),
        builtinSettings: encryptedMap(settings),
      })
      .save()

    // Values that were already unreadable on the source stay as they are.
    const unreadable = 'gcm.not-a-ciphertext.of.anything'
    const brokenMap = JSON.stringify({ BROKEN: 'never encrypted', FINE: source.encrypt('fine') })
    const broken = await createMcp(admin.id, { name: 'Unreadable secrets' })
    await broken.merge({ authBearer: unreadable, npmEnv: brokenMap, builtinSettings: '[]' }).save()

    // The last id was given, then freed: it must not be given again.
    const deleted = await createMcp(admin.id, { name: 'Deleted since' })
    await deleted.delete()

    const { token, plaintext } = await createAccessToken(admin.id, { name: 'Agent' })
    const approval = await ApprovalRequest.create({
      publicId: randomBytes(24).toString('base64url'),
      mcpId: mcp.id,
      accessTokenId: token.id,
      toolName: 'send_mail',
      arguments: source.encrypt('{"to":"someone@example.com"}'),
      argumentsHash: 'a'.repeat(64),
      summary: source.encrypt('{"title":"Send a mail"}'),
      status: 'pending',
      expiresAt: DateTime.utc().plus({ hours: 1 }),
    })
    await createMcpCallLog(token, { mcp, requestedToolName: 'every-secret__send_mail' })
    const invite = await createInvite(admin.id, { email: 'invited@example.com' })

    const instance = await InstanceSetting.current()
    await instance.merge({ gatewayToolMode: 'lazy', mcpLogRetentionDays: 30 }).save()
    await db.table('rate_limits').insert({ key: 'login-address:203.0.113.9', points: 3, expire: 1 })

    const exported = await receiveBytes(
      client.post('/settings/backup').loginAs(admin).withCsrfToken().redirects(0).form({
        password: BACKUP_PASSWORD,
        passwordConfirmation: BACKUP_PASSWORD,
        currentPassword: ADMIN_PASSWORD,
      })
    )
    exported.assertStatus(200)
    const file: Buffer = exported.body()
    const { metadata } = await openBackupFile(BACKUP_PASSWORD, file)
    assert.equal(JSON.parse(metadata).appKey, sourceKey)

    /** The new instance: this one again, with its own key and an empty database. */
    await resetBackupTests()
    assert.notEqual(backupRuntime.appKey(), sourceKey)

    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))

    // The accounts are those of the source, with their passwords.
    const users = await User.query().orderBy('id')
    assert.deepEqual(
      users.map((user) => [user.id, user.email, user.role, user.fullName]),
      [
        [admin.id, ADMIN_EMAIL, 'admin', 'Source Admin'],
        [member.id, 'member@example.com', 'member', 'Test User'],
      ]
    )
    assert.lengthOf(await User.rememberMeTokens.all(users[0]), 1)
    const signedIn = await signIn(client)
    signedIn.assertStatus(302)
    assertRedirectTo(assert, signedIn, '/')
    signedIn.assertSession('auth_web', admin.id)
    const wrongPassword = await signIn(client, ADMIN_EMAIL, 'another-password')
    wrongPassword.assertSessionMissing('auth_web')

    // Every secret reads with the key of this instance, and no longer with the other.
    const imported = await Mcp.findOrFail(mcp.id)
    for (const [column, value] of Object.entries(secrets) as Array<
      [keyof typeof secrets, string]
    >) {
      assert.equal(encryption.decrypt(imported[column]!), value, column)
      assert.equal(McpSecretStore.decrypt(imported[column]), value, column)
      assert.isNull(source.decrypt(imported[column]!), column)
    }
    assert.deepEqual(imported.npmEnvironment, environment)
    assert.deepEqual(imported.npmEnvNames, Object.keys(environment))
    assert.deepEqual(McpEnvironmentStore.decrypt(imported.builtinSettings), settings)
    assert.deepEqual(McpEnvironmentStore.names(imported.builtinSettings), Object.keys(settings))

    const stillBroken = await Mcp.findOrFail(broken.id)
    assert.equal(stillBroken.authBearer, unreadable)
    assert.equal(stillBroken.builtinSettings, '[]')
    const map = JSON.parse(stillBroken.npmEnv!)
    assert.deepEqual(Object.keys(map), ['BROKEN', 'FINE'])
    assert.equal(map.BROKEN, 'never encrypted')
    assert.equal(encryption.decrypt(map.FINE), 'fine')

    const request = await ApprovalRequest.findOrFail(approval.id)
    assert.equal(McpSecretStore.decrypt(request.arguments), '{"to":"someone@example.com"}')
    assert.equal(McpSecretStore.decrypt(request.summary), '{"title":"Send a mail"}')

    // The rest came as it was.
    const agent = await AccessToken.findByOrFail('tokenHash', AccessTokenService.hash(plaintext))
    assert.equal(agent.id, token.id)
    assert.lengthOf(await McpCallLog.all(), 1)
    const invited = await Invite.findOrFail(invite.id)
    assert.equal(invited.token, invite.token)
    const current = await InstanceSetting.current()
    assert.equal(current.gatewayToolMode, 'lazy')
    assert.equal(current.mcpLogRetentionDays, 30)

    // The counters of the rate limiter stayed with the instance that counted.
    assert.lengthOf(await db.from('rate_limits'), 0)

    // Every migration is recorded, and ids keep counting from where they were.
    assert.deepEqual(await ledger(), await migrationNames())
    const [sequence] = await db.from('sqlite_sequence').where('name', 'mcps')
    assert.equal(sequence.seq, deleted.id)
    const next = await createMcp(admin.id, { name: 'Created after the import' })
    assert.equal(next.id, deleted.id + 1)
  })

  test('imports a backup of its own key without touching its secrets', async ({
    client,
    assert,
  }) => {
    const ciphertext = encryption.encrypt('bearer of this instance')
    const file = await createBackupFile({
      change: (database) =>
        database
          .prepare(
            `INSERT INTO mcps (name, slug, transport, auth_type, auth_bearer, status, enabled, created_by, created_at)
             VALUES ('Same key', 'same-key', 'http', 'bearer', ?, 'ready', 1, 1, '2026-01-02 03:04:05')`
          )
          .run(ciphertext),
    })

    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))

    const mcp = await Mcp.findByOrFail('slug', 'same-key')
    assert.equal(mcp.authBearer, ciphertext)
    const signedIn = await signIn(client)
    signedIn.assertSession('auth_web', 1)
  })

  test('brings a backup of an older version to the current schema', async ({ client, assert }) => {
    // Before sessions had a version, MCPs their approvals, and approval requests a table.
    const older = await createBackupDatabase({
      rollback: 3,
      change: (database) => {
        const columns = (table: string) =>
          database.prepare('SELECT name FROM pragma_table_info(?)').pluck().all(table)
        assert.notInclude(columns('users'), 'session_version')
        assert.notInclude(columns('mcps'), 'builtin_settings')
        assert.lengthOf(columns('approval_requests'), 0)

        database
          .prepare(
            `INSERT INTO mcps (name, slug, transport, auth_type, auth_bearer, status, enabled, created_by, created_at)
             VALUES ('From before', 'from-before', 'http', 'bearer', ?, 'ready', 1, 1, '2026-01-02 03:04:05')`
          )
          .run(encryption.encrypt('bearer from before'))
      },
    })
    const all = await migrationNames()
    const file = await sealBackupFile({
      password: BACKUP_PASSWORD,
      metadata: backupMetadata(),
      database: older.content,
    })

    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))

    assert.deepEqual(await ledger(), all)
    const user = await User.findByOrFail('email', ADMIN_EMAIL)
    assert.equal(user.sessionVersion, 1)
    const mcp = await Mcp.findByOrFail('slug', 'from-before')
    assert.equal(McpSecretStore.decrypt(mcp.authBearer), 'bearer from before')
    assert.isNull(mcp.builtinSettings)
    assert.lengthOf(await ApprovalRequest.all(), 0)

    const signedIn = await signIn(client)
    signedIn.assertStatus(302)
    assertRedirectTo(assert, signedIn, '/')
    signedIn.assertSession('auth_web', user.id)
  })

  test('asks for a file and for its password', async ({ client, assert }) => {
    const file = await createBackupFile()

    await assertRefused(assert, await importBackupFile(client, null, BACKUP_PASSWORD), {
      backup: MESSAGES.noFile,
    })
    await assertRefused(assert, await importBackupFile(client, Buffer.alloc(0), BACKUP_PASSWORD), {
      backup: MESSAGES.noFile,
    })
    await assertRefused(assert, await importBackupFile(client, file, null), {
      password: MESSAGES.noPassword,
    })
    // Fields are trimmed like those of every form: spaces are no password.
    await assertRefused(assert, await importBackupFile(client, file, '   '), {
      password: MESSAGES.noPassword,
    })
    await assertRefused(assert, await importBackupFile(client, null, null), {
      backup: MESSAGES.noFile,
      password: MESSAGES.noPassword,
    })

    // A form that is not multipart has no file.
    const urlEncoded = await client
      .post('/onboarding/import')
      .withCsrfToken()
      .redirects(0)
      .form({ password: BACKUP_PASSWORD })
    await assertRefused(assert, urlEncoded, { backup: MESSAGES.noFile })
  })

  test('trims the password like every other field', async ({ client, assert }) => {
    const file = await createBackupFile()

    await assertImported(assert, await importBackupFile(client, file, `  ${BACKUP_PASSWORD}\n`))
  })

  test('refuses what is not a backup', async ({ client, assert }) => {
    const { content } = await createBackupDatabase()
    const files = [
      // A database that was never encrypted, noise, and less than a header.
      content,
      randomBytes(5000),
      Buffer.from('MYMCPSBK'),
      Buffer.from('x'),
    ]

    for (const file of files) {
      await assertRefused(assert, await importBackupFile(client, file, BACKUP_PASSWORD), {
        backup: MESSAGES.notABackup,
      })
    }
  })

  test('refuses a backup of a format it does not know', async ({ client, assert }) => {
    const file = await createBackupFile()
    const withByte = (offset: number, value: number) => {
      const changed = Buffer.from(file)
      changed[offset] = value
      return changed
    }
    // Version 2, another key derivation, and costs outside what a reader accepts.
    const unknown = [withByte(8, 2), withByte(9, 2), withByte(10, 13), withByte(10, 19)]
    unknown.push(withByte(11, 4), withByte(12, 2))

    for (const changed of unknown) {
      await assertRefused(assert, await importBackupFile(client, changed, BACKUP_PASSWORD), {
        backup: MESSAGES.newerVersion,
      })
    }
  })

  test('refuses a wrong password', async ({ client, assert }) => {
    const file = await createBackupFile()

    await assertRefused(assert, await importBackupFile(client, file, 'another-password'), {
      password: MESSAGES.wrongPassword,
    })
    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))
  })

  test('refuses a damaged or incomplete file', async ({ client, assert }) => {
    const file = await createBackupFile()
    const block = BACKUP_CHUNK_BYTES + 16
    assert.isAbove(file.length, BACKUP_HEADER_BYTES + 2 * block, 'the backup has several chunks')

    const flipped = Buffer.from(file)
    flipped[BACKUP_HEADER_BYTES + block + 10] ^= 0x01
    const damaged = [
      flipped,
      // Cut inside the second chunk, cut before the last one, and continued after it.
      file.subarray(0, BACKUP_HEADER_BYTES + block + 1000),
      file.subarray(0, BACKUP_HEADER_BYTES + 2 * block),
      Buffer.concat([file, Buffer.from('more')]),
      // A header and nothing behind it.
      file.subarray(0, BACKUP_HEADER_BYTES + 5),
    ]
    for (const changed of damaged) {
      await assertRefused(assert, await importBackupFile(client, changed, BACKUP_PASSWORD), {
        backup: MESSAGES.damaged,
      })
    }

    // When it is the first chunk that does not open, a wrong password is as likely.
    const first = Buffer.from(file)
    first[BACKUP_HEADER_BYTES + 10] ^= 0x01
    for (const changed of [first, file.subarray(0, BACKUP_HEADER_BYTES + block)]) {
      await assertRefused(assert, await importBackupFile(client, changed, BACKUP_PASSWORD), {
        password: MESSAGES.wrongPassword,
      })
    }
  })

  test('refuses a backup whose metadata or database is not what an export writes', async ({
    client,
    assert,
  }) => {
    const { content } = await createBackupDatabase()
    const sealed = (metadata: string, database = content) =>
      sealBackupFile({ password: BACKUP_PASSWORD, metadata, database })
    const key = env.get('APP_KEY').release()

    const refused = [
      await sealed('not json'),
      await sealed('["an", "array"]'),
      await sealed(JSON.stringify({ createdAt: '2026-01-02T03:04:05.000Z' })),
      await sealed(JSON.stringify({ appKey: key })),
      await sealed(JSON.stringify({ createdAt: '2026-01-02T03:04:05.000Z', appKey: 'too short' })),
      await sealed(JSON.stringify({ createdAt: 1, appKey: key })),
      await sealed(JSON.stringify({ createdAt: null, appKey: key })),
      await sealed(
        JSON.stringify({ createdAt: '2026-01-02T03:04:05.000Z', appKey: 'k'.repeat(513) })
      ),
      // Not a database, an empty one, and one cut short.
      await sealed(backupMetadata(), randomBytes(8192)),
      await sealed(backupMetadata(), Buffer.alloc(0)),
      await sealed(backupMetadata(), content.subarray(0, content.length - 4096)),
      await sealed(
        backupMetadata(),
        Buffer.concat([Buffer.from('SQLite format 3\0'), randomBytes(8192)])
      ),
    ]
    for (const file of refused) {
      // More files than an address may send in a quarter of an hour.
      await limiter.clear(['memory'])
      await assertRefused(assert, await importBackupFile(client, file, BACKUP_PASSWORD), {
        backup: MESSAGES.damaged,
      })
    }

    // Members it does not know are no reason to refuse, and the time of the
    // export is whatever string the exporting instance wrote.
    await limiter.clear(['memory'])
    const future = JSON.stringify({ createdAt: '', appKey: key, app: 7, more: { than: 1 } })
    await assertImported(
      assert,
      await importBackupFile(client, await sealed(future), BACKUP_PASSWORD)
    )
  })

  test('refuses a database that brings code, or rows that do not hold together', async ({
    client,
    assert,
  }) => {
    const refused: Array<[string, Parameters<typeof createBackupDatabase>[0]]> = [
      [
        'a trigger',
        {
          change: (database) =>
            database.exec(
              `CREATE TRIGGER promote AFTER INSERT ON users BEGIN UPDATE users SET role = 'admin'; END`
            ),
        },
      ],
      [
        'a view',
        { change: (database) => database.exec('CREATE VIEW people AS SELECT * FROM users') },
      ],
      ['a missing table', { change: (database) => database.exec('DROP TABLE invites') }],
      [
        'a missing column',
        { change: (database) => database.exec('ALTER TABLE invites DROP COLUMN accepted_at') },
      ],
      [
        'a column too many',
        { change: (database) => database.exec('ALTER TABLE invites ADD COLUMN note TEXT') },
      ],
      [
        'a table too many',
        { change: (database) => database.exec('CREATE TABLE notes (id INTEGER PRIMARY KEY)') },
      ],
      [
        'a virtual table',
        { change: (database) => database.exec('CREATE VIRTUAL TABLE notes USING fts5(body)') },
      ],
      [
        'a migration recorded twice',
        {
          change: (database) =>
            database.exec(
              'INSERT INTO adonis_schema (name, batch) SELECT name, 9 FROM adonis_schema LIMIT 1'
            ),
        },
      ],
      [
        'a row that refers to nothing',
        {
          change: (database) => {
            database.pragma('foreign_keys = OFF')
            database.exec(
              `INSERT INTO invites (email, role, token, created_by, expires_at, created_at)
               VALUES ('orphan@example.com', 'member', 'orphan', 999, '2030-01-01 00:00:00', '2026-01-02 03:04:05')`
            )
          },
        },
      ],
      [
        'two accounts with one email',
        {
          change: (database) => {
            database.exec('DROP INDEX users_email_unique')
            database.exec(
              `INSERT INTO users (full_name, email, password, role, created_at)
               SELECT 'Twin', email, password, 'member', created_at FROM users`
            )
          },
        },
      ],
      ['no migration ledger', { change: (database) => database.exec('DROP TABLE adonis_schema') }],
    ]

    for (const [what, options] of refused) {
      // More files than an address may send in a quarter of an hour.
      await limiter.clear(['memory'])
      const response = await importBackupFile(
        client,
        await createBackupFile(options),
        BACKUP_PASSWORD
      )
      assert.deepEqual(
        response.flashMessage('inputErrorsBag'),
        { backup: [MESSAGES.damaged] },
        what
      )
      await assertUntouched(assert)
    }
  }).timeout(60_000)

  test('copies columns under their names, whatever their order', async ({ client, assert }) => {
    const file = await createBackupFile({
      change: (database) => {
        database.pragma('foreign_keys = OFF')
        database.exec(
          `CREATE TABLE reordered (
             updated_at datetime, token varchar(255) not null, role varchar(32) not null,
             id integer not null primary key autoincrement, created_at datetime not null,
             expires_at datetime not null, accepted_at datetime, email varchar(254) not null,
             created_by integer not null references users (id) on delete cascade
           );
           INSERT INTO reordered (email, role, token, created_by, expires_at, created_at)
           VALUES ('invited@example.com', 'member', 'of-the-invite', 1, '2030-01-01 00:00:00', '2026-01-02 03:04:05');
           DROP TABLE invites;
           ALTER TABLE reordered RENAME TO invites`
        )
      },
    })

    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))

    const invite = await Invite.findByOrFail('email', 'invited@example.com')
    assert.deepInclude(invite.serialize(), { role: 'member', token: 'of-the-invite', createdBy: 1 })
    assert.isNull(invite.acceptedAt)
  })

  test('exports from, and imports into, a database in WAL mode', async ({ client, assert }) => {
    // What the Rust rewrite leaves behind when it has run on the same data.
    const [{ journal_mode: mode }] = await db.rawQuery('PRAGMA journal_mode = WAL')
    assert.equal(mode, 'wal')
    const admin = await createAdmin({ email: ADMIN_EMAIL, password: ADMIN_PASSWORD })

    const exported = await receiveBytes(
      client.post('/settings/backup').loginAs(admin).withCsrfToken().redirects(0).form({
        password: BACKUP_PASSWORD,
        passwordConfirmation: BACKUP_PASSWORD,
        currentPassword: ADMIN_PASSWORD,
      })
    )
    exported.assertStatus(200)
    const file: Buffer = exported.body()

    // The copy is one whole file in rollback-journal mode: bytes 18 and 19 of its header.
    const { database } = await openBackupFile(BACKUP_PASSWORD, file)
    assert.deepEqual([...database.subarray(18, 20)], [1, 1])

    await db.from('users').delete()
    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))
    assert.deepEqual(await userEmails(), [ADMIN_EMAIL])
    const signedIn = await signIn(client)
    signedIn.assertSession('auth_web', admin.id)
  })

  test('refuses a backup of a version it does not know yet', async ({ client, assert }) => {
    const file = await createBackupFile({
      change: (database) =>
        database.exec(
          `INSERT INTO adonis_schema (name, batch) VALUES ('database/migrations/9999999999999_from_the_future', 2)`
        ),
    })

    await assertRefused(assert, await importBackupFile(client, file, BACKUP_PASSWORD), {
      backup: MESSAGES.newerVersion,
    })
  })

  test('refuses a backup without an administrator', async ({ client, assert }) => {
    const members = await createBackupFile({ role: 'member' })
    await assertRefused(assert, await importBackupFile(client, members, BACKUP_PASSWORD), {
      backup: MESSAGES.noAdmin,
    })

    const nobody = await createBackupFile({
      change: (database) => database.exec('DELETE FROM users'),
    })
    await assertRefused(assert, await importBackupFile(client, nobody, BACKUP_PASSWORD), {
      backup: MESSAGES.noAdmin,
    })
  })

  test('refuses a file over the limit, on what the client announces', async ({
    client,
    assert,
  }) => {
    const file = await createBackupFile()
    // The limit is 4 GiB: lowered, so that a test can reach it.
    backupRuntime.maxFileBytes = file.length - 100_000
    let created = false
    const watching = setInterval(async () => {
      const [workspace] = await backupLeftovers()
      if (workspace) {
        const files = await readdir(join(backupWorkspaceRoot(), workspace)).catch(() => [])
        created ||= files.length > 0
      }
    }, 1)

    const response = await importBackupFile(client, file, BACKUP_PASSWORD)
    clearInterval(watching)

    await assertRefused(assert, response, { backup: MESSAGES.tooLarge })
    assert.isFalse(created, 'the file was written before it was refused')
  })

  test('refuses a file over the limit at the first byte too many', async ({ assert }) => {
    backupRuntime.maxFileBytes = 300_000
    const session = await openImportPage()
    const form = slowForm(BACKUP_PASSWORD)
    const response = form.post(session)

    // No length is announced: the file is refused while it arrives.
    const piece = randomBytes(100_000)
    for (let sent = 0; sent < 5; sent++) form.send(piece)
    form.finish()

    const refused = await response
    assert.equal(refused.status, 302)
    assert.deepEqual(await shownErrors(session), { backup: MESSAGES.tooLarge })
    await assertUntouched(assert)

    // A file of exactly the limit is taken: it is refused for what it is, not for its size.
    const exact = slowForm(BACKUP_PASSWORD)
    const second = exact.post(session)
    for (let sent = 0; sent < 3; sent++) exact.send(piece)
    exact.finish()
    const taken = await second
    assert.equal(taken.status, 302)
    assert.deepEqual(await shownErrors(session), { backup: MESSAGES.notABackup })
    await assertUntouched(assert)
  })

  test('runs one import at a time, and writes the file to disk as it arrives', async ({
    client,
    assert,
  }) => {
    const file = await createBackupFile()
    const session = await openImportPage()
    const form = slowForm(BACKUP_PASSWORD)
    const first = form.post(session)

    // The first import has received the start of its file, and waits for the rest.
    form.send(file.subarray(0, 50_000))
    const received = async () => {
      const [workspace] = await backupLeftovers()
      if (!workspace) return 0
      const upload = join(backupWorkspaceRoot(), workspace, 'upload.mymcps')
      return stat(upload).then(
        ({ size }) => size,
        () => 0
      )
    }
    await until(async () => (await received()) > 0)
    assert.isAtMost(await received(), 50_000)

    const second = await importBackupFile(client, file, BACKUP_PASSWORD)
    second.assertStatus(302)
    assertRedirectTo(assert, second, '/onboarding/import')
    // Nothing is wrong with what was sent: the refusal is under no field.
    assert.deepEqual(second.flashMessage('inputErrorsBag'), { import: [MESSAGES.busy] })
    assert.lengthOf(await backupLeftovers(), 1, 'the second import left the first one alone')
    assert.lengthOf(await User.all(), 0)

    form.send(file.subarray(50_000))
    form.finish()
    const answer = await first
    assert.equal(answer.status, 302)
    assert.equal(new URL(answer.headers.get('location')!, serverUrl('/')).pathname, '/login')
    assert.deepEqual(await userEmails(), [ADMIN_EMAIL])
    assert.deepEqual(await backupLeftovers(), [])

    // The place is free again, for an instance that would still be new.
    const release = beginBackupImport()
    assert.isFunction(release)
    release!()
  })

  test('frees the place and the disk when the client leaves halfway', async ({
    client,
    assert,
  }) => {
    const file = await createBackupFile()
    const session = await openImportPage()
    const form = slowForm(BACKUP_PASSWORD)
    const leaving = new AbortController()
    const first = form.post(session, leaving.signal).catch(() => null)

    // The start of a file has arrived, and the rest never will.
    form.send(file.subarray(0, 50_000))
    await until(async () => (await backupLeftoverCount()) === 1)
    leaving.abort()
    await first

    await until(async () => (await backupLeftoverCount()) === 0)
    await until(() => {
      const release = beginBackupImport()
      release?.()
      return release !== null
    })
    await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))
  })

  test('counts imports for each client address before reading them', async ({ client, assert }) => {
    const file = await createBackupFile()
    const from = (address: string, body: Buffer | null) =>
      importBackupFile(client, body, BACKUP_PASSWORD).header('x-forwarded-for', address)

    for (let attempt = 0; attempt < 10; attempt++) {
      const response = await from('198.51.100.7', null)
      response.assertStatus(302)
    }

    const limited = await from('198.51.100.7', file)
    limited.assertStatus(429)
    limited.assertHeader('retry-after')
    limited.assertTextIncludes('Too many requests')
    await assertUntouched(assert)

    // Another address has an allowance of its own.
    await assertImported(assert, await from('198.51.100.8', file))
  })

  test('stops when an account was created while the backup was being read', async ({ assert }) => {
    const file = await createBackupFile()
    const workspace = await createBackupWorkspace()
    const filePath = join(workspace, 'upload.mymcps')
    await writeFile(filePath, file)
    await createAdmin({ email: 'first@example.com' })

    try {
      await importBackup({ filePath, password: BACKUP_PASSWORD, workspace })
      assert.fail('The backup replaced the instance that was set up meanwhile')
    } catch (error) {
      assert.instanceOf(error, BackupImportError)
      assert.equal((error as BackupImportError).reason, 'already_set_up')
    }

    assert.deepEqual(await userEmails(), ['first@example.com'])
  })

  test('waits for a request that holds the database open', async ({ assert }) => {
    const file = await createBackupFile()
    const workspace = await createBackupWorkspace()
    const filePath = join(workspace, 'upload.mymcps')
    await writeFile(filePath, file)

    // Another connection is in the middle of a transaction, as a request can be.
    const other = openSqlite(app.tmpPath('test.sqlite3'))
    other.exec('BEGIN')
    other.prepare('SELECT count(*) FROM users').get()
    let released = false
    setTimeout(() => {
      other.exec('COMMIT')
      other.close()
      released = true
    }, 200)

    await importBackup({ filePath, password: BACKUP_PASSWORD, workspace })

    assert.isTrue(released, 'the import did not wait for the other transaction')
    assert.deepEqual(await userEmails(), [ADMIN_EMAIL])
  })

  test('reads a backup written with the lowest and the highest cost a reader accepts', async ({
    client,
    assert,
  }) => {
    const { content } = await createBackupDatabase()

    for (const logN of [14, 18]) {
      const file = await sealBackupFile({
        password: BACKUP_PASSWORD,
        metadata: backupMetadata(),
        database: content,
        params: testBackupParams({ logN }),
      })
      await assertImported(assert, await importBackupFile(client, file, BACKUP_PASSWORD))
      await resetBackupTests()
    }
  })

  test('imports a backup the Rust server exported', async ({ client, assert }) => {
    // Exported from the Settings page of the Rust rewrite: one administrator
    // and one MCP behind a bearer token, under a key of its own. The tests
    // of that server import the backup this app exported of the same instance.
    const file = await readFile(new URL('../fixtures/backup-from-rust.mymcps', import.meta.url))
    const opened = await openBackupFile('fixture backup password', file)
    const metadata = JSON.parse(opened.metadata)
    assert.equal(metadata.app.runtime, 'rust')
    assert.notEqual(metadata.appKey, env.get('APP_KEY').release())

    await assertImported(assert, await importBackupFile(client, file, 'fixture backup password'))

    const signedIn = await signIn(client, 'admin@fixture.example', 'fixture admin password')
    signedIn.assertSession('auth_web', 1)
    // The secret the Rust server encrypted with its key is read with ours.
    const mcp = await Mcp.findByOrFail('name', 'Fixture MCP')
    assert.equal(McpSecretStore.decrypt(mcp.authBearer), 'fixture-bearer-4390')
    assert.deepEqual(await ledger(), await migrationNames())
  })
})
