import { test } from '@japa/runner'
import type { VineValidator } from '@vinejs/vine'
import { BuiltinToolError } from '#services/builtin/definition'
import { isAddress } from '#services/builtin/icloud_mail/message'
import { callBuiltinTool, downloadBuiltinFile } from '#services/builtin/runtime'
import { toolInput } from '#services/builtin/tool_input'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import { createIcloudMailMcp, mockIcloudMail } from '#tests/helpers/icloud_mail'
import {
  attachmentReferenceValidator,
  compositionValidator,
  createUploadLinkValidator,
  getAttachmentLinkValidator,
  getMessageValidator,
  listMessagesValidator,
  markMessagesValidator,
  moveMessagesValidator,
  uploadReferenceValidator,
} from '#validators/builtin_icloud_mail'

type Validator = VineValidator<any, any>
type Args = Record<string, unknown>

/** The account the composition tools run for. */
const account = { username: 'thomas@icloud.com', aliases: ['Hello@Thomas.example'] }
const mail = { subject: 'Hi', text: 'Hello' }

const ADDRESSES =
  'must be a list of at most 50 email addresses such as name@example.com, without display names'
const ATTACHMENTS =
  'attachments must be a list of at most 10 upload IDs, as returned by create_upload_link'
const UPLOAD_ID = '3f2b8c1e-7a4d-4e9b-9c55-0a1b2c3d4e5f'

/** The sentence the agent reads when a tool refuses its arguments. */
async function refusal(validator: Validator, args: unknown) {
  try {
    await toolInput(validator, args, account)
    return null
  } catch (error) {
    if (!(error instanceof BuiltinToolError)) throw error
    return error.message
  }
}

function resultText(result: Awaited<ReturnType<typeof callBuiltinTool>>) {
  const [first] = result.content
  return first.type === 'text' ? first.text : ''
}

