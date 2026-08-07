import type { HttpContext } from '@adonisjs/core/http'
import type AccessToken from '#models/access_token'
import AccessTokenService from '#services/access_token_service'
import {
  GATEWAY_OAUTH_SCOPE,
  gatewayResourceUrl,
  protectedResourceMetadataUrl,
} from '#services/gateway_oauth'

/**
 * Authenticate agent requests to /mcp with a Bearer access token.
 * Attaches `accessToken` and `allowedMcps` on the HTTP context.
 */
export default class McpBearerMiddleware {
  async handle(ctx: HttpContext, next: () => Promise<void>) {
    const header = ctx.request.header('authorization')
    if (!header?.toLowerCase().startsWith('bearer ')) {
      this.challenge(ctx)
      return ctx.response.status(401).json({
        error: 'unauthorized',
        message: 'Missing Bearer access token',
      })
    }

    const plaintext = header.slice(7).trim()
    if (!plaintext) {
      this.challenge(ctx)
      return ctx.response.status(401).json({
        error: 'unauthorized',
        message: 'Empty Bearer access token',
      })
    }

    const token = await AccessTokenService.findUsableByPlaintext(plaintext)
    if (!token || !this.hasValidOauthAudienceAndScope(token)) {
      this.challenge(ctx, 'invalid_token')
      return ctx.response.status(401).json({
        error: 'unauthorized',
        message: 'Invalid, expired, or revoked access token',
      })
    }

    await AccessTokenService.touchLastUsed(token)
    const allowedMcps = await AccessTokenService.resolveAllowedMcps(token)

    ctx.accessToken = token
    ctx.allowedMcps = allowedMcps

    return next()
  }

  private hasValidOauthAudienceAndScope(token: AccessToken) {
    if (token.source !== 'oauth') return true

    try {
      return (
        token.oauthResource !== null &&
        new URL(token.oauthResource).href === gatewayResourceUrl() &&
        token.oauthScopes === GATEWAY_OAUTH_SCOPE
      )
    } catch {
      return false
    }
  }

  private challenge(ctx: HttpContext, error?: 'invalid_token') {
    let resourceMetadata: string | null = null
    try {
      resourceMetadata = protectedResourceMetadataUrl()
    } catch {
      // Keep bearer authentication available while refusing to publish insecure OAuth metadata.
    }
    const values = [
      ...(error ? [`error="${error}"`] : []),
      ...(resourceMetadata ? [`resource_metadata="${resourceMetadata}"`] : []),
      `scope="${GATEWAY_OAUTH_SCOPE}"`,
    ]
    ctx.response.header('WWW-Authenticate', `Bearer ${values.join(', ')}`)
  }
}

declare module '@adonisjs/core/http' {
  interface HttpContext {
    accessToken?: AccessToken
    allowedMcps?: Awaited<ReturnType<typeof AccessTokenService.resolveAllowedMcps>>
  }
}
