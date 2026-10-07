import type { HttpContext } from '@adonisjs/core/http'
import logger from '@adonisjs/core/services/logger'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcpValidator, updateMcpValidator } from '#validators/mcp'
import { oauthCallbackValidator, oauthStartValidator } from '#validators/oauth'
import { recordIdParamsValidator } from '#validators/route_params'
import { flashedRecordIdValidator } from '#validators/session'
import { testAndUpdateStatus } from '#services/upstream/manager'
import { removeMcpSandbox } from '#services/upstream/deno_runner'
import { removeBuiltinUploads } from '#services/builtin/upload_store'
import { McpNpmUpdateError, updateMcpToLatest } from '#services/mcp_npm_update_service'
import {
  clearOauthSession,
  exchangeAuthorizationCode,
  readOauthSession,
  startOauthFlow,
} from '#services/upstream/oauth'
import McpTransformer from '#transformers/mcp_transformer'
import type { Infer } from '@vinejs/vine/types'
import { sanitizeDiagnostic, sanitizeMcpDiagnostic } from '#services/security_redaction'
import { parseHttpUrl } from '#services/http_url'
import type {
  BuiltinOauthMcpDefinition,
  BuiltinPasswordMcpDefinition,
} from '#services/builtin/definition'
import { builtinMcp } from '#services/builtin/registry'
import { publicOauthAppUrl } from '#services/public_url'

type McpPayload = Infer<typeof createMcpValidator>

const MAX_NPM_ENV_TOTAL_BYTES = 64 * 1024

function npmEnvValidationError(field: string, message: string): never {
  throw new errors.E_VALIDATION_ERROR([
    {
      field,
      message,
      rule: 'npmEnvironment',
    },
  ])
}

/** The validator has checked the URL; this gives it its canonical form. */
function normalizedHttpUrl(value: string) {
  return parseHttpUrl(value, 'MCP URL').toString()
}

/**
 * `keepsSavedValues` is false when the MCP now runs another package: the saved
 * values were entered for the previous one and must be typed again.
 */
function assignNpmEnvironment(mcp: Mcp, payload: McpPayload, keepsSavedValues: boolean) {
  if (payload.transport !== 'npm') {
    mcp.npmEnv = null
    return
  }

  const entries = payload.npmEnv ?? []
  const seenNames = new Set<string>()
  const savedEnv = keepsSavedValues ? mcp.npmEnv : null

  for (const [index, entry] of entries.entries()) {
    if (seenNames.has(entry.name)) {
      npmEnvValidationError(`npmEnv.${index}.name`, 'Environment variable names must be unique')
    }
    seenNames.add(entry.name)

    if (entry.value === null && !McpEnvironmentStore.hasName(savedEnv, entry.name)) {
      npmEnvValidationError(
        `npmEnv.${index}.value`,
        McpEnvironmentStore.hasName(mcp.npmEnv, entry.name)
          ? 'Enter this value again: saved values are not passed on to a different package'
          : 'A value is required for a new environment variable'
      )
    }
  }

  const nextValue = McpEnvironmentStore.merge(savedEnv, entries)
  let environment: Record<string, string>
  try {
    environment = McpEnvironmentStore.decrypt(nextValue)
  } catch (error) {
    npmEnvValidationError(
      'npmEnv',
      error instanceof Error ? error.message : 'Environment variable configuration is invalid'
    )
  }

  const totalBytes = Object.entries(environment).reduce(
    (total, [name, value]) => total + Buffer.byteLength(name) + Buffer.byteLength(value),
    0
  )
  if (totalBytes > MAX_NPM_ENV_TOTAL_BYTES) {
    npmEnvValidationError('npmEnv', 'Environment variables must not exceed 64 KiB in total')
  }

  mcp.npmEnv = nextValue
}

function applySecrets(mcp: Mcp, payload: McpPayload) {
  if (payload.authBearer && payload.authBearer.length > 0) {
    mcp.authBearer = McpSecretStore.encrypt(payload.authBearer)
  }
  if (payload.authHeaderValue && payload.authHeaderValue.length > 0) {
    mcp.authHeaderValue = McpSecretStore.encrypt(payload.authHeaderValue)
  }
}

/**
 * Clear secrets that no longer apply to the selected auth type.
 */
