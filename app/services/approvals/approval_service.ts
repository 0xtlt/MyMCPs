import { createHash, randomBytes } from 'node:crypto'
import logger from '@adonisjs/core/services/logger'
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js'
import { DateTime } from 'luxon'
import type AccessToken from '#models/access_token'
import ApprovalRequest from '#models/approval_request'
import type Mcp from '#models/mcp'
import type User from '#models/user'
import { toolApprovalMode } from '#services/approvals/policy'
import { argumentsSummary, type SavedApprovalSummary } from '#services/approvals/summary'
import { BuiltinToolError } from '#services/builtin/definition'
import { requireBuiltinMcp } from '#services/builtin/registry'
import { describeBuiltinCall } from '#services/builtin/runtime'
import McpSecretStore from '#services/mcp_secret_store'
import { publicOauthAppUrl } from '#services/public_url'
import { sanitizeDiagnostic } from '#services/security_redaction'
import { probeUpstream } from '#services/upstream/manager'
import { savedApprovalSummaryValidator } from '#validators/approvals'

/** How long a person has to decide. */
export const APPROVAL_PENDING_HOURS = 24
/** How long the agent has to run a call once it is approved. */
export const APPROVAL_GRANT_HOURS = 24

/** An agent cannot bury a request under others, nor fill the table. */
const MAX_PENDING_PER_TOKEN = 20
/** Kept encrypted and shown whole on the page of the request. */
const MAX_ARGUMENT_BYTES = 256 * 1024
/** Decided and expired requests stay listed this long. */
const KEPT_DAYS = 30
const PRUNE_INTERVAL_MS = 60 * 60 * 1000

let lastPrunedAt = 0

type HeldCategory = 'approval_required' | 'approval_denied' | 'tool_error'

/**
 * Whether a call may run now. A held call comes with what the agent is told
 * and how the call log files it.
 */
export type ApprovalGate =
  { held: false } | { held: true; category: HeldCategory; reason: string; result: CallToolResult }

export type GatedCall = {
  accessToken: AccessToken
  mcp: Mcp
  toolName: string
  args: Record<string, unknown> | undefined
}

/** The call cannot be put to a person, and the agent is told why. */
class ApprovalRefusal extends Error {}

function sqlTime(time: DateTime) {
  return time.toSQL({ includeOffset: false })!
}

/**
 * How many rows an update changed. SQLite answers with that number in an
 * array, whether or not the query asks for the rows back.
 */
function changedRows(result: unknown) {
  return Number(Array.isArray(result) ? result[0] : result)
}

function sorted(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sorted)
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sorted((value as Record<string, unknown>)[key])])
    )
  }
  return value
}

/**
 * Identifies the arguments of a call whatever order their keys came in. An
 * approval is for one hash: any other value in the arguments is another call.
 */
export function argumentsHash(args: Record<string, unknown> | undefined) {
  return createHash('sha256')
    .update(JSON.stringify(sorted(args ?? {})))
    .digest('hex')
}

// The gateway serves parallel requests in one Node process. A call that asks
// first reads what is already waiting, then adds to it: two calls of the same
// access token doing that at once would both add, past the limit, or twice
// for the same call.
const turns = new Map<number, Promise<unknown>>()

/** Run `task` once the earlier tasks of the same access token have finished. */
function inTurn<Result>(accessTokenId: number, task: () => Promise<Result>): Promise<Result> {
  const result = (turns.get(accessTokenId) ?? Promise.resolve()).then(task, task)
  const settled = result.catch(() => {})
  turns.set(accessTokenId, settled)
  void settled.then(() => {
    if (turns.get(accessTokenId) === settled) turns.delete(accessTokenId)
  })
  return result
}

function held(category: HeldCategory, reason: string, text: string): ApprovalGate {
  return {
    held: true,
    category,
    reason,
    result: { content: [{ type: 'text', text }], isError: true },
  }
}

export default class ApprovalService {
  /** The page where a person reads and decides a request. `null` without `APP_URL`. */
  static url(request: ApprovalRequest) {
    const appUrl = publicOauthAppUrl()
    return appUrl ? `${appUrl}/approvals/${request.publicId}` : null
  }

  /**
   * Decide whether a tool call runs now. A tool set to ask is held until a
   * person approved this very call: same token, MCP, tool and arguments. The
   * approval is spent by the call it lets through.
   */
  static async gate(call: GatedCall): Promise<ApprovalGate> {
    if ((await toolApprovalMode(call.mcp, call.toolName)) === 'auto') {
      return { held: false }
    }
    return inTurn(call.accessToken.id, () => this.decided(call))
  }

