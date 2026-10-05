import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { test } from '@japa/runner'
import { listHttpTools } from '#services/upstream/http_client'
import {
  fetchWithSameOriginRedirects,
  MAX_UPSTREAM_ERROR_RESPONSE_BYTES,
  MAX_UPSTREAM_RESPONSE_BYTES,
} from '#services/upstream/safe_fetch'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function mockFetch(handler: (request: Request, call: number) => Response | Promise<Response>) {
  const original = globalThis.fetch
  let calls = 0
  globalThis.fetch = async (input, init) => handler(new Request(input, init), ++calls)
  return () => {
    globalThis.fetch = original
  }
}

/** A body of `chunks` chunks of `chunkBytes`, or endless, that records being cancelled. */
function streamedBody(chunkBytes: number, chunks = Number.POSITIVE_INFINITY) {
  const state = { sent: 0, cancelled: false }
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (state.sent >= chunks) {
        controller.close()
        return
      }
      state.sent++
      controller.enqueue(new Uint8Array(chunkBytes).fill(97))
    },
    cancel() {
      state.cancelled = true
    },
  })
  return { body, state }
}

test.group('Upstream fetch: abort signal', () => {
  test('still cancels a request that followed a same-origin redirect', async ({ assert }) => {
    const controller = new AbortController()
    const requests: Request[] = []
    const restore = mockFetch((request, call) => {
      requests.push(request)
      if (call === 1) {
        return new Response(null, { status: 307, headers: { Location: '/canonical' } })
      }
      // Like a server that accepted the request and never answers.
      return new Promise<Response>((_resolve, reject) => {
        request.signal.addEventListener('abort', () => reject(request.signal.reason))
      })
    })

    try {
      const pending = fetchWithSameOriginRedirects(
        'https://trusted.example/mcp',
        { method: 'POST', body: '{"jsonrpc":"2.0"}', signal: controller.signal },
        'MCP endpoint'
      )
      setTimeout(() => controller.abort(new Error('request timed out')), 20)

      await assert.rejects(() => pending, 'request timed out')
      assert.lengthOf(requests, 2)
      assert.equal(requests[1].url, 'https://trusted.example/canonical')
      assert.equal(requests[1].method, 'POST')
      assert.isTrue(requests[1].signal.aborted)
    } finally {
      restore()
    }
  })

  test('cancels the body read of a response reached through a redirect', async ({ assert }) => {
    const server = createServer((request, response) => {
      if (request.url === '/mcp') {
        response.writeHead(307, { Location: '/stalled' }).end()
        return
      }
      // Headers and a first chunk, then nothing more.
      response.writeHead(200, { 'Content-Type': 'text/plain' })
      response.write('partial')
    })
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
    const controller = new AbortController()

    try {
      const response = await fetchWithSameOriginRedirects(
        `http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp`,
        { signal: controller.signal },
        'MCP endpoint'
      )
      assert.equal(response.status, 200)
      const reading = response.text()
      setTimeout(() => controller.abort(new Error('request timed out')), 20)

      await assert.rejects(() => reading, 'request timed out')
    } finally {
      server.closeAllConnections()
      server.close()
    }
  })

  test("hands every hop the caller's own signal, not one derived by a Request", async ({
    assert,
  }) => {
    // A signal read from a Request stops following the caller's once that
    // Request is garbage-collected, which would leave later hops and the body
    // of the final response without a way to be cancelled.
    const controller = new AbortController()
    const original = globalThis.fetch
    const signals: Array<AbortSignal | null | undefined> = []
    globalThis.fetch = async (_input, init) => {
      signals.push(init?.signal)
      return signals.length < 3
        ? new Response(null, { status: 308, headers: { Location: `/hop-${signals.length}` } })
        : new Response('done')
    }

    try {
      const fromInit = await fetchWithSameOriginRedirects(
        'https://trusted.example/mcp',
        { signal: controller.signal },
        'MCP endpoint'
      )
      assert.equal(await fromInit.text(), 'done')
      assert.lengthOf(signals, 3)
      for (const signal of signals) {
        assert.strictEqual(signal, controller.signal)
      }

      signals.length = 0
      const input = new Request('https://trusted.example/mcp', { signal: controller.signal })
      await fetchWithSameOriginRedirects(input, undefined, 'MCP endpoint')
      for (const signal of signals) {
        assert.strictEqual(signal, input.signal)
      }
    } finally {
      globalThis.fetch = original
    }
  })
})