function clearUnusedAuthSecrets(mcp: Mcp) {
  if (mcp.authType !== 'bearer') {
    mcp.authBearer = null
  }
  if (mcp.authType !== 'header') {
    mcp.authHeaderName = null
    mcp.authHeaderValue = null
  }
  if (mcp.authType !== 'auto') {
    mcp.oauthAuthorizeUrl = null
    mcp.oauthTokenUrl = null
    mcp.oauthScopes = null
    mcp.oauthClientId = null
    mcp.oauthClientSecret = null
    mcp.oauthAccessToken = null
    mcp.oauthRefreshToken = null
    mcp.oauthTokenExpiresAt = null
    mcp.oauthIssuer = null
    mcp.oauthResource = null
    mcp.oauthRedirectUri = null
    mcp.oauthClientAuthMethod = null
    mcp.oauthTokenType = null
    mcp.oauthRequired = false
  }
}

type FieldFailure = { field: string; message: string; rule: string }

const MAX_BUILTIN_ALIASES = 20

/**
 * Save the API application credentials of a built-in MCP. Tokens belong to
 * the application that issued them, so new credentials disconnect the account.
 */
function assignOauthApplication(
  mcp: Mcp,
  payload: McpPayload,
  definition: BuiltinOauthMcpDefinition
) {
  const clientId = payload.oauthClientId ?? ''
  const savedSecret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  const clientSecret = payload.oauthClientSecret || savedSecret

  // Report both fields at once: they are copied from the same provider page.
  const failures: FieldFailure[] = []
  if (!clientId) {
    failures.push({
      field: 'oauthClientId',
      message: `Enter the Client ID of your ${definition.name} API application`,
      rule: 'required',
    })
  } else if (definition.oauth.clientIdPattern && !definition.oauth.clientIdPattern.test(clientId)) {
    failures.push({
      field: 'oauthClientId',
      message: definition.oauth.clientIdHint ?? `${definition.name} Client ID is not valid`,
      rule: 'regex',
    })
  }
  if (!clientSecret) {
    failures.push({
      field: 'oauthClientSecret',
      message: `Enter the Client Secret of your ${definition.name} API application`,
      rule: 'required',
    })
  }
  if (failures.length > 0 || !clientSecret) {
    throw new errors.E_VALIDATION_ERROR(failures)
  }

  if (mcp.oauthClientId !== clientId || savedSecret !== clientSecret) {
    clearOAuthConnection(mcp)
  }
  mcp.oauthClientId = clientId
  mcp.oauthClientSecret = McpSecretStore.encrypt(clientSecret)
}

/**
 * Save the account name and app password of a built-in MCP, what agents may
 * do with them, and which other addresses of the account they may act as. A
 * blank password keeps the saved one, which is never sent back to the browser.
 */
function assignPasswordSignIn(
  mcp: Mcp,
  payload: McpPayload,
  definition: BuiltinPasswordMcpDefinition
) {
  const username = payload.builtinUsername ?? ''
  const password = payload.builtinPassword || McpSecretStore.decrypt(mcp.builtinPassword)

  const failures: FieldFailure[] = []
  if (!definition.password.usernamePattern.test(username)) {
    failures.push({
      field: 'builtinUsername',
      message: definition.password.usernameHint,
      rule: username ? 'regex' : 'required',
    })
  }
  if (!password || !definition.password.passwordPattern.test(password)) {
    failures.push({
      field: 'builtinPassword',
      message: definition.password.passwordHint,
      rule: password ? 'regex' : 'required',
    })
  }
  const requested = payload.builtinPermissions ?? []
  const unknown = requested.find((name) => !definition.password.permissions.includes(name))
  if (unknown !== undefined || requested.length === 0) {
    failures.push({
      field: 'builtinPermissions',
      message:
        unknown === undefined
          ? 'Allow at least one permission'
          : `${definition.name} has no "${unknown}" permission`,
      rule: unknown === undefined ? 'required' : 'enum',
    })
  }
  const seen = new Set([username.toLowerCase()])
  const aliases = (payload.builtinAliases ?? []).filter((alias) => {
    const key = alias.toLowerCase()
    return !seen.has(key) && Boolean(seen.add(key))
  })
  if (
    aliases.length > MAX_BUILTIN_ALIASES ||
    aliases.some((alias) => !definition.password.usernamePattern.test(alias))
  ) {
    failures.push({
      field: 'builtinAliases',
      message: definition.password.aliasHint,
      rule: 'regex',
    })
  }
  if (failures.length > 0 || !password) {
    throw new errors.E_VALIDATION_ERROR(failures)
  }

  mcp.builtinUsername = username
  mcp.builtinPassword = McpSecretStore.encrypt(password)
  mcp.builtinPermissions = definition.password.permissions
    .filter((name) => requested.includes(name))
    .join(' ')
  mcp.builtinAliases = aliases.length > 0 ? aliases.join(' ') : null
}

