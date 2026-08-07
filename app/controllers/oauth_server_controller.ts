import type { HttpContext } from '@adonisjs/core/http'
import AccessTokenService from '#services/access_token_service'
import {
  GatewayOauthError,
  authenticateOauthClient,
  authorizationServerMetadata,
  createAuthorizationCode,
  exchangeAuthorizationCode,
  exchangeRefreshToken,
  oauthRedirect,
  oauthTokenResponse,
  parseAuthorizationRequest,
  protectedResourceMetadata,
  registerOauthClient,
} from '#services/gateway_oauth'
import {
  oauthAuthorizationRateLimiter,
  oauthRegistrationRateLimiter,
  oauthTokenRateLimiter,
} from '#start/limiter'
import { requirePublicAppUrl } from '#services/public_url'

function requiredString(input: Record<string, unknown>, key: string) {
  const value = input[key]
  if (typeof value !== 'string' || value.length === 0) {
    throw new GatewayOauthError('invalid_request', `${key} is required`)
  }
  return value
}

function authorizationReturnPath(request: Awaited<ReturnType<typeof parseAuthorizationRequest>>) {
  const params = new URLSearchParams({
    client_id: request.client.clientId,
    redirect_uri: request.redirectUri,
    response_type: 'code',
    code_challenge: request.codeChallenge,
    code_challenge_method: 'S256',
    scope: request.scopes,
    resource: request.resource,
  })
  if (request.state) params.set('state', request.state)
  return `/authorize?${params}`
}

function redirectToOauthClient(ctx: HttpContext, location: string) {
  if (ctx.request.header('x-inertia')) {
    ctx.response.header('X-Inertia-Location', location)
    return ctx.response.status(409).send('')
  }
  return ctx.response.redirect(location)
}

export default class OauthServerController {
  async authorizationMetadata({ response }: HttpContext) {
    try {
      const metadata = authorizationServerMetadata()
      response.header('Cache-Control', 'public, max-age=3600')
      return response.ok(metadata)
    } catch {
      response.header('Cache-Control', 'no-store')
      return response.status(503).json({
        error: 'temporarily_unavailable',
        error_description: 'OAuth requires APP_URL to be a public HTTPS origin',
      })
    }
  }

  async protectedResourceMetadata({ response }: HttpContext) {
    try {
      const metadata = protectedResourceMetadata()
      response.header('Cache-Control', 'public, max-age=3600')
      return response.ok(metadata)
    } catch {
      response.header('Cache-Control', 'no-store')
      return response.status(503).json({
        error: 'temporarily_unavailable',
        error_description: 'OAuth requires APP_URL to be a public HTTPS origin',
      })
    }
  }

  async register(ctx: HttpContext) {
    if (!this.isConfigured(ctx)) return

    if (
      !(await oauthRegistrationRateLimiter.attempt(
        `oauth-register:${ctx.request.ip()}`,
        () => true
      ))
    ) {
      return this.error(ctx, new GatewayOauthError('too_many_requests', 'Try again later', 429))
    }

    try {
      const client = await registerOauthClient(ctx.request.all())
      ctx.response.header('Cache-Control', 'no-store')
      return ctx.response.status(201).json(client)
    } catch (error) {
      return this.error(ctx, error)
    }
  }

  async authorize(ctx: HttpContext) {
    if (!this.isConfigured(ctx)) return

    if (
      !(await oauthAuthorizationRateLimiter.attempt(
        `oauth-authorize:${ctx.request.ip()}`,
        () => true
      ))
    ) {
      return this.error(
        ctx,
        new GatewayOauthError('temporarily_unavailable', 'Try again later', 429)
      )
    }

    const input = ctx.request.method() === 'GET' ? ctx.request.qs() : ctx.request.all()

    try {
      const authorizationRequest = await parseAuthorizationRequest(input)
      ctx.response.header('Cache-Control', 'no-store')

      if (!ctx.auth.user) {
        ctx.session.put('oauthReturnTo', authorizationReturnPath(authorizationRequest))
        return ctx.response.redirect().toRoute('session.create')
      }

      if (ctx.request.method() === 'GET') {
        const redirectUrl = new URL(authorizationRequest.redirectUri)
        return ctx.inertia.render('oauth/authorize', {
          clientName: authorizationRequest.client.clientName,
          redirectHost: redirectUrl.host,
          isLoopbackRedirect: ['localhost', '127.0.0.1', '[::1]'].includes(redirectUrl.hostname),
          scope: authorizationRequest.scopes,
          userEmail: ctx.auth.user.email,
          authorization: {
            clientId: authorizationRequest.client.clientId,
            redirectUri: authorizationRequest.redirectUri,
            state: authorizationRequest.state,
            codeChallenge: authorizationRequest.codeChallenge,
            resource: authorizationRequest.resource,
          },
        })
      }

      if (input.decision !== 'approve') {
        return redirectToOauthClient(
          ctx,
          oauthRedirect(authorizationRequest.redirectUri, {
            error: 'access_denied',
            error_description: 'The user denied the authorization request',
            state: authorizationRequest.state,
          })
        )
      }

      const code = await createAuthorizationCode(authorizationRequest, ctx.auth.user.id)
      return redirectToOauthClient(
        ctx,
        oauthRedirect(authorizationRequest.redirectUri, {
          code,
          state: authorizationRequest.state,
        })
      )
    } catch (error) {
      if (error instanceof GatewayOauthError && error.redirectUri) {
        return redirectToOauthClient(
          ctx,
          oauthRedirect(error.redirectUri, {
            error: error.code,
            error_description: error.message,
            state: error.state,
          })
        )
      }
      return this.error(ctx, error)
    }
  }

