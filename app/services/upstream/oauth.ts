import { randomBytes, createHash } from 'node:crypto'
import { DateTime } from 'luxon'
import type { HttpContext } from '@adonisjs/core/http'
import type { Infer } from '@vinejs/vine/types'
import env from '#start/env'
import type Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { oauthSessionValidator, oauthTokenResponseValidator } from '#validators/oauth'

type OauthSession = Infer<typeof oauthSessionValidator>

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

async function parseOauthTokenResponse(value: unknown): Promise<OauthTokenResponse> {
  try {
    const data = await oauthTokenResponseValidator.validate(value)
    return {
      accessToken: data.access_token,
      refreshToken: data.refresh_token,
      expiresIn: data.expires_in,
    }
  } catch {
    throw new Error('OAuth token response did not include access_token')
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

export async function readOauthSession(
  session: HttpContext['session']
): Promise<OauthSession | null> {
  try {
    return await oauthSessionValidator.validate(session.get('mcp_oauth'))
  } catch {
    return null
  }
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
    const body = await response.text()
    const text = body.trim()
    if (!text) {
      return 'empty response body'
    }
    return text.length > 300 ? `${text.slice(0, 300)}…` : text
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
      'Accept': 'application/json',
    },
    body,
  })

  if (!response.ok) {
    const detail = await readFailedOauthBody(response)
    throw new Error(`OAuth token exchange failed (${response.status}): ${detail}`)
  }

  const json = await parseOauthTokenResponse(await response.json())
  mcp.oauthAccessToken = McpSecretStore.encrypt(json.accessToken)
  if (json.refreshToken) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refreshToken)
  }
  mcp.oauthTokenExpiresAt = json.expiresIn ? DateTime.utc().plus({ seconds: json.expiresIn }) : null
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
      'Accept': 'application/json',
    },
    body,
  })

  if (!response.ok) {
    const detail = await readFailedOauthBody(response)
    throw new Error(`OAuth refresh failed (${response.status}): ${detail}`)
  }

  const json = await parseOauthTokenResponse(await response.json())
  mcp.oauthAccessToken = McpSecretStore.encrypt(json.accessToken)
  if (json.refreshToken) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refreshToken)
  }
  mcp.oauthTokenExpiresAt = json.expiresIn
    ? DateTime.utc().plus({ seconds: json.expiresIn })
    : mcp.oauthTokenExpiresAt
  await mcp.save()
}
