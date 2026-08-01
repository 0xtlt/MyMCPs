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

function parseNpmArgs(raw: string | undefined) {
  if (!raw?.trim()) {
    return [] as string[]
  }
  return raw
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean)
}

function applySecrets(mcp: Mcp, payload: Record<string, unknown>) {
  if (typeof payload.authBearer === 'string' && payload.authBearer.length > 0) {
    mcp.authBearer = McpSecretStore.encrypt(payload.authBearer)
  }
  if (typeof payload.authHeaderValue === 'string' && payload.authHeaderValue.length > 0) {
    mcp.authHeaderValue = McpSecretStore.encrypt(payload.authHeaderValue)
  }
  if (typeof payload.oauthClientSecret === 'string' && payload.oauthClientSecret.length > 0) {
    mcp.oauthClientSecret = McpSecretStore.encrypt(payload.oauthClientSecret)
  }
}

function serializeMcp(mcp: Mcp) {
  return {
    id: mcp.id,
    name: mcp.name,
    slug: mcp.slug,
    description: mcp.description,
    transport: mcp.transport,
    httpUrl: mcp.httpUrl,
    npmPackage: mcp.npmPackage,
    npmVersion: mcp.npmVersion,
    npmArgs: mcp.npmArgsList.join(' '),
    authType: mcp.authType,
    authHeaderName: mcp.authHeaderName,
    hasAuthBearer: McpSecretStore.hasSecret(mcp.authBearer),
    hasAuthHeaderValue: McpSecretStore.hasSecret(mcp.authHeaderValue),
    oauthAuthorizeUrl: mcp.oauthAuthorizeUrl,
    oauthTokenUrl: mcp.oauthTokenUrl,
    oauthScopes: mcp.oauthScopes,
    oauthClientId: mcp.oauthClientId,
    hasOauthClientSecret: McpSecretStore.hasSecret(mcp.oauthClientSecret),
    hasOauthAccessToken: McpSecretStore.hasSecret(mcp.oauthAccessToken),
    status: mcp.status,
    lastError: mcp.lastError,
    enabled: mcp.enabled,
    createdAt: mcp.createdAt.toISO(),
    updatedAt: mcp.updatedAt?.toISO() ?? null,
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

function validateTransportFields(payload: {
  transport: string
  httpUrl?: string
  npmPackage?: string
  authType: string
  authHeaderName?: string
  oauthAuthorizeUrl?: string
  oauthTokenUrl?: string
  oauthClientId?: string
}) {
  if (payload.transport === 'http' && !payload.httpUrl) {
    return 'HTTP URL is required for HTTP transport'
  }
  if (payload.transport === 'npm' && !payload.npmPackage) {
    return 'npm package is required for npm transport'
  }
  if (payload.authType === 'header' && !payload.authHeaderName) {
    return 'Header name is required for header auth'
  }
  if (payload.authType === 'oauth') {
    if (!payload.oauthAuthorizeUrl || !payload.oauthTokenUrl || !payload.oauthClientId) {
      return 'OAuth authorize URL, token URL, and client ID are required'
    }
  }
  return null
}

export default class McpsController {
  async index({ inertia, session }: HttpContext) {
    const mcps = await Mcp.query().orderBy('name', 'asc')
    const editingMcpId = session.flashMessages.get('editingMcpId') as number | undefined
    return inertia.render('mcps/index', {
      mcps: mcps.map(serializeMcp),
      editingMcpId: editingMcpId ?? null,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const payload = await request.validateUsing(createMcpValidator)
    const error = validateTransportFields(payload)
    if (error) {
      session.flash('error', error)
      return response.redirect().toRoute('mcps.index')
    }

    const mcp = new Mcp()
    mcp.name = payload.name
    mcp.slug = await uniqueSlug(payload.name)
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
    mcp.status = 'draft'
    mcp.createdBy = auth.user!.id
    applySecrets(mcp, payload)
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
    const error = validateTransportFields(payload)
    if (error) {
      session.flash('error', error)
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }

    mcp.name = payload.name
    mcp.slug = await uniqueSlug(payload.name, mcp.id)
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
    applySecrets(mcp, payload)
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
      session.flash('error', error instanceof Error ? error.message : 'OAuth start failed')
      session.flash('editingMcpId', mcp.id)
      return response.redirect().toRoute('mcps.index')
    }
  }

  async oauthCallback({ request, response, session }: HttpContext) {
    const oauth = readOauthSession(session)
    clearOauthSession(session)

    const code = request.input('code') as string | undefined
    const state = request.input('state') as string | undefined
    const oauthError = request.input('error') as string | undefined

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
      mcp.lastError = error instanceof Error ? error.message : String(error)
      await mcp.save()
      session.flash('error', mcp.lastError)
    }

    session.flash('editingMcpId', mcp.id)
    return response.redirect().toRoute('mcps.index')
  }
}
