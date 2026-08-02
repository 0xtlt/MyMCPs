import { McpCallLogSchema } from '#database/schema'
import { column } from '@adonisjs/lucid/orm'
import { scope } from '@adonisjs/lucid/orm'
import type { DateTime } from 'luxon'

export type McpCallOutcome = 'success' | 'error'
export type McpCallErrorCategory =
  'invalid_tool' | 'disallowed_mcp' | 'upstream_exception' | 'tool_error'

export default class McpCallLog extends McpCallLogSchema {
  static inPeriod = scope((query, start: DateTime) => {
    query.where('created_at', '>=', start.toSQL()!)
  })

  declare outcome: McpCallOutcome
  declare errorCategory: McpCallErrorCategory | null

  @column({ consume: (value) => Boolean(value), prepare: (value) => Boolean(value) })
  declare argumentsCaptured: boolean

  @column({ consume: (value) => Boolean(value), prepare: (value) => Boolean(value) })
  declare responseCaptured: boolean
}
