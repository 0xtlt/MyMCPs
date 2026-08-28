import logger from '@adonisjs/core/services/logger'
import { DateTime } from 'luxon'
import type AccessToken from '#models/access_token'
import InstanceSetting from '#models/instance_setting'
import McpCallLog, { type McpCallErrorCategory, type McpCallOutcome } from '#models/mcp_call_log'
import McpDebugSession from '#models/mcp_debug_session'
import type Mcp from '#models/mcp'
import {
  isCredentialKey,
  mcpSensitiveValues,
  sanitizeDiagnostic,
  sanitizeMcpDiagnostic,
} from '#services/security_redaction'

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
  startedAt: DateTime
  debugSession?: McpDebugSession | null
}

export function sanitizeErrorSummary(value: unknown, mcp?: Mcp | null): string | null {
  return mcp ? sanitizeMcpDiagnostic(value, mcp) : sanitizeDiagnostic(value)
}

type CapturedValue = {
  serialized: string
  originalBytes: number
  redacted: boolean
}

function safeJson(value: unknown) {
  try {
    return JSON.stringify(value) ?? 'null'
  } catch {
    return JSON.stringify({ unserializable: true })
  }
}

export function captureValue(
  value: unknown,
  options: { redact?: boolean; mcp?: Mcp | null } = {}
): CapturedValue {
  const original = safeJson(value)
  const originalBytes = Buffer.byteLength(original)
  let redacted = false
  let serialized = original

  if (options.redact) {
    const sensitiveValues = options.mcp ? mcpSensitiveValues(options.mcp) : []
    try {
      serialized =
        JSON.stringify(value, (key, current) => {
          if (key && isCredentialKey(key)) {
            redacted = true
            return '[REDACTED]'
          }
          if (typeof current !== 'string') return current

          const sanitized =
            sanitizeDiagnostic(current, Number.MAX_SAFE_INTEGER, sensitiveValues) ?? current
          if (sanitized !== current) redacted = true
          return sanitized
        }) ?? 'null'
    } catch {
      serialized = JSON.stringify({ unserializable: true })
    }
  }

  if (Buffer.byteLength(serialized) <= MAX_CAPTURE_BYTES) {
    return { serialized, originalBytes, redacted }
  }

  return {
    serialized: JSON.stringify({
      truncated: true,
      originalBytes,
      // Eight thousand characters leaves room for JSON escaping while keeping
      // the complete wrapper below the 64 KiB field budget.
      preview: serialized.slice(0, 8 * 1024),
    }),
    originalBytes,
    redacted,
  }
}

export function serializeCapturedValue(value: unknown) {
  return captureValue(value).serialized
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
      const currentDebugSession = input.debugSession
        ? await McpDebugSession.find(input.debugSession.id)
        : null
      const isDebugCapture =
        currentDebugSession?.status === 'active' &&
        currentDebugSession.stateVersion === input.debugSession?.stateVersion
      if (settings.mcpLogLevel === 'off' && !isDebugCapture) {
        return
      }

      const argumentsCaptured =
        isDebugCapture ||
        settings.mcpLogLevel === 'arguments' ||
        settings.mcpLogLevel === 'responses'
      const responseCaptured = isDebugCapture || settings.mcpLogLevel === 'responses'
      const argumentCapture = captureValue(input.args, {
        redact: isDebugCapture,
        mcp: input.mcp,
      })
      const responseCapture = captureValue(input.response, {
        redact: isDebugCapture,
        mcp: input.mcp,
      })
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
          argumentsCaptured && input.args !== undefined ? argumentCapture.serialized : null,
        argumentsCaptured,
        argumentsBytes: input.args === undefined ? 0 : argumentCapture.originalBytes,
        argumentsRedacted: argumentsCaptured && argumentCapture.redacted,
        response:
          responseCaptured && input.response !== undefined ? responseCapture.serialized : null,
        responseCaptured,
        responseBytes: input.response === undefined ? 0 : responseCapture.originalBytes,
        responseRedacted: responseCaptured && responseCapture.redacted,
        durationMs: Math.max(0, Math.round(input.durationMs)),
        startedAt: input.startedAt,
        debugSessionId: isDebugCapture ? currentDebugSession.id : null,
        debugSessionElapsedMs: isDebugCapture
          ? Math.max(
              0,
              Math.round(
                input.startedAt.diff(currentDebugSession.startedAt).as('milliseconds') -
                  currentDebugSession.pausedDurationMs
              )
            )
          : null,
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
