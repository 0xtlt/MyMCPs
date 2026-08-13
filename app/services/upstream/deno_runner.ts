import { execFile } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { existsSync, readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { promisify } from 'node:util'
import app from '@adonisjs/core/services/app'
import env from '#start/env'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import type Mcp from '#models/mcp'
import { applicationVersion } from '#services/application_version'
import { sanitizeMcpDiagnostic } from '#services/security_redaction'
import type { UpstreamTool } from '#services/upstream/http_client'

export type ConnectedDenoUpstream = {
  client: Client
  transport: StdioClientTransport
  close: () => Promise<void>
}

const execFileAsync = promisify(execFile)
const DENO_CACHE_RELOAD_TIMEOUT_MS = 120_000
/**
 * The host app has a package.json, so Deno would otherwise default to
 * `nodeModules: "manual"` and refuse `npm:` specifier entrypoints.
 * `none` keeps packages in `$DENO_DIR` instead of creating a local node_modules.
 */
const DENO_NODE_MODULES_DIR = '--node-modules-dir=none'

export function resolveDenoBinary() {
  const configured = env.get('DENO_PATH', '')
  if (configured && existsSync(configured)) {
    return configured
  }
  const candidates = ['deno', '/usr/local/bin/deno', '/home/ubuntu/.deno/bin/deno']
  for (const candidate of candidates) {
    if (candidate === 'deno') {
      return candidate
    }
    if (existsSync(candidate)) {
      return candidate
    }
  }
  return 'deno'
}

/**
 * Directory Deno uses for the npm package cache (`$DENO_DIR/npm/...`).
 * Docker sets `DENO_DIR=/app/tmp/deno-cache`; local installs follow Deno defaults.
 */
export function resolveDenoDir() {
  const configured = process.env.DENO_DIR?.trim()
  if (configured) {
    return configured
  }
  if (process.platform === 'darwin') {
    return join(homedir(), 'Library', 'Caches', 'deno')
  }
  if (process.platform === 'win32') {
    return join(process.env.LOCALAPPDATA || homedir(), 'deno')
  }
  const xdgCache = process.env.XDG_CACHE_HOME?.trim()
  return join(xdgCache || join(homedir(), '.cache'), 'deno')
}

function isSafePathSegment(value: string) {
  return Boolean(value) && !value.includes('..') && !value.includes('/') && !value.includes('\\')
}

function npmCachePackageDir(npmPackage: string) {
  const pkg = npmPackage.trim()
  if (!pkg || pkg.includes('..') || pkg.includes('\\') || pkg.startsWith('/')) {
    return null
  }
  const segments = pkg.split('/').filter(Boolean)
  if (segments.length === 0 || segments.length > 2 || !segments.every(isSafePathSegment)) {
    return null
  }
  return join(resolveDenoDir(), 'npm', 'registry.npmjs.org', ...segments)
}

function isLatestRequestedVersion(npmVersion: string | null | undefined) {
  const version = npmVersion?.trim()
  return !version || version.toLowerCase() === 'latest'
}

function readCachedLatestTag(packageDir: string) {
  const registryPath = join(packageDir, 'registry.json')
  if (!existsSync(registryPath)) {
    return null
  }
  try {
    const parsed = JSON.parse(readFileSync(registryPath, 'utf8')) as {
      'dist-tags'?: { latest?: string }
    }
    const latest = parsed['dist-tags']?.latest?.trim()
    return latest && isSafePathSegment(latest) ? latest : null
  } catch {
    return null
  }
}

/**
 * Semver currently present in the Deno npm cache for this package.
 * For `latest`, uses the cached `dist-tags.latest` when that version folder exists.
 * Pinned versions are returned only when that exact folder is cached.
 */
export function readCachedNpmPackageVersion(
  npmPackage: string | null | undefined,
  npmVersion: string | null | undefined
) {
  if (!npmPackage?.trim()) {
    return null
  }
  const packageDir = npmCachePackageDir(npmPackage)
  if (!packageDir || !existsSync(packageDir)) {
    return null
  }

  const requested = isLatestRequestedVersion(npmVersion)
    ? readCachedLatestTag(packageDir)
    : npmVersion!.trim()
  if (!requested || !isSafePathSegment(requested)) {
    return null
  }
  return existsSync(join(packageDir, requested)) ? requested : null
}

export function sandboxRootFor(mcpId: number) {
  return join(app.tmpPath('mcp-sandboxes'), String(mcpId))
}

/**
 * Build Deno permission flags for an npm MCP subprocess.
 *
 * Filesystem is deny-by-default outside `sandboxDir` and the Deno npm cache
 * (no Adonis DB / `.env` / app source). The cache must be readable because
 * Node packages often `readFileSync` their own packaged assets (for example
 * `@shopify/dev-mcp`). Network and env remain allowed because many MCP packages
 * need outbound HTTP and process env.
 * `homedir` sys access is required by Node packages that call `os.homedir()` at import time
 * (for example `@shopify/dev-mcp` via `env-paths`); HOME/TMPDIR still point at `sandboxDir`.
 * Treat upstream packages as trusted software, not a full multi-tenant isolation boundary.
 */
export function buildDenoArgs(mcp: Mcp, sandboxDir: string) {
  if (!mcp.npmPackage) {
    throw new Error('npm MCP is missing a package name')
  }

  const version = mcp.npmVersion?.trim() || 'latest'
  const npmSpec = `npm:${mcp.npmPackage}@${version}`
  const extraArgs = mcp.npmArgsList
  const denoDir = resolveDenoDir()

  return [
    'run',
    '--quiet',
    DENO_NODE_MODULES_DIR,
    `--allow-read=${sandboxDir},${denoDir}`,
    `--allow-write=${sandboxDir}`,
    '--allow-net',
    '--allow-env',
    '--allow-sys=homedir',
    '--no-prompt',
    npmSpec,
    ...extraArgs,
  ]
}

export function buildDenoEnvironment(mcp: Mcp, sandboxDir: string) {
  return {
    ...mcp.npmEnvironment,
    HOME: sandboxDir,
    TMPDIR: sandboxDir,
    NO_COLOR: '1',
  }
}

function execFileDetail(error: unknown) {
  if (error && typeof error === 'object') {
    const err = error as { stderr?: string; message?: string }
    const detail = (err.stderr || err.message || 'Unknown error').trim()
    return detail.slice(0, 300) || 'Unknown error'
  }
  return 'Unknown error'
}

/**
 * `deno cache --reload` args for an npm package at `@latest`.
 * `--node-modules-dir=none` is required when the process cwd is this Node app.
 */
export function buildDenoCacheReloadArgs(npmPackage: string) {
  const pkg = npmPackage.trim()
  if (!pkg) {
    throw new Error('npm MCP is missing a package name')
  }

  return ['cache', '--reload', '--quiet', DENO_NODE_MODULES_DIR, `npm:${pkg}@latest`]
}

/**
 * Reload the Deno npm cache for a package at `@latest` without changing any MCP row.
 */
export async function reloadDenoNpmPackageCache(npmPackage: string) {
  const pkg = npmPackage.trim()
  if (!pkg) {
    throw new Error('npm MCP is missing a package name')
  }

  const deno = resolveDenoBinary()

  try {
    await execFileAsync(deno, buildDenoCacheReloadArgs(pkg), {
      timeout: DENO_CACHE_RELOAD_TIMEOUT_MS,
      maxBuffer: 1024 * 1024,
      encoding: 'utf8',
    })
  } catch (error) {
    throw new Error(`Failed to reload Deno cache for "${pkg}". ${execFileDetail(error)}`)
  }
}

export function createDenoStartupError(mcp: Mcp, error: unknown) {
  const detail = sanitizeMcpDiagnostic(error, mcp, 300) ?? 'Unknown error'
  return new Error(`Failed to start Deno npm MCP "${mcp.npmPackage}". Is Deno installed? ${detail}`)
}

export async function connectDenoUpstream(mcp: Mcp): Promise<ConnectedDenoUpstream> {
  const sandboxDir = sandboxRootFor(mcp.id)
  await mkdir(sandboxDir, { recursive: true })

  const deno = resolveDenoBinary()
  const args = buildDenoArgs(mcp, sandboxDir)
  const transport = new StdioClientTransport({
    command: deno,
    args,
    cwd: sandboxDir,
    stderr: 'pipe',
    env: buildDenoEnvironment(mcp, sandboxDir),
  })

  const client = new Client({ name: 'mymcps-gateway', version: applicationVersion })

  try {
    await client.connect(transport)
  } catch (error) {
    throw createDenoStartupError(mcp, error)
  }

  return {
    client,
    transport,
    close: async () => {
      try {
        await client.close()
      } catch {
        // Connection teardown is best-effort.
      }
      try {
        await transport.close()
      } catch {
        // Connection teardown is best-effort.
      }
    },
  }
}

export async function listDenoTools(mcp: Mcp): Promise<UpstreamTool[]> {
  const connected = await connectDenoUpstream(mcp)
  try {
    const result = await connected.client.listTools()
    return (result.tools ?? []).map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: tool.inputSchema,
    }))
  } finally {
    await connected.close()
  }
}
