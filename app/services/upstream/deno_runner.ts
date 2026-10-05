import { execFile } from 'node:child_process'
import { mkdir, rm } from 'node:fs/promises'
import { delimiter, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { accessSync, constants, existsSync, readFileSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import type { Readable } from 'node:stream'
import { promisify, stripVTControlCharacters } from 'node:util'
import app from '@adonisjs/core/services/app'
import env from '#start/env'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import {
  getDefaultEnvironment,
  StdioClientTransport,
} from '@modelcontextprotocol/sdk/client/stdio.js'
import type Mcp from '#models/mcp'
import { applicationVersion } from '#services/application_version'
import { isReservedEnvironmentName } from '#services/mcp_environment_policy'
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
const DENO_NO_LOCK = '--no-lock'

const DENO_BINARY_NAME = process.platform === 'win32' ? 'deno.exe' : 'deno'
/** Usual install locations, tried after the server's PATH. */
const DENO_KNOWN_LOCATIONS = [
  '/usr/local/bin/deno',
  '/opt/homebrew/bin/deno',
  join(homedir(), '.deno', 'bin', DENO_BINARY_NAME),
  '/home/ubuntu/.deno/bin/deno',
]

/** stderr kept while an npm MCP starts, to explain a failed start. */
const STARTUP_STDERR_LIMIT_BYTES = 32 * 1024
const STARTUP_STDERR_TAIL_CHARS = 300
/** How long a failed start waits for the last stderr chunks of an exited child. */
const STARTUP_STDERR_SETTLE_MS = 250

function isExecutableFile(path: string) {
  try {
    accessSync(path, constants.X_OK)
    return statSync(path).isFile()
  } catch {
    return false
  }
}

/**
 * Absolute path of the Deno binary: `configured` (DENO_PATH) when it points at
 * one, else the first `deno` on `searchPath`, else a usual install location.
 *
 * A bare `deno` must never reach spawn. Node looks a bare command up in the
 * PATH of the environment it gives the child, and that environment carries
 * the variables operators set on the MCP.
 */
export function locateDenoBinary(
  configured: string | undefined,
  searchPath: string | undefined,
  knownLocations: readonly string[] = DENO_KNOWN_LOCATIONS
) {
  const candidates = [
    ...(configured ? [resolve(configured)] : []),
    // A relative PATH entry would be resolved against the working directory.
    ...(searchPath ?? '')
      .split(delimiter)
      .filter((directory) => isAbsolute(directory))
      .map((directory) => join(directory, DENO_BINARY_NAME)),
    ...knownLocations,
  ]
  const binary = candidates.find(isExecutableFile)
  if (!binary) {
    throw new Error(
      'Deno was not found. Install Deno, or set DENO_PATH to the absolute path of the deno binary.'
    )
  }
  return binary
}

let locatedDenoBinary: string | undefined

/** Looked up once on the server's own PATH; a failed lookup is retried. */
export function resolveDenoBinary() {
  locatedDenoBinary ??= locateDenoBinary(env.get('DENO_PATH', ''), process.env.PATH)
  return locatedDenoBinary
}

/** Seam for tests, which cannot rely on Deno being installed. */
export const denoRuntime = {
  binary: resolveDenoBinary,
}

export function resetDenoRuntime() {
  denoRuntime.binary = resolveDenoBinary
}

/**
 * The one Deno cache (`$DENO_DIR`, packages under `npm/...`) of every npm MCP.
 * It is handed to each MCP process and to cache reloads explicitly, so what
 * runs, what Update MCP refreshes and the cached version shown in the UI are
 * the same files. Packages may read it but not write it.
 * Docker sets `DENO_DIR=/app/tmp/deno-cache`; other installs use the directory
 * Deno itself would pick for the server's user.
 */
export function resolveDenoDir() {
  const configured = process.env.DENO_DIR?.trim()
  if (configured) {
    return resolve(configured)
  }
  if (process.platform === 'win32') {
    return join(process.env.LOCALAPPDATA || homedir(), 'deno')
  }
  // Deno honours XDG_CACHE_HOME on macOS as well.
  const xdgCache = process.env.XDG_CACHE_HOME?.trim()
  if (xdgCache) {
    return join(resolve(xdgCache), 'deno')
  }
  return process.platform === 'darwin'
    ? join(homedir(), 'Library', 'Caches', 'deno')
    : join(homedir(), '.cache', 'deno')
}

/** Whether `path` is `directory` or lies below it. */
function isWithin(path: string, directory: string) {
  const fromDirectory = relative(directory, path)
  return (
    fromDirectory !== '..' && !fromDirectory.startsWith(`..${sep}`) && !isAbsolute(fromDirectory)
  )
}

/**
 * Packages read the whole Deno cache directory and write their sandbox. A
 * cache directory holding the app would expose the database and `.env`, and
 * one inside a sandbox could be rewritten by the package it serves.
 */
function sandboxSafeDenoDir() {
  const denoDir = resolveDenoDir()
  if (isWithin(app.makePath(), denoDir) || isWithin(denoDir, sandboxesRoot())) {
    throw new Error(
      `The Deno cache directory "${denoDir}" must not contain the application or lie inside an MCP sandbox. Set DENO_DIR to a dedicated directory.`
    )
  }
  return denoDir
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

function sandboxesRoot() {
  return app.tmpPath('mcp-sandboxes')
}

export function sandboxRootFor(mcpId: number) {
  return join(sandboxesRoot(), String(mcpId))
}

/**
 * Delete what an npm MCP wrote to its sandbox. Called when the MCP is deleted
 * or runs another package, so nothing is inherited by different code or by a
 * later MCP that reuses the id.
 */
export async function removeMcpSandbox(mcpId: number) {
  if (!Number.isSafeInteger(mcpId) || mcpId <= 0) {
    return
  }
  await rm(sandboxRootFor(mcpId), { recursive: true, force: true })
}

/**
 * Until the cache directory was passed explicitly, Deno derived it from the
 * child's HOME and cached each package inside its own sandbox, where the
 * package could rewrite its code. Those copies are no longer read.
 */
async function removeLegacySandboxCaches(sandboxDir: string) {
  await Promise.all(
    [join(sandboxDir, '.cache', 'deno'), join(sandboxDir, 'Library', 'Caches', 'deno')].map(
      (directory) => rm(directory, { recursive: true, force: true }).catch(() => undefined)
    )
  )
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
  const denoDir = sandboxSafeDenoDir()

  return [
    'run',
    '--quiet',
    DENO_NODE_MODULES_DIR,
    DENO_NO_LOCK,
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

/**
 * Environment of the `deno` process. The variables of the MCP reach Deno and
 * its loader, not only the package, so reserved names are dropped here again:
 * rows saved before the validator refused them may still hold some.
 */
export function buildDenoEnvironment(mcp: Mcp, sandboxDir: string): Record<string, string> {
  const environment: Record<string, string> = {}
  for (const [name, value] of Object.entries(mcp.npmEnvironment)) {
    if (!isReservedEnvironmentName(name)) {
      environment[name] = value
    }
  }

  return {
    ...environment,
    // Last, so nothing stored can replace what the sandbox depends on.
    PATH: process.env.PATH ?? '',
    HOME: sandboxDir,
    TMPDIR: sandboxDir,
    DENO_DIR: sandboxSafeDenoDir(),
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
 * `--node-modules-dir=none` and `--no-lock` keep Deno from adopting or writing
 * project files next to a package.json in the working directory.
 */
export function buildDenoCacheReloadArgs(npmPackage: string) {
  const pkg = npmPackage.trim()
  if (!pkg) {
    throw new Error('npm MCP is missing a package name')
  }

  return ['cache', '--reload', '--quiet', DENO_NODE_MODULES_DIR, DENO_NO_LOCK, `npm:${pkg}@latest`]
}

/**
 * Reload the Deno npm cache for an MCP's package at `@latest` without changing
 * the MCP row. It runs in the directory and environment the MCP itself starts
 * with, so it refreshes the cache and the registry that process reads.
 */
export async function reloadDenoNpmPackageCache(mcp: Mcp) {
  const pkg = mcp.npmPackage?.trim()
  if (!pkg) {
    throw new Error('npm MCP is missing a package name')
  }

  const deno = denoRuntime.binary()
  const sandboxDir = sandboxRootFor(mcp.id)
  await mkdir(sandboxDir, { recursive: true })
  const environment = { ...getDefaultEnvironment(), ...buildDenoEnvironment(mcp, sandboxDir) }

  try {
    await execFileAsync(deno, buildDenoCacheReloadArgs(pkg), {
      cwd: sandboxDir,
      env: environment,
      timeout: DENO_CACHE_RELOAD_TIMEOUT_MS,
      maxBuffer: 1024 * 1024,
      encoding: 'utf8',
    })
  } catch (error) {
    throw new Error(`Failed to reload Deno cache for "${pkg}". ${execFileDetail(error)}`)
  }
}

/**
 * Read a child's stderr so it never blocks on a full pipe, keeping the output
 * of its start. Output past the limit is dropped and marks the capture as
 * incomplete: a cut can split a secret, which could then not be redacted.
 */
function captureStartupStderr(stream: Readable | null) {
  let chunks: Buffer[] = []
  let size = 0
  let complete = true
  let capturing = true

  stream?.on('data', (chunk: Buffer) => {
    if (!capturing) return
    size += chunk.length
    if (size > STARTUP_STDERR_LIMIT_BYTES) {
      complete = false
      capturing = false
      chunks = []
      return
    }
    chunks.push(chunk)
  })

  return {
    /** The start succeeded: keep draining, stop keeping. */
    discard() {
      capturing = false
      chunks = []
    },
    /** What the child wrote, once it has exited or after a short wait. */
    async read() {
      if (stream && !stream.readableEnded) {
        await new Promise<void>((done) => {
          const timer = setTimeout(done, STARTUP_STDERR_SETTLE_MS)
          stream.once('end', () => {
            clearTimeout(timer)
            done()
          })
        })
      }
      return { text: Buffer.concat(chunks).toString('utf8'), complete }
    },
  }
}

type StartupStderr = { text: string; complete: boolean }

function startupOutput(mcp: Mcp, stderr: StartupStderr | undefined) {
  if (!stderr) {
    return null
  }
  if (!stderr.complete) {
    return `Output exceeded ${STARTUP_STDERR_LIMIT_BYTES / 1024} KiB and is not shown.`
  }
  // Redact first, on the output as written: joining lines or keeping only the
  // end would otherwise break up a secret before it is recognized.
  const redacted = sanitizeMcpDiagnostic(stderr.text, mcp, stderr.text.length) ?? ''
  const text = stripVTControlCharacters(redacted).replace(/\s+/g, ' ').trim()
  if (!text) {
    return null
  }
  return text.length > STARTUP_STDERR_TAIL_CHARS
    ? `Output: …${text.slice(-STARTUP_STDERR_TAIL_CHARS)}`
    : `Output: ${text}`
}

export function createDenoStartupError(mcp: Mcp, error: unknown, stderr?: StartupStderr) {
  const output = startupOutput(mcp, stderr)
  if (!output) {
    const detail = sanitizeMcpDiagnostic(error, mcp, 300) ?? 'Unknown error'
    return new Error(
      `Failed to start Deno npm MCP "${mcp.npmPackage}". Is Deno installed? ${detail}`
    )
  }

  // Deno ran and said why, so its output gets the room.
  const detail = sanitizeMcpDiagnostic(error, mcp, 80) ?? 'Unknown error'
  return new Error(`Failed to start Deno npm MCP "${mcp.npmPackage}". ${detail}. ${output}`)
}

export async function connectDenoUpstream(mcp: Mcp): Promise<ConnectedDenoUpstream> {
  const sandboxDir = sandboxRootFor(mcp.id)
  await mkdir(sandboxDir, { recursive: true })
  await removeLegacySandboxCaches(sandboxDir)

  const deno = denoRuntime.binary()
  const args = buildDenoArgs(mcp, sandboxDir)
  const transport = new StdioClientTransport({
    command: deno,
    args,
    cwd: sandboxDir,
    stderr: 'pipe',
    env: buildDenoEnvironment(mcp, sandboxDir),
  })
  const stderr = captureStartupStderr(transport.stderr as Readable | null)

  const client = new Client({ name: 'mymcps-gateway', version: applicationVersion })

  try {
    await client.connect(transport)
  } catch (error) {
    throw createDenoStartupError(mcp, error, await stderr.read())
  }
  stderr.discard()

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
