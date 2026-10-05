import type Mcp from '#models/mcp'
import {
  BuiltinAuthorizationError,
  type BuiltinOauthMcpDefinition,
} from '#services/builtin/definition'
import McpSecretStore from '#services/mcp_secret_store'
import { fetchWithSameOriginRedirects } from '#services/upstream/safe_fetch'
import { builtinTokenFailureValidator } from '#validators/builtin_oauth'
import { oauthTokenResponseValidator } from '#validators/oauth'

const TOKEN_REQUEST_TIMEOUT_MS = 30_000

type TokenGrant =
  | { grant_type: 'authorization_code'; code: string }
  | { grant_type: 'refresh_token'; refresh_token: string }

export type BuiltinOauthTokens = {
  access_token: string
  token_type: string
  refresh_token?: string
  expires_in?: number
  scope?: string
}

/**
 * Providers disagree on the separator, so store and compare scopes as a list.
 * The callback's `scope` parameter passes through the browser, so anything that
 * is not shaped like a scope name is dropped.
 */
export function parseOauthScopes(value: string | null | undefined) {
  return value
    ? value
        .split(/[\s,]+/)
        .filter((scope) => /^[\w:.-]{1,64}$/.test(scope))
        .slice(0, 32)
    : []
}

/** Write scopes are only requested once the admin allowed write access. */
export function requestedBuiltinScopes(definition: BuiltinOauthMcpDefinition, mcp: Mcp) {
  return [
    ...definition.oauth.scopes,
    ...(mcp.builtinWriteEnabled ? definition.oauth.writeScopes : []),
  ]
}

export function builtinAuthorizationUrl(
  definition: BuiltinOauthMcpDefinition,
  options: { clientId: string; redirectUri: string; state: string; scopes: readonly string[] }
) {
  const url = new URL(definition.oauth.authorizeUrl)
  url.searchParams.set('client_id', options.clientId)
  url.searchParams.set('redirect_uri', options.redirectUri)
  url.searchParams.set('response_type', 'code')
  url.searchParams.set('scope', options.scopes.join(definition.oauth.scopeSeparator))
  url.searchParams.set('state', options.state)
  for (const [name, value] of Object.entries(definition.oauth.authorizeParams ?? {})) {
    url.searchParams.set(name, value)
  }
  return url.toString()
}

/**
 * Explain a rejected token request from an RFC 6749 error body or from the
 * `{ message, errors: [{ resource, field, code }] }` shape Strava returns.
 */
async function describeTokenFailure(body: unknown) {
  const [unexpected, failure] = await builtinTokenFailureValidator.tryValidate(body)
  if (unexpected) return ''

  const { error, error_description: description, message, errors = [] } = failure
  const faults = errors
    .map((fault) => [fault.resource, fault.field, fault.code].filter(Boolean).join(' '))
    .filter(Boolean)
    .join('; ')
  const summary = [description, error, message].find(Boolean)
  return [summary, faults ? `(${faults})` : null].filter(Boolean).join(' ').slice(0, 200)
}

async function requestTokens(
  definition: BuiltinOauthMcpDefinition,
  mcp: Mcp,
  grant: TokenGrant
): Promise<BuiltinOauthTokens> {
  const clientId = mcp.oauthClientId
  const clientSecret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  if (!clientId || !clientSecret) {
    throw new Error(`${definition.name} Client ID and Client Secret are required`)
  }

  const response = await fetchWithSameOriginRedirects(
    definition.oauth.tokenUrl,
    {
      method: 'POST',
      headers: {
        'Accept': 'application/json',
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams({
        client_id: clientId,
        client_secret: clientSecret,
        ...grant,
      }).toString(),
      signal: AbortSignal.timeout(TOKEN_REQUEST_TIMEOUT_MS),
    },
    `${definition.name} token endpoint`
  )
  const body: unknown = await response.json().catch(() => null)

  if (!response.ok) {
    const reason = await describeTokenFailure(body)
    const rejected = response.status === 400 || response.status === 401
    if (grant.grant_type === 'refresh_token' && rejected) {
      throw new BuiltinAuthorizationError(
        `${definition.name} refused to renew the saved authorization${reason ? ` (${reason})` : ''}. Check the Client ID and Client Secret, then re-authorize this MCP in MyMCPs.`
      )
    }
    throw new Error(
      `${definition.name} rejected the token request (HTTP ${response.status})${reason ? `: ${reason}` : ''}${rejected ? '. Check the Client ID and Client Secret.' : ''}`
    )
  }

  try {
    const tokens = await oauthTokenResponseValidator.validate(body)
    return { ...tokens, token_type: tokens.token_type ?? 'Bearer' }
  } catch {
    throw new Error(`${definition.name} returned an unexpected token response`)
  }
}

export function exchangeBuiltinAuthorizationCode(
  definition: BuiltinOauthMcpDefinition,
  mcp: Mcp,
  code: string
) {
  return requestTokens(definition, mcp, { grant_type: 'authorization_code', code })
}

export function refreshBuiltinTokens(
  definition: BuiltinOauthMcpDefinition,
  mcp: Mcp,
  refreshToken: string
) {
  return requestTokens(definition, mcp, {
    grant_type: 'refresh_token',
    refresh_token: refreshToken,
  })
}
