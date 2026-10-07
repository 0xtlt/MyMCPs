import { test } from '@japa/runner'
import type { MessageStructureObject } from 'imapflow'
import { htmlToText } from '#services/builtin/icloud_mail/html'
import { attachmentsOf, bodyPart, replyTo, tidyText } from '#services/builtin/icloud_mail/message'
import { builtinMcp } from '#services/builtin/registry'
import { builtinWriteGranted, callBuiltinTool, listBuiltinTools } from '#services/builtin/runtime'
import { sanitizeMcpDiagnostic } from '#services/security_redaction'
import { testAndUpdateStatus } from '#services/upstream/manager'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import {
  createIcloudMailMcp,
  ICLOUD_MAIL_PERMISSIONS,
  icloudMailSignIn,
  mockIcloudMail,
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

async function icloudMail(permissions: string[] = ['read']) {
  const admin = await createAdmin()
  sequence += 1
  return createIcloudMailMcp(admin.id, { name: `iCloud Mail ${sequence}`, permissions })
}

const fullAccess = () => icloudMail(ICLOUD_MAIL_PERMISSIONS)

test.group('Built-in iCloud Mail MCP: messages', () => {
  const plain: MessageStructureObject = { part: '1.1', type: 'text/plain', size: 10 }
  const html: MessageStructureObject = { part: '1.2', type: 'text/html', size: 40 }
  const pdf: MessageStructureObject = {
    part: '2',
    type: 'application/pdf',
    encoding: 'base64',
    size: 7800,
    disposition: 'attachment',
    dispositionParameters: { filename: 'menu.pdf' },
  }

  test('reads plain text before HTML and never an attached file', ({ assert }) => {
    const alternative: MessageStructureObject = {
      part: '1',
      type: 'multipart/alternative',
      childNodes: [html, plain],
    }
    const notes: MessageStructureObject = {
      part: '3',
      type: 'text/plain',
      disposition: 'attachment',
      dispositionParameters: { filename: 'notes.txt' },
    }

    assert.deepEqual(bodyPart({ type: 'multipart/mixed', childNodes: [notes, alternative, pdf] }), {
      id: '1.1',
      isHtml: false,
    })
    assert.deepEqual(bodyPart({ type: 'multipart/mixed', childNodes: [html, pdf] }), {
      id: '1.2',
      isHtml: true,
    })
    assert.isNull(bodyPart({ type: 'multipart/mixed', childNodes: [notes, pdf] }))
    // A message made of one part has no part number.
    assert.deepEqual(bodyPart({ type: 'text/html' }), { id: '1', isHtml: true })
  })

  test('lists named parts as attachments and keeps a forwarded message whole', ({ assert }) => {
    const forwarded: MessageStructureObject = {
      part: '3',
      type: 'message/rfc822',
      size: 9000,
      parameters: { name: 'Fwd.eml' },
      childNodes: [{ part: '3.1', type: 'text/plain', size: 500 }],
    }
    const photo: MessageStructureObject = {
      part: '4',
      type: 'image/jpeg',
      size: 300000,
      disposition: 'inline',
      dispositionParameters: { filename: 'IMG_0042.jpeg' },
    }
    const structure: MessageStructureObject = {
      type: 'multipart/mixed',
      childNodes: [plain, pdf, forwarded, photo],
    }

    assert.deepEqual(attachmentsOf(structure), [
      // Base64 makes a file about a third larger in transit.
      { part: '2', filename: 'menu.pdf', content_type: 'application/pdf', size: 5700 },
      { part: '3', filename: 'Fwd.eml', content_type: 'message/rfc822', size: 9000 },
      { part: '4', filename: 'IMG_0042.jpeg', content_type: 'image/jpeg', size: 300000 },
    ])
    assert.deepEqual(bodyPart(structure), { id: '1.1', isHtml: false })
    assert.deepEqual(attachmentsOf(undefined), [])
    // A message that is nothing but a file has no part number.
    assert.deepEqual(
      attachmentsOf({ type: 'application/zip', size: 10, parameters: { name: 'a.zip' } }),
      [{ part: '1', filename: 'a.zip', content_type: 'application/zip', size: 10 }]
    )
  })

  test('turns HTML into text without styles, images, or preview padding', async ({ assert }) => {
    const converted = await htmlToText(
      [
        '<html><head><style>p { color: red }</style></head><body>',
        '<p>Hello‌ ‌ ‌ ‌ </p>',
        '<img src="https://shop.example/pixel.gif" alt="tracking">',
        '<p><a href="https://shop.example/deals">See the deals</a></p>',
        '<p><a href="https://shop.example">https://shop.example</a></p>',
        '<p>This paragraph is long enough that a wrapping converter would break it across several lines of output.</p>',
        '</body></html>',
      ].join(''),
      1000
    )

    assert.isFalse(converted!.isTruncated)
    assert.equal(
      tidyText(converted!.text),
      [
        'Hello',
        'See the deals [https://shop.example/deals]',
        'https://shop.example',
        'This paragraph is long enough that a wrapping converter would break it across several lines of output.',
      ].join('\n\n')
    )
    assert.equal(tidyText('One  \r\n\r\n\r\n\r\nTwo\r\n'), 'One\n\nTwo')
  }).timeout(10_000)

  test('addresses a reply to the author, and to everyone else only when asked', ({ assert }) => {
    const original = {
      seq: 1,
      uid: 11,
      headers: Buffer.from('References: <root@example.com>\r\n <second@example.com>\r\n\r\n'),
      envelope: {
        subject: 'Lunch on Thursday?',
        messageId: '<lunch@example.com>',
        from: [{ name: 'Alice', address: 'alice@example.com' }],
        to: [{ address: 'Thomas@iCloud.com' }, { address: 'bob@example.com' }],
        cc: [{ address: 'carol@example.com' }, { address: 'BOB@example.com' }],
      },
    }

    assert.deepEqual(replyTo(original, ['thomas@icloud.com'], false), {
      from: 'thomas@icloud.com',
      to: ['alice@example.com'],
      cc: [],
      subject: 'Re: Lunch on Thursday?',
      inReplyTo: '<lunch@example.com>',
      references: ['<root@example.com>', '<second@example.com>', '<lunch@example.com>'],
    })
    assert.deepEqual(replyTo(original, ['thomas@icloud.com'], true).cc, [
      'bob@example.com',
      'carol@example.com',
    ])

    // Bob's address belongs to the account too: answer from it, and do not copy it.
    const asBob = replyTo(original, ['hello@thomas.example', 'bob@example.com'], true)
    assert.equal(asBob.from, 'bob@example.com')
    assert.deepEqual(asBob.cc, ['Thomas@iCloud.com', 'carol@example.com'])
  })

  test('follows Reply-To, continues a sent message, and ignores unusable addresses', ({
    assert,
  }) => {
    const list = replyTo(
      {
        seq: 1,
        uid: 1,
        envelope: {
          subject: 'RE: Minutes',
          from: [{ address: 'alice@example.com' }],
          replyTo: [{ address: 'team@example.com' }, { address: 'not an address' }],
          to: [{ address: 'thomas@icloud.com' }],
        },
      },
      ['hello@thomas.example'],
      false
    )
    assert.deepEqual(list.to, ['team@example.com'])
    // Sent to none of the known addresses: the caller picks the sender.
    assert.isUndefined(list.from)
    assert.equal(list.subject, 'RE: Minutes')
    assert.isUndefined(list.inReplyTo)
    assert.deepEqual(list.references, [])

    const followUp = replyTo(
      {
        seq: 1,
        uid: 5,
        envelope: {
          subject: 'Quote',
          messageId: '<quote@icloud.com>',
          from: [{ address: 'Hello@Thomas.example' }],
          to: [{ address: 'dave@example.com' }, { address: 'Dave <dave@example.com>, eve@x.io' }],
        },
      },
      ['thomas@icloud.com', 'hello@thomas.example'],
      false
    )
    assert.deepEqual(followUp.to, ['dave@example.com'])
    assert.equal(followUp.from, 'hello@thomas.example')
  })
})

test.group('Built-in iCloud Mail MCP: permissions', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('signs in with a password and lets the admin choose among four permissions', ({
    assert,
  }) => {
    const definition = builtinMcp('icloud-mail')!
    assert.equal(definition.name, 'iCloud Mail')
    assert.isUndefined(definition.oauth)
    assert.deepEqual(definition.password?.permissions, ICLOUD_MAIL_PERMISSIONS)
    assert.isTrue(definition.password!.passwordPattern.test('abcd-efgh-ijkl-mnop'))
    assert.isTrue(definition.password!.passwordPattern.test('abcdefghijklmnop'))
    assert.isFalse(definition.password!.passwordPattern.test('Tr0ub4dor&3-horse'))
    assert.deepEqual(
      [...new Set(definition.tools.flatMap((tool) => tool.requiresAnyScope ?? ['none']))],
      ICLOUD_MAIL_PERMISSIONS
    )
  })

  test('exposes only the tools of the allowed permissions, without signing in', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const names = async (permissions: string[]) =>
        listBuiltinTools(await icloudMail(permissions)).map((tool) => tool.name)

      assert.deepEqual(await names(['read']), [
        'list_mailboxes',
        'list_messages',
        'get_message',
        'get_attachment_link',
      ])
      // Either permission that writes a message can attach a file to it.
      assert.deepEqual(await names(['draft']), ['create_upload_link', 'create_draft'])
      assert.deepEqual(await names(['send']), ['create_upload_link', 'send_message'])
      assert.deepEqual(await names(['organize']), ['mark_messages', 'move_messages'])
      assert.lengthOf(await names(ICLOUD_MAIL_PERMISSIONS), 9)
      assert.deepEqual(await names([]), [])
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      icloud.restore()
    }
  })

  test('refuses a tool outside the allowed permissions before reaching iCloud', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const readOnly = await icloudMail(['read'])
      const refused = await callBuiltinTool(readOnly, 'send_message', {
        to: ['dave@example.com'],
        subject: 'Hello',
        text: 'Hi',
      })

      assert.isTrue(refused.isError)
      assert.equal(
        resultText(refused),
        'send_message needs the "send" permission, which is not allowed for this iCloud Mail MCP. An administrator can allow it from the MCPs page in MyMCPs.'
      )
      const unreadable = await callBuiltinTool(await icloudMail(['send']), 'list_messages', {})
      assert.isTrue(unreadable.isError)
      assert.lengthOf(icloud.signIns, 0)
      assert.lengthOf(icloud.sent, 0)
      // Nothing to re-authorize: the permissions are enforced by MyMCPs.
      assert.isTrue(builtinWriteGranted(readOnly))
    } finally {
      icloud.restore()
    }
  })

  test('does not let a reply read a message without the read permission', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const sendOnly = await icloudMail(['send', 'draft'])
      for (const tool of ['send_message', 'create_draft']) {
        const refused = await callBuiltinTool(sendOnly, tool, { reply_to_uid: 11, text: 'Yes' })
        assert.isTrue(refused.isError)
        assert.include(resultText(refused), 'the "read" permission is not allowed for this MCP')
      }
      assert.lengthOf(icloud.signIns, 0)

      const sent = await callBuiltinTool(sendOnly, 'send_message', {
        to: ['dave@example.com'],
        subject: 'Hello',
        text: 'Hi',
      })
      assert.isUndefined(sent.isError)
      assert.lengthOf(icloud.sent, 1)
    } finally {
      icloud.restore()
    }
  })
})

