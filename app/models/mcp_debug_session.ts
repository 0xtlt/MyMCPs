import { McpDebugSessionSchema } from '#database/schema'

export type McpDebugSessionStatus = 'active' | 'paused' | 'stopped'

export default class McpDebugSession extends McpDebugSessionSchema {
  declare status: McpDebugSessionStatus

  get isRunning() {
    return this.status === 'active'
  }

  get isOpen() {
    return this.status !== 'stopped'
  }
}
