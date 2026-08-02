import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { analyticsQueryValidator } from '#validators/mcp_call_log'

type AnalyticsRange = '24h' | '7d' | '30d'
type AggregateRow = {
  total: number | string
  errors: number | string
  average_duration_ms: number | string | null
}
type TimelineRow = {
  bucket: string
  total: number | string
  errors: number | string
}
type BreakdownRow = AggregateRow & { label: string }

function rangeConfig(range: AnalyticsRange) {
  const now = DateTime.utc()
  if (range === '24h') {
    return { start: now.startOf('hour').minus({ hours: 23 }), unit: 'hour' as const, count: 24 }
  }
  const count = range === '7d' ? 7 : 30
  return { start: now.startOf('day').minus({ days: count - 1 }), unit: 'day' as const, count }
}

function numeric(value: number | string | null | undefined) {
  return Number(value ?? 0)
}

export default class AnalyticsController {
  async index({ request, inertia }: HttpContext) {
    await McpCallLogService.pruneExpired()
    const filters = await request.validateUsing(analyticsQueryValidator)
    const range = filters.range ?? '7d'
    const config = rangeConfig(range)
    const periodQuery = () =>
      McpCallLog.query()
        .withScopes((scopes) => scopes.inPeriod(config.start))
        .pojo()

    const metricsQuery = periodQuery()
    const timelineQuery = periodQuery()
    const topMcpsQuery = periodQuery()
    const topToolsQuery = periodQuery()
    const topTokensQuery = periodQuery()
    const errorCountSql = "SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END)"
    const bucketSql =
      config.unit === 'hour'
        ? "strftime('%Y-%m-%dT%H:00:00Z', created_at)"
        : "strftime('%Y-%m-%dT00:00:00Z', created_at)"

    const metricsPromise = metricsQuery
      .select(metricsQuery.client.raw('COUNT(*) AS total'))
      .select(metricsQuery.client.raw(`${errorCountSql} AS errors`))
      .select(metricsQuery.client.raw('AVG(duration_ms) AS average_duration_ms'))
      .first() as Promise<AggregateRow | null>

    const timelinePromise = timelineQuery
      .select(timelineQuery.client.raw(`${bucketSql} AS bucket`))
      .select(timelineQuery.client.raw('COUNT(*) AS total'))
      .select(timelineQuery.client.raw(`${errorCountSql} AS errors`))
      .groupByRaw(bucketSql)
      .orderBy('bucket', 'asc') as Promise<TimelineRow[]>

    const breakdown = (query: ReturnType<typeof periodQuery>, labelSql: string) =>
      query
        .select(query.client.raw(`${labelSql} AS label`))
        .select(query.client.raw('COUNT(*) AS total'))
        .select(query.client.raw(`${errorCountSql} AS errors`))
        .select(query.client.raw('AVG(duration_ms) AS average_duration_ms'))
        .groupByRaw(labelSql)
        .orderBy('total', 'desc')
        .orderBy('label', 'asc')
        .limit(5) as Promise<BreakdownRow[]>

    const [metricsRow, timelineRows, topMcpRows, topToolRows, topTokenRows, settings] =
      await Promise.all([
        metricsPromise,
        timelinePromise,
        breakdown(topMcpsQuery, "COALESCE(mcp_name, mcp_slug, 'Unknown MCP')"),
        breakdown(topToolsQuery, 'COALESCE(tool_name, requested_tool_name)'),
        breakdown(topTokensQuery, 'access_token_name'),
        McpCallLogService.settings(),
      ])

    const bucketMap = new Map(
      timelineRows.map((row) => [
        row.bucket,
        { total: numeric(row.total), errors: numeric(row.errors) },
      ])
    )
    const timeline = Array.from({ length: config.count }, (_, index) => {
      const date = config.start.plus({ [config.unit === 'hour' ? 'hours' : 'days']: index })
      const key =
        config.unit === 'hour'
          ? date.toFormat("yyyy-MM-dd'T'HH:00:00'Z'")
          : date.toFormat("yyyy-MM-dd'T'00:00:00'Z'")
      return {
        bucket: key,
        label: config.unit === 'hour' ? date.toFormat('HH:mm') : date.toFormat('LLL d'),
        ...(bucketMap.get(key) ?? { total: 0, errors: 0 }),
      }
    })

    const serializeBreakdown = (rows: BreakdownRow[]) =>
      rows.map((row) => ({
        label: row.label,
        total: numeric(row.total),
        errors: numeric(row.errors),
        averageDurationMs: Math.round(numeric(row.average_duration_ms)),
      }))

    const total = numeric(metricsRow?.total)
    const errors = numeric(metricsRow?.errors)
    const averageDurationMs = Math.round(numeric(metricsRow?.average_duration_ms))

    return inertia.render('analytics/index', {
      range,
      loggingLevel: settings.mcpLogLevel,
      metrics: {
        total,
        successes: total - errors,
        errors,
        successRate: total === 0 ? 0 : ((total - errors) / total) * 100,
        errorRate: total === 0 ? 0 : (errors / total) * 100,
        averageDurationMs,
      },
      timeline,
      topMcps: serializeBreakdown(topMcpRows),
      topTools: serializeBreakdown(topToolRows),
      topTokens: serializeBreakdown(topTokenRows),
    })
  }
}
