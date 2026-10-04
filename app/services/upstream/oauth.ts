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
import {
  allowlistedUpstreamClient,
  registrationClientName,
  upstreamIdentityHeaders,
} from '#services/upstream/allowlisted_client'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'
import { parseHttpUrl } from '#services/http_url'
import {
  builtinAuthorizationUrl,
  exchangeBuiltinAuthorizationCode,
  parseOauthScopes,
  refreshBuiltinTokens,
} from '#services/builtin/oauth'
import { requireBuiltinMcp } from '#services/builtin/registry'

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
  codeVerifier?: string
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
  return url.pathname === '/' && !url.search && !url.username && !url.password
    ? url.origin
    : url.toString()
}

function comparableOAuthIssuer(value: string, label: string) {
  const url = parseHttpUrl(value, label)
  url.username = ''
  url.password = ''
  return url.pathname === '/' && !url.search ? url.origin : url.toString()
}

function fetchTargetUrl(input: Parameters<typeof fetch>[0]) {
  if (input instanceof URL) return input.toString()
  if (input instanceof Request) return input.url
  return String(input)
}

const oauthFetch: typeof fetch = (input, init) => {
  const identityHeaders = upstreamIdentityHeaders(fetchTargetUrl(input))
  if (Object.keys(identityHeaders).length === 0) {
    return fetchWithSameOriginRedirects(input, init, 'OAuth endpoint')
  }

  const headers = new Headers(input instanceof Request ? input.headers : undefined)
  if (init?.headers) {
    new Headers(init.headers).forEach((value, name) => {
      headers.set(name, value)
    })
  }
  for (const [name, value] of Object.entries(identityHeaders)) {
    if (!headers.has(name)) headers.set(name, value)
  }

  return fetchWithSameOriginRedirects(input, { ...init, headers }, 'OAuth endpoint')
}

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
      comparableOAuthIssuer(metadata.issuer, 'OAuth issuer') !==
        comparableOAuthIssuer(expectedIssuer, 'OAuth authorization server')
    ) {
      throw new Error('OAuth issuer metadata does not match the authorization server')
    }
  }
  return metadata
}

async function metadataForSameOriginIssuer(
  metadata: AuthorizationServerMetadata,
  authorizationServerUrl: string
) {
  if (!metadata.issuer) {
    return null
  }

  const issuer = normalizedHttpUrl(metadata.issuer, 'OAuth issuer')
  if (
    comparableOAuthIssuer(issuer, 'OAuth issuer') ===
    comparableOAuthIssuer(authorizationServerUrl, 'OAuth authorization server')
  ) {
    return null
  }

  if (new URL(issuer).origin !== new URL(authorizationServerUrl).origin) {
    return null
  }

  try {
    const issuerMetadata = await discoverAuthorizationServerMetadata(issuer, {
      fetchFn: oauthFetch,
    })
    if (!issuerMetadata) {
      return null
    }
    return {
      authorizationServerUrl: issuer,
      metadata: validateOAuthMetadata(issuerMetadata, issuer),
    }
  } catch {
    return null
  }
}

function normalizedTokenEndpoint(metadata: AuthorizationServerMetadata) {
  return normalizedHttpUrl(String(metadata.token_endpoint), 'OAuth token endpoint')
}

