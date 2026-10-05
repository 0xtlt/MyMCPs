import type { HttpContext } from '@adonisjs/core/http'
import AccessTokenService from '#services/access_token_service'
import {
  GatewayOauthError,
  authenticateOauthClient,
  authorizationServerMetadata,
  createAuthorizationCode,
  exchangeAuthorizationCode,
  exchangeRefreshToken,
  isLoopbackRedirectUri,
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
import { sanitizeDiagnostic } from '#services/security_redaction'

const MAX_SESSION_RETURN_PATH_BYTES = 1536

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
  const returnPath = `/authorize?${params}`
  if (Buffer.byteLength(returnPath, 'utf8') > MAX_SESSION_RETURN_PATH_BYTES) {
    throw new GatewayOauthError(
      'invalid_request',
      'The OAuth authorization request is too large',
      400,
      request.redirectUri,
      request.state
    )
  }
  return returnPath
}

function redirectToOauthClient(ctx: HttpContext, location: string) {
  if (ctx.request.header('x-inertia')) {
    ctx.response.header('X-Inertia-Location', location)
    return ctx.response.status(409).send('')
  }
  // The location is complete. Forwarding this request's query string would
  // append it after the last parameter and corrupt `state`.
  return ctx.response.redirect().withQs(false).toPath(location)
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
    // The GET route also answers HEAD, which carries no CSRF token and no
    // body. Only GET shows the consent screen and only POST records a decision.
    const method = ctx.request.method()
    if (method !== 'GET' && method !== 'POST') {
      ctx.response.header('Allow', 'GET, POST')
      return this.error(ctx, new GatewayOauthError('invalid_request', 'Method not allowed', 405))
    }

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

    // A decision is read from the CSRF-protected form body alone, never from
    // the query string.
    const input = method === 'GET' ? ctx.request.qs() : ctx.request.body()

    try {
      const authorizationRequest = await parseAuthorizationRequest(input)
      ctx.response.header('Cache-Control', 'no-store')

      if (!ctx.auth.user) {
        ctx.session.put('oauthReturnTo', authorizationReturnPath(authorizationRequest))
        return ctx.response.redirect().withQs(false).toRoute('session.create')
      }

      if (method === 'GET') {
        const redirectUrl = new URL(authorizationRequest.redirectUri)
        return ctx.inertia.render('oauth/authorize', {
          clientName: authorizationRequest.client.clientName,
          redirectHost: redirectUrl.host,
          isLoopbackRedirect: isLoopbackRedirectUri(authorizationRequest.redirectUri),
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
      // Anyone can register a client, so a rejected request is only sent back
      // to a redirect URI on the user's own device. Any other target would
      // make this endpoint an open redirect; the error is shown here instead.
      if (
        error instanceof GatewayOauthError &&
        error.redirectUri &&
        isLoopbackRedirectUri(error.redirectUri)
      ) {
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

    ctx.logger.error({ error: sanitizeDiagnostic(error) }, 'OAuth request failed')
    return ctx.response.status(500).json({
      error: 'server_error',
      error_description: 'The OAuth request could not be completed',
    })
  }
}
