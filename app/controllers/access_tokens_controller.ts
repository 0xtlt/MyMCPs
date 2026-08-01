import type { HttpContext } from '@adonisjs/core/http'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'
import AccessTokenService from '#services/access_token_service'
import { createAccessTokenValidator } from '#validators/mcp'
import env from '#start/env'

function serializeToken(token: AccessToken, mcpIds: number[] = []) {
  return {
    id: token.id,
    name: token.name,
    tokenPrefix: token.tokenPrefix,
    scopeMode: token.scopeMode,
    mcpIds,
    expiresAt: token.expiresAt?.toISO() ?? null,
    revokedAt: token.revokedAt?.toISO() ?? null,
    lastUsedAt: token.lastUsedAt?.toISO() ?? null,
    createdAt: token.createdAt.toISO(),
    isUsable: token.isUsable,
  }
}

export default class AccessTokensController {
  async index({ inertia, session, request }: HttpContext) {
    const tokens = await AccessToken.query().preload('mcps').orderBy('created_at', 'desc')
    const mcps = await Mcp.query().orderBy('name', 'asc')

    const configured = env.get('APP_URL').replace(/\/$/, '')
    const requestOrigin = `${request.protocol()}://${request.host()}`
    const appUrl = request.host()?.includes('localhost') ? requestOrigin : configured

    const createdPlaintext = session.flashMessages.get('createdPlaintext') as string | undefined

    return inertia.render('tokens/index', {
      tokens: tokens.map((token) => serializeToken(token, token.mcps.map((m) => m.id))),
      mcps: mcps.map((mcp) => ({
        id: mcp.id,
        name: mcp.name,
        slug: mcp.slug,
        enabled: mcp.enabled,
      })),
      gatewayUrl: `${appUrl}/mcp`,
      createdPlaintext: createdPlaintext ?? null,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const payload = await request.validateUsing(createAccessTokenValidator)

    let mcpIds = (payload.mcpIds ?? []).map((id) => Number(id)).filter((id) => Number.isFinite(id) && id > 0)

    if (payload.scopeMode === 'selected' && mcpIds.length === 0) {
      session.flash('error', 'Select at least one MCP, or choose access to all MCPs')
      return response.redirect().toRoute('tokens.index')
    }

    let expiresAt: DateTime | null = null
    if (payload.expiresAt?.trim()) {
      const parsed = DateTime.fromISO(payload.expiresAt, { zone: 'utc' })
      if (!parsed.isValid) {
        session.flash('error', 'Invalid expiration date')
        return response.redirect().toRoute('tokens.index')
      }
      expiresAt = parsed
    }

    if (payload.scopeMode === 'selected') {
      const existing = await Mcp.query().whereIn('id', mcpIds)
      if (existing.length !== mcpIds.length) {
        session.flash('error', 'One or more selected MCPs do not exist')
        return response.redirect().toRoute('tokens.index')
      }
    }

    const { plaintext } = await AccessTokenService.create({
      name: payload.name,
      scopeMode: payload.scopeMode,
      mcpIds: payload.scopeMode === 'selected' ? mcpIds : [],
      expiresAt,
      createdBy: auth.user!.id,
    })

    session.flash('success', 'Access token created — copy it now, it will not be shown again')
    session.flash('createdPlaintext', plaintext)
    return response.redirect().toRoute('tokens.index')
  }

  async revoke({ params, response, session }: HttpContext) {
    const token = await AccessToken.find(params.id)
    if (!token) {
      session.flash('error', 'Token not found')
      return response.redirect().toRoute('tokens.index')
    }
    if (token.isRevoked) {
      session.flash('error', 'Token already revoked')
      return response.redirect().toRoute('tokens.index')
    }
    await AccessTokenService.revoke(token)
    session.flash('success', 'Token revoked')
    return response.redirect().toRoute('tokens.index')
  }
}
