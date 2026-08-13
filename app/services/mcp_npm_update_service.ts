import logger from '@adonisjs/core/services/logger'
import type Mcp from '#models/mcp'
import { sanitizeMcpDiagnostic } from '#services/security_redaction'
import { reloadDenoNpmPackageCache } from '#services/upstream/deno_runner'
import { testAndUpdateStatus } from '#services/upstream/manager'

export class McpNpmUpdateError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'McpNpmUpdateError'
  }
}

export type McpNpmUpdateRunResult = {
  updated: number
  skipped: number
  failed: Array<{ id: number; slug: string; error: string }>
}

type McpLatestTracker = Pick<Mcp, 'transport' | 'npmVersion'>

async function defaultReload(mcp: Mcp) {
  await reloadDenoNpmPackageCache(mcp.npmPackage ?? '')
}

export const mcpNpmUpdateRuntime = {
  reload: defaultReload,
  probe: testAndUpdateStatus,
}

export function resetMcpNpmUpdateRuntime() {
  mcpNpmUpdateRuntime.reload = defaultReload
  mcpNpmUpdateRuntime.probe = testAndUpdateStatus
}

export function isLatestNpmVersion(npmVersion: string | null | undefined) {
  const version = npmVersion?.trim()
  return !version || version.toLowerCase() === 'latest'
}

export function isTrackingLatest(mcp: McpLatestTracker) {
  return mcp.transport === 'npm' && isLatestNpmVersion(mcp.npmVersion)
}

export async function updateMcpToLatest(mcp: Mcp) {
  if (mcp.transport !== 'npm') {
    throw new McpNpmUpdateError('Only Deno npm MCPs can be updated')
  }
  if (!isTrackingLatest(mcp)) {
    throw new McpNpmUpdateError('Pinned npm versions are not updated')
  }
  if (!mcp.npmPackage?.trim()) {
    throw new McpNpmUpdateError('npm MCP is missing a package name')
  }

  await mcpNpmUpdateRuntime.reload(mcp)
  return mcpNpmUpdateRuntime.probe(mcp)
}

let inFlight: Promise<McpNpmUpdateRunResult> | null = null

async function runLatestTrackingUpdates(): Promise<McpNpmUpdateRunResult> {
  const { default: McpModel } = await import('#models/mcp')
  const mcps = await McpModel.query().where('transport', 'npm').orderBy('id', 'asc')
  const result: McpNpmUpdateRunResult = { updated: 0, skipped: 0, failed: [] }

  for (const mcp of mcps) {
    if (!isTrackingLatest(mcp)) {
      result.skipped += 1
      continue
    }

    try {
      await updateMcpToLatest(mcp)
      result.updated += 1
    } catch (error) {
      const message =
        error instanceof McpNpmUpdateError
          ? error.message
          : (sanitizeMcpDiagnostic(error, mcp) ?? 'Unknown error')
      logger.warn(
        { mcpId: mcp.id, slug: mcp.slug, error: message },
        'Failed to update latest-tracking npm MCP'
      )
      result.failed.push({ id: mcp.id, slug: mcp.slug, error: message })
    }
  }

  return result
}

/**
 * Reload Deno caches for npm MCPs that already track `latest`. Pinned versions are skipped.
 */
export function updateLatestTrackingMcps() {
  if (inFlight) {
    return inFlight
  }

  inFlight = runLatestTrackingUpdates().finally(() => {
    inFlight = null
  })
  return inFlight
}
