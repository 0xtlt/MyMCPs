import type { HttpContext } from '@adonisjs/core/http'
import { DateTime, IANAZone } from 'luxon'
import AccessToken from '#models/access_token'
import McpCallLog from '#models/mcp_call_log'
import McpDebugSession from '#models/mcp_debug_session'
import McpCallLogTransformer from '#transformers/mcp_call_log_transformer'
import McpDebugSessionTransformer from '#transformers/mcp_debug_session_transformer'
import {
  debugSessionQueryValidator,
  startDebugSessionValidator,
  updateDebugSessionValidator,
} from '#validators/mcp_debug_session'

const MAX_SESSION_CALLS = 500

function resolveTimeZone(timeZone: string | undefined) {
  return timeZone && IANAZone.isValidZone(timeZone) ? timeZone : 'UTC'
}

export default class DebugSessionsController {
  async index({ request, inertia }: HttpContext) {
    const filters = await request.validateUsing(debugSessionQueryValidator)
    const sessions = await McpDebugSession.query().orderBy('started_at', 'desc').limit(50)
    const selectedSession = filters.sessionId
      ? (sessions.find((item) => item.id === filters.sessionId) ??
        (await McpDebugSession.find(filters.sessionId)))
      : sessions[0]
    if (selectedSession && !sessions.some((item) => item.id === selectedSession.id)) {
      sessions.push(selectedSession)
    }
    const calls = selectedSession
      ? await McpCallLog.query()
          .where('debug_session_id', selectedSession.id)
          .orderBy('started_at', 'asc')
          .limit(MAX_SESSION_CALLS)
      : []
    const selectedCall = filters.callId
      ? (calls.find((item) => item.id === filters.callId) ?? null)
      : null
    const allTokens = await AccessToken.query().orderBy('name', 'asc')
    const tokens = allTokens.filter((token) => token.isActive)

    return inertia.render('debug/index', {
      sessions: McpDebugSessionTransformer.transform(sessions),
      selectedSession: selectedSession
        ? McpDebugSessionTransformer.transform(selectedSession)
        : null,
      calls: McpCallLogTransformer.transform(calls),
      selectedCall: selectedCall ? McpCallLogTransformer.transform(selectedCall) : null,
      tokens: tokens.map((token) => ({
        id: token.id,
        name: token.name,
        prefix: token.tokenPrefix,
      })),
      filters: { timeZone: resolveTimeZone(filters.timeZone) },
      observedAt: DateTime.utc().toISO()!,
      maxSessionCalls: MAX_SESSION_CALLS,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const { accessTokenId } = await request.validateUsing(startDebugSessionValidator)
    const accessToken = await AccessToken.find(accessTokenId)
    if (!accessToken || !accessToken.isActive) {
      session.flash('error', 'Choose an active access token to start debugging')
      return response.redirect().toPath('/debug')
    }

    const existing = await McpDebugSession.query()
      .where('access_token_id', accessToken.id)
      .whereIn('status', ['active', 'paused'])
      .first()
    if (existing) {
      session.flash('error', 'This access token already has an open debug session')
      return response.redirect().toPath(`/debug?sessionId=${existing.id}`)
    }

    let debugSession: McpDebugSession
    try {
      debugSession = await McpDebugSession.create({
        accessTokenId: accessToken.id,
        accessTokenName: accessToken.name,
        accessTokenPrefix: accessToken.tokenPrefix,
        status: 'active',
        pausedDurationMs: 0,
        stateVersion: 0,
        createdBy: auth.user!.id,
        startedAt: DateTime.utc(),
        pausedAt: null,
        endedAt: null,
      })
    } catch (error) {
      const concurrent = await McpDebugSession.query()
        .where('access_token_id', accessToken.id)
        .whereIn('status', ['active', 'paused'])
        .first()
      if (concurrent) {
        session.flash('error', 'This access token already has an open debug session')
        return response.redirect().toPath(`/debug?sessionId=${concurrent.id}`)
      }
      throw error
    }

    session.flash('success', 'Debug session started')
    return response.redirect().toPath(`/debug?sessionId=${debugSession.id}`)
  }

  async update({ params, request, response, session }: HttpContext) {
    const payload = await request.validateUsing(updateDebugSessionValidator)
    const debugSession = await McpDebugSession.find(params.id)
    if (!debugSession) {
      session.flash('error', 'Debug session not found')
      return response.redirect().toPath('/debug')
    }

    const now = DateTime.utc()
    if (payload.action === 'pause' && debugSession.status === 'active') {
      debugSession.status = 'paused'
      debugSession.pausedAt = now
      session.flash('success', 'Debug capture paused')
    } else if (payload.action === 'resume' && debugSession.status === 'paused') {
      debugSession.pausedDurationMs += Math.max(
        0,
        Math.round(now.diff(debugSession.pausedAt!).as('milliseconds'))
      )
      debugSession.status = 'active'
      debugSession.pausedAt = null
      session.flash('success', 'Debug capture continued')
    } else if (payload.action === 'stop' && debugSession.status !== 'stopped') {
      if (debugSession.status === 'paused') {
        debugSession.pausedDurationMs += Math.max(
          0,
          Math.round(now.diff(debugSession.pausedAt!).as('milliseconds'))
        )
        debugSession.pausedAt = null
      }
      debugSession.status = 'stopped'
      debugSession.endedAt = now
      session.flash('success', 'Debug session stopped')
    } else {
      session.flash('error', 'That action is not available for this debug session')
      return response.redirect().toPath(`/debug?sessionId=${debugSession.id}`)
    }

    debugSession.stateVersion += 1
    await debugSession.save()
    return response.redirect().toPath(`/debug?sessionId=${debugSession.id}`)
  }
}
