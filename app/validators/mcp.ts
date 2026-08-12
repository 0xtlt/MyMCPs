import vine from '@vinejs/vine'

const transport = vine.enum(['http', 'npm'] as const)
const authType = vine.enum(['auto', 'bearer', 'header'] as const)
const reservedNpmEnvNames = ['HOME', 'TMPDIR', 'NO_COLOR']

/** RFC 9110 token for custom header names. */
const headerName = vine
  .string()
  .trim()
  .maxLength(120)
  .regex(/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/)

const npmEnvironment = vine
  .array(
    vine.object({
      name: vine
        .string()
        .trim()
        .maxLength(128)
        .regex(/^[A-Za-z_][A-Za-z0-9_]*$/)
        .notIn(reservedNpmEnvNames),
      // Do not trim secrets. Empty HTML inputs become null through the global Vine transform.
      value: vine.string().maxLength(8192).nullable(),
    })
  )
  .maxLength(50)
  .optional()

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
  npmPackage: vine.string().trim().maxLength(254).optional().requiredWhen('transport', '=', 'npm'),
  npmVersion: vine.string().trim().maxLength(64).optional(),
  npmArgs: vine
    .string()
    .trim()
    .maxLength(1000)
    .nullable()
    .optional()
    .transform((value): string[] => {
      if (!value) {
        return []
      }
      return value
        .split(/\s+/)
        .map((part) => part.trim())
        .filter(Boolean)
    }),
  npmEnv: npmEnvironment,
  authType,
  authBearer: vine.string().trim().maxLength(4000).optional(),
  authHeaderName: headerName.optional().requiredWhen('authType', '=', 'header'),
  authHeaderValue: vine.string().trim().maxLength(4000).optional(),
  /** CheckboxInput submits "on" when checked; omitted when unchecked. */
  enabled: vine.boolean().optional(),
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
const accessTokenPayload = {
  name: vine.string().trim().minLength(1).maxLength(120),
  scopeMode: vine.enum(['all', 'selected'] as const),
  mcpIds: vine
    .array(vine.number().withoutDecimals().min(1))
    .minLength(1)
    .optional()
    .requiredWhen('scopeMode', '=', 'selected'),
  expiresAt: vine
    .date({ formats: ['iso8601'] })
    .nullable()
    .optional(),
}

export const createAccessTokenValidator = vine.create(accessTokenPayload)

/**
 * Update an agent access token without rotating its secret.
 */
export const updateAccessTokenValidator = vine.create(accessTokenPayload)

/**
 * Route parameters for access-token mutations.
 */
export const accessTokenParamsValidator = vine.create({
  id: vine.number().withoutDecimals().positive(),
})

/**
 * Expired or revoked access tokens selected for permanent deletion.
 */
export const deleteAccessTokensValidator = vine.create({
  ids: vine.array(vine.number().withoutDecimals().positive()).minLength(1).maxLength(500),
})
