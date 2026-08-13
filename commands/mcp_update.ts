import { BaseCommand } from '@adonisjs/core/ace'
import type { CommandOptions } from '@adonisjs/core/types/ace'
import { updateLatestTrackingMcps } from '#services/mcp_npm_update_service'

export default class McpUpdate extends BaseCommand {
  static commandName = 'mcp:update'
  static description = 'Reload the Deno cache for npm MCPs that track latest'

  static options: CommandOptions = {
    startApp: true,
  }

  async run() {
    const result = await updateLatestTrackingMcps()
    this.logger.info(
      `Updated ${result.updated} npm MCP(s); skipped ${result.skipped} pinned MCP(s)`
    )

    for (const failure of result.failed) {
      this.logger.error(`${failure.slug}: ${failure.error}`)
    }

    if (result.failed.length > 0) {
      this.exitCode = 1
    }
  }
}
