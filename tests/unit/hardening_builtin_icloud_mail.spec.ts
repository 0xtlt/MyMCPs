import { test } from '@japa/runner'
import type { MessageAddressObject, MessageEnvelopeObject, MessageStructureObject } from 'imapflow'
import { htmlConversion, htmlToText } from '#services/builtin/icloud_mail/html'
import {
  attachmentsOf,
  isAddress,
  messageHeaders,
  messageSummary,
  replyTo,
  tidyText,
} from '#services/builtin/icloud_mail/message'
import { callBuiltinTool, downloadBuiltinFile } from '#services/builtin/runtime'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import {
  createIcloudMailMcp,
  ICLOUD_MAIL_PERMISSIONS,
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

/**
 * Longer than any step of a tool call may keep the server from answering. The
 * steps measured here take a few milliseconds, and took seconds before.
 */
const MAX_PAUSE_MS = 500

const defaultTimeoutMs = htmlConversion.timeoutMs

/** The longest the event loop went without running a timer while `run` was pending. */
async function longestPause<Result>(run: () => Promise<Result>) {
  let longest = 0
  let last = performance.now()
  const timer = setInterval(() => {
    const now = performance.now()
    longest = Math.max(longest, now - last)
    last = now
  }, 5)

  try {
    const result = await run()
    return { result, pause: Math.max(longest, performance.now() - last) }
  } finally {
    clearInterval(timer)
  }
}

function elapsed<Result>(run: () => Result) {
  const start = performance.now()
  const result = run()
  return { result, ms: performance.now() - start }
}

/** `count` distinct people at example.com. */
function people(count: number, prefix = 'person'): MessageAddressObject[] {
  return Array.from({ length: count }, (_, index) => ({
    name: `${prefix} ${index}`,
    address: `${prefix}${index}@example.com`,
  }))
}

let sequence = 0

async function icloudMail(permissions: string[] = ['read']) {
  const admin = await createAdmin()
  sequence += 1
  return createIcloudMailMcp(admin.id, { name: `iCloud Mail hardening ${sequence}`, permissions })
}

test.group('Built-in iCloud Mail hardening: message text', () => {
  /** What tidyText did before its patterns were made linear. */
  const original = (text: string) =>
    text
      .replace(/\r\n?/g, '\n')
      .replace(/(?:[\u00ad\u034f\u200b-\u200d\u2007\ufeff][ \u00a0]*){3,}/g, '')
      .replace(/[ \t]+\n/g, '\n')
      .replace(/\n{3,}/g, '\n\n')
      .trim()

  test('tidies a long run of spaces or tabs in linear time', ({ assert }) => {
    // 80,000 spaces took the previous pattern close to three seconds, and
    // twice as many four times as long.
    const spaces = elapsed(() => tidyText(`Hello${' '.repeat(160_000)}`))
    assert.equal(spaces.result, 'Hello')
    assert.isBelow(spaces.ms, MAX_PAUSE_MS)

    const tabs = elapsed(() => tidyText(`${'\t'.repeat(80_000)}x${' \t'.repeat(80_000)}y \n z`))
    assert.equal(tabs.result, `x${' \t'.repeat(80_000)}y\n z`)
    assert.isBelow(tabs.ms, MAX_PAUSE_MS)

    // The most text a call can ask for.
    const longest = elapsed(() => tidyText(' '.repeat(400_000)))
    assert.equal(longest.result, '')
    assert.isBelow(longest.ms, MAX_PAUSE_MS)
  }).timeout(60_000)

  test('removes preview padding of any length', ({ assert }) => {
    // Matched one repetition at a time, this many overflowed the regex stack.
    const padding = elapsed(() => tidyText(`Sale${'\u200c'.repeat(5_000_000)}today`))
    assert.equal(padding.result, 'Saletoday')
    assert.isBelow(padding.ms, MAX_PAUSE_MS)

    const spaced = elapsed(() => tidyText(`Sale${'\u200c\u00a0'.repeat(200_000)}today`))
    assert.equal(spaced.result, 'Saletoday')
    assert.isBelow(spaced.ms, MAX_PAUSE_MS)

    assert.equal(tidyText('a\u200b \u200bb'), 'a\u200b \u200bb')
    assert.equal(tidyText('a\u200b \u200b\u00a0\u200b  b'), 'ab')
  })

  test('tidies exactly like the patterns it replaces', ({ assert }) => {
    const samples = [
      'Hi Thomas,\r\n\r\nAre you free on Thursday?\r\n\r\nAlice',
      'One  \r\n\r\n\r\n\r\nTwo\r\n',
      'Preview\u00a0\u200c\u00a0\u200c\u00a0\u200c\u00a0\u200c then the body  \n\n\n\nFooter\t \n',
      '> quoted line   \n> \t\n>\n\n\n\n-- \nSignature',
      'soft\u00adhyphen and zero\u200bwidth stay inside words',
      'code:\n    indented\n\tand tabbed\t\n  \n  \nend',
      `${' '.repeat(500)}\n${'\t'.repeat(500)}x${' '.repeat(500)}`,
      `${'\u200b'.repeat(500)}a${'\u200b \u00a0'.repeat(500)}b\u200b\u200bc`,
      '\ufeffBOM at the start',
      ' \n\t\r\n ',
      '',
    ]
    for (const sample of samples) {
      assert.equal(tidyText(sample), original(sample))
    }

    // Short random texts over the characters the patterns treat specially.
    const alphabet = [' ', ' ', '\t', '\n', '\n', '\r', 'a', '\u00a0', '\u200b', '\u200c', '\ufeff']
    let seed = 42
    const pick = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff
      return alphabet[seed % alphabet.length]
    }
    for (let round = 0; round < 20_000; round++) {
      const text = Array.from({ length: round % 25 }, pick).join('')
      if (tidyText(text) !== original(text)) {
        assert.fail(`tidyText differs on ${JSON.stringify(text)}`)
      }
    }
  })
})

