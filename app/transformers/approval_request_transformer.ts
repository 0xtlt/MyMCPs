import type ApprovalRequest from '#models/approval_request'
import type { SavedApprovalSummary } from '#services/approvals/summary'
import { BaseTransformer } from '@adonisjs/core/transformers'

/** The summaries of the requests to transform, by request id. Read ahead: reading one is asynchronous. */
export type ApprovalSummaries = ReadonlyMap<number, SavedApprovalSummary | null>

/**
 * An approval request for the pages. Expects `mcp`, `accessToken` and
 * `decider` to be preloaded.
 */
export default class ApprovalRequestTransformer extends BaseTransformer<ApprovalRequest> {
  #summaries: ApprovalSummaries

  constructor(request: ApprovalRequest, summaries: ApprovalSummaries) {
    super(request)
    this.#summaries = summaries
  }

  /** One row of the list. */
  toObject() {
    const request = this.resource
    const summary = this.#summaries.get(request.id) ?? null
    return {
      id: request.publicId,
      state: request.state,
      toolName: request.toolName,
      // `null` when the summary can no longer be decrypted.
      title: summary?.title ?? null,
      interpreted: summary?.interpreted ?? false,
      mcp: { id: request.mcp.id, name: request.mcp.name, slug: request.mcp.slug },
      accessToken: { name: request.accessToken.name, prefix: request.accessToken.tokenPrefix },
      createdAt: request.createdAt.toISO()!,
      expiresAt: request.expiresAt.toISO()!,
      decidedAt: request.decidedAt?.toISO() ?? null,
      decidedBy: request.decider ? request.decider.fullName || request.decider.email : null,
      consumedAt: request.consumedAt?.toISO() ?? null,
    }
  }
}
