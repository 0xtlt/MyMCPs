import { createHash, randomBytes, timingSafeEqual } from 'node:crypto'
import { DateTime } from 'luxon'
import { OAuthClientMetadataSchema } from '@modelcontextprotocol/sdk/shared/auth.js'
import OauthAuthorizationCode from '#models/oauth_authorization_code'
import OauthClient from '#models/oauth_client'
import AccessTokenService from '#services/access_token_service'
import { requirePublicAppUrl } from '#services/public_url'
import db from '@adonisjs/lucid/services/db'

export const GATEWAY_OAUTH_SCOPE = 'mcp:tools'
export const OAUTH_ACCESS_TOKEN_TTL_SECONDS = 60 * 60
const AUTHORIZATION_CODE_TTL_MINUTES = 5
const CLIENT_SECRET_TTL_DAYS = 365
const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])
const CLIENT_AUTH_METHODS = new Set(['none', 'client_secret_post', 'client_secret_basic'])

export class GatewayOauthError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status = 400,
    readonly redirectUri: string | null = null,
    readonly state: string | null = null
  ) {
    super(message)
    this.name = 'GatewayOauthError'
  }
}

export function gatewayResourceUrl() {
  return new URL('/mcp', requirePublicAppUrl()).href
}

export function protectedResourceMetadataUrl() {
  return new URL('/.well-known/oauth-protected-resource/mcp', requirePublicAppUrl()).href
}

export function protectedResourceMetadata() {
  const issuer = requirePublicAppUrl()
  return {
    resource: gatewayResourceUrl(),
    authorization_servers: [issuer],
    scopes_supported: [GATEWAY_OAUTH_SCOPE],
    bearer_methods_supported: ['header'],
    resource_name: 'MyMCPs gateway',
  }
}

export function authorizationServerMetadata() {
  const issuer = requirePublicAppUrl()
  return {
    issuer,
    authorization_endpoint: new URL('/authorize', issuer).href,
    token_endpoint: new URL('/token', issuer).href,
    registration_endpoint: new URL('/register', issuer).href,
    revocation_endpoint: new URL('/revoke', issuer).href,
    scopes_supported: [GATEWAY_OAUTH_SCOPE],
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code', 'refresh_token'],
    token_endpoint_auth_methods_supported: ['none', 'client_secret_post', 'client_secret_basic'],
    revocation_endpoint_auth_methods_supported: [
      'none',
      'client_secret_post',
      'client_secret_basic',
    ],
    code_challenge_methods_supported: ['S256'],
  }
}

function stringValue(input: unknown) {
  return typeof input === 'string' ? input : null
}

function parseStringList(value: string | undefined, fallback: string[]) {
  return value === undefined ? fallback : value.split(' ').filter(Boolean)
}

function isAllowedRedirectUri(value: string) {
  if (value.length > 2048) return false

  try {
    const url = new URL(value)
    if (url.hash || url.username || url.password) return false
    if (url.protocol === 'https:') return true
    return url.protocol === 'http:' && LOOPBACK_HOSTS.has(url.hostname)
  } catch {
    return false
  }
}

/** RFC 8252 allows native loopback clients to choose their callback port at runtime. */
export function redirectUriMatches(requested: string, registered: string) {
  if (requested === registered) return true

  try {
    const requestUrl = new URL(requested)
    const registeredUrl = new URL(registered)
    if (!LOOPBACK_HOSTS.has(requestUrl.hostname) || !LOOPBACK_HOSTS.has(registeredUrl.hostname)) {
      return false
    }

    return (
      requestUrl.protocol === registeredUrl.protocol &&
      requestUrl.hostname === registeredUrl.hostname &&
      requestUrl.pathname === registeredUrl.pathname &&
      requestUrl.search === registeredUrl.search &&
      requestUrl.hash === registeredUrl.hash
    )
  } catch {
    return false
  }
}

