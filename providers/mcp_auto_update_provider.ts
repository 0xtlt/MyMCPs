import type { ApplicationService } from '@adonisjs/core/types'

export default class McpAutoUpdateProvider {
  constructor(protected app: ApplicationService) {}

  async ready() {
    const { startMcpAutoUpdateScheduler } = await import('#services/mcp_auto_update_scheduler')
    await startMcpAutoUpdateScheduler()
  }

  async shutdown() {
    const { stopMcpAutoUpdateScheduler } = await import('#services/mcp_auto_update_scheduler')
    stopMcpAutoUpdateScheduler()
  }
}
