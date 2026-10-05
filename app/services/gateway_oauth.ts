import { createHash, randomBytes, timingSafeEqual } from 'node:crypto'
import { DateTime } from 'luxon'
import { OAuthClientMetadataSchema } from '@modelcontextprotocol/sdk/shared/auth.js'
import OauthAuthorizationCode from '#models/oauth_authorization_code'
import OauthClient from '#models/oauth_client'
import AccessTokenService from '#services/access_token_service'
import { GATEWAY_OAUTH_SCOPE, LOOPBACK_HOSTS } from '#services/gateway_oauth_constants'
import { requirePublicAppUrl } from '#services/public_url'
import { sanitizeDiagnostic } from '#services/security_redaction'
import {
  authorizationClientIdValidator,
  authorizationRedirectUriValidator,
  authorizationResponseTypeValidator,
  authorizationStateLengthValidator,
  authorizationStateValidator,
  clientAuthMethodValidator,
  clientGrantTypesValidator,
  clientNameValidator,
  clientRedirectUrisValidator,
  clientResponseTypesValidator,
  gatewayResourceValidator,
  pkceChallengeValidator,
  pkceVerifierValidator,
  postedClientCredentialsValidator,
  refreshScopeValidator,
  requestedScopeValidator,
} from '#validators/gateway_oauth'
import logger from '@adonisjs/core/services/logger'
import db from '@adonisjs/lucid/services/db'

export { GATEWAY_OAUTH_SCOPE }
export const OAUTH_ACCESS_TOKEN_TTL_SECONDS = 60 * 60
const AUTHORIZATION_CODE_TTL_MINUTES = 5
const CLIENT_SECRET_TTL_DAYS = 365

/**
 * Registration is open to anyone, so the number of stored clients is bounded
 * and clients that nobody has used for the retention period are removed.
 */
export const MAX_OAUTH_CLIENTS = 1000
export const UNUSED_CLIENT_RETENTION_DAYS = 90
const CLIENT_PRUNE_INTERVAL_MS = 60 * 60 * 1000

let lastClientPruneAt = 0

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

/** True when the redirect URI points at the user's own device. */
export function isLoopbackRedirectUri(value: string) {
  try {
    return LOOPBACK_HOSTS.has(new URL(value).hostname)
  } catch {
    return false
  }
}

/**
 * Clients nobody is using: no pending authorization code, and no token that
 * still works or that was issued or used since `activeSince`. The second part
 * keeps a client known while its user signs in again after a grant expired.
 */
function unusedOauthClients(activeSince: DateTime) {
  const now = DateTime.utc().toSQL({ includeOffset: false })!
  const since = activeSince.toSQL({ includeOffset: false })!

  return OauthClient.query()
    .whereNotExists(
      db
        .from('oauth_authorization_codes')
        .whereColumn('oauth_authorization_codes.oauth_client_id', 'oauth_clients.id')
        .where('oauth_authorization_codes.expires_at', '>', now)
    )
    .whereNotExists(
      db
        .from('access_tokens')
        .whereColumn('access_tokens.oauth_client_id', 'oauth_clients.id')
        .where((token) => {
          token.where('access_tokens.updated_at', '>=', since).orWhere((live) => {
            live.whereNull('access_tokens.revoked_at').where((unexpired) => {
              unexpired
                .where('access_tokens.expires_at', '>', now)
                .orWhere('access_tokens.oauth_refresh_expires_at', '>', now)
            })
          })
        })
    )
}

/**
 * Remove clients that went unused for the whole retention period. Nothing
 * else deletes them, so this runs with registration, at most once an hour.
 */
