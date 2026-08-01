import { randomBytes, createHash } from 'node:crypto'
import { DateTime } from 'luxon'
import type { HttpContext } from '@adonisjs/core/http'
import env from '#start/env'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'

type OauthSession = {
  mcpId: number
  codeVerifier: string
  state: string
}

function base64Url(buffer: Buffer) {
  return buffer.toString('base64url')
}

function pkceChallenge(verifier: string) {
  return base64Url(createHash('sha256').update(verifier).digest())
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

export function readOauthSession(session: HttpContext['session']) {
  return session.get('mcp_oauth') as OauthSession | null
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
    const text = await response.text()
    throw new Error(`OAuth token exchange failed (${response.status}): ${text.slice(0, 300)}`)
  }

  const json = (await response.json()) as {
    access_token?: string
    refresh_token?: string
    expires_in?: number
  }

  if (!json.access_token) {
    throw new Error('OAuth token response did not include access_token')
  }

  mcp.oauthAccessToken = McpSecretStore.encrypt(json.access_token)
  if (json.refresh_token) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refresh_token)
  }
  mcp.oauthTokenExpiresAt = json.expires_in
    ? DateTime.utc().plus({ seconds: json.expires_in })
    : null
  mcp.status = 'ready'
  mcp.lastError = null
  await mcp.save()
}

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
    return
  }

  const json = (await response.json()) as {
    access_token?: string
    refresh_token?: string
    expires_in?: number
  }

  if (!json.access_token) {
    return
  }

  mcp.oauthAccessToken = McpSecretStore.encrypt(json.access_token)
  if (json.refresh_token) {
    mcp.oauthRefreshToken = McpSecretStore.encrypt(json.refresh_token)
  }
  mcp.oauthTokenExpiresAt = json.expires_in
    ? DateTime.utc().plus({ seconds: json.expires_in })
    : mcp.oauthTokenExpiresAt
  await mcp.save()
}
