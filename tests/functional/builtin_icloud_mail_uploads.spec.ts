import { randomUUID } from 'node:crypto'
import { readdir } from 'node:fs/promises'
import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import { signedUrlFor } from '@adonisjs/core/services/url_builder'
import limiter from '@adonisjs/limiter/services/main'
import type Mcp from '#models/mcp'
import { BUILTIN_FILE_PURPOSE, builtinFileUrl, builtinUploadUrl } from '#services/builtin/file_link'
import { findBuiltinUpload, readBuiltinUpload } from '#services/builtin/upload_store'
import env from '#start/env'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'
import {
  clearUploads,
  createIcloudMailMcp,
  mockIcloudMail,
  uploadFile,
  uploadsDirectory,
} from '#tests/helpers/icloud_mail'

type RpcResponse = {
  result?: { content?: Array<{ type: string; text: string }>; isError?: boolean }
}

async function callTool(
  client: ApiClient,
  plaintext: string,
  name: string,
  args: Record<string, unknown>
) {
  const response = await client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')
    .header('X-MyMCPs-Tool-Mode', 'eager')
    .json({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name, arguments: args } })
  response.assertStatus(200)

  const data = response
    .text()
    .split('\n')
    .find((line) => line.startsWith('data:'))
    ?.slice(5)
  const { result } = (data ? JSON.parse(data) : response.body()) as RpcResponse
  const [{ text }] = result!.content!
  return { isError: result!.isError, text, data: result!.isError ? null : JSON.parse(text) }
}

type Body = string | Buffer | FormData | ReadableStream<Uint8Array> | undefined

/**
 * Links name APP_URL, which need not be where the test server listens. The
 * signature covers the path and the query, so the same link works on both.
 * No cookie, no CSRF token, no access token: the signature is the credential.
 */
function sendFile(
  url: string | URL,
  body: Body,
  headers: Record<string, string> = {},
  signal = AbortSignal.timeout(20_000)
) {
  const served = new URL(url)
  served.host = `${env.get('HOST')}:${env.get('PORT')}`
  return fetch(served, {
    method: 'PUT',
    body,
    headers,
    signal,
    ...(body instanceof ReadableStream ? { duplex: 'half' } : {}),
  } as RequestInit)
}

async function answer(response: Promise<Response>) {
  const received = await response
  return [received.status, await received.text()] as const
}

async function status(response: Promise<Response>) {
  const [code] = await answer(response)
  return code
}

async function uploadedText(mcp: Mcp, upload: string) {
  const content = await readBuiltinUpload(mcp.id, upload)
  return content?.toString()
}

let sequence = 0

async function writingMcp(permissions = ['send'], name?: string) {
  const admin = await createAdmin()
  sequence += 1
  const mcp = await createIcloudMailMcp(admin.id, {
    name: name ?? `iCloud Mail uploads ${sequence}`,
    permissions,
  })
  return { admin, mcp }
}

/** The MCP whose tools the gateway names `icloud-mail__…`. */
const gatewayMcp = (permissions: string[]) => writingMcp(permissions, 'iCloud Mail')

/** A link like the ones create_upload_link hands out. */
function uploadLink(mcp: Mcp, filename = 'report.pdf', expiresInMs = 60_000) {
  const upload = randomUUID()
  return { upload, url: builtinUploadUrl(mcp.id, { upload, filename }, expiresInMs) }
}

async function storedFiles(mcp: Mcp) {
  try {
    const names = await readdir(uploadsDirectory(mcp.id))
    return names.filter((name) => !name.endsWith('.json'))
  } catch {
    return []
  }
}

async function storedCount(mcp: Mcp) {
  const names = await storedFiles(mcp)
  return names.length
}

async function until(condition: () => boolean | Promise<boolean>) {
  while (!(await condition())) {
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
}

/** A body that stays open until `finish` is called, like a large file on a slow line. */
function openBody(firstBytes = 'begin') {
  let controller: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(started) {
      controller = started
      controller.enqueue(Buffer.from(firstBytes))
    },
  })
  return { stream, finish: () => controller.close() }
}

