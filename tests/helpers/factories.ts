import { DateTime } from 'luxon'
import AccessTokenService from '#services/access_token_service'
import AccessToken from '#models/access_token'
import Invite from '#models/invite'
import Mcp from '#models/mcp'
import User from '#models/user'
import McpCallLog from '#models/mcp_call_log'
import McpDebugSession from '#models/mcp_debug_session'

let sequence = 0

function nextValue(prefix: string) {
  sequence += 1
  return `${prefix}-${sequence}`
}

export async function createUser(
  overrides: Partial<{
    fullName: string
    email: string
    password: string
    role: 'admin' | 'member'
  }> = {}
) {
  return User.create({
    fullName: overrides.fullName ?? 'Test User',
    email: overrides.email ?? `${nextValue('user')}@example.com`,
    password: overrides.password ?? 'password123',
    role: overrides.role ?? 'member',
  })
}

export function createAdmin(overrides: Parameters<typeof createUser>[0] = {}) {
  return createUser({ ...overrides, role: 'admin' })
}

export function createMember(overrides: Parameters<typeof createUser>[0] = {}) {
  return createUser({ ...overrides, role: 'member' })
}

export async function createInvite(
  createdBy: number,
  overrides: Partial<{
    email: string
    role: 'admin' | 'member'
    token: string
    acceptedAt: DateTime | null
    expiresAt: DateTime
  }> = {}
) {
  return Invite.create({
    email: overrides.email ?? `${nextValue('invite')}@example.com`,
    role: overrides.role ?? 'member',
    token: overrides.token ?? Invite.generateToken(),
    createdBy,
    acceptedAt: overrides.acceptedAt ?? null,
    expiresAt: overrides.expiresAt ?? DateTime.utc().plus({ days: 7 }),
  })
}

export async function createMcp(
  createdBy: number,
  overrides: Partial<{
    name: string
    slug: string
    enabled: boolean
    status: 'draft' | 'ready' | 'error'
    transport: 'http' | 'npm'
    authType: 'auto' | 'bearer' | 'header'
    oauthRequired: boolean
    httpUrl: string | null
    npmPackage: string | null
    npmVersion: string | null
  }> = {}
) {
  const name = overrides.name ?? nextValue('MCP')
  const transport = overrides.transport ?? 'http'

  return Mcp.create({
    name,
    slug: overrides.slug ?? Mcp.slugify(name),
    description: null,
    transport,
    httpUrl: overrides.httpUrl ?? (transport === 'http' ? 'http://127.0.0.1:9999/mcp' : null),
    npmPackage: overrides.npmPackage ?? (transport === 'npm' ? '@example/mcp' : null),
    npmVersion: overrides.npmVersion ?? null,
    npmArgs: null,
    npmEnv: null,
    authType: overrides.authType ?? 'auto',
    authBearer: null,
    authHeaderName: null,
    authHeaderValue: null,
    oauthAuthorizeUrl: null,
    oauthTokenUrl: null,
    oauthScopes: null,
    oauthClientId: null,
    oauthClientSecret: null,
    oauthAccessToken: null,
    oauthRefreshToken: null,
    oauthTokenExpiresAt: null,
    oauthIssuer: null,
    oauthResource: null,
    oauthRedirectUri: null,
    oauthRequired: overrides.oauthRequired ?? false,
    oauthClientAuthMethod: null,
    oauthTokenType: null,
    status: overrides.status ?? 'ready',
    lastError: null,
    enabled: overrides.enabled ?? true,
    createdBy,
  })
}

export async function createAccessToken(
  createdBy: number,
  options: {
    name?: string
    scopeMode?: 'all' | 'selected'
    mcpIds?: number[]
    expiresAt?: DateTime | null
  } = {}
) {
  return AccessTokenService.create({
    name: options.name ?? nextValue('token'),
    scopeMode: options.scopeMode ?? 'all',
    mcpIds: options.mcpIds ?? [],
    expiresAt: options.expiresAt ?? null,
    createdBy,
  })
}