function assignBuiltinCredentials(mcp: Mcp, payload: McpPayload) {
  const definition = payload.transport === 'builtin' ? builtinMcp(payload.builtinKey) : null
  if (!definition?.password) {
    mcp.builtinUsername = null
    mcp.builtinPassword = null
    mcp.builtinPermissions = null
    mcp.builtinAliases = null
  }
  if (!definition) {
    return
  }

  if (definition.oauth) {
    assignOauthApplication(mcp, payload, definition)
  } else {
    assignPasswordSignIn(mcp, payload, definition)
  }
}

/**
 * Strava reports the granted scopes as `scope=read,activity:read_all`, and the
 * query parser splits comma-separated values into an array. The value is only
 * a hint that is sanitized before use, so it is read outside the validator and
 * can never fail the callback.
 */
function grantedScopeFromCallback(value: unknown) {
  const scopes = (Array.isArray(value) ? value : [value]).filter(
    (scope): scope is string => typeof scope === 'string'
  )
  return scopes.length > 0 ? scopes.join(' ') : undefined
}

function clearOAuthConnection(mcp: Mcp) {
  mcp.oauthAuthorizeUrl = null
  mcp.oauthTokenUrl = null
  mcp.oauthScopes = null
  mcp.oauthClientId = null
  mcp.oauthClientSecret = null
  mcp.oauthAccessToken = null
  mcp.oauthRefreshToken = null
  mcp.oauthTokenExpiresAt = null
  mcp.oauthIssuer = null
  mcp.oauthResource = null
  mcp.oauthRedirectUri = null
  mcp.oauthClientAuthMethod = null
  mcp.oauthTokenType = null
  mcp.oauthRequired = false
}

function httpOrigin(httpUrl: string | null | undefined) {
  if (!httpUrl) {
    return null
  }
  try {
    return new URL(httpUrl).origin
  } catch {
    return null
  }
}

export async function assignMcpFromPayload(
  mcp: Mcp,
  payload: McpPayload,
  options?: { excludeId?: number }
) {
  const nextHttpUrl = payload.transport === 'http' ? normalizedHttpUrl(payload.httpUrl ?? '') : null
  const nextBuiltinKey = payload.transport === 'builtin' ? (payload.builtinKey ?? null) : null
  const nextNpmPackage = payload.transport === 'npm' ? (payload.npmPackage ?? null) : null
  const oauthServerChanged =
    mcp.transport !== payload.transport ||
    mcp.httpUrl !== nextHttpUrl ||
    (mcp.builtinKey ?? null) !== nextBuiltinKey
  if (oauthServerChanged) {
    clearOAuthConnection(mcp)
  }

  // Saved secrets are write-only and were entered for one destination. They
  // must not follow the MCP to another origin or package, where the probe
  // after saving would hand them over. A new path or version keeps them.
  const credentialTargetChanged =
    mcp.transport !== payload.transport ||
    httpOrigin(mcp.httpUrl) !== httpOrigin(nextHttpUrl) ||
    (mcp.npmPackage ?? null) !== nextNpmPackage
  if (credentialTargetChanged) {
    mcp.authBearer = null
    mcp.authHeaderValue = null
  }

  mcp.name = payload.name
  mcp.slug = await uniqueSlug(payload.name, options?.excludeId)
  mcp.description = payload.description || null
  mcp.transport = payload.transport
  mcp.httpUrl = nextHttpUrl
  mcp.builtinKey = nextBuiltinKey
  mcp.builtinWriteEnabled =
    payload.transport === 'builtin' ? (payload.builtinWriteEnabled ?? false) : false
  mcp.npmPackage = nextNpmPackage
  mcp.npmVersion = payload.transport === 'npm' ? payload.npmVersion || null : null
  mcp.npmArgsList = payload.transport === 'npm' ? (payload.npmArgs ?? []) : []
  assignNpmEnvironment(mcp, payload, !credentialTargetChanged)
  // Built-in MCPs sign in the one way their provider supports.
  mcp.authType = payload.transport === 'builtin' ? 'auto' : payload.authType
  mcp.authHeaderName = mcp.authType === 'header' ? (payload.authHeaderName ?? null) : null
  mcp.enabled = payload.enabled ?? false
  clearUnusedAuthSecrets(mcp)
  applySecrets(mcp, payload)
  assignBuiltinCredentials(mcp, payload)
}