test.group('Built-in iCloud Mail hardening: HTML', (group) => {
  group.each.teardown(() => {
    htmlConversion.timeoutMs = defaultTimeoutMs
  })

  test('converts a large newsletter without keeping the server from answering', async ({
    assert,
  }) => {
    const row =
      '<tr><td style="padding:0 12px;font-family:Helvetica,Arial,sans-serif">An offer, with a <a href="https://shop.example/deals">link to follow</a>.</td></tr>'
    const html = `<html><head><style>${'p{color:red}'.repeat(2000)}</style></head><body><table>${row.repeat(6000)}</table></body></html>`

    const { result, pause } = await longestPause(() => htmlToText(html, 5000))

    assert.isAbove(html.length, 900_000)
    assert.isTrue(result!.isTruncated)
    assert.lengthOf(result!.text, 5000)
    assert.isTrue(
      result!.text.startsWith('An offer, with a link to follow [https://shop.example/deals].')
    )
    assert.isBelow(pause, MAX_PAUSE_MS)
  }).timeout(30_000)

  test('gives up on markup that takes too long to convert', async ({ assert }) => {
    htmlConversion.timeoutMs = 400
    // The parser spends a time that grows with the square of the nesting on
    // tags that are never closed: seconds for these, in the server itself before.
    const shapes = [
      '<i>'.repeat(300_000),
      `${'<i>'.repeat(100_000)}${'</b>'.repeat(100_000)}`,
      `${'<blockquote>'.repeat(500)}<pre>${'x\n'.repeat(400_000)}`,
    ]

    for (const html of shapes) {
      const start = performance.now()
      const { result, pause } = await longestPause(() => htmlToText(html, 80_000))

      assert.isNull(result)
      assert.isBelow(pause, MAX_PAUSE_MS)
      assert.isBelow(performance.now() - start, 2000)
    }
  }).timeout(30_000)

  test('survives markup that makes the converter run out of memory', async ({ assert }) => {
    // Every link keeps its own copy of the words inside it: hundreds of
    // megabytes for this message, which only the converting process loses.
    const html = `${'<a href="https://x.example/y">'.repeat(200)}${'x '.repeat(400_000)}`

    const { result, pause } = await longestPause(() => htmlToText(html, 80_000))

    assert.isNull(result)
    assert.isBelow(pause, MAX_PAUSE_MS)

    // The next message is converted as usual.
    assert.deepEqual(await htmlToText('<p>Still <b>here</b></p>', 100), {
      text: 'Still here',
      isTruncated: false,
    })
  }).timeout(30_000)

  test('reads at most the text it was asked for from the converter', async ({ assert }) => {
    // 40 dashes for each rule: far more text than markup.
    const { result, pause } = await longestPause(() => htmlToText('<hr>'.repeat(100_000), 2000))

    assert.lengthOf(result!.text, 2000)
    assert.isTrue(result!.isTruncated)
    assert.isBelow(pause, MAX_PAUSE_MS)

    assert.deepEqual(await htmlToText('<pre>a\n  b</pre>', 6), {
      text: 'a\n  b',
      isTruncated: false,
    })
    assert.deepEqual(await htmlToText('<pre>a\n  b</pre>', 5), {
      text: 'a\n  b',
      isTruncated: false,
    })
    assert.deepEqual(await htmlToText('<pre>a\n  b</pre>', 4), { text: 'a\n  ', isTruncated: true })
    assert.deepEqual(await htmlToText('', 10), { text: '', isTruncated: false })
  }).timeout(30_000)

  test('converts messages one at a time, in the order they were asked for', async ({ assert }) => {
    const order: number[] = []
    const texts = await Promise.all(
      [1, 2, 3, 4].map(async (index) => {
        const converted = await htmlToText(`<p>Message ${index}</p>`, 100)
        order.push(index)
        return converted?.text
      })
    )

    assert.deepEqual(texts, ['Message 1', 'Message 2', 'Message 3', 'Message 4'])
    assert.deepEqual(order, [1, 2, 3, 4])
  }).timeout(30_000)
})

