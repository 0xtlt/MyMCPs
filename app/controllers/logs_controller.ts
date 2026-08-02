import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import McpCallLogTransformer from '#transformers/mcp_call_log_transformer'
import { logsQueryValidator } from '#validators/mcp_call_log'

const PAGE_SIZE = 50

export default class LogsController {
  async index({ request, inertia }: HttpContext) {
    await McpCallLogService.pruneExpired()

    const filters = await request.validateUsing(logsQueryValidator)
    const range = filters.range ?? '24h'
    const outcome = filters.outcome ?? ''
    const mcp = filters.mcp ?? ''
    const token = filters.token ?? ''
    const page = filters.page ?? 1
    const cutoff =
      range === 'all'
        ? null
        : DateTime.utc().minus(range === '24h' ? { hours: 24 } : { days: range === '7d' ? 7 : 30 })

    const buildQuery = () => {
      const query = McpCallLog.query()
      if (cutoff) query.withScopes((scopes) => scopes.inPeriod(cutoff))
      if (outcome) query.where('outcome', outcome)
      if (mcp) query.where('mcp_slug', mcp)
      if (token) query.where('access_token_prefix', token)
      return query
    }

    const paginator = await buildQuery().orderBy('created_at', 'desc').paginate(page, PAGE_SIZE)
    const logs = paginator.all()

    const selectedLog = filters.logId ? await McpCallLog.find(filters.logId) : null
    const mcpOptions = await McpCallLog.query()
      .select('mcp_slug', 'mcp_name')
      .whereNotNull('mcp_slug')
      .groupBy('mcp_slug', 'mcp_name')
      .orderBy('mcp_name', 'asc')
    const tokenOptions = await McpCallLog.query()
      .select('access_token_prefix', 'access_token_name')
      .groupBy('access_token_prefix', 'access_token_name')
      .orderBy('access_token_name', 'asc')
    const settings = await McpCallLogService.settings()

    return inertia.render('logs/index', {
      logs: McpCallLogTransformer.transform(logs),
      selectedLog: selectedLog ? McpCallLogTransformer.transform(selectedLog) : null,
      pagination: {
        page: paginator.currentPage,
        pageSize: paginator.perPage,
        total: paginator.total,
        totalPages: paginator.lastPage,
      },
      filters: { range, outcome, mcp, token },
      options: {
        mcps: mcpOptions.map((item) => ({
          value: item.mcpSlug!,
          label: item.mcpName ?? item.mcpSlug!,
        })),
        tokens: tokenOptions.map((item) => ({
          value: item.accessTokenPrefix,
          label: item.accessTokenName,
        })),
      },
      loggingLevel: settings.mcpLogLevel,
    })
  }
}
