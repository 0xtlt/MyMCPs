import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import type ApprovalRequest from '#models/approval_request'
import type User from '#models/user'
import ApprovalService from '#services/approvals/approval_service'
import ApprovalRequestTransformer from '#transformers/approval_request_transformer'
import { approvalDecisionValidator, approvalParamsValidator } from '#validators/approvals'

const MAX_LISTED_PAST_REQUESTS = 50

/** The requests this user may read and decide, with what the pages show of them. */
function requestsOf(user: User) {
  return ApprovalService.visibleTo(user).preload('mcp').preload('accessToken').preload('decider')
}

/**
 * The request a link names, or null when its `:id` is not one, matches none,
 * or names a request this user may not read: all three get the same answer.
 */
async function findRequest(params: HttpContext['params'], user: User) {
  const [, link] = await approvalParamsValidator.tryValidate(params)
  return link ? requestsOf(user).where('public_id', link.id).first() : null
}

const NOT_FOUND =
  'This approval request does not exist, was deleted after it expired, or belongs to the access token of another member'

async function summariesOf(requests: ApprovalRequest[]) {
  return new Map(
    await Promise.all(
      requests.map(async (request) => [request.id, await ApprovalService.summary(request)] as const)
    )
  )
}

/**
 * Where people decide the tool calls that agents may not make on their own:
 * administrators any of them, members the ones their own access tokens made.
 * What a page says about a call is read by MyMCPs from the call itself: the
 * agent only ever hands over the link.
 */
export default class ApprovalsController {
  async index({ inertia, auth }: HttpContext) {
    const now = DateTime.utc().toSQL({ includeOffset: false })!
    const waiting = await requestsOf(auth.user!)
      .where('status', 'pending')
      .where('expires_at', '>', now)
      .orderBy('created_at', 'desc')
    const past = await requestsOf(auth.user!)
      .where((query) => query.whereNot('status', 'pending').orWhere('expires_at', '<=', now))
      .orderBy('created_at', 'desc')
      .limit(MAX_LISTED_PAST_REQUESTS)

    const summaries = await summariesOf([...waiting, ...past])
    return inertia.render('approvals/index', {
      waiting: ApprovalRequestTransformer.transform(waiting, summaries),
      past: ApprovalRequestTransformer.transform(past, summaries),
    })
  }

  /**
   * The page behind the link an agent was given. Whoever follows it signs in
   * first and comes back here: the link names a request and grants nothing.
   */
  async show({ params, request, response, session, auth, inertia }: HttpContext) {
    // The router also sends HEAD requests here, which must not leave a return path behind.
    if (request.method() !== 'GET') {
      return response.header('Allow', 'GET').methodNotAllowed()
    }

    const [malformed, link] = await approvalParamsValidator.tryValidate(params)
    if (!auth.user) {
      if (!malformed) {
        session.put('approvalReturnTo', `/approvals/${link.id}`)
      }
      return response.redirect().withQs(false).toRoute('session.create')
    }

    const approval = await findRequest(params, auth.user)
    if (!approval) {
      session.flash('error', NOT_FOUND)
      return response.redirect().withQs(false).toRoute('approvals.index')
    }

    const summaries = await summariesOf([approval])
    const callArguments = ApprovalService.arguments(approval)
    response.header('Cache-Control', 'no-store')
    return inertia.render('approvals/show', {
      approval: ApprovalRequestTransformer.transform(approval, summaries),
      summary: summaries.get(approval.id) ?? null,
      // Pretty-printed here so the page shows the same text to everyone.
      arguments: callArguments ? JSON.stringify(callArguments, null, 2) : null,
      // Whether the call could still run if it were approved.
      runnable: approval.mcp.enabled && approval.accessToken.isUsable,
    })
  }

  async decide({ params, request, response, session, auth }: HttpContext) {
    const approval = await findRequest(params, auth.user!)
    if (!approval) {
      session.flash('error', NOT_FOUND)
      return response.redirect().toRoute('approvals.index')
    }

    const { decision } = await request.validateUsing(approvalDecisionValidator)
    if (await ApprovalService.decide(approval, decision, auth.user!)) {
      session.flash(
        'success',
        decision === 'approve'
          ? 'Approved. The agent can now run this call, once.'
          : 'Denied. The agent is told the call was refused.'
      )
    } else {
      session.flash('error', 'This request has expired or was already decided')
    }
    return response.redirect().toRoute('approvals.show', { id: approval.publicId })
  }
}