test.group('Built-in iCloud Mail hardening: reading', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)
  group.each.teardown(() => {
    htmlConversion.timeoutMs = defaultTimeoutMs
  })

  const envelope = {
    date: new Date('2026-10-04T08:00:00Z'),
    subject: 'Hello',
    messageId: '<hostile@example.com>',
    from: [{ address: 'mallory@example.com' }],
    to: [{ address: 'thomas@icloud.com' }],
  }

  test('reads a plain text message padded with spaces without stalling', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      icloud.mailbox('INBOX').messages.push({
        uid: 21,
        flags: [],
        envelope,
        bodyStructure: { type: 'text/plain', encoding: '7bit', size: 160_005 },
        parts: { '1': `Hello${' '.repeat(160_000)}` },
      })
      const mcp = await icloudMail()

      const { result, pause } = await longestPause(() =>
        callBuiltinTool(mcp, 'get_message', { uid: 21, max_chars: 40_000 })
      )

      assert.equal(resultData(result).text, 'Hello')
      assert.isTrue(resultData(result).text_truncated)
      assert.isBelow(pause, MAX_PAUSE_MS)
    } finally {
      icloud.restore()
    }
  }).timeout(60_000)

  test('reads an HTML message padded with spaces without stalling', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      icloud.mailbox('INBOX').messages.push({
        uid: 22,
        flags: [],
        envelope,
        bodyStructure: { type: 'text/html', encoding: '7bit', size: 999_999 },
        parts: { '1': `<p>Hello</p><pre>${' '.repeat(990_000)}</pre><p>hidden</p>` },
      })
      const mcp = await icloudMail()

      const { result, pause } = await longestPause(() =>
        callBuiltinTool(mcp, 'get_message', { uid: 22, max_chars: 100_000 })
      )

      // The spaces fill what is kept of the converted text, and tidying drops them.
      assert.equal(resultData(result).text, 'Hello')
      assert.isTrue(resultData(result).text_truncated)
      assert.notProperty(resultData(result), 'warning')
      assert.isBelow(pause, MAX_PAUSE_MS)
    } finally {
      icloud.restore()
    }
  }).timeout(60_000)

  test('returns the headers of an HTML message that cannot be converted', async ({ assert }) => {
    htmlConversion.timeoutMs = 400
    const icloud = mockIcloudMail()
    try {
      icloud.mailbox('INBOX').messages.push({
        uid: 23,
        flags: [],
        envelope,
        bodyStructure: {
          type: 'multipart/mixed',
          childNodes: [
            { part: '1', type: 'text/html', encoding: '7bit', size: 999_999 },
            {
              part: '2',
              type: 'application/pdf',
              encoding: 'base64',
              size: 780,
              disposition: 'attachment',
              dispositionParameters: { filename: 'invoice.pdf' },
            },
          ],
        },
        // 333,333 tags that are never closed, in the megabyte that is downloaded.
        parts: { '1': '<i>'.repeat(400_000), '2': 'PDF' },
      })
      const mcp = await icloudMail()

      let isRead = false
      const reading = longestPause(() => callBuiltinTool(mcp, 'get_message', { uid: 23 })).finally(
        () => {
          isRead = true
        }
      )
      // iCloud is signed out of before the conversion, not kept waiting for it.
      while (icloud.logouts === 0) {
        await new Promise((resolve) => setTimeout(resolve, 5))
      }
      assert.isFalse(isRead)
      const { result, pause } = await reading

      assert.isUndefined(result.isError)
      assert.deepEqual(resultData(result), {
        mailbox: 'INBOX',
        uid: 23,
        subject: 'Hello',
        from: 'mallory@example.com',
        to: ['thomas@icloud.com'],
        date: '2026-10-04T08:00:00.000Z',
        unread: true,
        flagged: false,
        answered: false,
        message_id: '<hostile@example.com>',
        text: '',
        warning:
          'This message is written in HTML that could not be converted to text, so its text is missing.',
        attachments: [
          { part: '2', filename: 'invoice.pdf', content_type: 'application/pdf', size: 570 },
        ],
      })
      assert.deepEqual(icloud.downloads, [{ uid: 23, part: '1', maxBytes: 1_000_000 }])
      assert.isBelow(pause, MAX_PAUSE_MS)

      // Other messages are still read.
      htmlConversion.timeoutMs = defaultTimeoutMs
      const newsletter = resultData(await callBuiltinTool(mcp, 'get_message', { uid: 12 }))
      assert.isTrue(newsletter.text.startsWith('Hello\n\nSee the deals'))
    } finally {
      icloud.restore()
    }
  }).timeout(60_000)
})

