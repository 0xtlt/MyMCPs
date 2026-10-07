import type { ApplicationService } from '@adonisjs/core/types'

/**
 * Deletes the files agents uploaded for built-in MCPs once they have expired,
 * including the ones a previous run of the server left behind.
 */
export default class BuiltinUploadProvider {
  constructor(protected app: ApplicationService) {}

  async ready() {
    const { startBuiltinUploadSweeper } = await import('#services/builtin/upload_store')
    const logger = await this.app.container.make('logger')
    startBuiltinUploadSweeper((error) =>
      logger.warn({ err: error }, 'Could not delete the expired uploads of built-in MCPs')
    )
  }

  async shutdown() {
    const { stopBuiltinUploadSweeper } = await import('#services/builtin/upload_store')
    stopBuiltinUploadSweeper()
  }
}
