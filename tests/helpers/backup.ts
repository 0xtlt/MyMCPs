import { randomUUID } from 'node:crypto'
import { mkdir, readdir, readFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import type { ApiRequest } from '@japa/api-client'
import app from '@adonisjs/core/services/app'
import hash from '@adonisjs/core/services/hash'
import { MigrationRunner } from '@adonisjs/lucid/migration'
import db from '@adonisjs/lucid/services/db'
import {
  type BackupParams,
  deriveBackupKey,
  encodeBackupPreamble,
  openBackup,
  BACKUP_HEADER_BYTES,
  parseBackupHeader,
  sealBackup,
  splitBackupPlaintext,
} from '#services/backup/container'
import { resetBackupRuntime } from '#services/backup/runtime'
import { openSqlite, type SqliteDatabase } from '#services/backup/sqlite'
import { backupWorkspaceRoot } from '#services/backup/workspace'
import env from '#start/env'
import { prepareTestDatabase } from '#tests/helpers/database'

/** Fast enough for a test, and the least a reader accepts. */
const TEST_LOG_N = 14

export const BACKUP_PASSWORD = 'backup-password'
export const ADMIN_EMAIL = 'admin@example.com'
export const ADMIN_PASSWORD = 'password123'

async function collect(chunks: AsyncIterable<Buffer>) {
  const parts: Buffer[] = []
  for await (const chunk of chunks) parts.push(chunk)
  return Buffer.concat(parts)
}

export function testBackupParams(overrides: Partial<BackupParams> = {}): BackupParams {
  return {
    logN: TEST_LOG_N,
    r: 8,
    p: 1,
    salt: Buffer.alloc(16, 0x5a),
    noncePrefix: Buffer.alloc(7, 0xa5),
    ...overrides,
  }
}

/** A whole backup file, made the way an export makes one, in memory. */
export async function sealBackupFile(input: {
  password: string
  metadata: string
  database: Buffer
  params?: BackupParams
}) {
  const params = input.params ?? testBackupParams()
  const key = await deriveBackupKey(input.password, params)
  return collect(sealBackup(key, params, [encodeBackupPreamble(input.metadata), input.database]))
}

/** What a backup file holds. Throws `BackupContainerError` like an import would. */
export async function openBackupFile(password: string, file: Buffer) {
  const header = file.subarray(0, BACKUP_HEADER_BYTES)
  const key = await deriveBackupKey(password, parseBackupHeader(header))
  const database: Buffer[] = []
  const metadata = await splitBackupPlaintext(
    openBackup(key, header, [file.subarray(BACKUP_HEADER_BYTES)]),
    async (chunk) => database.push(chunk)
  )
  return { metadata: metadata.toString('utf8'), database: Buffer.concat(database) }
}

/**
 * Keep the body of a response as the bytes that were sent. The client would
 * otherwise read a file as text.
 */
export function receiveBytes<Request extends ApiRequest>(request: Request) {
  request.request.buffer(true).parse((response, done) => {
    const chunks: Buffer[] = []
    response.on('data', (chunk: Buffer) => chunks.push(chunk))
    response.on('end', () => done(null, Buffer.concat(chunks)))
  })
  return request
}

/** What an instance with this key writes about itself in a backup. */
export function backupMetadata(appKey = env.get('APP_KEY').release()) {
  return JSON.stringify({ createdAt: new Date().toISOString(), appKey })
}

/** The directories exports and imports have not removed. None, once they are over. */
export async function backupLeftovers() {
  try {
    return await readdir(backupWorkspaceRoot())
  } catch {
    return []
  }
}

/** How many directories exports and imports have not removed. */
export async function backupLeftoverCount() {
  const leftovers = await backupLeftovers()
  return leftovers.length
}

/** Wait for what a response leaves to its end: the snapshot of an export goes once it is sent. */
export async function untilNoBackupLeftovers() {
  while ((await backupLeftoverCount()) > 0) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

/**
 * Backups go through connections of their own, which see what is committed
 * and nothing else: their tests cannot run inside a transaction. They start
 * and end with a new, empty database instead.
 */
export async function resetBackupTests() {
  resetBackupRuntime()
  await rm(backupWorkspaceRoot(), { recursive: true, force: true })
  await rm(scratchRoot(), { recursive: true, force: true })
  await prepareTestDatabase()
}

function scratchRoot() {
  return app.tmpPath('backup-tests')
}

/** A path for a file of the test, in a directory `resetBackupTests` removes. */
export async function scratchPath(name: string) {
  const directory = join(scratchRoot(), randomUUID())
  await mkdir(directory, { recursive: true })
  return join(directory, name)
}

/**
 * A database file with the schema of this version, or of the version
 * `rollback` migrations before it, holding one administrator. `change` can
 * make it something an import must refuse.
 */
export async function createBackupDatabase(
  options: {
    rollback?: number
    role?: 'admin' | 'member'
    change?: (database: SqliteDatabase) => void
  } = {}
) {
  const path = await scratchPath('database.sqlite3')
  const connectionName = `backup-test-${randomUUID()}`
  db.manager.add(connectionName, {
    client: 'better-sqlite3',
    connection: { filename: path },
    useNullAsDefault: true,
    migrations: { naturalSort: true, paths: ['database/migrations'] },
  })

  try {
    const up = new MigrationRunner(db, app, {
      direction: 'up',
      connectionName,
      disableLocks: true,
    })
    await up.run()
    if (up.error) throw up.error

    if (options.rollback) {
      const down = new MigrationRunner(db, app, {
        direction: 'down',
        step: options.rollback,
        connectionName,
        disableLocks: true,
      })
      await down.run()
      if (down.error) throw down.error
    }
  } finally {
    await db.manager.close(connectionName, true)
  }

  const password = await hash.make(ADMIN_PASSWORD)
  const database = openSqlite(path)
  try {
    database
      .prepare(
        `INSERT INTO users (full_name, email, password, role, created_at, updated_at)
         VALUES ('Backup Admin', ?, ?, ?, '2026-01-02 03:04:05', '2026-01-02 03:04:05')`
      )
      .run(ADMIN_EMAIL, password, options.role ?? 'admin')
    options.change?.(database)
  } finally {
    database.close()
  }

  return { path, content: await readFile(path) }
}

/** A backup file of this instance's own key around `createBackupDatabase`. */
export async function createBackupFile(options: Parameters<typeof createBackupDatabase>[0] = {}) {
  const { content } = await createBackupDatabase(options)
  return sealBackupFile({
    password: BACKUP_PASSWORD,
    metadata: backupMetadata(),
    database: content,
  })
}
