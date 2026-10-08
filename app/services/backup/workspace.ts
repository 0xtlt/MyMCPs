import { randomUUID } from 'node:crypto'
import { mkdir, rm } from 'node:fs/promises'
import { join } from 'node:path'
import app from '@adonisjs/core/services/app'

/**
 * Where an export keeps its snapshot of the database, and an import the file
 * it received and what it decrypted from it. Both hold every credential of
 * an instance in clear or nearly so: each gets a directory of its own that
 * only the server can read, deleted as soon as the work is over.
 */
export function backupWorkspaceRoot() {
  return app.tmpPath('backup-tmp')
}

export async function createBackupWorkspace() {
  const root = backupWorkspaceRoot()
  await mkdir(root, { recursive: true, mode: 0o700 })
  const directory = join(root, randomUUID())
  await mkdir(directory, { mode: 0o700 })
  return directory
}

export async function removeBackupWorkspace(directory: string) {
  await rm(directory, { recursive: true, force: true })
}

/**
 * Delete what a server that stopped halfway left behind. Called when the
 * server starts, before it takes a request.
 */
export async function clearBackupWorkspaces() {
  await rm(backupWorkspaceRoot(), { recursive: true, force: true })
}