const PDF = Buffer.from('%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n')
const UNAVAILABLE = [404, 'This upload link can no longer be used.'] as const
const INVALID = [403, 'This link is invalid or has expired.'] as const

test.group('Built-in iCloud Mail MCP: upload links', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(clearUploads)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(clearUploads)
  group.each.teardown(rollbackTestTransaction)

  test('hands out a link that takes one file, and attaches it to a message', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const { admin, mcp } = await gatewayMcp(['send'])
      const { plaintext } = await createAccessToken(admin.id)

      const link = await callTool(client, plaintext, 'icloud-mail__create_upload_link', {
        filename: 'Devis été.pdf',
        content_type: 'application/pdf',
      })
      assert.isUndefined(link.isError)
      assert.deepEqual(
        { ...link.data, url: undefined, upload_id: undefined, expires_at: undefined },
        {
          upload_id: undefined,
          url: undefined,
          method: 'PUT',
          expires_at: undefined,
          filename: 'Devis été.pdf',
          content_type: 'application/pdf',
          max_bytes: 20_000_000,
        }
      )
      assert.match(link.data.upload_id, /^[0-9a-f-]{36}$/)
      const expiresIn = new Date(link.data.expires_at).getTime() - Date.now()
      assert.isAbove(expiresIn, 14 * 60_000)
      assert.isAtMost(expiresIn, 15 * 60_000)

      const url = new URL(link.data.url)
      assert.equal(url.origin, 'http://localhost:3333')
      assert.match(url.pathname, new RegExp(`^/uploads/${mcp.id}/[\\w-]+$`))
      assert.deepEqual([...url.searchParams.keys()], ['signature'])
      // Asking for a link reaches neither iCloud nor the disk.
      assert.lengthOf(icloud.signIns, 0)
      assert.deepEqual(await storedFiles(mcp), [])

      const uploaded = await sendFile(link.data.url, PDF)
      assert.equal(uploaded.status, 201)
      const receipt = (await uploaded.json()) as Record<string, any>
      assert.deepEqual(
        { ...receipt, expires_at: undefined },
        {
          upload_id: link.data.upload_id,
          filename: 'Devis été.pdf',
          size: PDF.length,
          expires_at: undefined,
        }
      )
      const keptFor = new Date(receipt.expires_at).getTime() - Date.now()
      assert.isAbove(keptFor, 59 * 60_000)
      assert.isAtMost(keptFor, 60 * 60_000)

      // The link is used up, whoever else got hold of it.
      assert.deepEqual(await answer(sendFile(link.data.url, Buffer.from('something else'))), [
        409,
        'A file was already sent to this link. Ask for a new link to send another one.',
      ])

      const sent = await callTool(client, plaintext, 'icloud-mail__send_message', {
        to: 'dave@example.com',
        subject: 'Quote',
        text: 'Here it is.',
        attachments: [link.data.upload_id],
      })
      assert.isUndefined(sent.isError)
      assert.deepEqual(sent.data.attachments, [{ filename: 'Devis été.pdf', size: PDF.length }])
      assert.deepEqual(icloud.sent[0].attachments, [
        { filename: 'Devis été.pdf', contentType: 'application/pdf', content: PDF },
      ])
      const [copy] = icloud.mailbox('Sent Messages').appended
      assert.include(copy.raw, PDF.toString('base64'))
    } finally {
      icloud.restore()
    }
  })

  test('refuses to attach a file before it was sent to its link', async ({ client, assert }) => {
    const icloud = mockIcloudMail()
    try {
      const { admin } = await gatewayMcp(['draft'])
      const { plaintext } = await createAccessToken(admin.id)
      const link = await callTool(client, plaintext, 'icloud-mail__create_upload_link', {
        filename: 'notes.txt',
        expires_in_minutes: 60,
      })
      assert.notProperty(link.data, 'content_type')

      const draft = { subject: 'Notes', text: 'Attached.', attachments: link.data.upload_id }
      const early = await callTool(client, plaintext, 'icloud-mail__create_draft', draft)
      assert.isTrue(early.isError)
      assert.include(early.text, `No file is uploaded as "${link.data.upload_id}".`)
      assert.lengthOf(icloud.signIns, 0)

      assert.equal(await status(sendFile(link.data.url, 'Call back on Monday.\n')), 201)
      const saved = await callTool(client, plaintext, 'icloud-mail__create_draft', draft)
      assert.isUndefined(saved.isError)
      assert.deepEqual(saved.data.attachments, [{ filename: 'notes.txt', size: 21 }])
      const [stored] = icloud.mailbox('Drafts').appended
      assert.include(stored.raw, 'Content-Disposition: attachment; filename=notes.txt\r\n')
    } finally {
      icloud.restore()
    }
  })

  test('needs a permission that writes messages, and the public address of the instance', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    const appUrl = env.get('APP_URL')
    try {
      const { admin, mcp } = await gatewayMcp(['read', 'organize'])
      const { plaintext } = await createAccessToken(admin.id)
      const ask = () =>
        callTool(client, plaintext, 'icloud-mail__create_upload_link', { filename: 'a.pdf' })

      const readOnly = await ask()
      assert.isTrue(readOnly.isError)
      assert.equal(
        readOnly.text,
        'create_upload_link needs the "draft" or "send" permission, which is not allowed for this iCloud Mail MCP. An administrator can allow it from the MCPs page in MyMCPs.'
      )

      mcp.builtinPermissions = 'draft'
      await mcp.save()
      env.set('APP_URL', undefined)
      const unreachable = await ask()
      assert.isTrue(unreachable.isError)
      assert.equal(
        unreachable.text,
        'File links need the public address of this MyMCPs instance. An administrator must set APP_URL.'
      )
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      env.set('APP_URL', appUrl)
      icloud.restore()
    }
  })
})