  /** What the last request for this very call says, or a new request for it. */
  private static async decided(call: GatedCall): Promise<ApprovalGate> {
    const { accessToken, mcp, toolName, args } = call
    const latest = await ApprovalRequest.query()
      .where('access_token_id', accessToken.id)
      .where('mcp_id', mcp.id)
      .where('tool_name', toolName)
      .where('arguments_hash', argumentsHash(args))
      .whereNull('consumed_at')
      .where('expires_at', '>', sqlTime(DateTime.utc()))
      .orderBy('id', 'desc')
      .first()

    if (latest?.status === 'approved' && (await this.consume(latest))) {
      return { held: false }
    }
    if (latest?.status === 'denied') {
      // Said once. The same call made again is a new request, for the day
      // the person changes their mind.
      await this.consume(latest)
      return held(
        'approval_denied',
        'A person denied this call',
        `Denied: a person refused this call to ${toolName} on ${mcp.name} in MyMCPs, and it was not run. Do not make it again unless they ask you to.`
      )
    }
    if (latest?.status === 'pending') {
      return this.waiting(latest, call, false)
    }

    try {
      return this.waiting(await this.request(call), call, true)
    } catch (error) {
      if (error instanceof ApprovalRefusal || error instanceof BuiltinToolError) {
        return held('tool_error', 'The call was refused before asking for approval', error.message)
      }
      throw error
    }
  }

  private static waiting(request: ApprovalRequest, call: GatedCall, isNew: boolean): ApprovalGate {
    const { mcp, toolName } = call
    const url = this.url(request)
    if (!url) {
      return held(
        'approval_required',
        'Approval links need APP_URL',
        `${toolName} on ${mcp.name} needs the approval of a person, and it was not run. There is no link to give them, because this MyMCPs instance does not know its public address (APP_URL): ask them to open the Approvals page of MyMCPs and decide the call there, then call ${toolName} again with exactly the same arguments.`
      )
    }

    return held(
      'approval_required',
      isNew ? 'Waiting for approval' : 'Still waiting for approval',
      [
        `${isNew ? 'Approval required' : 'Still waiting for approval'}: ${toolName} on ${mcp.name} was not run.`,
        `A person has to approve this exact call in MyMCPs first. Give them this link: ${url}`,
        `They sign in, read what the call would do, and approve or deny it. The link works until ${request.expiresAt.toUTC().toISO({ suppressMilliseconds: true })}.`,
        `Once they have approved, call ${toolName} again with exactly the same arguments and it runs. Other arguments are another call, which needs its own approval.`,
      ].join('\n')
    )
  }

  /** Put a call to a person. Throws when it is not one to put to them. */
  private static async request(call: GatedCall) {
    const { accessToken, mcp, toolName, args } = call
    const serialized = JSON.stringify(args ?? {})
    if (Buffer.byteLength(serialized) > MAX_ARGUMENT_BYTES) {
      throw new ApprovalRefusal(
        `${toolName} on ${mcp.name} needs the approval of a person, who cannot be shown arguments of more than ${MAX_ARGUMENT_BYTES / 1024} KB. Make the call smaller.`
      )
    }

    const now = DateTime.utc()
    const [{ $extras: waiting }] = await ApprovalRequest.query()
      .where('access_token_id', accessToken.id)
      .where('status', 'pending')
      .where('expires_at', '>', sqlTime(now))
      .count('* as total')
    if (Number(waiting.total) >= MAX_PENDING_PER_TOKEN) {
      throw new ApprovalRefusal(
        `${MAX_PENDING_PER_TOKEN} calls of this access token already wait for approval in MyMCPs. Ask the person to decide them before asking for more.`
      )
    }

    const summary = await this.summarize(call)
    return ApprovalRequest.create({
      publicId: randomBytes(24).toString('base64url'),
      mcpId: mcp.id,
      accessTokenId: accessToken.id,
      toolName,
      arguments: McpSecretStore.encrypt(serialized)!,
      argumentsHash: argumentsHash(args),
      summary: McpSecretStore.encrypt(JSON.stringify(summary))!,
      status: 'pending',
      expiresAt: now.plus({ hours: APPROVAL_PENDING_HOURS }),
    })
  }

