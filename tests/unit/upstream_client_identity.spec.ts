import { test } from '@japa/runner'
import { applicationVersion } from '#services/application_version'
import McpSecretStore from '#services/mcp_secret_store'
import { buildUpstreamHeaders, connectHttpUpstream } from '#services/upstream/http_client'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

type ClientInfo = { name?: string; version?: string; title?: string }

type SeenRequest = {
  userAgent: string | null
  authorization: string | null
  clientInfo?: ClientInfo
}

function mcpServer() {
  const seen: SeenRequest[] = []
  const original = globalThis.fetch
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init)
    const entry: SeenRequest = {
      userAgent: request.headers.get('User-Agent'),
      authorization: request.headers.get('Authorization'),
    }
    seen.push(entry)
    if (request.method !== 'POST') return new Response(null, { status: 405 })

    const message = (await request.json()) as {
      id?: number
      method: string
      params?: { clientInfo?: ClientInfo; protocolVersion?: string }
    }
    if (message.method === 'initialize') {
      entry.clientInfo = message.params?.clientInfo
      return json({
        jsonrpc: '2.0',
        id: message.id,
        result: {
          protocolVersion: message.params?.protocolVersion,
          capabilities: {},
          serverInfo: { name: 'test', version: '1.0' },
        },
      })
    }
    return new Response(null, { status: 202 })
  }
  return {
    seen,
    restore() {
      globalThis.fetch = original
    },
  }
}

test.group('Upstream allowlisted client identity', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('sends Codex on Figma requests and Claude Code on Strava requests', async ({ assert }) => {
    const admin = await createAdmin()
    const figma = await createMcp(admin.id, {
      name: 'Figma',
      httpUrl: 'https://mcp.figma.com/mcp',
      authType: 'bearer',
    })
    figma.authBearer = McpSecretStore.encrypt('figma-token')
    await figma.save()
    const strava = await createMcp(admin.id, {
      name: 'Strava',
      httpUrl: 'https://mcp.strava.com/mcp',
    })
    const other = await createMcp(admin.id, {
      name: 'Other',
      httpUrl: 'https://mcp.example/mcp',
    })

    assert.deepEqual(buildUpstreamHeaders(figma), {
      'User-Agent': 'codex-mcp-client/0.0.0',
      'Authorization': 'Bearer figma-token',
    })
    assert.deepEqual(buildUpstreamHeaders(strava), {
      'User-Agent': 'claude-code/2.1.89 (cli)',
    })
    assert.deepEqual(buildUpstreamHeaders(other), {})

    const figmaServer = mcpServer()
    try {
      const connected = await connectHttpUpstream(figma)
      await connected.close()
    } finally {
      figmaServer.restore()
    }
    const figmaInitialize = figmaServer.seen.find((request) => request.clientInfo)
    assert.equal(figmaInitialize?.userAgent, 'codex-mcp-client/0.0.0')
    assert.equal(figmaInitialize?.authorization, 'Bearer figma-token')
    assert.deepEqual(figmaInitialize?.clientInfo, {
      name: 'codex-mcp-client',
      version: '0.0.0',
      title: 'Codex',
    })
    assert.isAbove(figmaServer.seen.length, 0)
    for (const request of figmaServer.seen) {
      assert.equal(request.userAgent, 'codex-mcp-client/0.0.0')
    }

    const stravaServer = mcpServer()
    try {
      const connected = await connectHttpUpstream(strava)
      await connected.close()
    } finally {
      stravaServer.restore()
    }
    const stravaInitialize = stravaServer.seen.find((request) => request.clientInfo)
    assert.equal(stravaInitialize?.userAgent, 'claude-code/2.1.89 (cli)')
    assert.isNull(stravaInitialize?.authorization)
    assert.deepEqual(stravaInitialize?.clientInfo, {
      name: 'claude-code',
      version: '2.1.89',
      title: 'Claude Code',
    })

    const otherServer = mcpServer()
    try {
      const connected = await connectHttpUpstream(other)
      await connected.close()
    } finally {
      otherServer.restore()
    }
    const otherInitialize = otherServer.seen.find((request) => request.clientInfo)
    assert.notEqual(otherInitialize?.userAgent, 'codex-mcp-client/0.0.0')
    assert.notEqual(otherInitialize?.userAgent, 'claude-code/2.1.89 (cli)')
    assert.deepEqual(otherInitialize?.clientInfo, {
      name: 'mymcps-gateway',
      version: applicationVersion,
    })
  })
})
