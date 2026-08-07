import { randomBytes } from 'node:crypto'
import { DateTime } from 'luxon'
import type { HttpContext } from '@adonisjs/core/http'
import type { Infer } from '@vinejs/vine/types'
import {
  discoverAuthorizationServerMetadata,
  discoverOAuthServerInfo,
  exchangeAuthorization,
  refreshAuthorization,
  registerClient,
  startAuthorization,
} from '@modelcontextprotocol/sdk/client/auth.js'
import type {
  AuthorizationServerMetadata,
  OAuthClientInformationMixed,
  OAuthClientMetadata,
  OAuthTokens,
} from '@modelcontextprotocol/sdk/shared/auth.js'
import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { requirePublicAppUrl } from '#services/public_url'
import { oauthSessionValidator } from '#validators/oauth'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'
import { parseHttpUrl } from '#services/http_url'

type OauthSession = Infer<typeof oauthSessionValidator>

type OAuthContext = {
  authorizationServerUrl: string
  metadata: AuthorizationServerMetadata
  resource?: string
  scope?: string
}

type OAuthStartOptions = {
  redirectUri: string
  authorizationServerUrl: string
  resource?: string
  clientId: string
  codeVerifier: string
  state: string
}

function base64Url(buffer: Buffer) {
  return buffer.toString('base64url')
}

function oauthSessionKey(state: string) {
  return `mcp_oauth:${state}`
}

function normalizedHttpUrl(value: string, label: string) {
  const url = parseHttpUrl(value, label)
  return url.pathname === '/' && !url.search ? url.origin : url.toString()
}

const oauthFetch: typeof fetch = (input, init) =>
  fetchWithSameOriginRedirects(input, init, 'OAuth endpoint')

function validateOAuthMetadata(metadata: AuthorizationServerMetadata, expectedIssuer?: string) {
  parseHttpUrl(String(metadata.authorization_endpoint), 'OAuth authorization endpoint')
  parseHttpUrl(String(metadata.token_endpoint), 'OAuth token endpoint')
  if (metadata.registration_endpoint) {
    parseHttpUrl(metadata.registration_endpoint, 'OAuth registration endpoint')
  }
  if (metadata.issuer) {
    parseHttpUrl(metadata.issuer, 'OAuth issuer')
    if (
      expectedIssuer &&
      normalizedHttpUrl(metadata.issuer, 'OAuth issuer') !==
        normalizedHttpUrl(expectedIssuer, 'OAuth authorization server')
    ) {
      throw new Error('OAuth issuer metadata does not match the authorization server')
    }
  }
  return metadata
}

function normalizedTokenEndpoint(metadata: AuthorizationServerMetadata) {
  return normalizedHttpUrl(String(metadata.token_endpoint), 'OAuth token endpoint')
}

function inferIssuer(mcp: Mcp) {
  const endpoint = mcp.oauthAuthorizeUrl || mcp.oauthTokenUrl
  if (!endpoint) {
    return null
  }
  return new URL(endpoint).origin
}

function fallbackMetadata(mcp: Mcp, issuer: string): AuthorizationServerMetadata | undefined {
  if (!mcp.oauthAuthorizeUrl || !mcp.oauthTokenUrl) {
    return undefined
  }

  return {
    issuer,
    authorization_endpoint: mcp.oauthAuthorizeUrl,
    token_endpoint: mcp.oauthTokenUrl,
    response_types_supported: ['code'],
    code_challenge_methods_supported: ['S256'],
    ...(mcp.oauthClientAuthMethod
      ? { token_endpoint_auth_methods_supported: [mcp.oauthClientAuthMethod] }
      : {}),
  }
}

function clientInformationFromMcp(mcp: Mcp): OAuthClientInformationMixed | null {
  if (!mcp.oauthClientId) {
    return null
  }

  const secret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  return {
    client_id: mcp.oauthClientId,
    ...(secret ? { client_secret: secret } : {}),
    ...(mcp.oauthClientAuthMethod ? { token_endpoint_auth_method: mcp.oauthClientAuthMethod } : {}),
  } as OAuthClientInformationMixed
}

function clientAuthMethod(client: OAuthClientInformationMixed) {
  if (
    'token_endpoint_auth_method' in client &&
    typeof client.token_endpoint_auth_method === 'string'
  ) {
    return client.token_endpoint_auth_method
  }
  return null
}