export async function registerOauthClient(input: unknown) {
  const parsed = OAuthClientMetadataSchema.safeParse(input)
  if (!parsed.success) {
    throw new GatewayOauthError('invalid_client_metadata', 'Invalid OAuth client metadata')
  }

  const metadata = parsed.data
  if (
    metadata.redirect_uris.length === 0 ||
    metadata.redirect_uris.length > 10 ||
    metadata.redirect_uris.some((uri) => !isAllowedRedirectUri(uri))
  ) {
    throw new GatewayOauthError(
      'invalid_redirect_uri',
      'Redirect URIs must use HTTPS or HTTP on an exact loopback host'
    )
  }

  const authMethod = metadata.token_endpoint_auth_method ?? 'client_secret_basic'
  if (!CLIENT_AUTH_METHODS.has(authMethod)) {
    throw new GatewayOauthError(
      'invalid_client_metadata',
      'Unsupported token endpoint authentication method'
    )
  }

  const grantTypes = metadata.grant_types ?? ['authorization_code', 'refresh_token']
  if (
    !grantTypes.includes('authorization_code') ||
    grantTypes.some((grant) => !['authorization_code', 'refresh_token'].includes(grant))
  ) {
    throw new GatewayOauthError('invalid_client_metadata', 'Unsupported OAuth grant type')
  }

  const responseTypes = metadata.response_types ?? ['code']
  if (responseTypes.length !== 1 || responseTypes[0] !== 'code') {
    throw new GatewayOauthError(
      'invalid_client_metadata',
      'Only the code response type is supported'
    )
  }

  const scopes = parseStringList(metadata.scope, [GATEWAY_OAUTH_SCOPE])
  if (scopes.length !== 1 || scopes[0] !== GATEWAY_OAUTH_SCOPE) {
    throw new GatewayOauthError('invalid_client_metadata', 'Unsupported OAuth scope')
  }

  const clientName = metadata.client_name?.trim() || 'MCP client'
  if (clientName.length > 120) {
    throw new GatewayOauthError('invalid_client_metadata', 'Client name is too long')
  }

  const clientId = `mcp_client_${randomBytes(24).toString('base64url')}`
  const clientSecret =
    authMethod === 'none' ? null : `mcp_secret_${randomBytes(32).toString('base64url')}`
  const secretExpiresAt = clientSecret
    ? DateTime.utc().plus({ days: CLIENT_SECRET_TTL_DAYS })
    : null
  const client = await OauthClient.create({
    clientId,
    clientSecretHash: clientSecret ? AccessTokenService.hash(clientSecret) : null,
    clientSecretPrefix: clientSecret ? AccessTokenService.prefix(clientSecret) : null,
    clientSecretExpiresAt: secretExpiresAt,
    clientName,
    redirectUris: JSON.stringify(metadata.redirect_uris),
    tokenEndpointAuthMethod: authMethod,
    grantTypes: JSON.stringify(grantTypes),
    responseTypes: JSON.stringify(responseTypes),
    scope: GATEWAY_OAUTH_SCOPE,
  })

  return {
    ...metadata,
    client_name: clientName,
    redirect_uris: client.redirectUriList,
    token_endpoint_auth_method: authMethod,
    grant_types: grantTypes,
    response_types: responseTypes,
    scope: GATEWAY_OAUTH_SCOPE,
    client_id: clientId,
    client_id_issued_at: Math.floor(client.createdAt.toSeconds()),
    ...(clientSecret
      ? {
          client_secret: clientSecret,
          client_secret_expires_at: Math.floor(secretExpiresAt!.toSeconds()),
        }
      : {}),
  }
}

export type GatewayAuthorizationRequest = {
  client: OauthClient
  redirectUri: string
  state: string | null
  codeChallenge: string
  scopes: string
  resource: string
}

