import { column } from '@adonisjs/lucid/orm'
import { InstanceSettingSchema } from '#database/schema'
import { DEFAULT_MCP_AUTO_UPDATE_CRON } from '#services/mcp_auto_update_cron'

export type McpLogLevel = 'off' | 'metadata' | 'arguments' | 'responses'
export type GatewayToolMode = 'eager' | 'lazy'

export default class InstanceSetting extends InstanceSettingSchema {
  declare mcpLogLevel: McpLogLevel
  declare gatewayToolMode: GatewayToolMode

  @column({ consume: (value) => Boolean(value), prepare: (value) => Boolean(value) })
  declare mcpAutoUpdateEnabled: boolean

  static current() {
    return this.firstOrCreate(
      { id: 1 },
      {
        id: 1,
        gatewayToolMode: 'eager',
        mcpLogLevel: 'metadata',
        mcpLogRetentionDays: 14,
        mcpAutoUpdateEnabled: false,
        mcpAutoUpdateCron: DEFAULT_MCP_AUTO_UPDATE_CRON,
        updatedBy: null,
      }
    )
  }
}
