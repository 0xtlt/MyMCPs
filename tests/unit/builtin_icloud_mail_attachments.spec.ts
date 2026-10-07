import { randomUUID } from 'node:crypto'
import { test } from '@japa/runner'
import { BuiltinToolError } from '#services/builtin/definition'
import { builtinUploadTarget, callBuiltinTool } from '#services/builtin/runtime'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import {
  clearUploads,
  createIcloudMailMcp,
  ICLOUD_MAIL_PERMISSIONS,
  mockIcloudMail,
  uploadFile,
} from '#tests/helpers/icloud_mail'

type ToolResult = Awaited<ReturnType<typeof callBuiltinTool>>

function resultText(result: ToolResult) {
  const [first] = result.content
  return first.type === 'text' ? first.text : ''
}

function resultData(result: ToolResult) {
  return JSON.parse(resultText(result))
}

let sequence = 0

async function icloudMail(permissions: string[] = ICLOUD_MAIL_PERMISSIONS) {
  const admin = await createAdmin()
  sequence += 1
  return createIcloudMailMcp(admin.id, { name: `iCloud Mail files ${sequence}`, permissions })
}

const PDF = '%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n'
const mail = { to: ['dave@example.com'], subject: 'Quote', text: 'Here it is.' }

function notUploaded(id: string) {
  return `No file is uploaded as "${id}". Send the file to the link create_upload_link returned with this upload_id, then try again. An uploaded file can be attached for 60 minutes.`
}

