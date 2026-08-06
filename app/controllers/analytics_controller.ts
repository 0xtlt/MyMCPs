import type { HttpContext } from '@adonisjs/core/http'
import { DateTime, IANAZone } from 'luxon'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { analyticsQueryValidator } from '#validators/mcp_call_log'

type AnalyticsRange = '24h' | '7d' | '30d' | 'custom'
type AggregateRow = {
  total: number | string
  errors: number | string
  average_duration_ms: number | string | null
}
type TimelineRow = {
  bucket: string | null
  total: number | string
  errors: number | string
}
type BreakdownRow = AggregateRow & { label: string }

function resolveTimeZone(timeZone: string | undefined) {
  return timeZone && IANAZone.isValidZone(timeZone) ? timeZone : 'UTC'
}

function presetRangeConfig(range: Exclude<AnalyticsRange, 'custom'>, timeZone: string) {
  const now = DateTime.now().setZone(timeZone)
  if (range === '24h') {
    const start = now.startOf('hour').minus({ hours: 23 })
    return { range, start, end: start.plus({ hours: 24 }), unit: 'hour' as const, count: 24 }
  }
  const count = range === '7d' ? 7 : 30
  const start = now.startOf('day').minus({ days: count - 1 })
  return { range, start, end: start.plus({ days: count }), unit: 'day' as const, count }
}

function rangeConfig(
  range: AnalyticsRange,
  timeZone: string,
  startInput?: string,
  endInput?: string
) {
  if (range !== 'custom' || !startInput || !endInput) {
    return presetRangeConfig(range === 'custom' ? '7d' : range, timeZone)
  }

  const hasExplicitOffset = (value: string) => /(?:Z|[+-]\d{2}:\d{2})$/i.test(value)
  const start = DateTime.fromISO(startInput, { setZone: true }).setZone(timeZone)
  const end = DateTime.fromISO(endInput, { setZone: true }).setZone(timeZone)
  if (
    !hasExplicitOffset(startInput) ||
    !hasExplicitOffset(endInput) ||
    !start.isValid ||
    !end.isValid ||
    start.millisecond !== 0 ||
    end.millisecond !== 0 ||
    end.toMillis() <= start.toMillis() ||
    end.toMillis() > start.plus({ days: 365 }).toMillis()
  ) {
    return presetRangeConfig('7d', timeZone)
  }

  const durationHours = end.diff(start, 'hours').hours
  const unit = durationHours <= 48 ? ('hour' as const) : ('day' as const)
  const duration = unit === 'hour' ? durationHours : end.diff(start, 'days').days
  const count = Math.max(1, Math.ceil(duration))

  return { range, start, end, unit, count }
}

function numeric(value: number | string | null | undefined) {
  return Number(value ?? 0)
}

function sqliteTimestamp(value: DateTime) {
  return value.toUTC().toFormat('yyyy-LL-dd HH:mm:ss')
}

export default class AnalyticsController {
  async index({ request, inertia }: HttpContext) {
    await McpCallLogService.pruneExpired()
    const filters = await request.validateUsing(analyticsQueryValidator)
    const requestedRange = filters.range ?? '7d'
    const timeZone = resolveTimeZone(filters.timeZone)
    const config = rangeConfig(requestedRange, timeZone, filters.start, filters.end)
    const buckets = Array.from({ length: config.count }, (_, index) => {
      const start = config.start.plus({ [config.unit === 'hour' ? 'hours' : 'days']: index })
      const candidateEnd = start.plus({ [config.unit === 'hour' ? 'hours' : 'days']: 1 })
      return {
        key: `bucket-${index}`,
        start,
        end: candidateEnd.toMillis() < config.end.toMillis() ? candidateEnd : config.end,
        label: config.unit === 'hour' ? start.toFormat('HH:mm') : start.toFormat('LLL d'),
      }
    })
    const periodQuery = () =>
      McpCallLog.query()
        .where('created_at', '>=', sqliteTimestamp(config.start))
        .where('created_at', '<', sqliteTimestamp(config.end))
        .pojo()

    const metricsQuery = periodQuery()
    const timelineQuery = periodQuery()
    const topMcpsQuery = periodQuery()
    const topToolsQuery = periodQuery()
    const topTokensQuery = periodQuery()
    const errorCountSql = "SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END)"
    const bucketSql = `CASE ${buckets
      .map(() => 'WHEN created_at >= ? AND created_at < ? THEN ?')
      .join(' ')} END`
    const bucketBindings = buckets.flatMap((bucket) => [
      sqliteTimestamp(bucket.start),
      sqliteTimestamp(bucket.end),
      bucket.key,
    ])

    const metricsPromise = metricsQuery
      .select(metricsQuery.client.raw('COUNT(*) AS total'))
      .select(metricsQuery.client.raw(`${errorCountSql} AS errors`))
      .select(metricsQuery.client.raw('AVG(duration_ms) AS average_duration_ms'))
      .first() as Promise<AggregateRow | null>

    const timelinePromise = timelineQuery
      .select(timelineQuery.client.raw(`${bucketSql} AS bucket`, bucketBindings))
      .select(timelineQuery.client.raw('COUNT(*) AS total'))
      .select(timelineQuery.client.raw(`${errorCountSql} AS errors`))
      .groupBy('bucket')
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
      timelineRows
        .filter((row): row is TimelineRow & { bucket: string } => row.bucket !== null)
        .map((row) => [row.bucket, { total: numeric(row.total), errors: numeric(row.errors) }])
    )
    const timeline = buckets.map((bucket) => ({
      bucket: bucket.start.toUTC().toISO()!,
      label: bucket.label,
      ...(bucketMap.get(bucket.key) ?? { total: 0, errors: 0 }),
    }))

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
      range: config.range,
      start: config.start.toISO({ suppressSeconds: true, suppressMilliseconds: true })!,
      end: config.end.toISO({ suppressSeconds: true, suppressMilliseconds: true })!,
      timeZone,
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