export async function pruneUnusedOauthClients(options: { force?: boolean } = {}) {
  const now = Date.now()
  if (!options.force && now - lastClientPruneAt < CLIENT_PRUNE_INTERVAL_MS) {
    return
  }
  lastClientPruneAt = now

  try {
    const cutoff = DateTime.utc().minus({ days: UNUSED_CLIENT_RETENTION_DAYS })
    await unusedOauthClients(cutoff)
      .where('created_at', '<', cutoff.toSQL({ includeOffset: false })!)
      .delete()
  } catch (error) {
    logger.warn({ error: sanitizeDiagnostic(error) }, 'Unused OAuth clients could not be pruned')
  }
}

/**
 * Keep the client table within its limit. At the limit the oldest unused
 * clients give way to the new one whatever their age, so that filling the
 * table with throwaway registrations cannot lock real clients out.
 */
async function makeRoomForOauthClient() {
  const [{ total }] = await db.from('oauth_clients').count('* as total')
  const excess = Number(total) - MAX_OAUTH_CLIENTS + 1
  if (excess <= 0) return

  const activeSince = DateTime.utc().minus({ days: UNUSED_CLIENT_RETENTION_DAYS })
  const evictable = await unusedOauthClients(activeSince)
    .select('id')
    .orderBy('id', 'asc')
    .limit(excess)
  if (evictable.length < excess) {
    throw new GatewayOauthError(
      'temporarily_unavailable',
      'Too many OAuth clients are registered',
      503
    )
  }

  await unusedOauthClients(activeSince)
    .whereIn(
      'id',
      evictable.map((client) => client.id)
    )
    .delete()
}