test.group('Built-in iCloud Mail MCP: attachments', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(clearUploads)
  group.each.teardown(clearUploads)
  group.each.teardown(rollbackTestTransaction)

  test('sends uploaded files as attachments, in the message and in its copy', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const quote = await uploadFile(mcp.id, 'Devis été.pdf', PDF, 'application/pdf')
      const notes = await uploadFile(mcp.id, 'notes.txt', 'Call back on Monday.\n')

      const result = resultData(
        await callBuiltinTool(mcp, 'send_message', {
          ...mail,
          // Naming a file twice attaches it once.
          attachments: [quote, notes, quote.toUpperCase()],
        })
      )

      assert.deepEqual(result, {
        sent: true,
        message_id: result.message_id,
        from: 'thomas@icloud.com',
        subject: 'Quote',
        to: ['dave@example.com'],
        attachments: [
          { filename: 'Devis été.pdf', size: PDF.length },
          { filename: 'notes.txt', size: 21 },
        ],
        saved_to: 'Sent Messages',
      })

      const [sent] = icloud.sent
      assert.deepEqual(sent.attachments, [
        { filename: 'Devis été.pdf', contentType: 'application/pdf', content: Buffer.from(PDF) },
        {
          filename: 'notes.txt',
          contentType: undefined,
          content: Buffer.from('Call back on Monday.\n'),
        },
      ])
      // The bytes are handed over: nothing in the mail names a file or a URL to read.
      assert.isTrue(sent.disableFileAccess)
      assert.isTrue(sent.disableUrlAccess)
      for (const attachment of sent.attachments!) {
        assert.notProperty(attachment, 'path')
        assert.notProperty(attachment, 'href')
      }

      const [copy] = icloud.mailbox('Sent Messages').appended
      assert.include(copy.raw, 'Content-Type: multipart/mixed;')
      assert.include(copy.raw, 'Here it is.\r\n')
      assert.include(copy.raw, 'Content-Type: application/pdf;')
      assert.include(copy.raw, `filename*0*=utf-8''Devis%20%C3%A9t%C3%A9.pdf`)
      assert.include(copy.raw, 'Content-Disposition: attachment;')
      assert.include(copy.raw, `${Buffer.from(PDF).toString('base64').slice(0, 60)}`)
      // Without a media type, the extension says what the file is.
      assert.include(copy.raw, 'Content-Type: text/plain; name=notes.txt\r\n')
      assert.include(copy.raw, 'Content-Disposition: attachment; filename=notes.txt\r\n')
    } finally {
      icloud.restore()
    }
  })

  test('keeps an upload for a draft and for the message sent after it', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail(['draft', 'send'])
      const quote = await uploadFile(mcp.id, 'quote.pdf', PDF)

      const draft = resultData(
        await callBuiltinTool(mcp, 'create_draft', { ...mail, attachments: quote })
      )
      assert.deepEqual(draft.attachments, [{ filename: 'quote.pdf', size: PDF.length }])
      assert.equal(draft.saved_to, 'Drafts')
      const [saved] = icloud.mailbox('Drafts').appended
      assert.include(saved.raw, 'Content-Type: application/pdf; name=quote.pdf\r\n')
      assert.lengthOf(icloud.sent, 0)

      const sent = resultData(
        await callBuiltinTool(mcp, 'send_message', { ...mail, attachments: [quote] })
      )
      assert.isTrue(sent.sent)
      assert.equal(icloud.sent[0].attachments![0].content!.toString(), PDF)

      // A message without attachments says nothing about them.
      const plain = resultData(await callBuiltinTool(mcp, 'send_message', mail))
      assert.notProperty(plain, 'attachments')
      assert.deepEqual(icloud.sent[1].attachments, [])
    } finally {
      icloud.restore()
    }
  })

  test('refuses what was not uploaded for this MCP before reaching iCloud', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const other = await icloudMail()
      const elsewhere = await uploadFile(other.id, 'private.pdf', PDF)
      const unknown = randomUUID()
      const text = async (attachments: unknown) =>
        resultText(await callBuiltinTool(mcp, 'send_message', { ...mail, attachments }))

      assert.equal(await text(unknown), notUploaded(unknown))
      // The upload of another MCP, even of the same account, is not one of this MCP.
      assert.equal(await text([elsewhere]), notUploaded(elsewhere))
      assert.equal(
        await text('/etc/passwd'),
        'attachments must be a list of at most 10 upload IDs, as returned by create_upload_link'
      )
      const draft = await callBuiltinTool(mcp, 'create_draft', { ...mail, attachments: unknown })
      assert.isTrue(draft.isError)
      assert.equal(resultText(draft), notUploaded(unknown))

      assert.lengthOf(icloud.signIns, 0)
      assert.lengthOf(icloud.sent, 0)
    } finally {
      icloud.restore()
    }
  })

  test('carries at most 20 MB of files in a message', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const half = Buffer.alloc(10_000_000, 'a')
      const first = await uploadFile(mcp.id, 'first.bin', half)
      const second = await uploadFile(mcp.id, 'second.bin', half)
      const small = await uploadFile(mcp.id, 'small.txt', 'x')

      const refused = await callBuiltinTool(mcp, 'send_message', {
        ...mail,
        attachments: [first, second, small],
      })
      assert.isTrue(refused.isError)
      assert.equal(
        resultText(refused),
        'These attachments take 20.0 MB together, and a message carries at most 20 MB. Attach fewer files, or send them in several messages.'
      )
      assert.lengthOf(icloud.signIns, 0)

      const sent = resultData(
        await callBuiltinTool(mcp, 'send_message', { ...mail, attachments: [first, second] })
      )
      assert.deepEqual(sent.attachments, [
        { filename: 'first.bin', size: 10_000_000 },
        { filename: 'second.bin', size: 10_000_000 },
      ])
      // Encoded for mail, the files fit in the 28,319,744 bytes Apple's server accepts.
      const [copy] = icloud.mailbox('Sent Messages').appended
      assert.isAbove(copy.raw.length, 27_000_000)
      assert.isBelow(copy.raw.length, 28_000_000)
    } finally {
      icloud.restore()
    }
  }).timeout(30_000)

  test('writes at most two messages with attachments at once for an MCP', async ({ assert }) => {
    let deliver = () => {}
    const delivered = new Promise<void>((resolve) => {
      deliver = resolve
    })
    let started = 0
    const icloud = mockIcloudMail({
      smtp: async () => {
        started += 1
        await delivered
        return { rejected: [] }
      },
    })
    try {
      const busy = await icloudMail()
      const other = await icloudMail()
      const file = await uploadFile(busy.id, 'quote.pdf', PDF)
      const elsewhere = await uploadFile(other.id, 'quote.pdf', PDF)
      const send = (mcp: typeof busy, attachments?: string[]) =>
        callBuiltinTool(mcp, 'send_message', { ...mail, attachments })

      const sending = [send(busy, [file]), send(busy, [file])]
      while (started < 2) {
        await new Promise((resolve) => setTimeout(resolve, 5))
      }

      const refused = await send(busy, [file])
      assert.isTrue(refused.isError)
      assert.equal(
        resultText(refused),
        'Too many messages with attachments are being written at once. Try again in a few seconds.'
      )
      const draft = await callBuiltinTool(busy, 'create_draft', { ...mail, attachments: file })
      assert.isTrue(draft.isError)
      // Messages without attachments, and those of another MCP, are not held back.
      const waiting = [send(busy), send(other, [elsewhere])]
      while (started < 4) {
        await new Promise((resolve) => setTimeout(resolve, 5))
      }

      deliver()
      for (const result of await Promise.all([...sending, ...waiting])) {
        assert.isUndefined(result.isError)
      }
      // Both places were given back, also by a message that failed.
      const failed = await send(busy, [file, randomUUID()])
      assert.isTrue(failed.isError)
      const next = await Promise.all([send(busy, [file]), send(busy, [file])])
      assert.deepEqual(
        next.map((result) => result.isError),
        [undefined, undefined]
      )
    } finally {
      icloud.restore()
    }
  })

  test('takes a file only for an MCP that may still write messages', async ({ assert }) => {
    const reference = {
      upload: randomUUID(),
      filename: 'quote.pdf',
      content_type: 'application/pdf',
    }
    const refusal = async (mcp: Awaited<ReturnType<typeof icloudMail>>, link: unknown) => {
      try {
        await builtinUploadTarget(mcp, link)
        return null
      } catch (error) {
        if (!(error instanceof BuiltinToolError)) throw error
        return error.message
      }
    }

    for (const permissions of [['draft'], ['send'], ICLOUD_MAIL_PERMISSIONS]) {
      assert.deepEqual(await builtinUploadTarget(await icloudMail(permissions), reference), {
        id: reference.upload,
        filename: 'quote.pdf',
        contentType: 'application/pdf',
        maxBytes: 20_000_000,
      })
    }
    assert.equal(
      await refusal(await icloudMail(['read', 'organize']), reference),
      'Neither the "draft" nor the "send" permission is allowed for this MCP any more'
    )

    const mcp = await icloudMail()
    // A download link refers to a message: it is not a place to store a file.
    assert.isString(await refusal(mcp, { mailbox: 'INBOX', uid: 11, part: '2' }))
    assert.isString(await refusal(mcp, { ...reference, upload: '../../db.sqlite3' }))
    assert.isString(await refusal(mcp, { ...reference, filename: '../quote.pdf' }))
    assert.isString(await refusal(mcp, null))
  })
})