/**
 * Whether saving left something for the admin to do in the dialog: connect an
 * account, or correct a sign-in the provider just rejected.
 */
function needsAttention(mcp: Mcp) {
  return (
    mcp.oauthRequired || (mcp.status === 'error' && Boolean(builtinMcp(mcp.builtinKey)?.password))
  )
}

/**
 * Files left in a sandbox are not worth failing a save or a delete over; a
 * leftover directory is reported for the operator to remove.
 */
async function discardSandbox(mcpId: number) {
  try {
    await removeMcpSandbox(mcpId)
  } catch (error) {
    logger.warn(
      { mcpId, error: sanitizeDiagnostic(error) },
      'Could not delete the sandbox directory of an npm MCP'
    )
  }
}

/** Files agents uploaded for a built-in MCP expire by themselves, so neither are these. */
async function discardUploads(mcpId: number) {
  try {
    await removeBuiltinUploads(mcpId)
  } catch (error) {
    logger.warn(
      { mcpId, error: sanitizeDiagnostic(error) },
      'Could not delete the uploaded files of a built-in MCP'
    )
  }
}

async function uniqueSlug(name: string, excludeId?: number) {
  const base = Mcp.slugify(name)
  let candidate = base
  let i = 2
  while (true) {
    const query = Mcp.query().where('slug', candidate)
    if (excludeId) {
      query.whereNot('id', excludeId)
    }
    const existing = await query.first()
    if (!existing) {
      return candidate
    }
    candidate = `${base}-${i}`
    i++
  }
}

/**
 * The MCP a route names, or null when its `:id` is not an id or matches none.
 */
async function findMcp(params: HttpContext['params']) {
  const [, route] = await recordIdParamsValidator.tryValidate(params)
  return route ? Mcp.find(route.id) : null
}