test.group('Built-in iCloud Mail MCP: reading', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('lists selectable mailboxes with their role and counts', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const result = await callBuiltinTool(await icloudMail(), 'list_mailboxes', {})

      assert.deepEqual(resultData(result), [
        { path: 'INBOX', role: 'inbox', messages: 3, unread: 1 },
        { path: 'Sent Messages', role: 'sent', messages: 1, unread: 0 },
        { path: 'Drafts', role: 'drafts', messages: 0, unread: 0 },
        { path: 'Deleted Messages', role: 'trash', messages: 0, unread: 0 },
        { path: 'Archive', role: 'archive', messages: 1, unread: 0 },
      ])
      assert.deepEqual(icloud.signIns, [icloudMailSignIn])
      assert.equal(icloud.logouts, 1)
    } finally {
      icloud.restore()
    }
  })

  test('lists the newest messages first without searching the mailbox', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const result = resultData(await callBuiltinTool(mcp, 'list_messages', {}))

      assert.deepEqual(result, {
        mailbox: 'INBOX',
        total: 3,
        page: 1,
        per_page: 20,
        messages: [
          {
            uid: 14,
            subject: 'Re: Invoice 42',
            from: 'andre@example.com',
            to: ['thomas@icloud.com'],
            date: '2026-10-03T15:45:00.000Z',
            unread: false,
            flagged: true,
            answered: false,
            attachments: 0,
          },
          {
            uid: 12,
            subject: 'Autumn sale',
            from: 'Shop <news@shop.example>',
            to: ['thomas@icloud.com'],
            date: '2026-10-02T06:00:00.000Z',
            unread: false,
            flagged: false,
            answered: false,
            attachments: 0,
          },
          {
            uid: 11,
            subject: 'Lunch on Thursday?',
            from: 'Alice Martin <alice@example.com>',
            to: ['Thomas <thomas@icloud.com>', 'bob@example.com'],
            date: '2026-10-01T09:30:00.000Z',
            unread: true,
            flagged: false,
            answered: false,
            attachments: 1,
          },
        ],
      })
      assert.lengthOf(icloud.searches, 0)
      assert.deepEqual(icloud.fetches, [{ range: '1:3', byUid: false }])
      assert.deepEqual(icloud.locks, [{ path: 'INBOX', readOnly: true }])

      const second = resultData(
        await callBuiltinTool(mcp, 'list_messages', { page: 2, per_page: 2 })
      )
      assert.deepEqual(
        second.messages.map((message: { uid: number }) => message.uid),
        [11]
      )
      assert.deepEqual(icloud.fetches[1], { range: '1:1', byUid: false })

      const beyond = resultData(await callBuiltinTool(mcp, 'list_messages', { page: 3 }))
      assert.deepEqual(beyond.messages, [])
      assert.equal(beyond.total, 3)
      assert.lengthOf(icloud.fetches, 2)
    } finally {
      icloud.restore()
    }
  })

  test('searches with every filter combined and pages through the matches', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const result = resultData(
        await callBuiltinTool(mcp, 'list_messages', {
          from: ' alice ',
          to: 'bob',
          subject: 'lunch',
          text: 'thursday',
          since: '2026-09-30',
          before: '2026-10-04T00:00:00Z',
          unread: true,
          flagged: 'false',
        })
      )

      assert.deepEqual(icloud.searches, [
        {
          from: 'alice',
          to: 'bob',
          subject: 'lunch',
          text: 'thursday',
          since: new Date('2026-09-30T00:00:00Z'),
          before: new Date('2026-10-04T00:00:00Z'),
          seen: false,
          flagged: false,
        },
      ])
      assert.equal(result.total, 1)
      assert.deepEqual(
        result.messages.map((message: { uid: number }) => message.uid),
        [11]
      )
      assert.deepEqual(icloud.fetches, [{ range: [11], byUid: true }])

      const read = resultData(
        await callBuiltinTool(mcp, 'list_messages', { unread: false, per_page: 1 })
      )
      assert.equal(read.total, 2)
      assert.deepEqual(icloud.fetches[1], { range: [14], byUid: true })

      const none = resultData(await callBuiltinTool(mcp, 'list_messages', { subject: 'nothing' }))
      assert.deepEqual([none.total, none.messages], [0, []])
      assert.lengthOf(icloud.fetches, 2)
    } finally {
      icloud.restore()
    }
  })

  test('reads the text part of a message and names its attachments', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const result = await callBuiltinTool(await icloudMail(), 'get_message', { uid: '11' })

      assert.deepEqual(resultData(result), {
        mailbox: 'INBOX',
        uid: 11,
        subject: 'Lunch on Thursday?',
        from: 'Alice Martin <alice@example.com>',
        to: ['Thomas <thomas@icloud.com>', 'bob@example.com'],
        date: '2026-10-01T09:30:00.000Z',
        unread: true,
        flagged: false,
        answered: false,
        cc: ['Carol <carol@example.com>'],
        message_id: '<lunch@example.com>',
        text: 'Hi Thomas,\n\nAre you free on Thursday?\n\nAlice',
        attachments: [
          { part: '2', filename: 'Menu été.pdf', content_type: 'application/pdf', size: 57000 },
        ],
      })
      assert.deepEqual(icloud.downloads, [{ uid: 11, part: '1.1', maxBytes: 80000 }])
      // Selected read-only, so reading cannot mark the message as read.
      assert.deepEqual(icloud.locks, [{ path: 'INBOX', readOnly: true }])
      assert.deepEqual(icloud.mailbox('INBOX').messages[0].flags, [])
    } finally {
      icloud.restore()
    }
  })

  test('converts an HTML-only message and reports text cut at max_chars', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const newsletter = resultData(await callBuiltinTool(mcp, 'get_message', { uid: 12 }))
      assert.equal(
        newsletter.text,
        'Hello\n\nSee the deals [https://shop.example/deals]\n\nhttps://shop.example'
      )
      assert.notProperty(newsletter, 'text_truncated')
      assert.deepEqual(icloud.downloads[0], { uid: 12, part: '1', maxBytes: 1000000 })

      const long = resultData(
        await callBuiltinTool(mcp, 'get_message', { uid: 14, max_chars: 500 })
      )
      assert.lengthOf(long.text, 500)
      assert.isTrue(long.text.startsWith('Paid today.\n\nThanks. Thanks.'))
      assert.isTrue(long.text_truncated)
      assert.deepEqual(icloud.downloads[1], { uid: 14, part: '1', maxBytes: 2000 })
    } finally {
      icloud.restore()
    }
  }).timeout(10_000)

  test('explains an unknown message, mailbox, or argument', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      const text = async (tool: string, args: Record<string, unknown>) => {
        const result = await callBuiltinTool(mcp, tool, args)
        assert.isTrue(result.isError)
        return resultText(result)
      }

      assert.equal(
        await text('get_message', { uid: 99 }),
        'Message 99 was not found in "INBOX". UIDs belong to one mailbox: call list_messages on it for the current ones.'
      )
      for (const mailbox of ['Nope', 'Projects']) {
        assert.equal(
          await text('list_messages', { mailbox }),
          `Mailbox "${mailbox}" does not exist. Call list_mailboxes for the exact paths.`
        )
      }
      assert.equal(await text('get_message', {}), 'uid is required')
      assert.equal(
        await text('list_messages', { mailbox: 'INBOX\r\nA1 DELETE INBOX' }),
        'mailbox must be a single line of text'
      )
      assert.equal(
        await text('list_messages', { per_page: 500 }),
        'per_page must be an integer between 1 and 50'
      )
      assert.include(await text('list_messages', { since: 'yesterday' }), 'since must be an ISO')
      assert.equal(
        await text('archive_everything', {}),
        'Unknown iCloud Mail tool: archive_everything'
      )
    } finally {
      icloud.restore()
    }
  })
})