test.group('Built-in iCloud Mail MCP: validators', () => {
  test('returns the arguments of each tool the way the tool uses them', async ({ assert }) => {
    const cases: Array<[Validator, Args, Args]> = [
      [listMessagesValidator, {}, {}],
      [
        listMessagesValidator,
        { mailbox: ' Archive ', page: '2', per_page: 50, from: ' alice ', subject: '', other: 1 },
        { mailbox: 'Archive', page: 2, per_page: 50, from: 'alice' },
      ],
      [
        listMessagesValidator,
        { unread: 'true', flagged: false, to: '   ', text: null },
        { unread: true, flagged: false },
      ],
      [getMessageValidator, { uid: '11', max_chars: 500 }, { uid: 11, max_chars: 500 }],
      [
        getAttachmentLinkValidator,
        { mailbox: 'INBOX', uid: 11, part: 2, expires_in_minutes: '60' },
        { mailbox: 'INBOX', uid: 11, part: '2', expires_in_minutes: 60 },
      ],
      [
        attachmentReferenceValidator,
        { mailbox: 'INBOX', uid: 11, part: '1.2', expires: 1 },
        { mailbox: 'INBOX', uid: 11, part: '1.2' },
      ],
      [
        markMessagesValidator,
        { uids: [11, '12', ' 14 ', 11], unread: false },
        { uids: [11, 12, 14, 11], unread: false },
      ],
      [
        moveMessagesValidator,
        { uids: [11], destination: ' Archive ' },
        { uids: [11], destination: 'Archive' },
      ],
    ]

    for (const [validator, args, expected] of cases) {
      assert.deepEqual(await toolInput(validator, args), expected, JSON.stringify(args))
    }

    const { since, before } = await toolInput(listMessagesValidator, {
      since: '2026-10-01',
      before: '2026-10-02T00:00:00+02:00',
    })
    assert.equal(since?.toJSDate().toISOString(), '2026-10-01T00:00:00.000Z')
    assert.equal(before?.toJSDate().toISOString(), '2026-10-01T22:00:00.000Z')
  })

  test('tells the agent which argument is wrong, and what it must be', async ({ assert }) => {
    const cases: Array<[Validator, unknown, string]> = [
      [listMessagesValidator, { mailbox: 5 }, 'mailbox must be text of at most 255 characters'],
      [listMessagesValidator, { mailbox: 'a\r\nb' }, 'mailbox must be a single line of text'],
      [listMessagesValidator, { page: 0 }, 'page must be an integer of at least 1'],
      [listMessagesValidator, { per_page: 51 }, 'per_page must be an integer between 1 and 50'],
      [
        listMessagesValidator,
        { from: 'x'.repeat(201) },
        'from must be text of at most 200 characters',
      ],
      [listMessagesValidator, { subject: 'a\nb' }, 'subject must be a single line of text'],
      [
        listMessagesValidator,
        { since: 'yesterday' },
        'since must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z',
      ],
      [listMessagesValidator, { unread: 'yes' }, 'unread must be true or false'],
      [getMessageValidator, {}, 'uid is required'],
      [getMessageValidator, { uid: 0 }, 'uid must be an integer between 1 and 4294967295'],
      [
        getMessageValidator,
        { uid: 4_294_967_296 },
        'uid must be an integer between 1 and 4294967295',
      ],
      [
        getMessageValidator,
        { uid: 1, max_chars: 499 },
        'max_chars must be an integer between 500 and 100000',
      ],
      [getAttachmentLinkValidator, { uid: 1 }, 'part is required'],
      [
        getAttachmentLinkValidator,
        { uid: 1, part: '2/../3' },
        'part must be the part of an attachment, such as 2, as returned by get_message',
      ],
      [
        getAttachmentLinkValidator,
        { uid: 1, part: '2', expires_in_minutes: 61 },
        'expires_in_minutes must be an integer between 1 and 60',
      ],
      [markMessagesValidator, { uids: [1] }, 'Set unread, flagged, or both'],
      [
        markMessagesValidator,
        { uids: [1], unread: '', flagged: null },
        'Set unread, flagged, or both',
      ],
      [markMessagesValidator, { uids: [1], unread: 'no' }, 'unread must be true or false'],
      [markMessagesValidator, { uids: [1], flagged: 0 }, 'flagged must be true or false'],
      [moveMessagesValidator, { uids: [1] }, 'destination is required'],
      [moveMessagesValidator, { uids: [1], destination: '  ' }, 'destination is required'],
      [
        moveMessagesValidator,
        { uids: [1], destination: 'a\nb' },
        'destination must be a single line of text',
      ],
      [attachmentReferenceValidator, { mailbox: 'INBOX', part: '2' }, 'uid is required'],
      [attachmentReferenceValidator, null, 'arguments is required'],
      [attachmentReferenceValidator, 'INBOX/11/2', 'arguments must be an object'],
    ]

    for (const [validator, args, sentence] of cases) {
      assert.equal(await refusal(validator, args), sentence, JSON.stringify(args))
    }
  })

  test('takes 1 to 100 message UIDs, and says which of the two is wrong', async ({ assert }) => {
    const list = 'uids must be a list of 1 to 100 message UIDs'
    const hundred = Array.from({ length: 100 }, (_, index) => index + 1)

    assert.deepEqual(await toolInput(markMessagesValidator, { uids: hundred, flagged: true }), {
      uids: hundred,
      flagged: true,
    })
    for (const uids of [undefined, null, '', 11, '11', {}, [], [...hundred, 101]]) {
      assert.equal(await refusal(markMessagesValidator, { uids }), list, JSON.stringify(uids))
    }
    assert.equal(
      await refusal(markMessagesValidator, { uids: [11, 0] }),
      'uids must be an integer between 1 and 4294967295'
    )
    assert.equal(await refusal(markMessagesValidator, { uids: [11, null] }), 'uids is required')
    assert.equal(await refusal(markMessagesValidator, { uids: [''] }), 'uids is required')
  })
})