test.group('Built-in upload links hardening', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(clearUploads)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(clearUploads)
  group.each.teardown(rollbackTestTransaction)

  test('stores the body as it is, whatever it is labelled', async ({ assert }) => {
    const { mcp } = await writingMcp()
    // Larger than the 1 MB a parsed body may take, and not valid JSON.
    const large = Buffer.concat([Buffer.from('{"rows":['), Buffer.alloc(2_000_000, '7')])
    const bodies: Array<[Body, Record<string, string>, Buffer]> = [
      [PDF, {}, PDF],
      // fetch labels a string as text/plain.
      ['id;name\r\n1;André\r\n', {}, Buffer.from('id;name\r\n1;André\r\n')],
      [large, { 'content-type': 'application/json' }, large],
      [
        'a=1&b=%zz',
        { 'content-type': 'application/x-www-form-urlencoded' },
        Buffer.from('a=1&b=%zz'),
      ],
      [
        Buffer.from([0, 255, 13, 10, 0]),
        { 'content-type': 'application/octet-stream' },
        Buffer.from([0, 255, 13, 10, 0]),
      ],
    ]

    for (const [body, headers, expected] of bodies) {
      const { upload, url } = uploadLink(mcp)
      const response = await sendFile(url, body, headers)
      assert.equal(response.status, 201, JSON.stringify(headers))
      const receipt = (await response.json()) as { size: number }
      assert.equal(receipt.size, expected.length)
      assert.deepEqual(await readBuiltinUpload(mcp.id, upload), expected)
    }
  })

  test('refuses a form, an empty body, and a file over 20 MB', async ({ assert }) => {
    const { mcp } = await writingMcp()
    const how =
      'Send the file itself as the body of the PUT request, for example with: curl -T <file> "<link>"'
    const { upload, url } = uploadLink(mcp)

    const form = new FormData()
    form.set('file', new Blob([PDF]), 'report.pdf')
    assert.deepEqual(await answer(sendFile(url, form)), [
      415,
      `A form cannot be stored as a file. ${how}`,
    ])
    assert.deepEqual(await answer(sendFile(url, undefined)), [
      400,
      `The request has no body. ${how}`,
    ])
    assert.deepEqual(await answer(sendFile(url, Buffer.alloc(0))), [
      400,
      `The request has no body. ${how}`,
    ])

    const tooLarge = [413, 'The file is larger than the 20 MB this link takes.'] as const
    // Refused on what the client says it will send.
    assert.deepEqual(await answer(sendFile(url, Buffer.alloc(20_000_001))), tooLarge)
    // And at the first byte too many when it does not say.
    const megabyte = Buffer.alloc(1_000_000, 'x')
    let pieces = 0
    const endless = new ReadableStream<Uint8Array>({
      pull(controller) {
        pieces += 1
        if (pieces > 40) controller.close()
        else controller.enqueue(megabyte)
      },
    })
    assert.deepEqual(await answer(sendFile(url, endless)), tooLarge)

    // Nothing was kept of any of them, and the link still takes a file that fits.
    assert.deepEqual(await storedFiles(mcp), [])
    const exact = await sendFile(url, Buffer.alloc(20_000_000, 'x'))
    assert.equal(exact.status, 201)
    const kept = await findBuiltinUpload(mcp.id, upload)
    assert.equal(kept?.size, 20_000_000)
  }).timeout(60_000)

  test('takes a file only with our signature for uploads', async ({ assert }) => {
    const { mcp } = await writingMcp()
    const { upload, url } = uploadLink(mcp)
    const reference = new URL(url).pathname.split('/').pop()!

    const forged = new URL(url)
    forged.searchParams.set('signature', 'forged')
    const unsigned = new URL(url)
    unsigned.search = ''
    // The reference of another link under this signature.
    const swapped = new URL(url)
    swapped.pathname = new URL(uploadLink(mcp, 'other.pdf').url).pathname
    // Signed by us, but to download a file.
    const download = signedUrlFor(
      'builtin.upload',
      { id: mcp.id, reference },
      { expiresIn: 60_000, purpose: BUILTIN_FILE_PURPOSE, prefixUrl: env.get('APP_URL') }
    )
    const attachment = new URL(
      builtinFileUrl(mcp.id, { mailbox: 'INBOX', uid: 11, part: '2' }, 60_000)
    )
    attachment.pathname = attachment.pathname.replace('/files/', '/uploads/')
    const expired = uploadLink(mcp, 'late.pdf', 1)
    await new Promise((resolve) => setTimeout(resolve, 20))

    for (const link of [forged, unsigned, swapped, download, attachment, expired.url]) {
      assert.deepEqual(await answer(sendFile(link, PDF)), INVALID, String(link))
    }

    // Signed for uploads, but not to what create_upload_link would write.
    const sign = (id: string | number, value: unknown) =>
      builtinUploadUrl(id as number, value, 60_000)
    for (const link of [
      sign(mcp.id, { mailbox: 'INBOX', uid: 11, part: '2' }),
      sign(mcp.id, { upload: '../../db.sqlite3', filename: 'report.pdf' }),
      sign(mcp.id, { upload, filename: '../report.pdf' }),
      sign(mcp.id, 'report.pdf'),
      sign(`${mcp.id}.5`, { upload, filename: 'report.pdf' }),
      sign(mcp.id + 1000, { upload, filename: 'report.pdf' }),
    ]) {
      assert.deepEqual(await answer(sendFile(link, PDF)), UNAVAILABLE, link)
    }
    assert.deepEqual(await storedFiles(mcp), [])

    // A download link cannot be read back from the upload route either.
    const served = new URL(url)
    served.host = `${env.get('HOST')}:${env.get('PORT')}`
    assert.equal(await status(fetch(served)), 404)

    assert.equal(await status(sendFile(url, PDF)), 201)
  })

  test('follows the MCP: its permissions, whether it is enabled, and its deletion', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const { admin, mcp } = await writingMcp(['draft', 'send'])
      const { url } = uploadLink(mcp)

      mcp.builtinPermissions = 'read organize'
      await mcp.save()
      assert.deepEqual(await answer(sendFile(url, PDF)), UNAVAILABLE)

      mcp.builtinPermissions = 'draft'
      mcp.enabled = false
      await mcp.save()
      assert.deepEqual(await answer(sendFile(url, PDF)), UNAVAILABLE)
      assert.deepEqual(await storedFiles(mcp), [])

      mcp.enabled = true
      await mcp.save()
      assert.equal(await status(sendFile(url, PDF)), 201)
      await uploadFile(mcp.id, 'second.pdf', PDF)
      assert.lengthOf(await storedFiles(mcp), 2)

      // Deleting the MCP deletes what was uploaded for it.
      const deleted = await client
        .delete(`/mcps/${mcp.id}`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
      deleted.assertStatus(302)
      deleted.assertFlashMessage('success', 'MCP deleted')
      await assert.rejects(() => readdir(uploadsDirectory(mcp.id)))
      assert.deepEqual(await answer(sendFile(uploadLink(mcp).url, PDF)), UNAVAILABLE)
      // Nothing here ever signed in to iCloud.
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      icloud.restore()
    }
  })

  test('counts the uploads of each MCP apart, without the ones that have no valid signature', async ({
    assert,
  }) => {
    const { mcp: busy } = await writingMcp()
    const { mcp: other } = await writingMcp()
    const upload = (mcp: Mcp) => sendFile(uploadLink(mcp).url, Buffer.from('x'))

    const forged = new URL(uploadLink(busy).url)
    forged.searchParams.set('signature', 'forged')
    for (let attempt = 0; attempt < 70; attempt++) {
      assert.equal(await status(sendFile(forged, Buffer.from('x'))), 403)
    }

    // An MCP holds 50 files at a time.
    for (let count = 0; count < 50; count++) {
      assert.equal(await status(upload(busy)), 201)
    }
    assert.deepEqual(await answer(upload(busy)), [
      429,
      'Too many uploaded files are waiting for this MCP. They are deleted an hour after their upload: try again later.',
    ])
    await clearUploads()

    // And takes 60 uploads in 15 minutes from one address, kept or not.
    for (let count = 51; count < 60; count++) {
      assert.equal(await status(upload(busy)), 201)
    }
    const refused = await upload(busy)
    assert.equal(refused.status, 429)
    assert.equal(await refused.text(), 'Too many uploads. Try again later.')
    const retryAfter = Number(refused.headers.get('retry-after'))
    assert.isAbove(retryAfter, 0)
    assert.isAtMost(retryAfter, 15 * 60)

    assert.equal(await status(upload(other)), 201)
  }).timeout(60_000)

  test('takes at most three uploads of an MCP at once, and keeps nothing of one that breaks off', async ({
    assert,
  }) => {
    const { mcp: busy } = await writingMcp()
    const { mcp: other } = await writingMcp()

    const open = [1, 2, 3].map(() => {
      const body = openBody()
      const link = uploadLink(busy)
      const client = new AbortController()
      const response = sendFile(link.url, body.stream, {}, client.signal)
      return { ...body, ...link, response, hangUp: () => client.abort() }
    })
    await until(async () => (await storedCount(busy)) === 3)

    const refused = await sendFile(uploadLink(busy).url, PDF)
    assert.equal(refused.status, 429)
    assert.equal(refused.headers.get('retry-after'), '5')
    assert.equal(await refused.text(), 'Too many uploads at once. Try again in a few seconds.')
    // A file that is still arriving cannot be attached.
    assert.isNull(await findBuiltinUpload(busy.id, open[0].upload))
    // Another MCP is served in the meantime.
    assert.equal(await status(sendFile(uploadLink(other).url, PDF)), 201)

    // One client hangs up: nothing is kept of its file, and its place is given back.
    const [abandoned, ...finished] = open
    const hungUp = abandoned.response.catch(() => null)
    abandoned.hangUp()
    assert.isNull(await hungUp)
    await until(async () => (await storedCount(busy)) === 2)
    assert.notInclude(await storedFiles(busy), abandoned.upload)
    assert.equal(await status(sendFile(uploadLink(busy).url, PDF)), 201)

    for (const upload of finished) {
      upload.finish()
      assert.equal(await status(upload.response), 201)
      assert.equal(await uploadedText(busy, upload.upload), 'begin')
    }
    const next = await Promise.all([1, 2, 3].map(() => sendFile(uploadLink(busy).url, PDF)))
    assert.deepEqual(
      next.map((response) => response.status),
      [201, 201, 201]
    )
  })
})