  async token(ctx: HttpContext) {
    if (!this.isConfigured(ctx)) return

    if (!(await oauthTokenRateLimiter.attempt(`oauth-token:${ctx.request.ip()}`, () => true))) {
      return this.error(
        ctx,
        new GatewayOauthError('temporarily_unavailable', 'Try again later', 429)
      )
    }

    const input = ctx.request.all()
    try {
      const client = await authenticateOauthClient(ctx.request.header('authorization'), input)
      const grantType = requiredString(input, 'grant_type')

      if (grantType === 'authorization_code') {
        const created = await exchangeAuthorizationCode({
          client,
          code: requiredString(input, 'code'),
          codeVerifier: requiredString(input, 'code_verifier'),
          redirectUri: requiredString(input, 'redirect_uri'),
          resource: requiredString(input, 'resource'),
        })
        return this.tokens(ctx, created)
      }

      if (grantType === 'refresh_token') {
        const created = await exchangeRefreshToken({
          client,
          refreshToken: requiredString(input, 'refresh_token'),
          scope: typeof input.scope === 'string' ? input.scope : null,
          resource: requiredString(input, 'resource'),
        })
        return this.tokens(ctx, created)
      }

      throw new GatewayOauthError(
        'unsupported_grant_type',
        'Only authorization_code and refresh_token grants are supported'
      )
    } catch (error) {
      return this.error(ctx, error)
    }
  }

  async revoke(ctx: HttpContext) {
    if (!this.isConfigured(ctx)) return

    if (!(await oauthTokenRateLimiter.attempt(`oauth-revoke:${ctx.request.ip()}`, () => true))) {
      return this.error(
        ctx,
        new GatewayOauthError('temporarily_unavailable', 'Try again later', 429)
      )
    }

    const input = ctx.request.all()
    try {
      const client = await authenticateOauthClient(ctx.request.header('authorization'), input)
      await AccessTokenService.revokeOauthToken(client.id, requiredString(input, 'token'))
      ctx.response.header('Cache-Control', 'no-store')
      return ctx.response.ok({})
    } catch (error) {
      return this.error(ctx, error)
    }
  }

  private tokens(ctx: HttpContext, created: Parameters<typeof oauthTokenResponse>[0]) {
    ctx.response.header('Cache-Control', 'no-store')
    ctx.response.header('Pragma', 'no-cache')
    return ctx.response.ok(oauthTokenResponse(created))
  }

  private isConfigured(ctx: HttpContext) {
    try {
      requirePublicAppUrl()
      return true
    } catch {
      ctx.response.header('Cache-Control', 'no-store')
      ctx.response.status(503).json({
        error: 'temporarily_unavailable',
        error_description: 'OAuth requires APP_URL to be a public HTTPS origin',
      })
      return false
    }
  }

  private error(ctx: HttpContext, error: unknown) {
    ctx.response.header('Cache-Control', 'no-store')
    ctx.response.header('Pragma', 'no-cache')

    if (error instanceof GatewayOauthError) {
      if (error.code === 'invalid_client') {
        ctx.response.header('WWW-Authenticate', 'Basic realm="MyMCPs OAuth"')
      }
      return ctx.response.status(error.status).json({
        error: error.code,
        error_description: error.message,
      })
    }

    ctx.logger.error({ err: error }, 'OAuth request failed')
    return ctx.response.status(500).json({
      error: 'server_error',
      error_description: 'The OAuth request could not be completed',
    })
  }
}
