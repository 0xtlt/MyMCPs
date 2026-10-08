// @ts-expect-error -- better-sqlite3 ships no types, and the rest of the app only reaches it through Lucid.
import BetterSqlite3 from 'better-sqlite3'

/** The part of better-sqlite3 that backups use. Every call blocks until SQLite answers. */
export type SqliteStatement = {
  run(...parameters: unknown[]): { changes: number; lastInsertRowid: number | bigint }
  get(...parameters: unknown[]): unknown
  all(...parameters: unknown[]): unknown[]
  /** Return the first column of each row instead of the row. */
  pluck(toggle?: boolean): SqliteStatement
}

export type SqliteDatabase = {
  readonly inTransaction: boolean
  prepare(sql: string): SqliteStatement
  exec(sql: string): SqliteDatabase
  pragma(source: string, options?: { simple?: boolean }): unknown
  close(): SqliteDatabase
}

export type SqliteOptions = {
  readonly?: boolean
  fileMustExist?: boolean
  /** How long a statement waits for a lock, in milliseconds, with the thread blocked. */
  timeout?: number
}

const Sqlite = BetterSqlite3 as new (filename: string, options?: SqliteOptions) => SqliteDatabase

/**
 * Open a SQLite file on a connection of its own, apart from the one Lucid
 * gives the requests. Nothing in the file is trusted to run code: functions
 * with side effects are refused in its views, triggers, indexes and
 * generated columns.
 */
export function openSqlite(filename: string, options?: SqliteOptions) {
  const database = new Sqlite(filename, options)
  try {
    database.pragma('trusted_schema = OFF')
  } catch (error) {
    database.close()
    throw error
  }
  return database
}
