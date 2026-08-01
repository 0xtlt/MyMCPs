import vine from '@vinejs/vine'

const transport = vine.enum(['http', 'npm'] as const)
const authType = vine.enum(['none', 'bearer', 'header', 'oauth'] as const)

/**
 * Create an upstream MCP registration.
 */
export const createMcpValidator = vine.create({
  name: vine.string().trim().minLength(1).maxLength(120),
  description: vine.string().trim().maxLength(500).optional(),
  transport,
  httpUrl: vine.string().trim().url().maxLength(2048).optional(),
  npmPackage: vine.string().trim().maxLength(254).optional(),
  npmVersion: vine.string().trim().maxLength(64).optional(),
  npmArgs: vine.string().trim().maxLength(1000).optional(),
  authType,
  authBearer: vine.string().trim().maxLength(4000).optional(),
  authHeaderName: vine.string().trim().maxLength(120).optional(),
  authHeaderValue: vine.string().trim().maxLength(4000).optional(),
  oauthAuthorizeUrl: vine.string().trim().url().maxLength(2048).optional(),
  oauthTokenUrl: vine.string().trim().url().maxLength(2048).optional(),
  oauthScopes: vine.string().trim().maxLength(500).optional(),
  oauthClientId: vine.string().trim().maxLength(254).optional(),
  oauthClientSecret: vine.string().trim().maxLength(4000).optional(),
  enabled: vine.string().optional(),
})

/**
 * Update an upstream MCP registration.
 */
export const updateMcpValidator = vine.create({
  name: vine.string().trim().minLength(1).maxLength(120),
  description: vine.string().trim().maxLength(500).optional(),
  transport,
  httpUrl: vine.string().trim().url().maxLength(2048).optional(),
  npmPackage: vine.string().trim().maxLength(254).optional(),
  npmVersion: vine.string().trim().maxLength(64).optional(),
  npmArgs: vine.string().trim().maxLength(1000).optional(),
  authType,
  authBearer: vine.string().trim().maxLength(4000).optional(),
  authHeaderName: vine.string().trim().maxLength(120).optional(),
  authHeaderValue: vine.string().trim().maxLength(4000).optional(),
  oauthAuthorizeUrl: vine.string().trim().url().maxLength(2048).optional(),
  oauthTokenUrl: vine.string().trim().url().maxLength(2048).optional(),
  oauthScopes: vine.string().trim().maxLength(500).optional(),
  oauthClientId: vine.string().trim().maxLength(254).optional(),
  oauthClientSecret: vine.string().trim().maxLength(4000).optional(),
  enabled: vine.string().optional(),
})

/**
 * Create an agent access token (identifier).
 */
export const createAccessTokenValidator = vine.create({
  name: vine.string().trim().minLength(1).maxLength(120),
  scopeMode: vine.enum(['all', 'selected'] as const),
  mcpIds: vine.array(vine.string()).optional(),
  expiresAt: vine.string().optional(),
})
