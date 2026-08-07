import type { HttpContext } from '@adonisjs/core/http'
import type AccessToken from '#models/access_token'
import AccessTokenService from '#services/access_token_service'
import { GATEWAY_OAUTH_SCOPE } from '#services/gateway_oauth'
import { publicAppUrl } from '#services/public_url'

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
    if (!token) {
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

  private challenge(ctx: HttpContext, error?: 'invalid_token') {
    const appUrl = publicAppUrl()
    const values = [
      ...(error ? [`error="${error}"`] : []),
      ...(appUrl
        ? [
            `resource_metadata="${new URL('/.well-known/oauth-protected-resource/mcp', appUrl).href}"`,
          ]
        : []),
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