export async function parseAuthorizationRequest(
  input: Record<string, unknown>
): Promise<GatewayAuthorizationRequest> {
  const clientId = stringValue(input.client_id)
  if (!clientId) {
    throw new GatewayOauthError('invalid_request', 'client_id is required')
  }

  const client = await OauthClient.findBy('client_id', clientId)
  if (!client) {
    throw new GatewayOauthError('invalid_client', 'Unknown OAuth client')
  }

  const redirectUri = stringValue(input.redirect_uri)
  if (!redirectUri || !client.redirectUriList.some((uri) => redirectUriMatches(redirectUri, uri))) {
    throw new GatewayOauthError('invalid_request', 'Unregistered redirect_uri')
  }

  const state = stringValue(input.state)
  const redirectError = (code: string, message: string) =>
    new GatewayOauthError(code, message, 400, redirectUri, state)

  if (stringValue(input.response_type) !== 'code') {
    throw redirectError('unsupported_response_type', 'Only the code response type is supported')
  }

  const codeChallenge = stringValue(input.code_challenge)
  if (
    !codeChallenge ||
    !/^[A-Za-z0-9_-]{43,128}$/.test(codeChallenge) ||
    stringValue(input.code_challenge_method) !== 'S256'
  ) {
    throw redirectError('invalid_request', 'PKCE with the S256 method is required')
  }

  const requestedScopes = parseStringList(stringValue(input.scope) ?? undefined, [
    GATEWAY_OAUTH_SCOPE,
  ])
  if (requestedScopes.length !== 1 || requestedScopes[0] !== GATEWAY_OAUTH_SCOPE) {
    throw redirectError('invalid_scope', 'Unsupported OAuth scope')
  }

  const resource = stringValue(input.resource)
  if (!resource || !sameUrl(resource, gatewayResourceUrl())) {
    throw redirectError('invalid_target', 'The OAuth resource must be the MyMCPs gateway')
  }

  if (state && state.length > 2048) {
    throw redirectError('invalid_request', 'OAuth state is too long')
  }

  return {
    client,
    redirectUri,
    state,
    codeChallenge,
    scopes: GATEWAY_OAUTH_SCOPE,
    resource: gatewayResourceUrl(),
  }
}

export function oauthRedirect(
  redirectUri: string,
  params: Record<string, string | null | undefined>
) {
  const url = new URL(redirectUri)
  for (const [key, value] of Object.entries(params)) {
    if (value !== null && value !== undefined) url.searchParams.set(key, value)
  }
  return url.href
}

export async function createAuthorizationCode(
  request: GatewayAuthorizationRequest,
  userId: number
) {
  const plaintext = randomBytes(32).toString('base64url')
  await OauthAuthorizationCode.query()
    .where('expires_at', '<', DateTime.utc().toSQL({ includeOffset: false }))
    .delete()
  await OauthAuthorizationCode.create({
    codeHash: AccessTokenService.hash(plaintext),
    oauthClientId: request.client.id,
    userId,
    redirectUri: request.redirectUri,
    codeChallenge: request.codeChallenge,
    scopes: request.scopes,
    resource: request.resource,
    expiresAt: DateTime.utc().plus({ minutes: AUTHORIZATION_CODE_TTL_MINUTES }),
  })
  return plaintext
}

function sameUrl(first: string, second: string) {
  try {
    return new URL(first).href === new URL(second).href
  } catch {
    return false
  }
}

export function verifyCodeChallenge(codeVerifier: string, expectedChallenge: string) {
  if (!/^[A-Za-z0-9._~-]{43,128}$/.test(codeVerifier)) return false
  const actual = createHash('sha256').update(codeVerifier).digest('base64url')
  const actualBuffer = Buffer.from(actual)
  const expectedBuffer = Buffer.from(expectedChallenge)
  return (
    actualBuffer.length === expectedBuffer.length && timingSafeEqual(actualBuffer, expectedBuffer)
  )
}

type ClientCredentials = { clientId: string; clientSecret: string | null; method: string }

function basicClientCredentials(header: string | undefined): ClientCredentials | null {
  if (!header?.startsWith('Basic ')) return null

  try {
    const decoded = Buffer.from(header.slice(6), 'base64').toString('utf8')
    const separator = decoded.indexOf(':')
    if (separator < 0) return null
    return {
      clientId: decodeURIComponent(decoded.slice(0, separator)),
      clientSecret: decodeURIComponent(decoded.slice(separator + 1)),
      method: 'client_secret_basic',
    }
  } catch {
    return null
  }
}

function secureSecretMatch(plaintext: string, expectedHash: string) {
  const actual = Buffer.from(AccessTokenService.hash(plaintext), 'hex')
  const expected = Buffer.from(expectedHash, 'hex')
  return actual.length === expected.length && timingSafeEqual(actual, expected)
}

