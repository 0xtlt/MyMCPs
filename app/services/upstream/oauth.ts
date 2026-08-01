import { randomBytes, createHash } from 'node:crypto'
import { DateTime } from 'luxon'
import type { HttpContext } from '@adonisjs/core/http'
import env from '#start/env'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { isRecord, sanitizeErrorMessage } from '#services/unknown'

type OauthSession = {
  mcpId: number
  codeVerifier: string
  state: string
}

type OauthTokenResponse = {
  accessToken: string
  refreshToken?: string
  expiresIn?: number
}

function base64Url(buffer: Buffer) {
  return buffer.toString('base64url')
}

function pkceChallenge(verifier: string) {
  return base64Url(createHash('sha256').update(verifier).digest())
}

function isOauthSession(value: unknown): value is OauthSession {
  if (!isRecord(value)) {
    return false
  }
  return (
    typeof value.mcpId === 'number' &&
    Number.isFinite(value.mcpId) &&
    typeof value.codeVerifier === 'string' &&
    value.codeVerifier.length > 0 &&
    typeof value.state === 'string' &&
    value.state.length > 0
  )
}

function parseOauthTokenResponse(value: unknown): OauthTokenResponse {
  if (!isRecord(value) || typeof value.access_token !== 'string' || !value.access_token) {
    throw new Error('OAuth token response did not include access_token')
  }

  return {
    accessToken: value.access_token,
    refreshToken: typeof value.refresh_token === 'string' ? value.refresh_token : undefined,
    expiresIn: typeof value.expires_in === 'number' ? value.expires_in : undefined,
  }
}

export function oauthCallbackUrl() {
  return `${env.get('APP_URL').replace(/\/$/, '')}/mcps/oauth/callback`
}

export function startOauthSession(session: HttpContext['session'], mcp: Mcp) {
  const state = base64Url(randomBytes(24))
  const codeVerifier = base64Url(randomBytes(32))
  const payload: OauthSession = { mcpId: mcp.id, codeVerifier, state }
  session.put('mcp_oauth', payload)
  return payload
}

export function readOauthSession(session: HttpContext['session']): OauthSession | null {
  const raw = session.get('mcp_oauth')
  return isOauthSession(raw) ? raw : null
}

export function clearOauthSession(session: HttpContext['session']) {
  session.forget('mcp_oauth')
}

export function buildAuthorizeRedirect(mcp: Mcp, oauth: OauthSession) {
  if (!mcp.oauthAuthorizeUrl || !mcp.oauthClientId) {
    throw new Error('OAuth authorize URL and client ID are required')
  }

  const url = new URL(mcp.oauthAuthorizeUrl)
  url.searchParams.set('response_type', 'code')
  url.searchParams.set('client_id', mcp.oauthClientId)
  url.searchParams.set('redirect_uri', oauthCallbackUrl())
  url.searchParams.set('state', oauth.state)
  url.searchParams.set('code_challenge', pkceChallenge(oauth.codeVerifier))
  url.searchParams.set('code_challenge_method', 'S256')
  if (mcp.oauthScopes) {
    url.searchParams.set('scope', mcp.oauthScopes)
  }
  return url.toString()
}

async function readFailedOauthBody(response: Response) {
  try {
    const text = await response.text()
    return sanitizeErrorMessage(text, 300)
  } catch {
    return 'unable to read response body'
  }
}

export async function exchangeAuthorizationCode(mcp: Mcp, code: string, codeVerifier: string) {
  if (!mcp.oauthTokenUrl || !mcp.oauthClientId) {
    throw new Error('OAuth token URL and client ID are required')
  }

  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    redirect_uri: oauthCallbackUrl(),
    client_id: mcp.oauthClientId,
    code_verifier: codeVerifier,
  })

  const secret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  if (secret) {
    body.set('client_secret', secret)
  }

  const response = await fetch(mcp.oauthTokenUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      Accept: 'application/json',
    },
    body,
  })

  if (!response.ok) {
    const detail = await readFailedOauthBody(response)
    throw new Error(`OAuth token exchange failed (${response.status}): ${detail}`)
  }

  const json = parseOauthTokenResponse(await response.json())
  mcp.oauthAccessToken = McpSecretStore.encrypt(json.accessToken)
  if (json.refreshToken) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refreshToken)
  }
  mcp.oauthTokenExpiresAt = json.expiresIn
    ? DateTime.utc().plus({ seconds: json.expiresIn })
    : null
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
  if (!refresh || !mcp.oauthTokenUrl || !mcp.oauthClientId) {
    return
  }

  if (mcp.oauthTokenExpiresAt && mcp.oauthTokenExpiresAt > DateTime.utc().plus({ minutes: 2 })) {
    return
  }

  const body = new URLSearchParams({
    grant_type: 'refresh_token',
    refresh_token: refresh,
    client_id: mcp.oauthClientId,
  })
  const secret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  if (secret) {
    body.set('client_secret', secret)
  }

  const response = await fetch(mcp.oauthTokenUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      Accept: 'application/json',
    },
    body,
  })

  if (!response.ok) {
    const detail = await readFailedOauthBody(response)
    throw new Error(`OAuth refresh failed (${response.status}): ${detail}`)
  }

  const json = parseOauthTokenResponse(await response.json())
  mcp.oauthAccessToken = McpSecretStore.encrypt(json.accessToken)
  if (json.refreshToken) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refreshToken)
  }
  mcp.oauthTokenExpiresAt = json.expiresIn
    ? DateTime.utc().plus({ seconds: json.expiresIn })
    : mcp.oauthTokenExpiresAt
  await mcp.save()
}