export default class McpsController {
  async index({ inertia, session }: HttpContext) {
    const mcps = await Mcp.query().orderBy('name', 'asc')
    const [, editingMcpId] = await flashedRecordIdValidator.tryValidate(
      session.flashMessages.get('editingMcpId')
    )
    const appUrl = publicOauthAppUrl()
    return inertia.render('mcps/index', {
      mcps: McpTransformer.transform(mcps),
      editingMcpId,
      // Providers of built-in MCPs ask for these when the admin registers an app.
      publicApp: appUrl ? { url: appUrl, hostname: new URL(appUrl).hostname } : null,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const payload = await request.validateUsing(createMcpValidator)

    const mcp = new Mcp()
    mcp.status = 'draft'
    mcp.createdBy = auth.user!.id
    await assignMcpFromPayload(mcp, payload)
    await mcp.save()

    await testAndUpdateStatus(mcp)
    if (needsAttention(mcp)) {
      session.flash('editingMcpId', mcp.id)
    }
    session.flash('success', 'MCP created')
    return response.redirect().toRoute('mcps.index')
  }

  async show({ params, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }

  async update({ params, request, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    const payload = await request.validateUsing(updateMcpValidator)
    const previousNpmPackage = mcp.transport === 'npm' ? mcp.npmPackage : null
    await assignMcpFromPayload(mcp, payload, { excludeId: mcp.id })
    await mcp.save()
    // Another package must not start in what the previous one left behind.
    if (previousNpmPackage && previousNpmPackage !== mcp.npmPackage) {
      await discardSandbox(mcp.id)
    }

    await testAndUpdateStatus(mcp)
    if (needsAttention(mcp)) {
      session.flash('editingMcpId', mcp.id)
    }
    session.flash('success', 'MCP updated')
    return response.redirect().toRoute('mcps.index')
  }

  async destroy({ params, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    await mcp.delete()
    await discardSandbox(mcp.id)
    await discardUploads(mcp.id)
    session.flash('success', 'MCP deleted')
    return response.redirect().toRoute('mcps.index')
  }

  async probe({ params, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    await testAndUpdateStatus(mcp)
    if (mcp.status === 'ready') {
      session.flash('success', 'Connection OK')
    } else {
      session.flash('error', mcp.lastError || 'Connection failed')
    }
    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }

  async updateNpm({ params, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    try {
      await updateMcpToLatest(mcp)
      if (mcp.status === 'ready') {
        session.flash('success', 'MCP updated to latest')
      } else {
        session.flash('error', mcp.lastError || 'Update failed')
      }
    } catch (error) {
      session.flash(
        'error',
        error instanceof McpNpmUpdateError
          ? error.message
          : (sanitizeMcpDiagnostic(error, mcp) ?? 'Update failed')
      )
    }

    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }

  /**
   * Starting a flow registers a client with the provider and replaces the
   * saved OAuth configuration, on a route that has to stay a GET navigation.
   */
  async oauthStart({ params, request, response, session }: HttpContext) {
    // The router also sends HEAD requests to GET routes.
    if (request.method() !== 'GET') {
      return response.header('Allow', 'GET').methodNotAllowed()
    }
    // Browsers say where a request comes from. Another site must not be able
    // to start, and so reset, an authorization by linking to or embedding this URL.
    const [fromAnotherSite] = await request.tryValidateUsing(oauthStartValidator)
    if (fromAnotherSite) {
      session.flash('error', 'Start the OAuth connection from the MCPs page')
      return response.redirect().withQs(false).toRoute('mcps.index')
    }

    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    const usesOauth =
      mcp.transport === 'builtin'
        ? Boolean(builtinMcp(mcp.builtinKey)?.oauth)
        : mcp.authType === 'auto' && (mcp.oauthRequired || Boolean(mcp.oauthAccessToken))
    if (!usesOauth) {
      session.flash('error', 'This MCP does not require OAuth authorization')
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }

    try {
      // Redirects forward the query string by default, which would append
      // whatever followed this URL to the provider's authorization request.
      return response
        .redirect()
        .withQs(false)
        .toPath(await startOauthFlow(session, mcp))
    } catch (error) {
      session.flash('error', sanitizeMcpDiagnostic(error, mcp) ?? 'Failed to start OAuth')
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }
  }

  async oauthCallback({ request, response, session }: HttpContext) {
    let callback
    try {
      callback = await request.validateUsing(oauthCallbackValidator)
    } catch (error) {
      if (!(error instanceof errors.E_VALIDATION_ERROR)) throw error
      // A validation error would send the browser back without a word.
      session.flash('error', 'Invalid OAuth callback')
      return response.redirect().withQs(false).toRoute('mcps.index')
    }
    const { code, state, error: oauthError } = callback
    const oauth = await readOauthSession(session, state)
    clearOauthSession(session, state)

    if (oauthError) {
      const mcp = oauth ? await Mcp.find(oauth.mcpId) : null
      const message = `OAuth error: ${oauthError}`
      const callbackCredentials = [code, state].filter((value): value is string => Boolean(value))
      session.flash(
        'error',
        mcp
          ? (sanitizeMcpDiagnostic(message, mcp, 500, callbackCredentials) ??
              'OAuth authorization failed')
          : (sanitizeDiagnostic(message, 500, callbackCredentials) ?? 'OAuth authorization failed')
      )
      if (oauth) {
        session.flash('editingMcpId', oauth.mcpId)
      }
      return response.redirect().withQs(false).toRoute('mcps.index')
    }

    if (!oauth || !code || !state || state !== oauth.state) {
      session.flash('error', 'Invalid OAuth callback')
      return response.redirect().withQs(false).toRoute('mcps.index')
    }

    const mcp = await Mcp.find(oauth.mcpId)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().withQs(false).toRoute('mcps.index')
    }

    try {
      await exchangeAuthorizationCode(
        mcp,
        oauth,
        code,
        grantedScopeFromCallback(request.input('scope'))
      )
      await testAndUpdateStatus(mcp)
      if (mcp.status === 'ready') {
        session.flash('success', 'OAuth connected')
      } else {
        session.flash('error', mcp.lastError || 'OAuth connected, but the connection test failed')
      }
    } catch (error) {
      mcp.status = 'error'
      mcp.lastError = sanitizeMcpDiagnostic(error, mcp, 500, [code, state]) ?? 'Unknown error'
      await mcp.save()
      session.flash('error', mcp.lastError)
    }

    session.flash('editingMcpId', mcp.id)
    return response.redirect().withQs(false).toRoute('mcps.index')
  }
}
