import type { ApplicationService } from '@adonisjs/core/types'

/**
 * Deletes what the exports and imports of a previous run left behind: a
 * server that stopped halfway cannot have removed its snapshot of the
 * database, or the backup it was decrypting. Done before the server takes a
 * request, so nothing of this run is there yet.
 */
export default class BackupProvider {
  constructor(protected app: ApplicationService) {}

  async boot() {
    const { clearBackupWorkspaces } = await import('#services/backup/workspace')
    try {
      await clearBackupWorkspaces()
    } catch (error) {
      const logger = await this.app.container.make('logger')
      logger.warn({ err: error }, 'Could not delete the leftovers of backup exports and imports')
    }
  }
}