test.group('Built-in iCloud Mail MCP: sending', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('sends a message and keeps a copy with its blind copies in Sent', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const result = resultData(
        await callBuiltinTool(await fullAccess(), 'send_message', {
          to: ['dave@example.com', ' Dave@example.com '],
          bcc: 'boss@example.com',
          subject: 'Quote for October',
          text: 'Hello Dave,\n\nHere is the quote.',
        })
      )

      assert.match(result.message_id, /^<[0-9a-f-]{36}@icloud\.com>$/)
      assert.deepEqual(result, {
        sent: true,
        message_id: result.message_id,
        from: 'thomas@icloud.com',
        subject: 'Quote for October',
        to: ['dave@example.com'],
        bcc: ['boss@example.com'],
        saved_to: 'Sent Messages',
      })

      const [mail] = icloud.sent
      assert.lengthOf(icloud.sent, 1)
      assert.deepEqual(
        { ...mail, date: undefined },
        {
          from: { name: '', address: 'thomas@icloud.com' },
          to: [{ name: '', address: 'dave@example.com' }],
          cc: [],
          bcc: [{ name: '', address: 'boss@example.com' }],
          subject: 'Quote for October',
          text: 'Hello Dave,\n\nHere is the quote.',
          attachments: [],
          inReplyTo: undefined,
          references: undefined,
          messageId: result.message_id,
          date: undefined,
          xMailer: false,
          newline: 'windows',
          disableFileAccess: true,
          disableUrlAccess: true,
        }
      )

      const [copy] = icloud.mailbox('Sent Messages').appended
      assert.deepEqual(copy.flags, ['\\Seen'])
      assert.include(copy.raw, 'From: thomas@icloud.com\r\n')
      assert.include(copy.raw, 'To: dave@example.com\r\n')
      assert.include(copy.raw, 'Bcc: boss@example.com\r\n')
      assert.include(copy.raw, `Message-ID: ${result.message_id}\r\n`)
      assert.include(copy.raw, 'Subject: Quote for October\r\n')
      assert.isTrue(copy.raw.endsWith('\r\n\r\nHello Dave,\r\n\r\nHere is the quote.\r\n'))
      assert.notInclude(copy.raw, 'X-Mailer')
    } finally {
      icloud.restore()
    }
  })

  test('answers a message in its conversation and marks it answered', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await fullAccess()
      const result = resultData(
        await callBuiltinTool(mcp, 'send_message', {
          reply_to_uid: 11,
          reply_all: true,
          cc: ['erin@example.com', 'alice@example.com'],
          text: 'Thursday works.',
        })
      )

      assert.deepEqual(
        [result.subject, result.to, result.cc],
        [
          'Re: Lunch on Thursday?',
          ['alice@example.com'],
          ['erin@example.com', 'bob@example.com', 'carol@example.com'],
        ]
      )
      assert.equal(icloud.sent[0].inReplyTo, '<lunch@example.com>')
      assert.deepEqual(icloud.sent[0].references, ['<root@example.com>', '<lunch@example.com>'])
      assert.include(
        icloud.mailbox('Sent Messages').appended[0].raw,
        'References: <root@example.com> <lunch@example.com>\r\n'
      )
      assert.deepEqual(icloud.mailbox('INBOX').messages[0].flags, ['\\Answered'])
      assert.deepEqual(icloud.locks, [
        { path: 'INBOX', readOnly: true },
        { path: 'INBOX', readOnly: false },
      ])

      // A follow-up on a sent message goes to the people it was sent to.
      const followUp = resultData(
        await callBuiltinTool(mcp, 'send_message', {
          reply_to_uid: 5,
          reply_to_mailbox: 'Sent Messages',
          text: 'Any news?',
        })
      )
      assert.deepEqual([followUp.subject, followUp.to], ['Re: Quote', ['dave@example.com']])
    } finally {
      icloud.restore()
    }
  })

  test('sends from another address of the account only when the administrator allowed it', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id, {
        name: 'iCloud Mail senders',
        permissions: ICLOUD_MAIL_PERMISSIONS,
        aliases: ['hello@thomas.example', 'tt@icloud.com'],
      })
      const message = { to: ['dave@example.com'], subject: 'Hello', text: 'Hi' }

      const sent = resultData(
        await callBuiltinTool(mcp, 'send_message', { ...message, from: 'HELLO@thomas.example' })
      )
      assert.equal(sent.from, 'hello@thomas.example')
      assert.match(sent.message_id, /@thomas\.example>$/)
      assert.deepEqual(icloud.sent[0].from, { name: '', address: 'hello@thomas.example' })
      assert.include(
        icloud.mailbox('Sent Messages').appended[0].raw,
        'From: hello@thomas.example\r\n'
      )

      const refused = await callBuiltinTool(mcp, 'send_message', {
        ...message,
        from: 'ceo@bank.example',
      })
      assert.isTrue(refused.isError)
      assert.equal(
        resultText(refused),
        'from must be one of the sender addresses allowed for this MCP: thomas@icloud.com, hello@thomas.example, tt@icloud.com'
      )
      assert.lengthOf(icloud.sent, 1)

      // A message written to an alias is answered from that alias.
      const answer = resultData(
        await callBuiltinTool(mcp, 'create_draft', {
          reply_to_uid: 3,
          reply_to_mailbox: 'Archive',
          text: 'Yes, tell me more.',
        })
      )
      assert.deepEqual(
        [answer.from, answer.to, answer.subject],
        ['hello@thomas.example', ['erin@example.com'], 'Re: Website enquiry']
      )

      const chosen = resultData(
        await callBuiltinTool(mcp, 'create_draft', {
          reply_to_uid: 3,
          reply_to_mailbox: 'Archive',
          from: 'tt@icloud.com',
          text: 'Yes.',
        })
      )
      assert.equal(chosen.from, 'tt@icloud.com')
    } finally {
      icloud.restore()
    }
  })

  test('still reports a delivered message when its copy cannot be saved', async ({ assert }) => {
    const icloud = mockIcloudMail({
      failAppend: true,
      smtp: async () => ({ rejected: ['typo@example.invalid'] }),
    })
    try {
      const result = await callBuiltinTool(await fullAccess(), 'send_message', {
        to: ['dave@example.com', 'typo@example.invalid'],
        subject: 'Hello',
        text: 'Hi',
      })

      assert.isUndefined(result.isError)
      assert.include(resultData(result), {
        sent: true,
        warning:
          'The message was sent, but its copy could not be saved to the Sent mailbox. Do not send it again.',
      })
      assert.deepEqual(resultData(result).rejected, ['typo@example.invalid'])
      assert.notProperty(resultData(result), 'saved_to')
    } finally {
      icloud.restore()
    }
  })

  test('says when it is unknown whether iCloud accepted the message', async ({ assert }) => {
    const failure = (code: string, response?: string) => async () => {
      throw Object.assign(new Error(`SMTP failure with ${icloudMailSignIn.password}`), {
        code,
        response,
      })
    }
    const outcome = async (smtp: () => Promise<never>) => {
      const icloud = mockIcloudMail({ smtp })
      try {
        const result = await callBuiltinTool(await fullAccess(), 'send_message', {
          to: ['dave@example.com'],
          subject: 'Hello',
          text: 'Hi',
        })
        assert.isTrue(result.isError)
        assert.lengthOf(icloud.mailbox('Sent Messages').appended, 0)
        return resultText(result)
      } finally {
        icloud.restore()
      }
    }

    assert.equal(
      await outcome(failure('ESOCKET')),
      'iCloud Mail did not confirm the message, so it may or may not have been sent. Check with the user before sending it again.'
    )
    assert.equal(
      await outcome(failure('EDNS')),
      'Could not reach iCloud Mail. Nothing was sent. Try again.'
    )
    assert.equal(
      await outcome(failure('EENVELOPE', '550 5.1.1 unknown recipient')),
      'iCloud Mail rejected the recipients: 550 5.1.1 unknown recipient'
    )
    assert.include(await outcome(failure('EAUTH')), 'iCloud Mail rejected the sign-in.')
  })

  test('validates a message before reaching iCloud', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await fullAccess()
      const text = async (args: Record<string, unknown>) => {
        const result = await callBuiltinTool(mcp, 'send_message', args)
        assert.isTrue(result.isError)
        return resultText(result)
      }
      const message = { to: ['dave@example.com'], subject: 'Hello', text: 'Hi' }

      assert.equal(await text({ ...message, text: '' }), 'text is required')
      assert.equal(
        await text({ ...message, subject: undefined }),
        'subject is required unless reply_to_uid is set'
      )
      assert.equal(
        await text({ ...message, subject: 'Hello\r\nBcc: eve@example.com' }),
        'subject must be a single line of text'
      )
      for (const to of [
        ['Dave <dave@example.com>'],
        ['dave@example.com, eve@example.com'],
        ['dave'],
        [42],
      ]) {
        assert.include(await text({ ...message, to }), 'to must be a list of at most 50 email')
      }
      assert.lengthOf(icloud.signIns, 0)

      assert.equal(
        await text({ ...message, to: [] }),
        'Add at least one recipient in to, cc, or bcc'
      )
      assert.lengthOf(icloud.sent, 0)
    } finally {
      icloud.restore()
    }
  })

  test('saves a draft for the user to review without sending it', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const result = resultData(
        await callBuiltinTool(await fullAccess(), 'create_draft', {
          reply_to_uid: 14,
          text: 'Received, thank you.',
        })
      )

      assert.deepEqual(result, {
        saved_to: 'Drafts',
        uid: 101,
        message_id: result.message_id,
        from: 'thomas@icloud.com',
        subject: 'Re: Invoice 42',
        to: ['andre@example.com'],
      })
      const [draft] = icloud.mailbox('Drafts').appended
      assert.deepEqual(draft.flags, ['\\Draft', '\\Seen'])
      assert.include(draft.raw, 'In-Reply-To: <invoice@example.com>\r\n')
      assert.lengthOf(icloud.sent, 0)
      // Only a message that was sent counts as an answer.
      assert.notInclude(icloud.mailbox('INBOX').messages[2].flags, '\\Answered')

      const blank = await callBuiltinTool(await fullAccess(), 'create_draft', {
        subject: 'Ideas',
        text: 'To finish later.',
      })
      assert.deepEqual(resultData(blank).to, [])
    } finally {
      icloud.restore()
    }
  })
})

