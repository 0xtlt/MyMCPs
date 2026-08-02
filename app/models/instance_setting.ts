import { InstanceSettingSchema } from '#database/schema'

export type McpLogLevel = 'off' | 'metadata' | 'arguments' | 'responses'

export default class InstanceSetting extends InstanceSettingSchema {
  declare mcpLogLevel: McpLogLevel
}