export async function createStoredAccessToken(
  createdBy: number,
  overrides: Partial<{
    name: string
    tokenHash: string
    tokenPrefix: string
    scopeMode: 'all' | 'selected'
    expiresAt: DateTime | null
    revokedAt: DateTime | null
    lastUsedAt: DateTime | null
  }> = {}
) {
  return AccessToken.create({
    name: overrides.name ?? nextValue('stored-token'),
    tokenHash: overrides.tokenHash ?? 'stored-token-hash',
    tokenPrefix: overrides.tokenPrefix ?? 'mcp_stored',
    scopeMode: overrides.scopeMode ?? 'all',
    expiresAt: overrides.expiresAt ?? null,
    revokedAt: overrides.revokedAt ?? null,
    lastUsedAt: overrides.lastUsedAt ?? null,
    createdBy,
  })
}

export async function createMcpCallLog(
  accessToken: AccessToken,
  overrides: Partial<{
    mcp: Mcp | null
    requestedToolName: string
    toolName: string | null
    outcome: 'success' | 'error'
    errorCategory: 'invalid_tool' | 'disallowed_mcp' | 'upstream_exception' | 'tool_error' | null
    errorSummary: string | null
    arguments: string | null
    argumentsCaptured: boolean
    response: string | null
    responseCaptured: boolean
    callerIp: string | null
    durationMs: number
    debugSessionId: number | null
    debugSessionElapsedMs: number | null
    argumentsBytes: number
    argumentsRedacted: boolean
    responseBytes: number
    responseRedacted: boolean
    startedAt: DateTime | null
    createdAt: DateTime
  }> = {}
) {
  const mcp = overrides.mcp ?? null
  return McpCallLog.create({
    accessTokenId: accessToken.id,
    accessTokenName: accessToken.name,
    accessTokenPrefix: accessToken.tokenPrefix,
    callerIp: overrides.callerIp ?? null,
    mcpId: mcp?.id ?? null,
    mcpName: mcp?.name ?? null,
    mcpSlug: mcp?.slug ?? null,
    requestedToolName: overrides.requestedToolName ?? `${mcp?.slug ?? 'test'}__echo`,
    toolName: overrides.toolName === undefined ? 'echo' : overrides.toolName,
    outcome: overrides.outcome ?? 'success',
    errorCategory: overrides.errorCategory ?? null,
    errorSummary: overrides.errorSummary ?? null,
    arguments: overrides.arguments ?? null,
    argumentsCaptured: overrides.argumentsCaptured ?? false,
    response: overrides.response ?? null,
    responseCaptured: overrides.responseCaptured ?? false,
    argumentsBytes: overrides.argumentsBytes ?? 0,
    argumentsRedacted: overrides.argumentsRedacted ?? false,
    responseBytes: overrides.responseBytes ?? 0,
    responseRedacted: overrides.responseRedacted ?? false,
    debugSessionId: overrides.debugSessionId ?? null,
    debugSessionElapsedMs: overrides.debugSessionElapsedMs ?? null,
    durationMs: overrides.durationMs ?? 25,
    startedAt: overrides.startedAt ?? overrides.createdAt,
    createdAt: overrides.createdAt,
  })
}

export async function createMcpDebugSession(
  accessToken: AccessToken,
  createdBy: number,
  overrides: Partial<{
    status: 'active' | 'paused' | 'stopped'
    startedAt: DateTime
    pausedAt: DateTime | null
    pausedDurationMs: number
    stateVersion: number
    endedAt: DateTime | null
  }> = {}
) {
  const status = overrides.status ?? 'active'
  return McpDebugSession.create({
    accessTokenId: accessToken.id,
    accessTokenName: accessToken.name,
    accessTokenPrefix: accessToken.tokenPrefix,
    status,
    createdBy,
    startedAt: overrides.startedAt ?? DateTime.utc(),
    pausedAt: overrides.pausedAt ?? (status === 'paused' ? DateTime.utc() : null),
    pausedDurationMs: overrides.pausedDurationMs ?? 0,
    stateVersion: overrides.stateVersion ?? 0,
    endedAt: overrides.endedAt ?? (status === 'stopped' ? DateTime.utc() : null),
  })
}
