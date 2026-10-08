import { mkdir, readdir, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'
import { test } from '@japa/runner'
import app from '@adonisjs/core/services/app'
import BackupProvider from '#providers/backup_provider'
import {
  backupWorkspaceRoot,
  clearBackupWorkspaces,
  createBackupWorkspace,
  removeBackupWorkspace,
} from '#services/backup/workspace'

test.group('backup workspaces', (group) => {
  group.each.setup(clearBackupWorkspaces)
  group.each.teardown(clearBackupWorkspaces)

  test('gives each export and import a private directory of its own', async ({ assert }) => {
    const first = await createBackupWorkspace()
    const second = await createBackupWorkspace()

    assert.equal(backupWorkspaceRoot(), app.tmpPath('backup-tmp'))
    assert.equal(dirname(first), backupWorkspaceRoot())
    assert.notEqual(first, second)
    for (const directory of [backupWorkspaceRoot(), first, second]) {
      const { mode } = await stat(directory)
      assert.equal(mode & 0o777, 0o700, directory)
    }

    await writeFile(join(first, 'database.sqlite3'), 'rows')
    await removeBackupWorkspace(first)
    assert.deepEqual(await readdir(backupWorkspaceRoot()), [basename(second)])
  })

  test('deletes what a previous run left behind when the server starts', async ({ assert }) => {
    const leftover = await createBackupWorkspace()
    await writeFile(join(leftover, 'database.sqlite3'), 'every credential of the instance')
    await mkdir(join(leftover, 'nested'))

    // What the server runs before it takes a request.
    await new BackupProvider(app).boot()

    await assert.rejects(() => stat(backupWorkspaceRoot()))
    // And there is nothing to delete on a first start.
    await new BackupProvider(app).boot()
  })
})
