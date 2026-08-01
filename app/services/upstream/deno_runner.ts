import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { existsSync } from 'node:fs'
import app from '@adonisjs/core/services/app'
import env from '#start/env'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import type Mcp from '#models/mcp'
import type { UpstreamTool } from '#services/upstream/http_client'

export type ConnectedDenoUpstream = {
  client: Client
  transport: StdioClientTransport
  close: () => Promise<void>
}

function resolveDenoBinary() {
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

export function sandboxRootFor(mcpId: number) {
  return join(app.tmpPath('mcp-sandboxes'), String(mcpId))
}

/**
 * Build Deno permission flags so the MCP cannot read the Adonis DB, .env, or app source.
 * Deno is deny-by-default: only the sandbox workdir is readable/writable.
 */
export function buildDenoArgs(mcp: Mcp, sandboxDir: string) {
  if (!mcp.npmPackage) {
    throw new Error('npm MCP is missing a package name')
  }

  const version = mcp.npmVersion?.trim() || 'latest'
  const npmSpec = `npm:${mcp.npmPackage}@${version}`
  const extraArgs = mcp.npmArgsList

  return [
    'run',
    '--quiet',
    `--allow-read=${sandboxDir}`,
    `--allow-write=${sandboxDir}`,
    '--allow-net',
    '--allow-env',
    '--no-prompt',
    npmSpec,
    ...extraArgs,
  ]
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
    env: {
      HOME: sandboxDir,
      TMPDIR: sandboxDir,
      NO_COLOR: '1',
    },
  })

  const client = new Client({ name: 'mymcps-gateway', version: '0.1.0' })

  try {
    await client.connect(transport)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new Error(
      `Failed to start Deno npm MCP "${mcp.npmPackage}". Is Deno installed? ${message}`
    )
  }

  return {
    client,
    transport,
    close: async () => {
      try {
        await client.close()
      } catch {
        // ignore
      }
      try {
        await transport.close()
      } catch {
        // ignore
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
      inputSchema: (tool.inputSchema as Record<string, unknown>) ?? { type: 'object' },
    }))
  } finally {
    await connected.close()
  }
}
