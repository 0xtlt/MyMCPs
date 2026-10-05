import { Readable } from 'node:stream'
import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import { builtinFileUrl } from '#services/builtin/file_link'
import { icloudMailServers } from '#services/builtin/icloud_mail/connection'
import env from '#start/env'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import {
  createIcloudMailMcp,
  ICLOUD_MAIL_ATTACHMENT,
  mockIcloudMail,
} from '#tests/helpers/icloud_mail'

const attachment = { mailbox: 'INBOX', uid: 11, part: '2' }

/**
 * Links name APP_URL, which need not be where the test server listens. The
 * signature covers the path and the query, so the same link works on both.
 */
function fetchLink(url: string | URL, signal = AbortSignal.timeout(10_000)) {
  const served = new URL(url)
  served.host = `${env.get('HOST')}:${env.get('PORT')}`
  return fetch(served, { signal })
}

async function status(url: string | URL) {
  const response = await fetchLink(url)
  await response.arrayBuffer()
  return response.status
}

async function until(condition: () => boolean) {
  while (!condition()) {
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
}

let sequence = 0

async function linkedMcp() {
  const admin = await createAdmin()
  sequence += 1
  const mcp = await createIcloudMailMcp(admin.id, { name: `iCloud Mail files ${sequence}` })
  return { mcp, link: builtinFileUrl(mcp.id, attachment, 60_000) }
}

/**
 * Hold every download of one MCP at iCloud until `open` is called, the way a
 * large attachment keeps its connection busy.
 */
function holdDownloads(mcpId: number) {
  const imap = icloudMailServers.imap
  const held = { started: 0, open: () => {} }
  const opened = new Promise<void>((resolve) => {
    held.open = resolve
  })

  icloudMailServers.imap = (signIn) => {
    const client = imap(signIn)
    if (signIn.mcpId !== mcpId) return client

    const download = client.download.bind(client)
    client.download = (async (...args: Parameters<typeof download>) => {
      held.started += 1
      await opened
      return download(...args)
    }) as typeof client.download
    return client
  }
  return held
}

/** Deliver the content of every download in the given pieces instead. */
function downloadInPieces(pieces: Buffer[]) {
  const imap = icloudMailServers.imap
  icloudMailServers.imap = (signIn) => {
    const client = imap(signIn)
    client.download = (async () => ({
      meta: {},
      content: Readable.from(pieces, { objectMode: false }),
    })) as unknown as typeof client.download
    return client
  }
}

test.group('Built-in file links hardening', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('does not count requests that have no valid signature', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const { link } = await linkedMcp()
      const forged = new URL(link)
      forged.searchParams.set('signature', 'forged')
      const unsigned = new URL(link)
      unsigned.search = ''

      // More than the 60 downloads allowed in 15 minutes, from the same address.
      for (let attempt = 0; attempt < 70; attempt++) {
        assert.equal(await status(attempt % 2 === 0 ? forged : unsigned), 403)
      }
      assert.lengthOf(icloud.signIns, 0)

      const download = await fetchLink(link)
      assert.equal(download.status, 200)
      assert.equal(Buffer.from(await download.arrayBuffer()).toString(), ICLOUD_MAIL_ATTACHMENT)
    } finally {
      icloud.restore()
    }
  })

  test('counts the downloads of each MCP apart, and says when to try again', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const busy = await linkedMcp()
      const other = await linkedMcp()

      for (let download = 0; download < 60; download++) {
        assert.equal(await status(busy.link), 200)
      }

      const refused = await fetchLink(busy.link)
      assert.equal(refused.status, 429)
      assert.equal(await refused.text(), 'Too many downloads. Try again later.')
      const retryAfter = Number(refused.headers.get('retry-after'))
      assert.isAbove(retryAfter, 0)
      assert.isAtMost(retryAfter, 15 * 60)
      assert.lengthOf(icloud.signIns, 60)

      // Using up the downloads of one MCP leaves those of another alone.
      assert.equal(await status(other.link), 200)
    } finally {
      icloud.restore()
    }
  })

  test('serves at most three downloads of an MCP at once', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const busy = await linkedMcp()
      const other = await linkedMcp()
      const held = holdDownloads(busy.mcp.id)

      const downloads = [1, 2, 3].map(() => fetchLink(busy.link))
      await until(() => held.started === 3)

      const refused = await fetchLink(busy.link)
      assert.equal(refused.status, 429)
      assert.equal(refused.headers.get('retry-after'), '5')
      assert.equal(await refused.text(), 'Too many downloads at once. Try again in a few seconds.')
      // Refused before signing in to iCloud a fourth time.
      assert.equal(held.started, 3)
      assert.lengthOf(icloud.signIns, 3)

      // Another MCP is served in the meantime.
      assert.equal(await status(other.link), 200)

      held.open()
      for (const download of await Promise.all(downloads)) {
        assert.equal(download.status, 200)
        assert.equal(Buffer.from(await download.arrayBuffer()).toString(), ICLOUD_MAIL_ATTACHMENT)
      }

      // Every place was given back.
      const next = await Promise.all([1, 2, 3].map(() => status(busy.link)))
      assert.deepEqual(next, [200, 200, 200])
    } finally {
      icloud.restore()
    }
  })

  test('gives a place back when a download fails, and keeps it while one is running', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const { mcp, link } = await linkedMcp()

      // The message text is not a file: each of these fails after signing in.
      const notAFile = builtinFileUrl(mcp.id, { ...attachment, part: '1.1' }, 60_000)
      for (let attempt = 0; attempt < 5; attempt++) {
        assert.equal(await status(notAFile), 404)
      }

      // A client that hangs up does not stop its download, so it keeps its place.
      const held = holdDownloads(mcp.id)
      const client = new AbortController()
      const abandoned = fetchLink(link, client.signal).catch(() => null)
      await until(() => held.started === 1)
      client.abort()
      assert.isNull(await abandoned)

      const downloads = [1, 2].map(() => fetchLink(link))
      await until(() => held.started === 3)
      assert.equal(await status(link), 429)

      held.open()
      for (const download of await Promise.all(downloads)) {
        assert.equal(download.status, 200)
        await download.arrayBuffer()
      }
      await until(() => icloud.logouts === 8)
      assert.deepEqual(await Promise.all([1, 2, 3].map(() => status(link))), [200, 200, 200])
    } finally {
      icloud.restore()
    }
  })

  test('sends a file received in pieces whole, and refuses one over 30 MB', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const { link } = await linkedMcp()

      downloadInPieces([Buffer.from('%PDF-'), Buffer.from('1.4\n'), Buffer.from('%%EOF\n')])
      const whole = await fetchLink(link)
      assert.equal(whole.status, 200)
      assert.equal(whole.headers.get('content-length'), '15')
      assert.equal(whole.headers.get('content-type'), 'application/pdf')
      assert.equal(await whole.text(), '%PDF-1.4\n%%EOF\n')

      downloadInPieces([])
      const empty = await fetchLink(link)
      assert.equal(empty.status, 200)
      assert.equal(empty.headers.get('content-length'), '0')
      assert.equal(await empty.text(), '')

      const piece = Buffer.alloc(10_000_000)
      downloadInPieces([piece, piece, piece, Buffer.alloc(1)])
      const tooLarge = await fetchLink(link)
      assert.equal(tooLarge.status, 404)
      assert.equal(await tooLarge.text(), 'This file is no longer available.')
    } finally {
      icloud.restore()
    }
  })

  test('names a download whose filename is not well-formed Unicode', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const { link } = await linkedMcp()
      // Half of a surrogate pair, as a mis-encoded header decodes to.
      const [, file] = icloud.mailbox('INBOX').messages[0].bodyStructure.childNodes!
      file.dispositionParameters = { filename: `Menu ${String.fromCharCode(0xd83d)}.pdf` }

      const download = await fetchLink(link)

      assert.equal(download.status, 200)
      assert.equal(
        download.headers.get('content-disposition'),
        `attachment; filename="Menu _.pdf"; filename*=UTF-8''Menu%20%EF%BF%BD.pdf`
      )
      assert.equal(Buffer.from(await download.arrayBuffer()).toString(), ICLOUD_MAIL_ATTACHMENT)
    } finally {
      icloud.restore()
    }
  })
})
