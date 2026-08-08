import logger from '@adonisjs/core/services/logger'
import { DateTime } from 'luxon'
import type AccessToken from '#models/access_token'
import InstanceSetting from '#models/instance_setting'
import McpCallLog, { type McpCallErrorCategory, type McpCallOutcome } from '#models/mcp_call_log'
import type Mcp from '#models/mcp'
import { sanitizeDiagnostic, sanitizeMcpDiagnostic } from '#services/security_redaction'

const PRUNE_INTERVAL_MS = 60 * 60 * 1000
const MAX_PENDING_WRITES = 1_000
export const MAX_CAPTURE_BYTES = 64 * 1024

let lastPrunedAt = 0

export type McpCallLogInput = {
  accessToken: AccessToken
  callerIp: string | null
  mcp?: Mcp | null
  mcpSlug?: string | null
  requestedToolName: string
  toolName?: string | null
  args: Record<string, unknown> | undefined
  response?: unknown
  outcome: McpCallOutcome
  errorCategory?: McpCallErrorCategory | null
  errorSummary?: unknown
  durationMs: number
}

export function sanitizeErrorSummary(value: unknown, mcp?: Mcp | null): string | null {
  return mcp ? sanitizeMcpDiagnostic(value, mcp) : sanitizeDiagnostic(value)
}

export function serializeCapturedValue(value: unknown) {
  const serialized = JSON.stringify(value) ?? 'null'
  const originalBytes = Buffer.byteLength(serialized)
  if (originalBytes <= MAX_CAPTURE_BYTES) {
    return serialized
  }

  return JSON.stringify({
    truncated: true,
    originalBytes,
    // Eight thousand characters leaves room for JSON escaping while keeping
    // the complete wrapper below the 64 KiB field budget.
    preview: serialized.slice(0, 8 * 1024),
  })
}

export default class McpCallLogService {
  static #pendingWrites = 0
  static #writeQueue: Promise<void> = Promise.resolve()

  static async settings() {
    return InstanceSetting.current()
  }

  static record(input: McpCallLogInput) {
    if (this.#pendingWrites >= MAX_PENDING_WRITES) {
      logger.warn(
        { maxPendingWrites: MAX_PENDING_WRITES },
        'MCP call log queue is full; record was dropped'
      )
      return
    }

    this.#pendingWrites += 1
    this.#writeQueue = this.#writeQueue
      .then(() => this.persist(input))
      .catch((error) => {
        logger.warn({ err: error }, 'MCP call log could not be persisted')
      })
      .finally(() => {
        this.#pendingWrites -= 1
      })
  }

  static async flush() {
    await this.#writeQueue
  }

  private static async persist(input: McpCallLogInput) {
    try {
      const settings = await this.settings()
      if (settings.mcpLogLevel === 'off') {
        return
      }

      const argumentsCaptured =
        settings.mcpLogLevel === 'arguments' || settings.mcpLogLevel === 'responses'
      const responseCaptured = settings.mcpLogLevel === 'responses'
      await McpCallLog.create({
        accessTokenId: input.accessToken.id,
        accessTokenName: input.accessToken.name,
        accessTokenPrefix: input.accessToken.tokenPrefix,
        callerIp: input.callerIp,
        mcpId: input.mcp?.id ?? null,
        mcpName: input.mcp?.name ?? null,
        mcpSlug: input.mcp?.slug ?? input.mcpSlug ?? null,
        requestedToolName: input.requestedToolName.slice(0, 512),
        toolName: input.toolName?.slice(0, 254) ?? null,
        outcome: input.outcome,
        errorCategory: input.errorCategory ?? null,
        errorSummary: sanitizeErrorSummary(input.errorSummary, input.mcp),
        arguments:
          argumentsCaptured && input.args !== undefined ? serializeCapturedValue(input.args) : null,
        argumentsCaptured,
        response:
          responseCaptured && input.response !== undefined
            ? serializeCapturedValue(input.response)
            : null,
        responseCaptured,
        durationMs: Math.max(0, Math.round(input.durationMs)),
      })
    } catch (error) {
      logger.warn({ err: error }, 'MCP call log could not be persisted')
    }
  }

  static async pruneExpired(options: { force?: boolean } = {}) {
    const now = Date.now()
    if (!options.force && now - lastPrunedAt < PRUNE_INTERVAL_MS) {
      return 0
    }
    lastPrunedAt = now

    try {
      const settings = await this.settings()
      const cutoff = DateTime.utc().minus({ days: settings.mcpLogRetentionDays })
      const deleted = await McpCallLog.query().where('created_at', '<', cutoff.toSQL()!).delete()
      return Array.isArray(deleted) ? deleted.length : Number(deleted)
    } catch (error) {
      logger.warn({ err: error }, 'Expired MCP call logs could not be pruned')
      return 0
    }
  }
}