test.group('Built-in iCloud Mail MCP: organizing', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('marks the messages that exist as read, unread, or flagged', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await fullAccess()
      const result = resultData(
        await callBuiltinTool(mcp, 'mark_messages', {
          uids: [11, '14', 11, 99],
          unread: false,
          flagged: true,
        })
      )
      assert.deepEqual(result, { mailbox: 'INBOX', uids: [11, 14], unread: false, flagged: true })
      assert.deepEqual(icloud.searches, [{ uid: '11,14,99' }])
      assert.deepEqual(icloud.mailbox('INBOX').messages[0].flags, ['\\Seen', '\\Flagged'])
      assert.deepEqual(icloud.locks, [{ path: 'INBOX', readOnly: false }])

      await callBuiltinTool(mcp, 'mark_messages', { uids: [14], unread: true, flagged: false })
      assert.deepEqual(icloud.mailbox('INBOX').messages[2].flags, [])

      const text = async (args: Record<string, unknown>) =>
        resultText(await callBuiltinTool(mcp, 'mark_messages', args))
      assert.equal(await text({ uids: [11] }), 'Set unread, flagged, or both')
      assert.equal(await text({ unread: true }), 'uids must be a list of 1 to 100 message UIDs')
      assert.equal(
        await text({ uids: [98, 99], flagged: true }),
        'None of these UIDs exist in "INBOX". UIDs belong to one mailbox: call list_messages on it for the current ones.'
      )
    } finally {
      icloud.restore()
    }
  })

  test('moves messages to another mailbox and returns their new UIDs', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await fullAccess()
      const result = resultData(
        await callBuiltinTool(mcp, 'move_messages', {
          uids: [12, 14, 99],
          destination: 'Deleted Messages',
        })
      )

      assert.deepEqual(result, {
        mailbox: 'INBOX',
        destination: 'Deleted Messages',
        uids: [12, 14],
        new_uids: { 12: 1, 14: 2 },
      })
      assert.deepEqual(
        icloud.mailbox('INBOX').messages.map(({ uid }) => uid),
        [11]
      )
      assert.lengthOf(icloud.mailbox('Deleted Messages').messages, 2)

      const missing = await callBuiltinTool(mcp, 'move_messages', {
        uids: [11],
        destination: 'Nope',
      })
      assert.isTrue(missing.isError)
      assert.equal(
        resultText(missing),
        'iCloud Mail could not move these messages to "Nope". Call list_mailboxes for the exact destination path.'
      )
      assert.equal(
        resultText(await callBuiltinTool(mcp, 'move_messages', { uids: [11] })),
        'destination is required'
      )
    } finally {
      icloud.restore()
    }
  })
})

