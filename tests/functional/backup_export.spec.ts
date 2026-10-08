import { readdir, writeFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import app from '@adonisjs/core/services/app'
import limiter from '@adonisjs/limiter/services/main'
import type User from '#models/user'
import { applicationVersion } from '#services/application_version'
import { backupFileName } from '#services/backup/export'
import { openSqlite } from '#services/backup/sqlite'
import env from '#start/env'
import {
  BACKUP_PASSWORD,
  backupLeftovers,
  openBackupFile,
  resetBackupTests,
  scratchPath,
  receiveBytes,
  untilNoBackupLeftovers,
} from '#tests/helpers/backup'
import { createAdmin, createMcp, createMember } from '#tests/helpers/factories'
import { assertRedirectTo } from '#tests/helpers/http'

function exportBackup(client: ApiClient, fields: Record<string, string>, user?: User) {
  const request = client.post('/settings/backup').withCsrfToken().redirects(0)
  return (user ? request.loginAs(user) : request).form(fields)
}

const validFields = (overrides: Record<string, string> = {}) => ({
  password: BACKUP_PASSWORD,
  passwordConfirmation: BACKUP_PASSWORD,
  currentPassword: 'password123',
  ...overrides,
})

test.group('backup export', (group) => {
  group.each.setup(resetBackupTests)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(resetBackupTests)

  test('is for administrators only', async ({ client, assert }) => {
    await createAdmin()
    const member = await createMember()

    const signedOut = await exportBackup(client, validFields())
    signedOut.assertStatus(302)
    assertRedirectTo(assert, signedOut, '/login')

    const asMember = await exportBackup(client, validFields(), member)
    asMember.assertStatus(302)
    assertRedirectTo(assert, asMember, '/')
    asMember.assertFlashMessage('error', 'Admin access required')

    assert.deepEqual(await backupLeftovers(), [])
  })

  test('is refused without a valid CSRF token', async ({ client, assert }) => {
    const admin = await createAdmin()

    const response = await client
      .post('/settings/backup')
      .loginAs(admin)
      .redirects(0)
      .form(validFields())

    response.assertStatus(302)
    response.assertFlashMessage('error', 'Invalid or expired CSRF token')
    // The passwords of the form do not travel back in the session.
    assert.deepEqual(Object.keys(response.flashMessages()).sort(), ['error', 'errorsBag'])
    assert.deepEqual(await backupLeftovers(), [])
  })

  test('asks for the current password, and counts the wrong ones', async ({ client, assert }) => {
    const admin = await createAdmin()

    for (let guess = 0; guess < 5; guess++) {
      const wrong = await exportBackup(
        client,
        validFields({ currentPassword: `wrong-password-${guess}` }),
        admin
      )
      wrong.assertStatus(302)
      assertRedirectTo(assert, wrong, '/settings')
      assert.deepEqual(wrong.flashMessage('inputErrorsBag'), {
        currentPassword: ['The current password is incorrect'],
      })
    }

    // The allowance is the one of the email and password changes, and it is used up.
    const refused = await exportBackup(client, validFields(), admin)
    refused.assertStatus(429)
    refused.assertHeader('retry-after')
    refused.assertTextIncludes('Too many requests')

    const email = await client
      .patch('/settings/email')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ email: 'updated@example.com', currentPassword: 'password123' })
    email.assertStatus(429)

    assert.deepEqual(await backupLeftovers(), [])
  })

  test('a correct current password clears the count', async ({ client, assert }) => {
    const admin = await createAdmin()

    for (let round = 0; round < 2; round++) {
      for (let guess = 0; guess < 4; guess++) {
        await exportBackup(client, validFields({ currentPassword: 'wrong-password' }), admin)
      }
      const response = await exportBackup(client, validFields(), admin)
      response.assertStatus(200)
    }
    await untilNoBackupLeftovers()
    assert.deepEqual(await backupLeftovers(), [])
  })

  test('refuses a password that is short, long, or not confirmed', async ({ client, assert }) => {
    const admin = await createAdmin()
    const refusals: Array<[Record<string, string>, string]> = [
      [{ password: 'short12', passwordConfirmation: 'short12' }, 'password'],
      [{ password: 'x'.repeat(129), passwordConfirmation: 'x'.repeat(129) }, 'password'],
      [{ passwordConfirmation: 'another-password' }, 'passwordConfirmation'],
      [{ currentPassword: '' }, 'currentPassword'],
    ]

    for (const [fields, field] of refusals) {
      const response = await exportBackup(client, validFields(fields), admin)

      response.assertStatus(302)
      assertRedirectTo(assert, response, '/settings')
      assert.property(response.flashMessage('inputErrorsBag'), field)
      // The page is told that these errors are those of the export form.
      assert.isTrue(response.flashMessage('backupExportRefused'))
      // What was typed does not travel back in the session.
      assert.deepEqual(Object.keys(response.flashMessages()).sort(), [
        'backupExportRefused',
        'errorsBag',
        'inputErrorsBag',
      ])
    }

    // Refused before the current password is counted against the account.
    const accepted = await exportBackup(client, validFields(), admin)
    accepted.assertStatus(200)
    await untilNoBackupLeftovers()
  })

  test('tells the Settings page that an export was refused, once', async ({ client }) => {
    const admin = await createAdmin()

    const page = await client.get('/settings').withInertia().loginAs(admin)
    page.assertInertiaPropsContains({ backup: { exportRefused: false } })

    const afterRefusal = await client
      .get('/settings')
      .withInertia()
      .loginAs(admin)
      .withFlashMessages({
        backupExportRefused: true,
        inputErrorsBag: { currentPassword: ['The current password is incorrect'] },
      })
    afterRefusal.assertInertiaPropsContains({
      backup: { exportRefused: true },
      errors: { currentPassword: 'The current password is incorrect' },
    })

    // Members get no token to post the form with.
    const member = await createMember()
    const asMember = await client.get('/settings').withInertia().loginAs(member)
    asMember.assertInertiaPropsContains({ backup: null })
  })

  test('sends the instance as an encrypted file', async ({ client, assert }) => {
    const admin = await createAdmin({ email: 'admin@example.com' })
    await createMember({ email: 'member@example.com' })
    await createMcp(admin.id, { name: 'Exported MCP' })

    const before = Date.now()
    const response = await receiveBytes(exportBackup(client, validFields(), admin))

    response.assertStatus(200)
    response.assertHeader('content-type', 'application/octet-stream')
    response.assertHeader('cache-control', 'no-store')
    const file: Buffer = response.body()
    assert.equal(response.header('content-length'), String(file.length))

    const disposition = response.header('content-disposition')!
    const [, name] = disposition.match(/^attachment; filename="([^"]+)"$/) ?? []
    assert.match(name, /^mymcps-backup-\d{8}-\d{6}\.mymcps$/)

    // Written with the cost the specification asks of a writer.
    assert.equal(
      file.subarray(0, 13).toString('hex'),
      Buffer.concat([Buffer.from('MYMCPSBK'), Buffer.from([1, 1, 17, 8, 1])]).toString('hex')
    )

    const backup = await openBackupFile(BACKUP_PASSWORD, file)
    const metadata = JSON.parse(backup.metadata)
    assert.deepEqual(Object.keys(metadata), ['createdAt', 'appKey', 'app'])
    assert.equal(metadata.appKey, env.get('APP_KEY').release())
    assert.deepEqual(metadata.app, { runtime: 'node', version: applicationVersion })
    assert.match(metadata.createdAt, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
    assert.closeTo(Date.parse(metadata.createdAt), before, 10_000)
    assert.equal(name, backupFileName(new Date(metadata.createdAt)))

    // The database is one whole SQLite file, with the rows of the instance.
    assert.equal(backup.database.subarray(0, 16).toString('latin1'), 'SQLite format 3\0')
    const path = await scratchPath('exported.sqlite3')
    await writeFile(path, backup.database)
    const database = openSqlite(path, { readonly: true })
    try {
      assert.equal(database.pragma('quick_check', { simple: true }), 'ok')
      assert.deepEqual(database.prepare('SELECT email, role FROM users ORDER BY id').all(), [
        { email: 'admin@example.com', role: 'admin' },
        { email: 'member@example.com', role: 'member' },
      ])
      assert.deepEqual(database.prepare('SELECT name FROM mcps').pluck().all(), ['Exported MCP'])
      const migrations = await readdir(app.migrationsPath())
      assert.lengthOf(database.prepare('SELECT name FROM adonis_schema').all(), migrations.length)
    } finally {
      database.close()
    }

    // The snapshot the file was made from is gone.
    await untilNoBackupLeftovers()
    assert.deepEqual(await backupLeftovers(), [])
  })

  test('names the file after the time of the export, in UTC', ({ assert }) => {
    assert.equal(
      backupFileName(new Date('2026-10-08T15:30:00.000Z')),
      'mymcps-backup-20261008-153000.mymcps'
    )
    assert.equal(
      backupFileName(new Date('2026-01-02T03:04:05.678+02:00')),
      'mymcps-backup-20260102-010405.mymcps'
    )
  })
})
