import type { HttpContext } from '@adonisjs/core/http'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'
import AccessTokenService from '#services/access_token_service'
import {
  accessTokenParamsValidator,
  createAccessTokenValidator,
  deleteAccessTokensValidator,
  updateAccessTokenValidator,
} from '#validators/mcp'
import { publicOauthAppUrl } from '#services/public_url'
import AccessTokenTransformer from '#transformers/access_token_transformer'
import McpTransformer from '#transformers/mcp_transformer'

class AccessTokenCleanupError extends Error {
  constructor(readonly reason: 'missing' | 'active') {
    super(`Access token cleanup failed: ${reason}`)
  }
}

export default class AccessTokensController {
  async index({ inertia, session }: HttpContext) {
    const tokens = await AccessToken.query()
      .preload('mcps')
      .preload('oauthClient')
      .orderBy('created_at', 'desc')
    const mcps = await Mcp.query().orderBy('name', 'asc')

    const createdPlaintextRaw = session.flashMessages.get('createdPlaintext')
    const createdPlaintext = typeof createdPlaintextRaw === 'string' ? createdPlaintextRaw : null
    const appUrl = publicOauthAppUrl()

    return inertia.render('tokens/index', {
      tokens: AccessTokenTransformer.transform(tokens),
      mcps: McpTransformer.transform(mcps).useVariant('toOption'),
      gatewayUrl: appUrl ? `${appUrl}/mcp` : null,
      createdPlaintext,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const payload = await request.validateUsing(createAccessTokenValidator)
    const mcpIds = payload.mcpIds ?? []

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
      expiresAt: payload.expiresAt?.toUTC() ?? null,
      createdBy: auth.user!.id,
    })

    session.flash('success', 'Access token created — copy it now, it will not be shown again')
    session.flash('createdPlaintext', plaintext)
    return response.redirect().toRoute('tokens.index')
  }

  async update({ params, request, response, session }: HttpContext) {
    const { id } = await accessTokenParamsValidator.validate(params)
    const token = await AccessToken.find(id)
    if (!token) {
      session.flash('error', 'Token not found')
      return response.redirect().toRoute('tokens.index')
    }
    if (token.isRevoked) {
      session.flash('error', 'Revoked tokens cannot be edited')
      return response.redirect().toRoute('tokens.index')
    }
    if (token.source === 'oauth') {
      session.flash('error', 'OAuth connections cannot be edited — revoke the connection instead')
      return response.redirect().toRoute('tokens.index')
    }

    const payload = await request.validateUsing(updateAccessTokenValidator)
    const mcpIds = payload.mcpIds ?? []

    if (payload.scopeMode === 'selected') {
      const existing = await Mcp.query().whereIn('id', mcpIds)
      if (existing.length !== mcpIds.length) {
        session.flash('error', 'One or more selected MCPs do not exist')
        return response.redirect().toRoute('tokens.index')
      }
    }

    await AccessTokenService.update(token, {
      name: payload.name,
      scopeMode: payload.scopeMode,
      mcpIds,
      expiresAt: payload.expiresAt?.toUTC() ?? null,
    })

    session.flash('success', 'Token updated')
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

  async destroy({ request, response, session }: HttpContext) {
    const { ids } = await request.validateUsing(deleteAccessTokensValidator)
    const uniqueIds = [...new Set(ids)]

    try {
      await db.transaction(async (trx) => {
        const tokens = await AccessToken.query({ client: trx }).whereIn('id', uniqueIds)
        if (tokens.length !== uniqueIds.length) {
          throw new AccessTokenCleanupError('missing')
        }

        const now = DateTime.utc().toSQL({ includeOffset: false })!
        const [deletedCount] = await AccessToken.query({ client: trx })
          .whereIn('id', uniqueIds)
          .where((query) => {
            query
              .whereNotNull('revoked_at')
              .orWhere((manual) => {
                manual
                  .where('source', 'manual')
                  .whereNotNull('expires_at')
                  .where('expires_at', '<', now)
              })
              .orWhere((oauth) => {
                oauth
                  .where('source', 'oauth')
                  .whereNull('revoked_at')
                  .where((connection) => {
                    connection
                      .where((refresh) => {
                        refresh
                          .whereNotNull('oauth_refresh_expires_at')
                          .where('oauth_refresh_expires_at', '<', now)
                      })
                      .orWhere((fallback) => {
                        fallback
                          .whereNull('oauth_refresh_expires_at')
                          .whereNotNull('expires_at')
                          .where('expires_at', '<', now)
                      })
                  })
              })
          })
          .delete()

        if (deletedCount !== uniqueIds.length) {
          throw new AccessTokenCleanupError('active')
        }
      })
    } catch (error) {
      if (!(error instanceof AccessTokenCleanupError)) throw error

      session.flash(
        'error',
        error.reason === 'missing'
          ? 'One or more tokens no longer exist'
          : 'Active tokens must be revoked before they can be deleted'
      )
      return response.redirect().toRoute('tokens.index')
    }

    const suffix = uniqueIds.length === 1 ? '' : 's'
    session.flash('success', `${uniqueIds.length} token${suffix} deleted`)
    return response.redirect().toRoute('tokens.index')
  }
}
