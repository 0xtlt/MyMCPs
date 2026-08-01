import vine from '@vinejs/vine'

const transport = vine.enum(['http', 'npm'] as const)
const authType = vine.enum(['none', 'bearer', 'header', 'oauth'] as const)

/** RFC 9110 token for custom header names. */
const headerName = vine
  .string()
  .trim()
  .maxLength(120)
  .regex(/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/)

const mcpPayload = {
  name: vine.string().trim().minLength(1).maxLength(120),
  description: vine.string().trim().maxLength(500).optional(),
  transport,
  httpUrl: vine
    .string()
    .trim()
    .url()
    .maxLength(2048)
    .optional()
    .requiredWhen('transport', '=', 'http'),
  npmPackage: vine
    .string()
    .trim()
    .maxLength(254)
    .optional()
    .requiredWhen('transport', '=', 'npm'),
  npmVersion: vine.string().trim().maxLength(64).optional(),
  npmArgs: vine.string().trim().maxLength(1000).optional(),
  authType,
  authBearer: vine.string().trim().maxLength(4000).optional(),
  authHeaderName: headerName.optional().requiredWhen('authType', '=', 'header'),
  authHeaderValue: vine.string().trim().maxLength(4000).optional(),
  oauthAuthorizeUrl: vine
    .string()
    .trim()
    .url()
    .maxLength(2048)
    .optional()
    .requiredWhen('authType', '=', 'oauth'),
  oauthTokenUrl: vine
    .string()
    .trim()
    .url()
    .maxLength(2048)
    .optional()
    .requiredWhen('authType', '=', 'oauth'),
  oauthScopes: vine.string().trim().maxLength(500).optional(),
  oauthClientId: vine
    .string()
    .trim()
    .maxLength(254)
    .optional()
    .requiredWhen('authType', '=', 'oauth'),
  oauthClientSecret: vine.string().trim().maxLength(4000).optional(),
  enabled: vine.string().optional(),
}

/**
 * Create an upstream MCP registration.
 */
export const createMcpValidator = vine.create(mcpPayload)

/**
 * Update an upstream MCP registration.
 */
export const updateMcpValidator = vine.create(mcpPayload)

/**
 * Create an agent access token (identifier).
 */
export const createAccessTokenValidator = vine.create({
  name: vine.string().trim().minLength(1).maxLength(120),
  scopeMode: vine.enum(['all', 'selected'] as const),
  mcpIds: vine
    .array(vine.number().withoutDecimals().min(1))
    .optional()
    .requiredWhen('scopeMode', '=', 'selected'),
  expiresAt: vine.string().optional(),
})