test.group('Built-in iCloud Mail hardening: what a sender controls', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  const crowded = () => ({
    seq: 1,
    uid: 31,
    flags: new Set<string>(),
    envelope: {
      date: new Date('2026-10-04T08:00:00Z'),
      subject: 'S'.repeat(5000),
      messageId: `<${'m'.repeat(5000)}@example.com>`,
      from: people(60, 'from'),
      replyTo: people(60, 'reply'),
      to: people(300, 'to'),
      cc: people(51, 'cc'),
      bcc: [{ name: 'N'.repeat(1000), address: 'bcc@example.com' }],
    },
  })

  test('cuts long subjects and address lists, and says which', ({ assert }) => {
    const summary = messageSummary(crowded())

    assert.lengthOf(summary.subject, 999)
    assert.isTrue(summary.subject.endsWith('S…'))
    assert.lengthOf(summary.to, 50)
    assert.equal(summary.to[49], 'to 49 <to49@example.com>')
    assert.lengthOf(summary.from.split(', '), 50)
    assert.deepInclude(summary, {
      subject_truncated: true,
      from_truncated: true,
      to_truncated: true,
    })

    const headers = messageHeaders(crowded())
    assert.lengthOf(headers.cc!, 50)
    assert.lengthOf(headers.reply_to!.split(', '), 50)
    assert.lengthOf(headers.bcc![0], 321)
    assert.lengthOf(headers.message_id!, 999)
    assert.deepInclude(headers, {
      subject_truncated: true,
      from_truncated: true,
      to_truncated: true,
      reply_to_truncated: true,
      cc_truncated: true,
      bcc_truncated: true,
      message_id_truncated: true,
    })
    // Everything a hostile message can put in a listing fits in a few pages.
    assert.isBelow(JSON.stringify(summary).length, 12_000)
    assert.isBelow(JSON.stringify(headers).length, 24_000)
  })

  test('flags nothing on a message within the limits', ({ assert }) => {
    const message = {
      seq: 1,
      uid: 32,
      flags: new Set<string>(),
      envelope: {
        subject: 's'.repeat(998),
        messageId: '<ok@example.com>',
        from: [{ name: 'Alice', address: 'alice@example.com' }],
        to: people(50, 'to'),
        cc: people(50, 'cc'),
      },
    }

    const headers = messageHeaders(message)
    assert.lengthOf(headers.subject, 998)
    assert.lengthOf(headers.to, 50)
    assert.lengthOf(headers.cc!, 50)
    assert.isEmpty(Object.keys(headers).filter((key) => key.endsWith('_truncated')))
  })

  test('names at most 100 attachments and shortens their names', async ({ assert }) => {
    const file = (index: number): MessageStructureObject => ({
      part: String(index + 2),
      type: 'application/pdf',
      encoding: 'base64',
      size: 780,
      disposition: 'attachment',
      dispositionParameters: { filename: index === 0 ? 'n'.repeat(4000) : `file-${index}.pdf` },
    })
    const bodyStructure: MessageStructureObject = {
      type: 'multipart/mixed',
      childNodes: [
        { part: '1', type: 'text/plain', size: 5 },
        ...Array.from({ length: 150 }, (_, index) => file(index)),
        { part: '152', type: `application/${'x'.repeat(4000)}`, parameters: { name: 'odd' } },
      ],
    }

    const attachments = attachmentsOf(bodyStructure)
    assert.lengthOf(attachments, 151)
    assert.equal(attachments[0].filename, `${'n'.repeat(255)}…`)
    assert.lengthOf(attachments[150].content_type, 256)

    const icloud = mockIcloudMail()
    try {
      icloud.mailbox('INBOX').messages.push({
        uid: 33,
        flags: [],
        envelope: { subject: 'Files', from: [{ address: 'mallory@example.com' }] },
        bodyStructure,
        parts: { '1': 'Files', '151': 'PDF' },
      })
      const mcp = await icloudMail()

      const message = resultData(await callBuiltinTool(mcp, 'get_message', { uid: 33 }))
      assert.lengthOf(message.attachments, 100)
      assert.isTrue(message.attachments_truncated)
      assert.equal(message.attachments[99].part, '101')

      const listed = resultData(await callBuiltinTool(mcp, 'list_messages', {}))
      assert.equal(listed.messages[0].attachments, 151)

      // An attachment that is not named can still be downloaded, and a wrong
      // part is explained without naming all of them.
      const unnamed = await downloadBuiltinFile(mcp, { mailbox: 'INBOX', uid: 33, part: '151' })
      assert.equal(unnamed.filename, 'file-149.pdf')
      const missing = await downloadBuiltinFile(mcp, { mailbox: 'INBOX', uid: 33, part: '999' })
        .then(() => '')
        .catch((error: Error) => error.message)
      assert.isTrue(missing.endsWith(', 100, 101, and more.'))
      assert.isBelow(missing.length, 600)

      const untruncated = resultData(await callBuiltinTool(mcp, 'get_message', { uid: 11 }))
      assert.lengthOf(untruncated.attachments, 1)
      assert.notProperty(untruncated, 'attachments_truncated')
    } finally {
      icloud.restore()
    }
  })

  test('refuses addresses written to be slow to check as quickly as others', ({ assert }) => {
    // The longest an address can be, with a dot at every place the domain
    // could be split: the previous pattern tried each of them.
    const crafted = [`a@${'b.'.repeat(125)}b `, `a@${'b.'.repeat(120)}b@c.d`]

    const { result, ms } = elapsed(() => {
      let accepted = 0
      for (let round = 0; round < 50_000; round++) {
        accepted += crafted.filter(isAddress).length
      }
      return accepted
    })
    assert.equal(result, 0)
    assert.isBelow(ms, MAX_PAUSE_MS)

    // The same addresses pass as before.
    const verdicts = [
      ['name@example.com', true],
      [`a@${'b.'.repeat(125)}bb`, true],
      ['a@.b.c', true],
      ['a@b..c', true],
      ['a@b', false],
      ['a@.b', false],
      ['a@b.', false],
      ['@b.c', false],
      ['a@b@c.d', false],
      ['Dave <dave@example.com>', false],
      ['a@b.c,d@e.f', false],
      [`a@${'b'.repeat(250)}.cc`, false],
    ] as const
    for (const [address, isAccepted] of verdicts) {
      assert.equal(isAddress(address), isAccepted, address)
    }
  }).timeout(60_000)

  test('works out the recipients of a reply to a crowded message without stalling', ({
    assert,
  }) => {
    const original = {
      seq: 1,
      uid: 34,
      envelope: {
        subject: 'Everyone',
        from: [{ address: 'mallory@example.com' }],
        replyTo: people(20_000, 'reply'),
        to: people(20_000, 'to'),
        cc: [...people(20_000, 'cc'), ...people(20_000, 'reply')],
      },
    }

    const { result, ms } = elapsed(() => replyTo(original, ['thomas@icloud.com'], true))

    assert.lengthOf(result.to, 20_000)
    assert.lengthOf(result.cc, 40_000)
    assert.isBelow(ms, MAX_PAUSE_MS)
  }).timeout(60_000)
})

