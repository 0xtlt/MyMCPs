import type { HttpContext } from '@adonisjs/core/http'
import { errors } from '@vinejs/vine'
import Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcpValidator, updateMcpValidator } from '#validators/mcp'
import { oauthCallbackValidator } from '#validators/oauth'
import { testAndUpdateStatus } from '#services/upstream/manager'
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

function normalizedHttpUrl(value: string) {
  try {
    return parseHttpUrl(value, 'MCP URL').toString()
  } catch (error) {
    throw new errors.E_VALIDATION_ERROR([
      {
        field: 'httpUrl',
        message: error instanceof Error ? error.message : 'MCP URL is invalid',
        rule: 'url',
      },
    ])
  }
}

function assignNpmEnvironment(mcp: Mcp, payload: McpPayload) {
  if (payload.transport !== 'npm') {
    mcp.npmEnv = null
    return
  }

  const entries = payload.npmEnv ?? []
  const seenNames = new Set<string>()

  for (const [index, entry] of entries.entries()) {
    if (seenNames.has(entry.name)) {
      npmEnvValidationError(`npmEnv.${index}.name`, 'Environment variable names must be unique')
    }
    seenNames.add(entry.name)

    if (entry.value === null && !McpEnvironmentStore.hasName(mcp.npmEnv, entry.name)) {
      npmEnvValidationError(
        `npmEnv.${index}.value`,
        'A value is required for a new environment variable'
      )
    }
  }

  const nextValue = McpEnvironmentStore.merge(mcp.npmEnv, entries)
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

/**
 * Save the API application credentials of a built-in MCP. Tokens belong to
 * the application that issued them, so new credentials disconnect the account.
 */
function assignBuiltinCredentials(mcp: Mcp, payload: McpPayload) {
  if (payload.transport !== 'builtin') {
    return
  }

  const definition = builtinMcp(payload.builtinKey)!
  const clientId = payload.oauthClientId ?? ''
  const savedSecret = McpSecretStore.decrypt(mcp.oauthClientSecret)
  const clientSecret = payload.oauthClientSecret || savedSecret

  // Report both fields at once: they are copied from the same provider page.
  const failures: Array<{ field: string; message: string; rule: string }> = []
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

export async function assignMcpFromPayload(
  mcp: Mcp,
  payload: McpPayload,
  options?: { excludeId?: number }
) {
  const nextHttpUrl = payload.transport === 'http' ? normalizedHttpUrl(payload.httpUrl ?? '') : null
  const nextBuiltinKey = payload.transport === 'builtin' ? (payload.builtinKey ?? null) : null
  const oauthServerChanged =
    mcp.transport !== payload.transport ||
    mcp.httpUrl !== nextHttpUrl ||
    (mcp.builtinKey ?? null) !== nextBuiltinKey
  if (oauthServerChanged) {
    clearOAuthConnection(mcp)
  }

  mcp.name = payload.name
  mcp.slug = await uniqueSlug(payload.name, options?.excludeId)
  mcp.description = payload.description || null
  mcp.transport = payload.transport
  mcp.httpUrl = nextHttpUrl
  mcp.builtinKey = nextBuiltinKey
  mcp.builtinWriteEnabled =
    payload.transport === 'builtin' ? (payload.builtinWriteEnabled ?? false) : false
  mcp.npmPackage = payload.transport === 'npm' ? (payload.npmPackage ?? null) : null
  mcp.npmVersion = payload.transport === 'npm' ? payload.npmVersion || null : null
  mcp.npmArgsList = payload.transport === 'npm' ? (payload.npmArgs ?? []) : []
  assignNpmEnvironment(mcp, payload)
  // Built-in MCPs always authorize through their provider's OAuth.
  mcp.authType = payload.transport === 'builtin' ? 'auto' : payload.authType
  mcp.authHeaderName = mcp.authType === 'header' ? (payload.authHeaderName ?? null) : null
  mcp.enabled = payload.enabled ?? false
  clearUnusedAuthSecrets(mcp)
  applySecrets(mcp, payload)
  assignBuiltinCredentials(mcp, payload)
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

export default class McpsController {
  async index({ inertia, session }: HttpContext) {
    const mcps = await Mcp.query().orderBy('name', 'asc')
    const editingMcpIdRaw = session.flashMessages.get('editingMcpId')
    const editingMcpId =
      typeof editingMcpIdRaw === 'number' && Number.isFinite(editingMcpIdRaw)
        ? editingMcpIdRaw
        : null
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
    if (mcp.oauthRequired) {
      session.flash('editingMcpId', mcp.id)
    }
    session.flash('success', 'MCP created')
    return response.redirect().toRoute('mcps.index')
  }

  async show({ params, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }

  async update({ params, request, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    const payload = await request.validateUsing(updateMcpValidator)
    await assignMcpFromPayload(mcp, payload, { excludeId: mcp.id })
    await mcp.save()

    await testAndUpdateStatus(mcp)
    if (mcp.oauthRequired) {
      session.flash('editingMcpId', mcp.id)
    }
    session.flash('success', 'MCP updated')
    return response.redirect().toRoute('mcps.index')
  }

  async destroy({ params, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    await mcp.delete()
    session.flash('success', 'MCP deleted')
    return response.redirect().toRoute('mcps.index')
  }

  async probe({ params, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
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
    const mcp = await Mcp.find(params.id)
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

  async oauthStart({ params, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    const usesOauth =
      mcp.transport === 'builtin' ||
      (mcp.authType === 'auto' && (mcp.oauthRequired || Boolean(mcp.oauthAccessToken)))
    if (!usesOauth) {
      session.flash('error', 'This MCP does not require OAuth authorization')
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }

    try {
      return response.redirect(await startOauthFlow(session, mcp))
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