test.group('Built-in iCloud Mail MCP: validators of an upload', () => {
  test('names the file the way the recipient sees it', async ({ assert }) => {
    assert.deepEqual(
      await toolInput(createUploadLinkValidator, {
        filename: ' Devis été 2026.pdf ',
        content_type: ' application/pdf ',
        expires_in_minutes: '30',
        path: '/etc/passwd',
      }),
      { filename: 'Devis été 2026.pdf', content_type: 'application/pdf', expires_in_minutes: 30 }
    )
    assert.deepEqual(
      await toolInput(createUploadLinkValidator, { filename: 'notes', content_type: '' }),
      { filename: 'notes' }
    )

    const folder = 'filename must be the name of the file, such as report.pdf, without its folder'
    const mediaType = 'content_type must be a media type, such as application/pdf'
    const cases: Array<[Args, string]> = [
      [{}, 'filename is required'],
      [{ filename: '   ' }, 'filename is required'],
      [{ filename: '/tmp/report.pdf' }, folder],
      [{ filename: '..\\report.pdf' }, folder],
      [{ filename: 'a\r\nContent-Type: text/html' }, 'filename must be a single line of text'],
      [{ filename: `${'n'.repeat(252)}.pdf` }, 'filename must be text of at most 255 characters'],
      [{ filename: 42 }, 'filename must be text of at most 255 characters'],
      [{ filename: 'a.pdf', content_type: 'pdf' }, mediaType],
      [{ filename: 'a.pdf', content_type: 'text/html; charset=utf-8' }, mediaType],
      [{ filename: 'a.pdf', content_type: 'text/plain\r\nBcc: eve@example.com' }, mediaType],
      [{ filename: 'a.pdf', content_type: `application/${'x'.repeat(101)}` }, mediaType],
      [
        { filename: 'a.pdf', expires_in_minutes: 61 },
        'expires_in_minutes must be an integer between 1 and 60',
      ],
    ]
    for (const [args, sentence] of cases) {
      assert.equal(await refusal(createUploadLinkValidator, args), sentence, JSON.stringify(args))
    }
  })

  test('reads back what a link was made for, and nothing else', async ({ assert }) => {
    assert.deepEqual(
      await toolInput(uploadReferenceValidator, {
        upload: UPLOAD_ID,
        filename: 'report.pdf',
        content_type: 'application/pdf',
        mailbox: 'INBOX',
      }),
      { upload: UPLOAD_ID, filename: 'report.pdf', content_type: 'application/pdf' }
    )

    const references: unknown[] = [
      null,
      UPLOAD_ID,
      { upload: UPLOAD_ID },
      { filename: 'report.pdf' },
      { upload: '../../db.sqlite3', filename: 'report.pdf' },
      { upload: `${UPLOAD_ID}/..`, filename: 'report.pdf' },
      { upload: UPLOAD_ID.replaceAll('-', ''), filename: 'report.pdf' },
      { upload: UPLOAD_ID, filename: '../report.pdf' },
      { upload: UPLOAD_ID, filename: 'report.pdf', content_type: 'pdf' },
      // What get_attachment_link puts in a download link.
      { mailbox: 'INBOX', uid: 11, part: '2' },
    ]
    for (const reference of references) {
      assert.isString(await refusal(uploadReferenceValidator, reference), JSON.stringify(reference))
    }
  })
})

