import { BaseTransformer } from '@adonisjs/core/transformers'
import type McpDebugSession from '#models/mcp_debug_session'

export default class McpDebugSessionTransformer extends BaseTransformer<McpDebugSession> {
  toObject() {
    return {
      ...this.pick(this.resource, [
        'id',
        'accessTokenId',
        'accessTokenName',
        'accessTokenPrefix',
        'status',
        'pausedDurationMs',
      ]),
      startedAt: this.resource.startedAt.toISO()!,
      pausedAt: this.resource.pausedAt?.toISO() ?? null,
      endedAt: this.resource.endedAt?.toISO() ?? null,
    }
  }
}