test.group('Upstream fetch: response size', () => {
  test('documents its limits', ({ assert }) => {
    assert.equal(MAX_UPSTREAM_RESPONSE_BYTES, 32 * 1024 * 1024)
    assert.equal(MAX_UPSTREAM_ERROR_RESPONSE_BYTES, 64 * 1024)
  })

  test('fails the read of a successful body past its limit and stops the download', async ({
    assert,
  }) => {
    const endless = streamedBody(400)
    const restore = mockFetch(() => new Response(endless.body, { status: 200 }))

    try {
      const response = await fetchWithSameOriginRedirects(
        'https://upstream.example/mcp',
        {},
        'MCP endpoint',
        { maxResponseBytes: 1000 }
      )

      await assert.rejects(() => response.text(), 'MCP endpoint response exceeded 1000 bytes')
      assert.isTrue(endless.state.cancelled)
      assert.isAtMost(endless.state.sent, 4)
    } finally {
      restore()
    }
  })

  test('cuts an error body short instead of reading it whole', async ({ assert }) => {
    const endless = streamedBody(400)
    const restore = mockFetch(
      () => new Response(endless.body, { status: 502, statusText: 'Bad Gateway' })
    )

    try {
      const response = await fetchWithSameOriginRedirects(
        'https://upstream.example/mcp',
        {},
        'MCP endpoint',
        { maxErrorResponseBytes: 1000 }
      )

      assert.equal(response.status, 502)
      assert.equal(response.statusText, 'Bad Gateway')
      // A clone is what the 401 diagnostic reads; both copies stay bounded.
      const copy = response.clone()
      assert.equal(await response.text(), 'a'.repeat(1000))
      assert.equal(await copy.text(), 'a'.repeat(1000))
      assert.isTrue(endless.state.cancelled)
    } finally {
      restore()
    }
  })

  test('passes bodies within the limit, and bodiless responses, through unchanged', async ({
    assert,
  }) => {
    const restore = mockFetch((request) => {
      if (request.url.endsWith('/empty')) return new Response(null, { status: 204 })
      const exact = streamedBody(250, 4)
      return new Response(exact.body, {
        status: 200,
        headers: { 'Content-Type': 'text/plain', 'Mcp-Session-Id': 'session-1' },
      })
    })

    try {
      const response = await fetchWithSameOriginRedirects(
        'https://upstream.example/mcp',
        {},
        'MCP endpoint',
        { maxResponseBytes: 1000 }
      )
      assert.equal(response.headers.get('Mcp-Session-Id'), 'session-1')
      assert.equal(response.headers.get('Content-Type'), 'text/plain')
      assert.equal(await response.text(), 'a'.repeat(1000))

      const empty = await fetchWithSameOriginRedirects(
        'https://upstream.example/empty',
        {},
        'MCP endpoint',
        { maxResponseBytes: 1 }
      )
      assert.equal(empty.status, 204)
      assert.isNull(empty.body)
    } finally {
      restore()
    }
  })
})

type Upstream = { url: string; close: () => Promise<void> }

/** A real HTTP server, so the SDK client reads through the limited body as in production. */
async function upstream(
  answer: (message: { id?: number; method: string }, response: ServerResponse) => void
): Promise<Upstream> {
  const server = createServer(async (request: IncomingMessage, response: ServerResponse) => {
    if (request.method !== 'POST') {
      response.writeHead(405).end()
      return
    }
    const chunks: Buffer[] = []
    for await (const chunk of request) chunks.push(chunk as Buffer)
    const message = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    if (message.method === 'initialize') {
      response.writeHead(200, { 'Content-Type': 'application/json' })
      response.end(
        JSON.stringify({
          jsonrpc: '2.0',
          id: message.id,
          result: {
            protocolVersion: message.params.protocolVersion,
            capabilities: { tools: {} },
            serverInfo: { name: 'local', version: '1.0.0' },
          },
        })
      )
      return
    }
    if (message.id === undefined) {
      response.writeHead(202).end()
      return
    }
    answer(message, response)
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    url: `http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp`,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections()
        server.close(() => resolve())
      }),
  }
}

const TOOLS = { tools: [{ name: 'echo', inputSchema: { type: 'object' } }] }

test.group('Upstream fetch: MCP client through the limited body', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  async function httpMcp(url: string) {
    const admin = await createAdmin()
    return createMcp(admin.id, { httpUrl: url, authType: 'bearer' })
  }

  async function toolNames(url: string) {
    const tools = await listHttpTools(await httpMcp(url))
    return tools.map((tool) => tool.name)
  }

  test('reads JSON and event-stream answers as before', async ({ assert }) => {
    const json = await upstream((message, response) => {
      response.writeHead(200, { 'Content-Type': 'application/json' })
      response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: TOOLS }))
    })
    const stream = await upstream((message, response) => {
      response.writeHead(200, { 'Content-Type': 'text/event-stream' })
      // Split mid-event, as a network would.
      const event = `event: message\ndata: ${JSON.stringify({ jsonrpc: '2.0', id: message.id, result: TOOLS })}\n\n`
      response.write(event.slice(0, 20))
      setTimeout(() => response.end(event.slice(20)), 10)
    })

    try {
      assert.deepEqual(await toolNames(json.url), ['echo'])
      assert.deepEqual(await toolNames(stream.url), ['echo'])
    } finally {
      await json.close()
      await stream.close()
    }
  }).timeout(20_000)

  test('gives up on an answer larger than the limit', async ({ assert }) => {
    let written = 0
    const oversized = await upstream((_message, response) => {
      response.writeHead(200, { 'Content-Type': 'application/json' })
      const chunk = Buffer.alloc(1024 * 1024, 'a')
      // Twice the limit, sent only as fast as the client reads it.
      const send = () => {
        while (written < 64) {
          written++
          if (!response.write(chunk)) {
            response.once('drain', send)
            return
          }
        }
        response.end()
      }
      response.on('error', () => undefined)
      send()
    })

    try {
      await assert.rejects(
        async () => listHttpTools(await httpMcp(oversized.url)),
        /MCP endpoint response exceeded 33554432 bytes/
      )
      assert.isBelow(written, 64)
    } finally {
      await oversized.close()
    }
  }).timeout(60_000)

  test('quotes only the start of an oversized error page', async ({ assert }) => {
    const failing = await upstream((_message, response) => {
      response.writeHead(500, { 'Content-Type': 'text/html' })
      response.end(Buffer.alloc(4 * 1024 * 1024, 'e'))
    })

    try {
      let failure: unknown
      try {
        await listHttpTools(await httpMcp(failing.url))
      } catch (error) {
        failure = error
      }
      assert.instanceOf(failure, Error)
      const message = (failure as Error).message
      assert.include(message, 'Error POSTing to endpoint')
      assert.isAtMost(message.length, MAX_UPSTREAM_ERROR_RESPONSE_BYTES + 200)
    } finally {
      await failing.close()
    }
  }).timeout(20_000)
})
