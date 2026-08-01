import type { HttpContext } from '@adonisjs/core/http'
import Mcp from '#models/mcp'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcpValidator, updateMcpValidator } from '#validators/mcp'
import { testAndUpdateStatus } from '#services/upstream/manager'
import {
  buildAuthorizeRedirect,
  clearOauthSession,
  exchangeAuthorizationCode,
  readOauthSession,
  startOauthSession,
} from '#services/upstream/oauth'
import vine from '@vinejs/vine'
import { sanitizeErrorMessage } from '#services/error_message'
import McpTransformer from '#transformers/mcp_transformer'
import type { Infer } from '@vinejs/vine/types'

/**
 * Flash values are untyped; only accept finite numbers or numeric strings (session may stringify).
 */
function flashNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value
  }
  if (typeof value === 'string' && value.trim() !== '' && vine.helpers.isNumeric(value)) {
    const parsed = vine.helpers.asNumber(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

type McpPayload = Infer<typeof createMcpValidator>

function parseNpmArgs(raw: string | undefined): string[] {
  if (!raw?.trim()) {
    return []
  }
  return raw
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean)
}

function applySecrets(mcp: Mcp, payload: McpPayload) {
  if (payload.authBearer && payload.authBearer.length > 0) {
    mcp.authBearer = McpSecretStore.encrypt(payload.authBearer)
  }
  if (payload.authHeaderValue && payload.authHeaderValue.length > 0) {
    mcp.authHeaderValue = McpSecretStore.encrypt(payload.authHeaderValue)
  }
  if (payload.oauthClientSecret && payload.oauthClientSecret.length > 0) {
    mcp.oauthClientSecret = McpSecretStore.encrypt(payload.oauthClientSecret)
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
  if (mcp.authType !== 'oauth') {
    mcp.oauthAuthorizeUrl = null
    mcp.oauthTokenUrl = null
    mcp.oauthScopes = null
    mcp.oauthClientId = null
    mcp.oauthClientSecret = null
    mcp.oauthAccessToken = null
    mcp.oauthRefreshToken = null
    mcp.oauthTokenExpiresAt = null
  }
}

async function assignMcpFromPayload(mcp: Mcp, payload: McpPayload, options?: { excludeId?: number }) {
  mcp.name = payload.name
  mcp.slug = await uniqueSlug(payload.name, options?.excludeId)
  mcp.description = payload.description || null
  mcp.transport = payload.transport
  mcp.httpUrl = payload.transport === 'http' ? (payload.httpUrl ?? null) : null
  mcp.npmPackage = payload.transport === 'npm' ? (payload.npmPackage ?? null) : null
  mcp.npmVersion = payload.transport === 'npm' ? (payload.npmVersion || null) : null
  mcp.setNpmArgsList(payload.transport === 'npm' ? parseNpmArgs(payload.npmArgs) : [])
  mcp.authType = payload.authType
  mcp.authHeaderName = payload.authType === 'header' ? (payload.authHeaderName ?? null) : null
  mcp.oauthAuthorizeUrl =
    payload.authType === 'oauth' ? (payload.oauthAuthorizeUrl ?? null) : null
  mcp.oauthTokenUrl = payload.authType === 'oauth' ? (payload.oauthTokenUrl ?? null) : null
  mcp.oauthScopes = payload.authType === 'oauth' ? (payload.oauthScopes || null) : null
  mcp.oauthClientId = payload.authType === 'oauth' ? (payload.oauthClientId ?? null) : null
  mcp.enabled = payload.enabled === 'on'
  clearUnusedAuthSecrets(mcp)
  applySecrets(mcp, payload)
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
    const editingMcpId = flashNumber(session.flashMessages.get('editingMcpId'))
    return inertia.render('mcps/index', {
      mcps: McpTransformer.transform(mcps),
      editingMcpId,
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

  async oauthStart({ params, response, session }: HttpContext) {
    const mcp = await Mcp.find(params.id)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }
    if (mcp.authType !== 'oauth') {
      session.flash('error', 'This MCP is not configured for OAuth')
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }

    try {
      const oauth = startOauthSession(session, mcp)
      return response.redirect(buildAuthorizeRedirect(mcp, oauth))
    } catch (error) {
      session.flash('error', sanitizeErrorMessage(error))
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }
  }

  async oauthCallback({ request, response, session }: HttpContext) {
    const oauth = await readOauthSession(session)
    clearOauthSession(session)

    const codeRaw = request.input('code')
    const stateRaw = request.input('state')
    const oauthErrorRaw = request.input('error')
    const code = typeof codeRaw === 'string' ? codeRaw : undefined
    const state = typeof stateRaw === 'string' ? stateRaw : undefined
    const oauthError = typeof oauthErrorRaw === 'string' ? oauthErrorRaw : undefined

    if (oauthError) {
      session.flash('error', `OAuth error: ${oauthError}`)
      return response.redirect().toRoute('mcps.index')
    }

    if (!oauth || !code || !state || state !== oauth.state) {
      session.flash('error', 'Invalid OAuth callback')
      return response.redirect().toRoute('mcps.index')
    }

    const mcp = await Mcp.find(oauth.mcpId)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    try {
      await exchangeAuthorizationCode(mcp, code, oauth.codeVerifier)
      await testAndUpdateStatus(mcp)
      session.flash('success', 'OAuth connected')
    } catch (error) {
      mcp.status = 'error'
      mcp.lastError = sanitizeErrorMessage(error)
      await mcp.save()
      session.flash('error', mcp.lastError)
    }

    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }
}