async function discoverOAuthContext(mcp: Mcp): Promise<OAuthContext> {
  if (mcp.transport !== 'http' || !mcp.httpUrl) {
    throw new Error('OAuth is supported only for HTTP MCPs')
  }

  const serverUrl = parseHttpUrl(mcp.httpUrl, 'MCP URL')
  let serverInfo: Awaited<ReturnType<typeof discoverOAuthServerInfo>> | undefined
  let discoveryError: unknown

  try {
    serverInfo = await discoverOAuthServerInfo(serverUrl, { fetchFn: oauthFetch })
  } catch (error) {
    discoveryError = error
  }

  const discoveredAuthorizationServerUrl =
    serverInfo?.authorizationServerUrl ?? mcp.oauthIssuer ?? inferIssuer(mcp)
  if (!discoveredAuthorizationServerUrl) {
    throw new Error(
      discoveryError instanceof Error
        ? `OAuth discovery failed: ${discoveryError.message}`
        : 'OAuth provider metadata could not be discovered'
    )
  }
  const authorizationServerUrl = normalizedHttpUrl(
    discoveredAuthorizationServerUrl,
    'OAuth authorization server'
  )

  let metadata = serverInfo?.authorizationServerMetadata
  if (!metadata) {
    try {
      metadata = await discoverAuthorizationServerMetadata(authorizationServerUrl, {
        fetchFn: oauthFetch,
      })
    } catch (error) {
      discoveryError = error
    }
  }
  metadata ??= fallbackMetadata(mcp, authorizationServerUrl)

  if (!metadata) {
    throw new Error(
      discoveryError instanceof Error
        ? `OAuth provider metadata could not be discovered: ${discoveryError.message}`
        : 'OAuth provider metadata could not be discovered'
    )
  }
  validateOAuthMetadata(metadata, authorizationServerUrl)

  const discoveredResource =
    serverInfo?.resourceMetadata?.resource ?? mcp.oauthResource ?? undefined
  const resource = discoveredResource
    ? normalizedHttpUrl(discoveredResource, 'OAuth resource')
    : undefined
  const scope =
    mcp.oauthScopes?.trim() ||
    serverInfo?.resourceMetadata?.scopes_supported?.join(' ') ||
    metadata.scopes_supported?.join(' ') ||
    undefined

  return {
    authorizationServerUrl,
    metadata,
    resource,
    scope,
  }
}

async function metadataForAuthorizationServer(
  mcp: Mcp,
  authorizationServerUrl: string
): Promise<AuthorizationServerMetadata> {
  const normalizedAuthorizationServerUrl = normalizedHttpUrl(
    authorizationServerUrl,
    'OAuth authorization server'
  )
  let metadata: AuthorizationServerMetadata | undefined
  try {
    metadata = await discoverAuthorizationServerMetadata(normalizedAuthorizationServerUrl, {
      fetchFn: oauthFetch,
    })
  } catch {
    // Manual endpoint settings remain a fallback for OAuth providers without discovery.
  }
  metadata ??= fallbackMetadata(mcp, authorizationServerUrl)
  if (!metadata) {
    throw new Error('OAuth provider metadata could not be discovered')
  }
  return validateOAuthMetadata(metadata, normalizedAuthorizationServerUrl)
}

function saveOAuthConfiguration(
  mcp: Mcp,
  context: OAuthContext,
  client: OAuthClientInformationMixed,
  redirectUri: string,
  registered: boolean
) {
  mcp.oauthIssuer = context.authorizationServerUrl
  mcp.oauthResource = context.resource ?? null
  mcp.oauthRedirectUri = redirectUri
  mcp.oauthAuthorizeUrl = normalizedHttpUrl(
    String(context.metadata.authorization_endpoint),
    'OAuth authorization endpoint'
  )
  mcp.oauthTokenUrl = normalizedTokenEndpoint(context.metadata)
  if (context.scope) {
    mcp.oauthScopes = context.scope
  }
  mcp.oauthClientAuthMethod = clientAuthMethod(client)
  mcp.oauthClientId = client.client_id

  if (registered) {
    const secret = 'client_secret' in client ? client.client_secret : undefined
    mcp.oauthClientSecret = secret ? McpSecretStore.encrypt(secret) : null
  }
}

function saveOAuthTokens(mcp: Mcp, tokens: OAuthTokens) {
  mcp.oauthAccessToken = McpSecretStore.encrypt(tokens.access_token)
  mcp.oauthRefreshToken = tokens.refresh_token ? McpSecretStore.encrypt(tokens.refresh_token) : null
  mcp.oauthTokenType =
    !tokens.token_type || tokens.token_type.toLowerCase() === 'bearer'
      ? 'Bearer'
      : tokens.token_type
  mcp.oauthTokenExpiresAt =
    typeof tokens.expires_in === 'number'
      ? DateTime.utc().plus({ seconds: tokens.expires_in })
      : null
  if (tokens.scope) {
    mcp.oauthScopes = tokens.scope
  }
  mcp.oauthRequired = false
}

export function oauthCallbackUrl() {
  return `${requirePublicAppUrl()}/mcps/oauth/callback`
}

export function startOauthSession(
  session: HttpContext['session'],
  mcp: Mcp,
  options: OAuthStartOptions
) {
  const payload: OauthSession = { mcpId: mcp.id, ...options }
  session.put(oauthSessionKey(payload.state), payload)
  return payload
}

