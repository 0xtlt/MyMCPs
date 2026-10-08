import { createReadStream } from 'node:fs'
import { stat } from 'node:fs/promises'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import db from '@adonisjs/lucid/services/db'
import { applicationVersion } from '#services/application_version'
import {
  BACKUP_CHUNK_BYTES,
  deriveBackupKey,
  encodeBackupPreamble,
  newBackupParams,
  sealBackup,
  sealedBackupSize,
} from '#services/backup/container'
import { backupRuntime } from '#services/backup/runtime'
import { createBackupWorkspace, removeBackupWorkspace } from '#services/backup/workspace'

export type BackupExport = {
  fileName: string
  /** Known before the first byte is sent: encryption adds a fixed amount to each chunk. */
  size: number
  content: Readable
  /** Delete the snapshot. To call once the content has been read, or given up. */
  discard: () => Promise<void>
}

/** `mymcps-backup-YYYYMMDD-HHMMSS.mymcps`, in UTC. */
export function backupFileName(time: Date) {
  const [date, clock] = time.toISOString().slice(0, 19).split('T')
  return `mymcps-backup-${date.replaceAll('-', '')}-${clock.replaceAll(':', '')}.mymcps`
}

/** What the backup says about itself. The key is what its secrets were encrypted with. */
function backupMetadata(time: Date) {
  return JSON.stringify({
    createdAt: time.toISOString(),
    appKey: backupRuntime.appKey(),
    app: { runtime: 'node', version: applicationVersion },
  })
}

async function* backupPlaintext(preamble: Buffer, databasePath: string) {
  yield preamble
  yield* createReadStream(databasePath, { highWaterMark: BACKUP_CHUNK_BYTES })
}

/**
 * Take a snapshot of the whole database and prepare its encrypted copy. The
 * snapshot is one consistent file, made by SQLite itself, in a directory of
 * its own; neither it nor the encrypted file is ever held in memory.
 */
export async function createBackupExport(
  password: string,
  time = new Date()
): Promise<BackupExport> {
  const workspace = await createBackupWorkspace()
  const discard = () => removeBackupWorkspace(workspace)

  try {
    const databasePath = join(workspace, 'database.sqlite3')
    await db.rawQuery('VACUUM INTO ?', [databasePath])
    const { size } = await stat(databasePath)

    const preamble = encodeBackupPreamble(backupMetadata(time))
    const params = newBackupParams()
    const key = await deriveBackupKey(password, params)

    return {
      fileName: backupFileName(time),
      size: sealedBackupSize(preamble.length + size),
      content: Readable.from(sealBackup(key, params, backupPlaintext(preamble, databasePath)), {
        objectMode: false,
      }),
      discard,
    }
  } catch (error) {
    await discard()
    throw error
  }
}
