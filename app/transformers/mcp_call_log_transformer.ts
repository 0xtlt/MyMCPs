import { BaseTransformer } from '@adonisjs/core/transformers'
import type McpCallLog from '#models/mcp_call_log'

export default class McpCallLogTransformer extends BaseTransformer<McpCallLog> {
  toObject() {
    return {
      ...this.pick(this.resource, [
        'id',
        'accessTokenId',
        'accessTokenName',
        'accessTokenPrefix',
        'callerIp',
        'mcpId',
        'mcpName',
        'mcpSlug',
        'requestedToolName',
        'toolName',
        'outcome',
        'errorCategory',
        'errorSummary',
        'arguments',
        'argumentsCaptured',
        'response',
        'responseCaptured',
        'durationMs',
      ]),
      createdAt: this.resource.createdAt.toISO()!,
    }
  }
}