export async function registerOauthClient(input: unknown) {
  const parsed = OAuthClientMetadataSchema.safeParse(input)
  if (!parsed.success) {
    throw new GatewayOauthError('invalid_client_metadata', 'Invalid OAuth client metadata')
  }

  // The first check that fails decides the error the client is given.
  const metadata = parsed.data
  const [unsafeRedirectUri] = await clientRedirectUrisValidator.tryValidate(metadata.redirect_uris)
  if (unsafeRedirectUri) {
    throw new GatewayOauthError(
      'invalid_redirect_uri',
      'Redirect URIs must use HTTPS, HTTP on an exact loopback host, or an approved native-app callback'
    )
  }

  const [unsupportedAuthMethod, authMethod] = await clientAuthMethodValidator.tryValidate(
    metadata.token_endpoint_auth_method
  )
  if (unsupportedAuthMethod) {
    throw new GatewayOauthError(
      'invalid_client_metadata',
      'Unsupported token endpoint authentication method'
    )
  }

  const [unsupportedGrantType, grantTypes] = await clientGrantTypesValidator.tryValidate(
    metadata.grant_types
  )
  if (unsupportedGrantType) {
    throw new GatewayOauthError('invalid_client_metadata', 'Unsupported OAuth grant type')
  }

  const [unsupportedResponseType, responseTypes] = await clientResponseTypesValidator.tryValidate(
    metadata.response_types
  )
  if (unsupportedResponseType) {
    throw new GatewayOauthError(
      'invalid_client_metadata',
      'Only the code response type is supported'
    )
  }

  const [unsupportedScope] = await requestedScopeValidator.tryValidate(metadata.scope)
  if (unsupportedScope) {
    throw new GatewayOauthError('invalid_client_metadata', 'Unsupported OAuth scope')
  }

  const [nameTooLong, name] = await clientNameValidator.tryValidate(metadata.client_name)
  if (nameTooLong) {
    throw new GatewayOauthError('invalid_client_metadata', 'Client name is too long')
  }
  const clientName = name || 'MCP client'

  await pruneUnusedOauthClients()
  await makeRoomForOauthClient()

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
  const [noClientId, clientId] = await authorizationClientIdValidator.tryValidate(input.client_id)
  if (noClientId) {
    throw new GatewayOauthError('invalid_request', 'client_id is required')
  }

  const client = await OauthClient.findBy('client_id', clientId)
  if (!client) {
    throw new GatewayOauthError('invalid_client', 'Unknown OAuth client')
  }

  const [unregistered, redirectUri] = await authorizationRedirectUriValidator.tryValidate(
    input.redirect_uri,
    { meta: { registeredRedirectUris: client.redirectUriList } }
  )
  if (unregistered) {
    throw new GatewayOauthError('invalid_request', 'Unregistered redirect_uri')
  }

  // From here on an error can be returned to the client, and carries its
  // state. The state is therefore read now, and its length is checked last.
  const [, state = null] = await authorizationStateValidator.tryValidate(input.state)
  const redirectError = (code: string, message: string) =>
    new GatewayOauthError(code, message, 400, redirectUri, state)

  const [unsupportedResponseType] = await authorizationResponseTypeValidator.tryValidate(
    input.response_type
  )
  if (unsupportedResponseType) {
    throw redirectError('unsupported_response_type', 'Only the code response type is supported')
  }

  const [withoutPkce, pkce] = await pkceChallengeValidator.tryValidate(input)
  if (withoutPkce) {
    throw redirectError('invalid_request', 'PKCE with the S256 method is required')
  }

  const [unsupportedScope] = await requestedScopeValidator.tryValidate(input.scope)
  if (unsupportedScope) {
    throw redirectError('invalid_scope', 'Unsupported OAuth scope')
  }

  const [foreignResource] = await gatewayResourceValidator.tryValidate(input.resource, {
    meta: { gatewayResource: gatewayResourceUrl() },
  })
  if (foreignResource) {
    throw redirectError('invalid_target', 'The OAuth resource must be the MyMCPs gateway')
  }

  const [stateTooLong] = await authorizationStateLengthValidator.tryValidate(state)
  if (stateTooLong) {
    throw redirectError('invalid_request', 'OAuth state is too long')
  }

  return {
    client,
    redirectUri,
    state,
    codeChallenge: pkce.code_challenge,
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

/** Whether a well-formed code verifier is the one behind the stored challenge. */
export function verifyCodeChallenge(codeVerifier: string, expectedChallenge: string) {
  const actual = createHash('sha256').update(codeVerifier).digest('base64url')
  const actualBuffer = Buffer.from(actual)
  const expectedBuffer = Buffer.from(expectedChallenge)
  return (
    actualBuffer.length === expectedBuffer.length && timingSafeEqual(actualBuffer, expectedBuffer)
  )
}

type ClientCredentials = { clientId: string; clientSecret: string | null; method: string }

function basicClientCredentials(header: string | undefined): ClientCredentials | null {
  const match = header?.match(/^Basic\s+(.+)$/i)
  if (!match) return null

  try {
    const decoded = Buffer.from(match[1], 'base64').toString('utf8')
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
  const [, posted] = await postedClientCredentialsValidator.tryValidate(input)
  const credentials: ClientCredentials | null =
    basic ??
    (posted
      ? {
          clientId: posted.client_id,
          clientSecret: posted.client_secret ?? null,
          method: posted.client_secret ? 'client_secret_post' : 'none',
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
  // A malformed verifier or a resource other than the gateway is answered
  // like a code that does not match.
  const [malformedVerifier] = await pkceVerifierValidator.tryValidate(params.codeVerifier)
  const [foreignResource] = await gatewayResourceValidator.tryValidate(params.resource, {
    meta: { gatewayResource: gatewayResourceUrl() },
  })

  if (
    malformedVerifier ||
    foreignResource ||
    !authorizationCode ||
    authorizationCode.oauthClientId !== params.client.id ||
    authorizationCode.expiresAt <= DateTime.utc() ||
    authorizationCode.redirectUri !== params.redirectUri ||
    !sameUrl(authorizationCode.resource, params.resource) ||
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

  const [unsupportedScope] = await refreshScopeValidator.tryValidate(params.scope)
  if (unsupportedScope) {
    throw new GatewayOauthError('invalid_scope', 'Unsupported OAuth scope')
  }

  const [foreignResource] = await gatewayResourceValidator.tryValidate(params.resource, {
    meta: { gatewayResource: gatewayResourceUrl() },
  })
  if (foreignResource) {
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