test.group('Built-in iCloud Mail MCP: validators of a message to write', () => {
  const compose = (args: Args) => toolInput(compositionValidator, args, account)

  test('takes one upload ID or a list of at most ten', async ({ assert }) => {
    const ids = Array.from({ length: 10 }, (_, index) => UPLOAD_ID.replace(/.$/, String(index)))

    assert.deepEqual(await compose({ ...mail, attachments: ` ${UPLOAD_ID.toUpperCase()} ` }), {
      ...mail,
      attachments: [UPLOAD_ID],
    })
    assert.deepEqual(await compose({ ...mail, attachments: ids }), { ...mail, attachments: ids })
    assert.deepEqual(await compose({ ...mail, attachments: [] }), { ...mail, attachments: [] })
    for (const none of [undefined, null, '']) {
      assert.deepEqual(await compose({ ...mail, attachments: none }), mail)
    }

    const refused: unknown[] = [
      'report.pdf',
      '/tmp/report.pdf',
      'https://example.com/report.pdf',
      [UPLOAD_ID, '../../db.sqlite3'],
      [UPLOAD_ID, null],
      [{ path: '/etc/passwd' }],
      [{ filename: 'a.txt', content: 'aGk=' }],
      42,
      [...ids, UPLOAD_ID],
    ]
    for (const attachments of refused) {
      assert.equal(
        await refusal(compositionValidator, { ...mail, attachments }),
        ATTACHMENTS,
        JSON.stringify(attachments)
      )
    }
  })

  test('takes one address or a list, trimmed, and nothing but bare addresses', async ({
    assert,
  }) => {
    assert.deepEqual(await compose({ ...mail, to: ' bob@example.com ', cc: [], bcc: '' }), {
      ...mail,
      to: ['bob@example.com'],
      cc: [],
    })
    assert.deepEqual(
      await compose({ ...mail, to: ['bob@example.com', ' Bob@Example.com '], bcc: null }),
      { ...mail, to: ['bob@example.com', 'Bob@Example.com'] }
    )

    const fifty = Array.from({ length: 50 }, (_, index) => `user${index}@example.com`)
    const { bcc } = await compose({ ...mail, bcc: fifty })
    assert.lengthOf(bcc!, 50)

    for (const name of ['to', 'cc', 'bcc']) {
      for (const value of [
        'Bob <bob@example.com>',
        'bob@example.com, carol@example.com',
        ['bob@example.com', 'carol'],
        ['bob@example.com', null],
        ['bob@example.com', ['carol@example.com']],
        [''],
        5,
        {},
        [...fifty, 'one@more.example'],
      ]) {
        assert.equal(
          await refusal(compositionValidator, { ...mail, [name]: value }),
          `${name} ${ADDRESSES}`,
          JSON.stringify(value)
        )
      }
    }
  })

  test('checks an address with the pattern that checks the addresses of a message', async ({
    assert,
  }) => {
    const addresses = [
      'bob@example.com',
      'bob+tag@sub.example.co',
      'bob@example',
      'bob@.example.com',
      'bob@example..com',
      'bob@exa mple.com',
      '"bob"@example.com',
      'bob@[127.0.0.1]',
      'bob@example.com.',
      `${'b'.repeat(242)}@example.com`,
      `${'b'.repeat(243)}@example.com`,
    ]

    for (const address of addresses) {
      assert.equal(
        (await refusal(compositionValidator, { ...mail, to: [address] })) === null,
        isAddress(address),
        address
      )
    }
    assert.isAbove(addresses.filter(isAddress).length, 2)
    assert.isBelow(addresses.filter(isAddress).length, addresses.length - 2)
  })

  test('sends from an address of the account only, in the spelling that was saved', async ({
    assert,
  }) => {
    const sender = async (from: string) => {
      const input = await compose({ ...mail, from })
      return input.from
    }
    assert.equal(await sender(' THOMAS@icloud.com '), 'thomas@icloud.com')
    assert.equal(await sender('hello@thomas.example'), 'Hello@Thomas.example')
    assert.notProperty(await compose({ ...mail, from: '  ' }), 'from')

    assert.equal(
      await refusal(compositionValidator, { ...mail, from: 'boss@example.com' }),
      'from must be one of the sender addresses allowed for this MCP: thomas@icloud.com, Hello@Thomas.example'
    )
    assert.equal(
      await refusal(compositionValidator, { ...mail, from: 'thomas@icloud.com\nBcc: x@y.z' }),
      'from must be a single line of text'
    )
    assert.equal(
      await refusal(compositionValidator, { ...mail, from: 'x'.repeat(255) }),
      'from must be text of at most 254 characters'
    )
  })

  test('requires a subject unless the message answers another', async ({ assert }) => {
    const sentence = 'subject is required unless reply_to_uid is set'

    assert.equal(await refusal(compositionValidator, { text: 'Hello' }), sentence)
    assert.equal(await refusal(compositionValidator, { text: 'Hello', subject: '  ' }), sentence)
    assert.equal(
      await refusal(compositionValidator, { text: 'Hello', subject: null, reply_to_uid: '' }),
      sentence
    )
    assert.deepEqual(await compose({ text: 'Yes', reply_to_uid: '11' }), {
      reply_to_uid: 11,
      text: 'Yes',
    })
    assert.equal(
      await refusal(compositionValidator, { ...mail, subject: 'Re: a\nBcc: x@y.z' }),
      'subject must be a single line of text'
    )
    assert.equal(
      await refusal(compositionValidator, { ...mail, subject: 'x'.repeat(256) }),
      'subject must be text of at most 255 characters'
    )
  })

  test('requires a text, of which spaces alone are one', async ({ assert }) => {
    assert.equal(await refusal(compositionValidator, { subject: 'Hi' }), 'text is required')
    assert.equal(
      await refusal(compositionValidator, { subject: 'Hi', text: '' }),
      'text is required'
    )
    assert.deepEqual(await compose({ subject: 'Hi', text: ' \n' }), { subject: 'Hi', text: ' \n' })
    assert.equal(
      await refusal(compositionValidator, { subject: 'Hi', text: 'x'.repeat(100_001) }),
      'text must be text of at most 100000 characters'
    )
  })

  test('only looks at what describes the reply when there is a message to answer', async ({
    assert,
  }) => {
    assert.deepEqual(await compose({ ...mail, reply_to_mailbox: 5, reply_all: 'maybe' }), mail)
    assert.deepEqual(
      await compose({
        text: 'Yes',
        reply_to_uid: 5,
        reply_to_mailbox: ' Sent ',
        reply_all: 'true',
      }),
      { reply_to_uid: 5, text: 'Yes', reply_to_mailbox: 'Sent', reply_all: true }
    )
    assert.equal(
      await refusal(compositionValidator, { text: 'Yes', reply_to_uid: 5, reply_all: 'maybe' }),
      'reply_all must be true or false'
    )
    assert.equal(
      await refusal(compositionValidator, { text: 'Yes', reply_to_uid: 5, reply_to_mailbox: 5 }),
      'reply_to_mailbox must be text of at most 255 characters'
    )
    assert.equal(
      await refusal(compositionValidator, { ...mail, reply_to_uid: 0 }),
      'reply_to_uid must be an integer between 1 and 4294967295'
    )
  })
})