test.group('Built-in iCloud Mail hardening: replies', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  const message = (uid: number, envelope: MessageEnvelopeObject) => ({
    uid,
    flags: [],
    envelope: {
      date: new Date('2026-10-04T08:00:00Z'),
      subject: 'Sign the petition',
      messageId: `<petition-${uid}@example.com>`,
      from: [{ address: 'mallory@example.com' }],
      to: [{ address: 'thomas@icloud.com' }],
      ...envelope,
    },
    bodyStructure: { type: 'text/plain', encoding: '7bit', size: 5 },
    parts: { '1': 'Hello' },
  })

  test('refuses to answer more addresses than an agent may name itself', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      icloud
        .mailbox('INBOX')
        .messages.push(
          message(41, { replyTo: people(51, 'target') }),
          message(42, { to: people(30, 'to'), cc: people(21, 'cc') }),
          message(43, { replyTo: people(50, 'target'), cc: people(50, 'cc') })
        )
      const mcp = await icloudMail(ICLOUD_MAIL_PERMISSIONS)

      for (const tool of ['send_message', 'create_draft']) {
        const toAll = await callBuiltinTool(mcp, tool, { reply_to_uid: 41, text: 'Signed.' })
        assert.isTrue(toAll.isError)
        assert.equal(
          resultText(toAll),
          'The message being answered asks for replies to 51 addresses, and at most 50 are allowed. Pass to with the addresses to answer.'
        )

        const copyAll = await callBuiltinTool(mcp, tool, {
          reply_to_uid: 42,
          reply_all: true,
          text: 'Signed.',
        })
        assert.isTrue(copyAll.isError)
        assert.equal(
          resultText(copyAll),
          'Replying to all would copy 51 addresses, and at most 50 are allowed. Set reply_all to false, and pass to and cc with the addresses to answer.'
        )

        // The agent's own copies count towards the same limit.
        const oneMore = await callBuiltinTool(mcp, tool, {
          reply_to_uid: 43,
          reply_all: true,
          cc: ['extra@example.com'],
          text: 'Signed.',
        })
        assert.isTrue(oneMore.isError)
        assert.include(resultText(oneMore), 'Replying to all would copy 51 addresses')
      }
      assert.lengthOf(icloud.sent, 0)
      assert.lengthOf(icloud.mailbox('Drafts').appended, 0)
      assert.notInclude(icloud.mailbox('INBOX').messages[3].flags, '\\Answered')
    } finally {
      icloud.restore()
    }
  })

  test('answers a crowded message once the agent names the recipients', async ({ assert }) => {
    const icloud = mockIcloudMail()
    try {
      icloud
        .mailbox('INBOX')
        .messages.push(
          message(41, { replyTo: people(51, 'target') }),
          message(43, { replyTo: people(50, 'target'), cc: people(50, 'cc') })
        )
      const mcp = await icloudMail(ICLOUD_MAIL_PERMISSIONS)

      const chosen = resultData(
        await callBuiltinTool(mcp, 'send_message', {
          reply_to_uid: 41,
          to: ['target0@example.com'],
          text: 'Signed.',
        })
      )
      assert.deepEqual(chosen.to, ['target0@example.com'])
      assert.equal(chosen.subject, 'Re: Sign the petition')
      assert.equal(icloud.sent[0].inReplyTo, '<petition-41@example.com>')

      // Exactly at the limit: 50 addresses to answer, and 50 to copy.
      const full = resultData(
        await callBuiltinTool(mcp, 'create_draft', {
          reply_to_uid: 43,
          reply_all: true,
          text: 'Signed.',
        })
      )
      assert.lengthOf(full.to, 50)
      assert.lengthOf(full.cc, 50)
    } finally {
      icloud.restore()
    }
  })
})