  /**
   * Read the call for the person who decides. A built-in tool checks the
   * arguments and describes the change itself. For any other tool the
   * arguments are listed as they are, beside what its MCP says the tool does.
   */
  private static async summarize(call: GatedCall): Promise<SavedApprovalSummary> {
    const { mcp, toolName, args } = call
    if (mcp.transport === 'builtin') {
      const described = await describeBuiltinCall(mcp, toolName, args)
      if (described) {
        return { ...described, interpreted: true, toolDescription: null }
      }
      const tool = requireBuiltinMcp(mcp).tools.find((candidate) => candidate.name === toolName)
      return argumentsSummary(mcp, toolName, args, tool?.description)
    }

    const tools = await probeUpstream(mcp)
    const tool = tools.find((candidate) => candidate.name === toolName)
    if (!tool) {
      throw new ApprovalRefusal(`${mcp.name} has no tool named "${toolName}"`)
    }
    return argumentsSummary(mcp, toolName, args, tool.description)
  }

  /**
   * Mark a decided request as acted on by the agent. Only one of two calls
   * made at the same moment gets the approval.
   */
  private static async consume(request: ApprovalRequest) {
    const consumed = await ApprovalRequest.query()
      .where('id', request.id)
      .where('status', request.status)
      .whereNull('consumed_at')
      .update({ consumedAt: sqlTime(DateTime.utc()) })
    return changedRows(consumed) === 1
  }

  /**
   * Record what a person decided. False when the request was decided in the
   * meantime or has expired. An approval gives the agent a day to run the call.
   */
  static async decide(request: ApprovalRequest, decision: 'approve' | 'deny', user: User) {
    const now = DateTime.utc()
    const decided = await ApprovalRequest.query()
      .where('id', request.id)
      .where('status', 'pending')
      .where('expires_at', '>', sqlTime(now))
      .update({
        status: decision === 'approve' ? 'approved' : 'denied',
        decidedBy: user.id,
        decidedAt: sqlTime(now),
        updatedAt: sqlTime(now),
        ...(decision === 'approve'
          ? { expiresAt: sqlTime(now.plus({ hours: APPROVAL_GRANT_HOURS })) }
          : {}),
      })
    return changedRows(decided) === 1
  }

  /** The arguments of the call, exactly as the agent sent them. */
  static arguments(request: ApprovalRequest): Record<string, unknown> | null {
    const serialized = McpSecretStore.decrypt(request.arguments)
    if (!serialized) return null
    try {
      return JSON.parse(serialized)
    } catch {
      return null
    }
  }

  /** `null` when the summary can no longer be read, after `APP_KEY` changed for instance. */
  static async summary(request: ApprovalRequest): Promise<SavedApprovalSummary | null> {
    const serialized = McpSecretStore.decrypt(request.summary)
    if (!serialized) return null
    try {
      const [unreadable, summary] = await savedApprovalSummaryValidator.tryValidate(
        JSON.parse(serialized)
      )
      return unreadable ? null : summary
    } catch {
      return null
    }
  }

  /**
   * The requests a user may read and decide: an administrator all of them, a
   * member the ones their own access tokens made. A request shows the
   * arguments of a call, which the call log only shows to administrators.
   */
  static visibleTo(user: User) {
    const query = ApprovalRequest.query()
    if (!user.isAdmin) {
      query.whereHas('accessToken', (token) => token.where('created_by', user.id))
    }
    return query
  }

  /** How many requests wait for a decision this user can make. */
  static async pendingCount(user: User) {
    const [{ $extras: waiting }] = await this.visibleTo(user)
      .where('status', 'pending')
      .where('expires_at', '>', sqlTime(DateTime.utc()))
      .count('* as total')
    return Number(waiting.total)
  }

  /** Delete the requests nobody can act on any more, a month after they ended. */
  static async pruneExpired(options: { force?: boolean } = {}) {
    const now = Date.now()
    if (!options.force && now - lastPrunedAt < PRUNE_INTERVAL_MS) {
      return
    }
    lastPrunedAt = now

    try {
      await ApprovalRequest.query()
        .where('expires_at', '<', sqlTime(DateTime.utc().minus({ days: KEPT_DAYS })))
        .delete()
    } catch (error) {
      logger.warn(
        { error: sanitizeDiagnostic(error) },
        'Expired approval requests could not be pruned'
      )
    }
  }
}