test.group('Built-in iCloud Mail MCP: validated tools', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('names a UID or an address once, however often the agent names it', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id, { permissions: ['draft', 'organize'] })

      const marked = await callBuiltinTool(mcp, 'mark_messages', {
        uids: [11, '11', 12, ' 12 '],
        flagged: true,
      })
      assert.isUndefined(marked.isError)
      assert.deepEqual(icloud.searches, [{ uid: '11,12' }])

      const draft = await callBuiltinTool(mcp, 'create_draft', {
        subject: 'Hi',
        text: 'Hello',
        to: ['bob@example.com', 'BOB@example.com'],
        cc: ['Bob@example.com', 'carol@example.com'],
      })
      const saved = JSON.parse(resultText(draft))
      assert.deepEqual(saved.to, ['bob@example.com'])
      assert.deepEqual(saved.cc, ['carol@example.com'])
    } finally {
      icloud.restore()
    }
  })

  test('refuses to answer a message without the read permission, once the arguments are right', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id, { permissions: ['draft'] })
      const answer = (args: Args) => callBuiltinTool(mcp, 'create_draft', args).then(resultText)

      assert.include(
        await answer({ reply_to_uid: 11, text: 'Yes' }),
        'reply_to_uid reads the message being answered, and the "read" permission is not allowed'
      )
      assert.equal(await answer({ reply_to_uid: 11 }), 'text is required')
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      icloud.restore()
    }
  })

  test('serves a file only for a reference a link could hold', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id)

      const file = await downloadBuiltinFile(mcp, { mailbox: 'INBOX', uid: '11', part: 2 })
      assert.equal(file.filename, 'Menu été.pdf')

      for (const reference of [
        undefined,
        null,
        'INBOX',
        [],
        { uid: 11 },
        { uid: 'x', part: '2' },
      ]) {
        const error = await downloadBuiltinFile(mcp, reference).catch((reason) => reason)
        assert.instanceOf(error, BuiltinToolError, JSON.stringify(reference))
      }
      assert.lengthOf(icloud.signIns, 1)
    } finally {
      icloud.restore()
    }
  })
})
