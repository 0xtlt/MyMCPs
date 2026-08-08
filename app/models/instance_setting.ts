import { InstanceSettingSchema } from '#database/schema'

export type McpLogLevel = 'off' | 'metadata' | 'arguments' | 'responses'
export type GatewayToolMode = 'eager' | 'lazy'

export default class InstanceSetting extends InstanceSettingSchema {
  declare mcpLogLevel: McpLogLevel
  declare gatewayToolMode: GatewayToolMode

  static current() {
    return this.firstOrCreate(
      { id: 1 },
      {
        id: 1,
        gatewayToolMode: 'eager',
        mcpLogLevel: 'metadata',
        mcpLogRetentionDays: 14,
        updatedBy: null,
      }
    )
  }
}