function inferIssuer(mcp: Mcp) {
  const endpoint = mcp.oauthAuthorizeUrl || mcp.oauthTokenUrl
  if (!endpoint) {
    return null
  }
  const url = parseHttpUrl(endpoint, 'OAuth endpoint')
  url.pathname = '/'
  url.search = ''
  return url.username || url.password ? url.toString() : url.origin
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

/**
 * Whether the provider redirects to a loopback URL the admin must paste back.
 * The loopback URI, when one is required, lives on the allowlisted-client map.
 */
export function usesPastedOauthCallback(mcp: Mcp) {
  if (mcp.transport !== 'http') {
    return false
  }
  return Boolean(allowlistedUpstreamClient(mcp.httpUrl)?.loopbackRedirectUri)
}

function oauthRedirectUri(mcp: Mcp) {
  return allowlistedUpstreamClient(mcp.httpUrl)?.loopbackRedirectUri ?? oauthCallbackUrl()
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
  let authorizationServerUrl = normalizedHttpUrl(
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

  // Legacy MCP discovery falls back to the resource server's origin when
  // protected-resource metadata is unavailable. If root metadata advertises a
  // path-based issuer on that same origin, use it only after independently
  // retrieving and validating metadata from the issuer's RFC 8414 location.
  if (metadata && !serverInfo?.resourceMetadata?.authorization_servers?.length) {
    const issuerDiscovery = await metadataForSameOriginIssuer(metadata, authorizationServerUrl)
    if (issuerDiscovery) {
      authorizationServerUrl = issuerDiscovery.authorizationServerUrl
      metadata = issuerDiscovery.metadata
    }
  }

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
 * Built-in MCPs use the API application the admin registered with the
 * provider, so there is nothing to discover or register.
 */
function startBuiltinOauthFlow(session: HttpContext['session'], mcp: Mcp) {
  const definition = requireBuiltinMcp(mcp)
  if (!mcp.oauthClientId || !McpSecretStore.decrypt(mcp.oauthClientSecret)) {
    throw new Error(`Add the ${definition.name} Client ID and Client Secret before connecting`)
  }

  const redirectUri = oauthCallbackUrl()
  const state = base64Url(randomBytes(24))
  startOauthSession(session, mcp, {
    redirectUri,
    authorizationServerUrl: definition.oauth.issuer,
    clientId: mcp.oauthClientId,
    state,
  })

  return builtinAuthorizationUrl(definition, { clientId: mcp.oauthClientId, redirectUri, state })
}

/**
 * Discover an upstream's OAuth provider, register a public client when needed,
 * and create the browser authorization redirect.
 */
export async function startOauthFlow(session: HttpContext['session'], mcp: Mcp) {
  if (mcp.transport === 'builtin') {
    return startBuiltinOauthFlow(session, mcp)
  }

  const redirectUri = oauthRedirectUri(mcp)
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
      client_name: registrationClientName(mcp.httpUrl),
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

/**
 * `grantedScope` is the callback's `scope` parameter, for providers that let
 * the user uncheck permissions and report what is left there.
 */
export async function exchangeAuthorizationCode(
  mcp: Mcp,
  oauth: OauthSession,
  code: string,
  grantedScope?: string
) {
  const client = clientInformationFromMcp(mcp)
  if (!client || client.client_id !== oauth.clientId) {
    throw new Error('OAuth client information is no longer available')
  }

  if (mcp.transport === 'builtin') {
    const tokens = await exchangeBuiltinAuthorizationCode(requireBuiltinMcp(mcp), mcp, code)
    const scopes = parseOauthScopes(tokens.scope ?? grantedScope)
    saveOAuthTokens(mcp, { ...tokens, scope: undefined })
    // Never keep the scopes of an earlier authorization for these tokens.
    mcp.oauthScopes = scopes.length > 0 ? scopes.join(' ') : null
    mcp.status = 'ready'
    mcp.lastError = null
    await mcp.save()
    return
  }

  if (!oauth.codeVerifier) {
    throw new Error('OAuth session is missing its PKCE code verifier')
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

// The gateway serves parallel requests in one Node process. All model instances
// of the same MCP must share the rotation, including the save of the new pair.
// Do not key this by model identity or plaintext credentials.
const pendingRefreshes = new Map<number, Promise<void>>()

/** Refresh once per MCP; callers never proceed with a stale token on failure. */
export async function refreshOauthAccessToken(mcp: Mcp) {
  let pending = pendingRefreshes.get(mcp.id)
  if (!pending) {
    pending = refreshCurrentOauthAccessToken(mcp)
    pendingRefreshes.set(mcp.id, pending)
  }

  try {
    await pending
  } finally {
    if (pendingRefreshes.get(mcp.id) === pending) {
      pendingRefreshes.delete(mcp.id)
    }
  }

  // Waiting callers have their own Lucid instances. They must use the saved
  // access token, not the old token from before they joined the shared refresh.
  await mcp.refresh()
}

async function refreshCurrentOauthAccessToken(mcp: Mcp) {
  // A caller can hold an old model even after an earlier rotation has completed.
  await mcp.refresh()
  if (mcp.authType !== 'auto') return

  const refresh = McpSecretStore.decrypt(mcp.oauthRefreshToken)
  const isFresh =
    mcp.oauthTokenExpiresAt && mcp.oauthTokenExpiresAt > DateTime.utc().plus({ minutes: 2 })

  if (mcp.transport === 'builtin') {
    if (!refresh || isFresh) return
    const tokens = await refreshBuiltinTokens(requireBuiltinMcp(mcp), mcp, refresh)
    saveOAuthTokens(mcp, {
      ...tokens,
      refresh_token: tokens.refresh_token ?? refresh,
      scope: undefined,
    })
    await mcp.save()
    return
  }

  const client = clientInformationFromMcp(mcp)
  const authorizationServerUrl = mcp.oauthIssuer ?? inferIssuer(mcp)
  if (!refresh || !client || !authorizationServerUrl) {
    return
  }

  if (isFresh) {
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