test.group('Built-in iCloud Mail MCP: sign-in', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('proves the saved password with one sign-in', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      mcp.status = 'draft'
      await testAndUpdateStatus(mcp)

      assert.equal(mcp.status, 'ready')
      assert.isNull(mcp.lastError)
      assert.isFalse(Boolean(mcp.oauthRequired))
      assert.deepEqual(icloud.signIns, [icloudMailSignIn])
      assert.equal(icloud.logouts, 1)
    } finally {
      icloud.restore()
    }
  })

  test('reports a rejected password as an error to fix, not as an OAuth step', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail({ rejectSignIn: true })
    try {
      const mcp = await icloudMail()
      await testAndUpdateStatus(mcp)

      assert.equal(mcp.status, 'error')
      assert.equal(
        mcp.lastError,
        'iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.'
      )
      assert.isFalse(Boolean(mcp.oauthRequired))

      const result = await callBuiltinTool(mcp, 'list_mailboxes', {})
      assert.isTrue(result.isError)
      assert.equal(resultText(result), mcp.lastError)
      // Each attempt tries both usernames Apple documents, and nothing else.
      assert.deepEqual(
        icloud.signIns.map(({ username }) => username),
        ['thomas@icloud.com', 'thomas', 'thomas@icloud.com', 'thomas']
      )
      assert.equal(icloud.logouts, 0)
    } finally {
      icloud.restore()
    }
  })

  test('signs in with the name before the @ when the full address is refused', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail({ imapUsername: 'thomas' })
    try {
      const mcp = await icloudMail()
      await testAndUpdateStatus(mcp)
      assert.equal(mcp.status, 'ready')
      assert.deepEqual(
        icloud.signIns.map(({ username }) => username),
        ['thomas@icloud.com', 'thomas']
      )

      // The form that worked is remembered, so iCloud sees no more failed sign-ins.
      const result = await callBuiltinTool(mcp, 'list_mailboxes', {})
      assert.isUndefined(result.isError)
      assert.deepEqual(
        icloud.signIns.map(({ username }) => username),
        ['thomas@icloud.com', 'thomas', 'thomas']
      )
      assert.equal(icloud.logouts, 2)
    } finally {
      icloud.restore()
    }
  })

  test('is not connected without a saved password', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      const mcp = await icloudMail()
      mcp.builtinPassword = null
      await mcp.save()

      assert.throws(
        () => listBuiltinTools(mcp),
        'iCloud Mail is not connected. Connect it from the MCPs page in MyMCPs.'
      )
      const result = await callBuiltinTool(mcp, 'list_mailboxes', {})
      assert.isTrue(result.isError)
      assert.include(resultText(result), 'iCloud Mail is not connected')
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      icloud.restore()
    }
  })

  test('redacts the app-specific password from diagnostics', async ({ assert }) => {
    const mcp = await icloudMail()
    assert.equal(
      sanitizeMcpDiagnostic(`AUTHENTICATE failed for ${icloudMailSignIn.password}`, mcp),
      'AUTHENTICATE failed for [REDACTED]'
    )
  })
})