export async function readOauthSession(
  session: HttpContext['session'],
  state: string | undefined
): Promise<OauthSession | null> {
  if (!state) {
    return null
  }

  try {
    return await oauthSessionValidator.validate(session.get(oauthSessionKey(state)))
  } catch {
    return null
  }
}

export function clearOauthSession(session: HttpContext['session'], state: string | undefined) {
  if (state) {
    session.forget(oauthSessionKey(state))
  }
}

/**
 * Discover an upstream's OAuth provider, register a public client when needed,
 * and create the browser authorization redirect.
 */
export async function startOauthFlow(session: HttpContext['session'], mcp: Mcp) {
  const redirectUri = oauthCallbackUrl()
  const context = await discoverOAuthContext(mcp)
  const existingClient = clientInformationFromMcp(mcp)
  const canReuseExisting =
    Boolean(existingClient) &&
    (!mcp.oauthRedirectUri || mcp.oauthRedirectUri === redirectUri) &&
    (!mcp.oauthIssuer || mcp.oauthIssuer === context.authorizationServerUrl)

  let client = existingClient
  let registered = false
  if (!canReuseExisting) {
    if (!context.metadata.registration_endpoint) {
      throw new Error(
        existingClient
          ? 'The OAuth redirect origin changed, but this provider does not support automatic client registration'
          : 'This OAuth provider does not support automatic client registration'
      )
    }

    const clientMetadata: OAuthClientMetadata = {
      client_name: 'MyMCPs',
      redirect_uris: [redirectUri],
      grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'],
      token_endpoint_auth_method: 'none',
      ...(context.scope ? { scope: context.scope } : {}),
    }
    client = await registerClient(context.authorizationServerUrl, {
      metadata: context.metadata,
      clientMetadata,
      scope: context.scope,
      fetchFn: oauthFetch,
    })
    registered = true
  }

  if (!client) {
    throw new Error('OAuth client registration did not return a client ID')
  }

  const state = base64Url(randomBytes(24))
  const { authorizationUrl, codeVerifier } = await startAuthorization(
    context.authorizationServerUrl,
    {
      metadata: context.metadata,
      clientInformation: client,
      redirectUrl: redirectUri,
      scope: context.scope,
      state,
      resource: context.resource ? new URL(context.resource) : undefined,
    }
  )

  saveOAuthConfiguration(mcp, context, client, redirectUri, registered)
  await mcp.save()
  startOauthSession(session, mcp, {
    redirectUri,
    authorizationServerUrl: context.authorizationServerUrl,
    resource: context.resource,
    clientId: client.client_id,
    codeVerifier,
    state,
  })

  return parseHttpUrl(authorizationUrl.toString(), 'OAuth authorization URL').toString()
}

export async function exchangeAuthorizationCode(mcp: Mcp, oauth: OauthSession, code: string) {
  const client = clientInformationFromMcp(mcp)
  if (!client || client.client_id !== oauth.clientId) {
    throw new Error('OAuth client information is no longer available')
  }

  const metadata = await metadataForAuthorizationServer(mcp, oauth.authorizationServerUrl)
  const tokens = await exchangeAuthorization(oauth.authorizationServerUrl, {
    metadata,
    clientInformation: client,
    authorizationCode: code,
    codeVerifier: oauth.codeVerifier,
    redirectUri: oauth.redirectUri,
    resource: oauth.resource ? new URL(oauth.resource) : undefined,
    fetchFn: oauthFetch,
  })

  saveOAuthTokens(mcp, tokens)
  mcp.status = 'ready'
  mcp.lastError = null
  await mcp.save()
}

/**
 * Refresh the access token when expired (or about to expire).
 * Throws when a refresh is required but fails so callers do not use a stale token.
 */
export async function refreshOauthAccessToken(mcp: Mcp) {
  const refresh = McpSecretStore.decrypt(mcp.oauthRefreshToken)
  const client = clientInformationFromMcp(mcp)
  const authorizationServerUrl = mcp.oauthIssuer ?? inferIssuer(mcp)
  if (!refresh || !client || !authorizationServerUrl) {
    return
  }

  if (mcp.oauthTokenExpiresAt && mcp.oauthTokenExpiresAt > DateTime.utc().plus({ minutes: 2 })) {
    return
  }

  const metadata = await metadataForAuthorizationServer(mcp, authorizationServerUrl)
  const tokens = await refreshAuthorization(authorizationServerUrl, {
    metadata,
    clientInformation: client,
    refreshToken: refresh,
    resource: mcp.oauthResource ? new URL(mcp.oauthResource) : undefined,
    fetchFn: oauthFetch,
  })

  saveOAuthTokens(mcp, tokens)
  await mcp.save()
}
