import { randomUUID } from 'node:crypto'
import { open, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { setTimeout as wait } from 'node:timers/promises'
import { configProvider } from '@adonisjs/core'
import { Encryption } from '@adonisjs/core/encryption'
import app from '@adonisjs/core/services/app'
import encryption from '@adonisjs/core/services/encryption'
import { MigrationRunner } from '@adonisjs/lucid/migration'
import db from '@adonisjs/lucid/services/db'
import {
  BACKUP_HEADER_BYTES,
  BACKUP_TAG_BYTES,
  BackupContainerError,
  deriveBackupKey,
  openBackup,
  parseBackupHeader,
  splitBackupPlaintext,
  type BackupContainerFault,
} from '#services/backup/container'
import { backupRuntime } from '#services/backup/runtime'
import { openSqlite, type SqliteDatabase } from '#services/backup/sqlite'
import { resyncMcpAutoUpdateScheduler } from '#services/mcp_auto_update_scheduler'
import { backupMetadataValidator } from '#validators/backup'

/**
 * Why a backup was not imported. `already_set_up` has no message: someone
 * created the first account while the backup was being read, and the answer
 * is the one the setup screen gives once an account exists.
 */
export type BackupImportRefusal =
  | 'not_a_backup'
  | 'newer_version'
  | 'wrong_password'
  | 'damaged'
  | 'no_admin'
  | 'too_large'
  | 'busy'
  | 'already_set_up'

const MESSAGES: Record<Exclude<BackupImportRefusal, 'already_set_up'>, string> = {
  not_a_backup: 'This file is not a MyMCPs backup',
  newer_version:
    'This backup was made by a newer version of MyMCPs. Update this instance, then import it again.',
  wrong_password: 'The password is incorrect, or the backup file is damaged',
  damaged: 'The backup file is damaged or incomplete',
  no_admin: 'This backup holds no administrator account',
  too_large: 'The backup file is larger than 4 GB',
  busy: 'Another import is in progress. Try again in a moment.',
}

export class BackupImportError extends Error {
  constructor(
    public reason: BackupImportRefusal,
    options?: ErrorOptions
  ) {
    super(
      reason === 'already_set_up' ? 'This instance is already set up' : MESSAGES[reason],
      options
    )
    this.name = 'BackupImportError'
  }
}

const CONTAINER_FAULTS: Record<BackupContainerFault, BackupImportRefusal> = {
  not_a_backup: 'not_a_backup',
  unsupported: 'newer_version',
  wrong_password: 'wrong_password',
  damaged: 'damaged',
}

/** Every SQLite file starts with these bytes. */
const SQLITE_HEADER = Buffer.from('SQLite format 3\0', 'latin1')

/**
 * Where the credentials of the instance are kept, each encrypted with its
 * `APP_KEY`. A column of `values` holds one ciphertext; a column of `maps`
 * holds a JSON object whose values are ciphertexts.
 */
const SECRET_COLUMNS = [
  {
    table: 'mcps',
    values: [
      'auth_bearer',
      'auth_header_value',
      'oauth_client_secret',
      'oauth_access_token',
      'oauth_refresh_token',
      'builtin_password',
    ],
    maps: ['npm_env', 'builtin_settings'],
  },
  { table: 'approval_requests', values: ['arguments', 'summary'], maps: [] },
]
const SECRET_ROWS_AT_ONCE = 200

/** Counters of the rate limiter belong to the instance that counted them. */
const TABLES_NEVER_IMPORTED = ['rate_limits']

/** A request that holds the database, such as one creating the first account, is over quickly. */
const BUSY_ATTEMPTS = 40
const BUSY_WAIT_MS = 50

type Encrypter = {
  encrypt(value: unknown): string
  decrypt(value: string): unknown
}

let importing = false

/**
 * An import holds two copies of a backup on disk and takes the memory of a
 * key derivation, so one runs at a time. Returns how to give the place back,
 * or `null` while another import has it.
 */
export function beginBackupImport() {
  if (importing) return null

  importing = true
  return () => {
    importing = false
  }
}

function quoted(identifier: string) {
  return `"${identifier.replaceAll('"', '""')}"`
}

function sqliteCode(error: unknown) {
  const code = error instanceof Error && 'code' in error ? error.code : null
  return typeof code === 'string' && code.startsWith('SQLITE_') ? code : null
}

function isBusy(error: unknown) {
  const code = sqliteCode(error)
  return Boolean(code && (code.startsWith('SQLITE_BUSY') || code.startsWith('SQLITE_LOCKED')))
}

/**
 * Whether SQLite refused the content of the backup, as opposed to failing on
 * the disk or on the live database.
 */
function isRefusedContent(error: unknown) {
  const code = sqliteCode(error)
  return Boolean(
    code &&
    ['CONSTRAINT', 'ERROR', 'CORRUPT', 'NOTADB', 'MISMATCH', 'TOOBIG', 'RANGE'].some((kind) =>
      code.startsWith(`SQLITE_${kind}`)
    )
  )
}

/** The database file the instance runs on. */
function liveDatabase() {
  const config = db.getRawConnection(db.primaryConnectionName)?.config
  if (config?.client !== 'better-sqlite3') {
    throw new Error('Backups are imported into the SQLite database of the instance')
  }
  return config
}

/**
 * Steps 1 and 2: check the header, derive the key, and decrypt the database
 * to a file. Returns the metadata. The header is checked before the key is
 * derived: it decides how much memory the derivation takes.
 */
async function decryptBackup(filePath: string, password: string, databasePath: string) {
  const file = await open(filePath, 'r')
  try {
    const start = Buffer.alloc(BACKUP_HEADER_BYTES)
    const { bytesRead } = await file.read(start, 0, BACKUP_HEADER_BYTES, 0)
    const header = start.subarray(0, bytesRead)
    const params = parseBackupHeader(header)

    // A header and not even the tag of one chunk: no key is derived for that.
    const { size } = await file.stat()
    if (size < BACKUP_HEADER_BYTES + BACKUP_TAG_BYTES) {
      throw new BackupContainerError('damaged')
    }
    const key = await deriveBackupKey(password, params)

    const database = await open(databasePath, 'wx', 0o600)
    try {
      const body = file.createReadStream({ start: BACKUP_HEADER_BYTES, autoClose: false })
      return await splitBackupPlaintext(openBackup(key, header, body), (chunk) =>
        database.write(chunk)
      )
    } finally {
      await database.close()
    }
  } catch (error) {
    if (error instanceof BackupContainerError) {
      throw new BackupImportError(CONTAINER_FAULTS[error.reason], { cause: error })
    }
    throw error
  } finally {
    await file.close()
  }
}

/** Step 3: what the backup says about itself. */
async function readBackupMetadata(bytes: Buffer) {
  let parsed: unknown
  try {
    parsed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
  } catch (error) {
    throw new BackupImportError('damaged', { cause: error })
  }

  const [invalid, metadata] = await backupMetadataValidator.tryValidate(parsed)
  if (invalid) {
    throw new BackupImportError('damaged', { cause: invalid })
  }
  return metadata
}

/** The migrations this version has, whether the live database ran them yet or not. */
async function knownMigrations() {
  const migrator = new MigrationRunner(db, app, {
    direction: 'up',
    connectionName: db.primaryConnectionName,
    disableLocks: true,
  })
  const migrations = await migrator.getList()
  return new Set(
    migrations
      .filter(({ status }) => status === 'pending' || status === 'migrated')
      .map(({ name }) => name)
  )
}

/**
 * Step 4: look at the decrypted database before anything trusts it. It comes
 * from whoever holds the setup screen, and its password proves nothing about
 * what is inside. It is opened read-only, with its schema untrusted, and must
 * be a sound SQLite file made of plain tables and indexes (no trigger, no
 * view, no virtual table), by a version this one knows, with an
 * administrator to sign in as afterwards.
 */
async function inspectBackupDatabase(databasePath: string, migrations: Set<string>) {
  const file = await open(databasePath, 'r')
  try {
    const start = Buffer.alloc(SQLITE_HEADER.length)
    const { bytesRead } = await file.read(start, 0, start.length, 0)
    if (bytesRead < start.length || !start.equals(SQLITE_HEADER)) {
      throw new BackupImportError('damaged')
    }
  } finally {
    await file.close()
  }

  let database: SqliteDatabase | undefined
  try {
    database = openSqlite(databasePath, { readonly: true, fileMustExist: true })
    // Cells that reach outside their page are refused when read, not followed.
    database.pragma('cell_size_check = ON')

    if (database.pragma('quick_check', { simple: true }) !== 'ok') {
      throw new BackupImportError('damaged')
    }

    const count = (sql: string) => Number(database!.prepare(sql).pluck().get())
    // A trigger or a view is code, and a virtual table has no page of its own.
    const foreign = `SELECT count(*) FROM sqlite_master
      WHERE type NOT IN ('table', 'index') OR rootpage IS NULL OR rootpage < 1`
    if (count(foreign) > 0) {
      throw new BackupImportError('damaged')
    }
    if (
      count("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'adonis_schema'") <
      1
    ) {
      throw new BackupImportError('damaged')
    }

    const ran = database.prepare('SELECT name FROM adonis_schema').pluck().all()
    if (ran.some((name) => typeof name !== 'string' || !migrations.has(name))) {
      throw new BackupImportError('newer_version')
    }
    // Every name is known, so more rows than migrations means one is there twice.
    if (ran.length > migrations.size) {
      throw new BackupImportError('damaged')
    }

    if (count("SELECT count(*) FROM users WHERE role = 'admin'") < 1) {
      throw new BackupImportError('no_admin')
    }
  } catch (error) {
    if (error instanceof BackupImportError) throw error
    throw new BackupImportError('damaged', { cause: error })
  } finally {
    database?.close()
  }
}

/**
 * Step 5: bring the decrypted database to the schema of this version, so a
 * backup from an older one has every table and column its rows are copied
 * into. The migrations run through Lucid on a connection made for this file.
 */
async function migrateBackupDatabase(databasePath: string) {
  const connectionName = `backup-import-${randomUUID()}`
  db.manager.add(connectionName, {
    client: 'better-sqlite3',
    connection: { filename: databasePath },
    useNullAsDefault: true,
    ...{ compileSqlOnError: false },
    migrations: liveDatabase().migrations,
    pool: {
      afterCreate(
        connection: SqliteDatabase,
        done: (error: Error | null, connection: SqliteDatabase) => void
      ) {
        connection.pragma('trusted_schema = OFF')
        done(null, connection)
      },
    },
  })

  try {
    const migrator = new MigrationRunner(db, app, {
      direction: 'up',
      connectionName,
      disableLocks: true,
    })
    await migrator.run()
    if (migrator.error) {
      throw new BackupImportError('damaged', { cause: migrator.error })
    }
  } finally {
    await db.manager.close(connectionName, true)
  }
}

/**
 * An encrypter for the key of another instance, made the way this instance
 * makes its own: the driver of `config/encryption.ts`, with that key.
 */
async function encrypterFor(appKey: string): Promise<Encrypter> {
  const config = await configProvider.resolve<{
    default?: string
    list: Record<string, ConstructorParameters<typeof Encryption>[0]>
  }>(app, app.config.get('encryption'))
  const own = config?.default ? config.list[config.default] : undefined
  if (!own) {
    throw new Error('config/encryption.ts names no default encrypter')
  }
  return new Encryption({ driver: own.driver, keys: [appKey] })
}

/**
 * Step 6: the backup comes from an instance with another `APP_KEY`. Decrypt
 * each of its secrets with that key and encrypt it with ours. A value that
 * does not decrypt stays as it is: it was already unreadable there.
 */
function reencryptSecrets(databasePath: string, from: Encrypter, to: Encrypter) {
  const reencrypt = (ciphertext: unknown) => {
    if (typeof ciphertext !== 'string' || !ciphertext) return null
    const plaintext = from.decrypt(ciphertext)
    return plaintext === null ? null : to.encrypt(plaintext)
  }

  /** Names and order are kept; only the values that decrypt change. */
  const reencryptMap = (serialized: unknown) => {
    if (typeof serialized !== 'string' || !serialized) return null

    let map: unknown
    try {
      map = JSON.parse(serialized)
    } catch {
      return null
    }
    if (!map || typeof map !== 'object' || Array.isArray(map)) return null

    let changed = false
    const entries = Object.entries(map).map(([name, ciphertext]) => {
      const next = reencrypt(ciphertext)
      if (next === null) return [name, ciphertext]
      changed = true
      return [name, next]
    })
    return changed ? JSON.stringify(Object.fromEntries(entries)) : null
  }

  const database = openSqlite(databasePath, { fileMustExist: true })
  try {
    database.exec('BEGIN')

    for (const { table, values, maps } of SECRET_COLUMNS) {
      const columns = [...values, ...maps]
      const page = database.prepare(
        `SELECT rowid AS "rowid", ${columns.map(quoted).join(', ')} FROM ${quoted(table)}
         WHERE rowid > ? ORDER BY rowid LIMIT ${SECRET_ROWS_AT_ONCE}`
      )
      const update = Object.fromEntries(
        columns.map((column) => [
          column,
          database.prepare(`UPDATE ${quoted(table)} SET ${quoted(column)} = ? WHERE rowid = ?`),
        ])
      )

      let last = Number.MIN_SAFE_INTEGER
      for (;;) {
        const rows = page.all(last) as Array<Record<string, unknown> & { rowid: number }>
        if (rows.length === 0) break

        for (const row of rows) {
          for (const column of columns) {
            const next = values.includes(column)
              ? reencrypt(row[column])
              : reencryptMap(row[column])
            if (next !== null) update[column].run(next, row.rowid)
          }
        }
        last = rows[rows.length - 1].rowid
      }
    }

    database.exec('COMMIT')
  } catch (error) {
    throw isRefusedContent(error) ? new BackupImportError('damaged', { cause: error }) : error
  } finally {
    database.close()
  }
}

/**
 * Step 7, in one go: nothing else runs in this process between the first
 * statement and the commit, and nothing of it is kept unless all of it is.
 * The connection is one of its own, so that no request shares it.
 */
function replaceRows(livePath: string, backupPath: string) {
  const database = openSqlite(livePath, { fileMustExist: true, timeout: 0 })
  try {
    database.prepare('ATTACH DATABASE ? AS backup').run(backupPath)

    const names = (sql: string, ...parameters: unknown[]) =>
      database
        .prepare(sql)
        .pluck()
        .all(...parameters) as string[]

    /** The tables of one of the two databases that hold what an instance stores. */
    const tablesOf = (schema: 'main' | 'backup') =>
      names(
        `SELECT name FROM ${schema}.sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name`
      ).filter((table) => !TABLES_NEVER_IMPORTED.includes(table))
    const sameNames = (ours: string[], theirs: string[]) =>
      ours.length === theirs.length && ours.every((name) => theirs.includes(name))

    // The backup must have the tables of this instance and no other, each
    // made of the same columns, in whatever order: its rows are copied
    // column by column, under their names.
    const tables = tablesOf('main')
    if (!sameNames(tables, tablesOf('backup'))) {
      throw new BackupImportError('damaged')
    }
    const columns = new Map<string, string[]>()
    for (const table of tables) {
      const live = names("SELECT name FROM pragma_table_info(?, 'main')", table)
      if (!sameNames(live, names("SELECT name FROM pragma_table_info(?, 'backup')", table))) {
        throw new BackupImportError('damaged')
      }
      columns.set(table, live)
    }
    // Where SQLite keeps the last id it gave in each table.
    const sequences = names(
      "SELECT name FROM backup.sqlite_master WHERE type = 'table' AND name = 'sqlite_sequence'"
    )
    if (sequences.length !== 1) {
      throw new BackupImportError('damaged')
    }

    database.exec('BEGIN IMMEDIATE')
    // Rows arrive table by table: what they refer to is checked when all are there.
    database.pragma('defer_foreign_keys = ON')

    if (Number(database.prepare('SELECT count(*) FROM main.users').pluck().get()) > 0) {
      throw new BackupImportError('already_set_up')
    }

    for (const table of tables) {
      database.exec(`DELETE FROM main.${quoted(table)}`)
    }
    for (const table of tables) {
      const list = columns.get(table)!.map(quoted).join(', ')
      database.exec(
        `INSERT INTO main.${quoted(table)} (${list}) SELECT ${list} FROM backup.${quoted(table)}`
      )
    }

    // Ids keep counting from where they were, so none is ever given twice.
    const forget = database.prepare('DELETE FROM main.sqlite_sequence WHERE name = ?')
    const resume = database.prepare(
      'INSERT INTO main.sqlite_sequence (name, seq) SELECT name, seq FROM backup.sqlite_sequence WHERE name = ?'
    )
    for (const table of tables) {
      forget.run(table)
      resume.run(table)
    }

    const orphans = database.pragma('main.foreign_key_check') as unknown[]
    if (orphans.length > 0) {
      throw new BackupImportError('damaged')
    }
    database.exec('COMMIT')
  } catch (error) {
    if (database.inTransaction) database.exec('ROLLBACK')
    throw isRefusedContent(error) ? new BackupImportError('damaged', { cause: error }) : error
  } finally {
    database.close()
  }
}

/**
 * Step 7: replace the rows of the live database with those of the backup.
 * SQLite refuses to start or to commit while a request holds the database
 * open in a transaction; that request cannot finish while this one waits
 * without yielding, so the wait is ours and the work starts again.
 */
async function replaceLiveRows(backupPath: string) {
  const livePath = liveDatabase().connection.filename

  for (let attempt = 1; ; attempt++) {
    try {
      return replaceRows(livePath, backupPath)
    } catch (error) {
      if (!isBusy(error) || attempt >= BUSY_ATTEMPTS) throw error
      await wait(BUSY_WAIT_MS)
    }
  }
}

/**
 * Make this instance the one of the backup. `workspace` is a directory of
 * the import's own, which the caller deletes whatever happens. Throws
 * `BackupImportError` when the backup is refused, and leaves the live
 * database as it was.
 */
export async function importBackup(input: {
  filePath: string
  password: string
  workspace: string
}) {
  const databasePath = join(input.workspace, 'database.sqlite3')

  const metadata = await readBackupMetadata(
    await decryptBackup(input.filePath, input.password, databasePath)
  )
  // The file that was sent has given all it holds.
  await rm(input.filePath, { force: true })

  await inspectBackupDatabase(databasePath, await knownMigrations())
  await migrateBackupDatabase(databasePath)

  if (metadata.appKey !== backupRuntime.appKey()) {
    reencryptSecrets(databasePath, await encrypterFor(metadata.appKey), encryption)
  }

  await replaceLiveRows(databasePath)

  // Step 8: what the server keeps in memory about the rows it had. Nothing
  // else caches them: settings, tokens and MCPs are read at each use.
  await resyncMcpAutoUpdateScheduler()
}