export async function authenticateOauthClient(
  authorizationHeader: string | undefined,
  input: Record<string, unknown>
) {
  const basic = basicClientCredentials(authorizationHeader)
  const bodyClientId = stringValue(input.client_id)
  const credentials: ClientCredentials | null =
    basic ??
    (bodyClientId
      ? {
          clientId: bodyClientId,
          clientSecret: stringValue(input.client_secret),
          method: stringValue(input.client_secret) ? 'client_secret_post' : 'none',
        }
      : null)

  if (!credentials) {
    throw new GatewayOauthError('invalid_client', 'OAuth client authentication is required', 401)
  }

  const client = await OauthClient.findBy('client_id', credentials.clientId)
  if (!client || client.tokenEndpointAuthMethod !== credentials.method) {
    throw new GatewayOauthError('invalid_client', 'Invalid OAuth client credentials', 401)
  }

  if (client.clientSecretHash) {
    if (
      !credentials.clientSecret ||
      !secureSecretMatch(credentials.clientSecret, client.clientSecretHash) ||
      (client.clientSecretExpiresAt !== null && client.clientSecretExpiresAt <= DateTime.utc())
    ) {
      throw new GatewayOauthError('invalid_client', 'Invalid OAuth client credentials', 401)
    }
  }

  return client
}

export async function exchangeAuthorizationCode(params: {
  client: OauthClient
  code: string
  codeVerifier: string
  redirectUri: string
  resource: string
}) {
  const authorizationCode = await OauthAuthorizationCode.query()
    .where('code_hash', AccessTokenService.hash(params.code))
    .first()

  if (
    !authorizationCode ||
    authorizationCode.oauthClientId !== params.client.id ||
    authorizationCode.expiresAt <= DateTime.utc() ||
    authorizationCode.redirectUri !== params.redirectUri ||
    !sameUrl(authorizationCode.resource, params.resource) ||
    !sameUrl(params.resource, gatewayResourceUrl()) ||
    !verifyCodeChallenge(params.codeVerifier, authorizationCode.codeChallenge)
  ) {
    throw new GatewayOauthError('invalid_grant', 'Invalid or expired authorization code')
  }

  const clientSupportsRefresh = params.client.grantTypeList.includes('refresh_token')
  return db.transaction(async (trx) => {
    const deleted = await OauthAuthorizationCode.query({ client: trx })
      .where('id', authorizationCode.id)
      .delete()
      .returning('id')
    if (deleted.length !== 1) {
      throw new GatewayOauthError('invalid_grant', 'Authorization code was already used')
    }

    return AccessTokenService.createOauthGrant({
      name: params.client.clientName,
      clientId: params.client.id,
      clientSupportsRefresh,
      scopes: authorizationCode.scopes,
      resource: authorizationCode.resource,
      createdBy: authorizationCode.userId,
      trx,
    })
  })
}

export async function exchangeRefreshToken(params: {
  client: OauthClient
  refreshToken: string
  scope: string | null
  resource: string
}) {
  if (!params.client.grantTypeList.includes('refresh_token')) {
    throw new GatewayOauthError('unauthorized_client', 'This OAuth client cannot refresh tokens')
  }

  if (params.scope !== null && params.scope !== GATEWAY_OAUTH_SCOPE) {
    throw new GatewayOauthError('invalid_scope', 'Unsupported OAuth scope')
  }

  if (!sameUrl(params.resource, gatewayResourceUrl())) {
    throw new GatewayOauthError('invalid_target', 'The OAuth resource must be the MyMCPs gateway')
  }

  const rotated = await AccessTokenService.rotateOauthGrant({
    refreshToken: params.refreshToken,
    clientId: params.client.id,
    resource: gatewayResourceUrl(),
  })
  if (rotated.status !== 'rotated') {
    throw new GatewayOauthError('invalid_grant', 'Invalid, expired, or revoked refresh token')
  }

  return rotated.tokens
}

export function oauthTokenResponse(
  created: Awaited<ReturnType<typeof AccessTokenService.createOauthGrant>>
) {
  return {
    access_token: created.plaintext,
    token_type: 'Bearer',
    expires_in: OAUTH_ACCESS_TOKEN_TTL_SECONDS,
    scope: created.token.oauthScopes ?? GATEWAY_OAUTH_SCOPE,
    ...(created.refreshToken ? { refresh_token: created.refreshToken } : {}),
  }
}
