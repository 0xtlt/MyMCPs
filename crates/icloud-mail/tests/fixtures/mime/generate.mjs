// Writes the fixtures of tests/mime_differential.rs: mails, and what
// nodemailer 10.0.14 makes of each on Node 24.21.
//
//   node crates/icloud-mail/tests/fixtures/mime/generate.mjs [path to node_modules/nodemailer]
//
// from the root of a checkout that has the Node version's node_modules.
//
// - corpus.jsonl: mails written one by one, for every path through nodemailer
// - random.jsonl: mails drawn from a seeded generator over the same inputs
// - addresses.jsonl: addresses, and how nodemailer writes each
// - address_sweep.json: digests of how it writes addresses built around
//   every Unicode code point
//
// The mails are the ones the iCloud Mail tools can ask for, and the options
// are the ones they pass. Each mail is built with and without `keepBcc`, and
// the first is checked against what `streamTransport` returns. The mails of
// the corpus are also sent to an SMTP server on localhost, to record the
// commands and to check the message it receives.
import crypto from 'node:crypto'
import { writeFileSync } from 'node:fs'
import net from 'node:net'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { readFileSync } from 'node:fs'

const here = dirname(fileURLToPath(import.meta.url))
const nodemailerPath = resolve(
  process.argv[2] ??
    'node_modules/.pnpm/nodemailer@10.0.14/node_modules/nodemailer'
)
const { version } = JSON.parse(readFileSync(join(nodemailerPath, 'package.json'), 'utf8'))
if (version !== '10.0.14') throw new Error(`The port follows nodemailer 10.0.14, and this is ${version}`)
if (process.versions.ada !== '4.0.0' || process.versions.unicode !== '17.0') {
  throw new Error(
    `The port follows Node 24.21 (ada 4.0.0, Unicode 17.0), and this is ada ${process.versions.ada} with Unicode ${process.versions.unicode}`
  )
}
const library = (file) => import(pathToFileURL(join(nodemailerPath, 'dist/esm', file)).href)
const { default: MailComposer } = await library('mail-composer/index.js')
const { default: MimeNode } = await library('mime-node/index.js')
const { default: DataStream } = await library('smtp-connection/data-stream.js')
const nodemailer = await library('nodemailer.js')

// nodemailer draws the boundary of a message with `crypto.randomBytes(8)`.
let boundary = '0000000000000000'
const randomBytes = crypto.randomBytes
crypto.randomBytes = (size) => (size === 8 ? Buffer.from(boundary, 'hex') : randomBytes(size))

// A small generator, so that the fixtures are the same on every run.
let state = 0x2545f491
function random() {
  state ^= state << 13
  state >>>= 0
  state ^= state >>> 17
  state ^= state << 5
  state >>>= 0
  return state
}
const below = (limit) => random() % limit
const pick = (list) => list[below(list.length)]
const chance = (percent) => below(100) < percent
const hex = (digits) => Array.from({ length: digits }, () => '0123456789abcdef'[below(16)]).join('')
const uuid = () => `${hex(8)}-${hex(4)}-4${hex(3)}-${'89ab'[below(4)]}${hex(3)}-${hex(12)}`
const repeat = (count, make) => Array.from({ length: count }, (_, index) => make(index))

/** The bytes the Rust test generates for `{ seed, length }`. */
function generatedBytes(seed, length) {
  const bytes = Buffer.alloc(length)
  let value = seed >>> 0 || 1
  for (let index = 0; index < length; index++) {
    value ^= value << 13
    value >>>= 0
    value ^= value >>> 17
    value ^= value << 5
    value >>>= 0
    bytes[index] = value & 0xff
  }
  return bytes
}

// What the tools accept, as app/validators/builtin_icloud_mail.ts checks it.

const ADDRESS = /^[^\s@<>(),;:"\\[\]]+@[^\s@<>(),;:"\\[\]][^\s@<>(),;:"\\[\].]*\.[^\s@<>(),;:"\\[\]]+$/
const MEDIA_TYPE = /^[\w.+-]{1,100}\/[\w.+-]{1,100}$/
const isWellFormed = (text) => text.isWellFormed()
const isLine = (text, max) => text.length <= max && text === text.trim() && !/\p{Cc}/u.test(text) && isWellFormed(text)

function checkInputs(name, mail) {
  const fail = (what) => {
    throw new Error(`${name}: ${what} is not something the tools pass`)
  }
  for (const address of [mail.from, ...mail.to, ...mail.cc, ...mail.bcc]) {
    if (address.length > 254 || !ADDRESS.test(address) || !isWellFormed(address) || address !== address.trim()) {
      fail(`the address ${JSON.stringify(address)}`)
    }
  }
  for (const list of [mail.to, mail.cc, mail.bcc]) {
    if (list.length > 50 || new Set(list).size !== list.length) fail('a list of recipients')
  }
  if (!isLine(mail.subject, 255)) fail('the subject')
  if (mail.text.length === 0 || mail.text.length > 100_000 || !isWellFormed(mail.text)) fail('the text')
  if (mail.attachments.length > 10) fail('the number of attachments')
  let bytes = 0
  for (const { filename, contentType, content } of mail.attachments) {
    if (filename.length === 0 || !isLine(filename, 255) || /[\\/]/.test(filename)) fail(`the file name ${JSON.stringify(filename)}`)
    if (contentType !== undefined && !MEDIA_TYPE.test(contentType)) fail(`the media type ${contentType}`)
    if (content.length === 0) fail('an empty file')
    bytes += content.length
  }
  if (bytes > 20_000_000) fail('the size of the attachments')
  if (mail.inReplyTo !== undefined && (!isWellFormed(mail.inReplyTo) || mail.inReplyTo === '')) fail('inReplyTo')
  if (mail.references !== undefined) {
    if (mail.references.length > 20) fail('the number of references')
    const fromHeader = mail.inReplyTo === undefined ? mail.references : mail.references.slice(0, -1)
    if (mail.inReplyTo !== undefined && mail.references.at(-1) !== mail.inReplyTo) fail('the last reference')
    for (const reference of fromHeader) {
      if (!/^<[^<>\s]+>$/.test(reference) || /[^\u0000-\u00ff]/.test(reference)) fail(`the reference ${JSON.stringify(reference)}`)
    }
  }
}

// What nodemailer makes of a mail.

/** `mailOptions` of app/services/builtin/icloud_mail/tools.ts. */
function mailOptions(mail) {
  const recipients = (addresses) => addresses.map((address) => ({ name: '', address }))
  return {
    ...mail,
    // Only for a boundary that eight random bytes could not give.
    ...(mail.baseBoundary === undefined ? {} : { baseBoundary: mail.baseBoundary }),
    from: { name: '', address: mail.from },
    to: recipients(mail.to),
    cc: recipients(mail.cc),
    bcc: recipients(mail.bcc),
    xMailer: false,
    newline: 'windows',
    disableFileAccess: true,
    disableUrlAccess: true,
  }
}

function dataStream(message) {
  return new Promise((done) => {
    const stream = new DataStream()
    const chunks = []
    stream.on('data', (chunk) => chunks.push(chunk))
    stream.on('end', () => done(Buffer.concat(chunks)))
    stream.end(message)
  })
}

const streamTransport = nodemailer.createTransport({ streamTransport: true, newline: 'windows', buffer: true })

/** A server that takes every message, and keeps what it was sent. */
function listen() {
  const session = { commands: [], data: null }
  const server = net.createServer((socket) => {
    let received = Buffer.alloc(0)
    let isData = false
    // Where to look for the end of the message from: a large one arrives in many pieces.
    let searched = 0
    socket.on('error', () => {})
    socket.write('220 localhost ESMTP\r\n')
    socket.on('data', (chunk) => {
      received = Buffer.concat([received, chunk])
      for (;;) {
        if (isData) {
          const end = received.subarray(0, 3).toString('latin1') === '.\r\n' ? -2 : received.indexOf('\r\n.\r\n', searched)
          searched = Math.max(0, received.length - 4)
          if (end === -1) return
          searched = 0
          session.data = received.subarray(0, end + 5)
          received = received.subarray(end + 5)
          isData = false
          socket.write('250 2.0.0 OK\r\n')
          continue
        }
        const end = received.indexOf('\r\n')
        if (end === -1) return
        const command = received.subarray(0, end).toString('utf8')
        received = received.subarray(end + 2)
        if (/^EHLO /i.test(command)) {
          socket.write('250-localhost\r\n250-PIPELINING\r\n250-SIZE 28311552\r\n250-8BITMIME\r\n250-SMTPUTF8\r\n250 OK\r\n')
        } else if (/^DATA$/i.test(command)) {
          isData = true
          socket.write('354 Go ahead\r\n')
        } else {
          session.commands.push(command)
          socket.write('250 OK\r\n')
        }
      }
    })
  })
  return new Promise((done) => server.listen(0, '127.0.0.1', () => done({ server, session })))
}

/** The commands and the message nodemailer sends to a server that offers every extension it looks for. */
async function sendThroughSmtp(mail) {
  const { server, session } = await listen()
  const transport = nodemailer.createTransport({
    host: '127.0.0.1',
    port: server.address().port,
    secure: false,
    ignoreTLS: true,
    disableFileAccess: true,
    disableUrlAccess: true,
  })
  let error = null
  try {
    await transport.sendMail(mailOptions(mail))
  } catch (failure) {
    error = failure.message
  }
  transport.close()
  await new Promise((done) => server.close(done))
  return { ...session, error }
}

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex')

/** The message in full when it is short, as text when it is UTF-8. Its digest always. */
function describe(bytes, shownUpTo) {
  const described = { length: bytes.length, sha256: sha256(bytes) }
  if (bytes.length <= shownUpTo) {
    const text = bytes.toString('utf8')
    if (Buffer.from(text, 'utf8').equals(bytes)) described.text = text
    else described.base64 = bytes.toString('base64')
  }
  return described
}

async function fixture(name, mail, { isBeyond = false, isSent = false, shownUpTo = 16_384 } = {}) {
  if (!isBeyond) checkInputs(name, mail)
  boundary = mail.baseBoundary ?? hex(16)

  const compose = (keepBcc) => {
    const message = new MailComposer(mailOptions(mail)).compile()
    message.keepBcc = keepBcc
    return message
  }
  const keptMessage = compose(true)
  const envelope = keptMessage.getEnvelope()
  const kept = await keptMessage.build()
  const dropped = await compose(false).build()
  const data = await dataStream(dropped)

  // `streamTransport` keeps Bcc, and goes through `sendMail` like the SMTP transport.
  const streamed = await streamTransport.sendMail(mailOptions(mail))
  if (!streamed.message.equals(kept)) throw new Error(`${name}: streamTransport writes another message`)
  if (JSON.stringify(streamed.envelope) !== JSON.stringify(envelope)) throw new Error(`${name}: sendMail has another envelope`)

  let smtp
  if (isSent) {
    const session = await sendThroughSmtp(mail)
    if (session.data ? !session.data.equals(data) : envelope.to.length > 0) {
      throw new Error(`${name}: the SMTP transport sends another message`)
    }
    smtp = session.data ? { commands: session.commands } : { error: session.error }
  }

  const attachments = mail.attachments.map(({ filename, contentType, content, generated }) => ({
    filename,
    content_type: contentType ?? null,
    content: generated ?? { base64: content.toString('base64') },
  }))
  return JSON.stringify({
    name,
    ...(isBeyond ? { beyond: true } : {}),
    mail: {
      from: mail.from,
      to: mail.to,
      cc: mail.cc,
      bcc: mail.bcc,
      subject: mail.subject,
      ...(mail.textRepeat ? { text_repeat: mail.textRepeat } : { text: mail.text }),
      attachments,
      in_reply_to: mail.inReplyTo ?? null,
      references: mail.references ?? null,
      message_id: mail.messageId,
      date_ms: mail.date.getTime(),
    },
    boundary,
    envelope: { from: envelope.from || '', to: envelope.to },
    ...(smtp ? { smtp } : {}),
    kept: describe(kept, shownUpTo),
    dropped: describe(dropped, 0),
    ...(isSent ? { data: describe(data, 0) } : {}),
  })
}

// Mails.

const file = (filename, contentType, content) => ({
  filename,
  contentType,
  content: typeof content === 'string' ? Buffer.from(content, 'utf8') : content,
})
const generatedFile = (filename, contentType, seed, length) => ({
  filename,
  contentType,
  content: generatedBytes(seed, length),
  generated: { seed, length },
})

/** A mail as `composeMail` of the tools returns it. */
function compose(fields = {}) {
  const from = fields.from ?? 'thomas@icloud.com'
  const { textRepeat } = fields
  return {
    from,
    to: ['dave@example.com'],
    cc: [],
    bcc: [],
    subject: 'Quote for October',
    text: textRepeat ? textRepeat[0].repeat(textRepeat[1]) : 'Hello Dave,\n\nHere is the quote.',
    attachments: [],
    inReplyTo: undefined,
    references: undefined,
    messageId: `<${uuid()}@${from.split('@').pop()}>`,
    date: new Date(Date.UTC(2026, 9, 7, 8, 9, 10, 123)),
    ...fields,
  }
}

const corpus = []
const add = (name, fields, options) => corpus.push([name, compose(fields), { isSent: true, ...options }])

// What the tests of the app look at.
add('app: a draft with a blind copy', { bcc: ['boss@example.com'] })
add('app: a reply', {
  subject: 'Re: Lunch',
  text: 'See you there.',
  inReplyTo: '<invoice@example.com>',
  references: ['<root@example.com>', '<lunch@example.com>', '<invoice@example.com>'],
})
add('app: references without the message answered', { references: ['<root@example.com>', '<lunch@example.com>'] })
add('app: attachments', {
  attachments: [
    file('Devis été.pdf', 'application/pdf', '%PDF-1.4 quote'),
    file('notes.txt', undefined, 'some notes\n'),
    file('quote.pdf', 'application/pdf', '%PDF-1.4 other quote'),
  ],
})
add('app: two files of 10 MB', {
  attachments: [
    generatedFile('first.bin', undefined, 1, 10_000_000),
    generatedFile('second.bin', 'application/octet-stream', 2, 10_000_000),
  ],
})

// Recipients.
add('recipients: none at all', { to: [] })
add('recipients: only copied', { to: [], cc: ['carol@example.com'] })
add('recipients: only blind-copied', { to: [], bcc: ['boss@example.com'] })
add('recipients: all three lists', { to: ['a@example.com', 'b@example.com'], cc: ['c@example.com'], bcc: ['d@example.com', 'e@example.com'] })
add('recipients: 50 in each list', {
  to: repeat(50, (index) => `to-${index}@example.com`),
  cc: repeat(50, (index) => `copied.recipient.number.${index}@a-rather-long-domain-name.example.org`),
  bcc: repeat(50, (index) => `b${index}@x.io`),
})
add('recipients: 50 as long as an address gets', {
  to: repeat(50, (index) => `${String(index).padStart(2, '0')}${'x'.repeat(62)}@${'d'.repeat(63)}.${'e'.repeat(63)}.${'f'.repeat(57)}.org`),
})
add('recipients: the same one in two lists', { to: ['dave@example.com'], cc: ['carol@example.com'], bcc: ['dave@example.com', 'carol@example.com'] })
add('recipients: the same one in two spellings of the domain', { to: ['dave@example.com'], bcc: ['dave@EXAMPLE.COM', 'Dave@example.com'] })
for (const length of [56, 57, 58, 59, 60, 61, 62, 70, 71, 72, 73, 74, 75, 76, 77, 78, 150]) {
  add(`recipients: an address that makes a line of ${length + 4} characters`, { to: [`${'a'.repeat(length - 12)}@example.com`, 'dave@example.com'] })
}

// Addresses.
const addresses = [
  ['plain', 'dave@example.com'],
  ['upper case', 'Dave.Smith@EXAMPLE.Com'],
  ['punctuation in the local part', "o'brien!#$%&*+-/=?^_`{|}~@example.com"],
  ['a local part that starts with a dot', '.dave@example.com'],
  ['a local part that ends with a dot', 'dave.@example.com'],
  ['a local part with two dots in a row', 'dave..smith@example.com'],
  ['a local part that is a dot', '.@example.com'],
  ['a control character in the local part', 'da\u0001ve@example.com'],
  ['a control character before the at sign', 'dave\u001f@example.com'],
  ['a delete character in the local part', 'da\u007fve@example.com'],
  ['a C1 control character in the local part', 'da\u0085ve@example.com'],
  ['a control character in the domain', 'dave@exa\u0002mple.com'],
  ['a control character at the end of the domain', 'dave@example.com\u0003'],
  ['a local part that is not ASCII', 'dévé@example.com'],
  ['a local part in Chinese', '用户@example.com'],
  ['a local part with an emoji', 'da😀ve@example.com'],
  ['a domain that is not ASCII', 'dave@bücher.example'],
  ['a domain in Japanese', 'dave@例え.jp'],
  ['a domain in Arabic', 'dave@مثال.إختبار'],
  ['a domain in Hebrew with a number', 'dave@דוגמה1.co.il'],
  ['a domain in Russian', 'dave@пример.рф'],
  ['an address that is not ASCII on both sides', '用户@例え.jp'],
  ['a local part that is not ASCII with an encoded domain', 'dévé@xn--bcher-kva.example'],
  ['a local part that is not ASCII with a label that does not decode', 'dévé@xn--a.example'],
  ['a local part that is not ASCII with an empty encoded label', 'dévé@xn--.ｅｘａｍｐｌｅ.com'],
  ['a local part that is not ASCII with a label that decodes and one that does not', 'dévé@xn--a.xn--ls8h.example'],
  ['a local part that is not ASCII with a label encoded twice', 'dévé@xn--ls8h.xn--xn---epa.example'],
  ['a local part that is not ASCII with an encoded label that starts with a hyphen', 'dévé@xn---9ca.example'],
  ['a domain in upper case that is not ASCII', 'dave@BÜCHER.Example'],
  ['a domain with a sharp s', 'dave@straße.de'],
  ['a domain with a final sigma', 'dave@ΟΔΟΣ.gr'],
  ['a domain with a dotted capital I', 'dave@İstanbul.example'],
  ['a domain with full-width letters', 'dave@ｅｘａｍｐｌｅ.com'],
  ['a domain with an ideographic full stop', 'dave@例え。jp.example'],
  ['a domain with a soft hyphen', 'dave@exam\u00adple.com'],
  ['a domain with a zero width joiner', 'dave@exam\u200dple.com'],
  ['a domain with an emoji', 'dave@😀.example'],
  ['a domain that is not normalized', 'dave@cafe\u0301.example'],
  ['a domain with an accent and a cedilla in the wrong order', 'dave@\u00e9\u0327.example'],
  ['a domain with a Hangul syllable and a loose final consonant', 'dave@\uac00\u11a8.example'],
  ['a domain with a right-to-left label and a number label', 'dave@1.مثال.example'],
  ['a domain with a label that mixes directions', 'dave@aمثال.example'],
  ['a domain with a number before a right-to-left letter', 'dave@1مثال.example'],
  ['a domain with a character of Unicode 16', 'dave@\u{10d4a}\u{10d4b}.example'],
  ['a domain that starts with a combining mark', 'dave@\u0301a.example'],
  ['a domain with an unassigned character', 'dave@a\u0378b.example'],
  ['a domain with a private use character', 'dave@a\ue000b.example'],
  ['a domain with a replacement character', 'dave@a\ufffdb.example'],
  ['a domain with a caret', 'dave@exa^mple.com'],
  ['a domain with a pipe that is not ASCII', 'dave@bü|cher.example'],
  ['a domain with a percent sign', 'dave@exa%41mple.com'],
  ['a domain with a slash', 'dave@example.com/evil.example'],
  ['a domain with a question mark', 'dave@example.com?x=1'],
  ['a domain with a number sign', 'dave@example.com#x'],
  ['a domain with a slash that is not ASCII', 'dave@bücher.example/x'],
  ['a domain with other punctuation', "dave@a!b$c&d'e*f+g=h_i`j{k}l~m.example"],
  ['a domain with an encoded label', 'dave@xn--bcher-kva.example'],
  ['a domain with an encoded label in upper case', 'dave@XN--BCHER-KVA.example'],
  ['a domain with a label that does not decode', 'dave@xn--a.xn--9adcb9.example'],
  ['a domain with an encoded label next to one that is not ASCII', 'dave@xn--bcher-kva.bücher.example'],
  ['a domain with a bad encoded label next to one that is not ASCII', 'dave@xn--a.bücher.example'],
  ['a domain that is four numbers', 'dave@192.168.0.1'],
  ['a domain that is two numbers', 'dave@127.1'],
  ['a domain that is hexadecimal numbers', 'dave@0x7f.0x1'],
  ['a domain that is octal numbers', 'dave@0300.0250.0.01'],
  ['a domain that ends in a number', 'dave@example.123'],
  ['a domain that is too many numbers', 'dave@1.2.3.4.5'],
  ['a domain that is full-width numbers', 'dave@１９２.１６８.０.１'],
  ['a domain that ends with a dot', 'dave@example.com.'],
  ['a domain that starts with a dot', 'dave@.example.com'],
  ['a domain with an empty label', 'dave@example..com'],
  ['a domain with hyphens at the ends of a label', 'dave@-example-.com'],
  ['a domain with a label of 63 characters that is not ASCII', `dave@${'ü'.repeat(63)}.example`],
  ['an address of 254 characters', `${'l'.repeat(64)}@${'d'.repeat(63)}.${'e'.repeat(63)}.${'f'.repeat(57)}.org`],
  ['an address of 254 characters that is not ASCII', `${'é'.repeat(64)}@${'ü'.repeat(63)}.${'ö'.repeat(63)}.${'ä'.repeat(57)}.org`],
]
for (const [what, address] of addresses) {
  add(`address: a recipient with ${what}`, { to: [address, 'dave@example.org'], cc: [address], bcc: [address] })
  add(`address: a sender with ${what}`, { from: address })
}

// Subjects.
const word = (length, letter = 'x') => letter.repeat(length)
add('subject: empty', { subject: '' })
for (const length of [1, 66, 67, 68, 69, 75, 76, 77, 142, 143, 144, 255]) {
  add(`subject: one word of ${length} characters`, { subject: word(length) })
  add(`subject: ${length} characters in words of 7`, { subject: repeat(Math.ceil(length / 8), () => word(7)).join(' ').slice(0, length).trim() })
}
for (const length of [58, 59, 60, 66, 67, 68]) {
  add(`subject: a space after ${length} characters`, { subject: `${word(length)} ${word(20)}` })
  add(`subject: three spaces after ${length} characters`, { subject: `${word(length)}   ${word(20)} ${word(80)}` })
}
add('subject: spaces only where a line may not start', { subject: `a ${word(90)} b` })
add('subject: many short words', { subject: repeat(100, (index) => 'ab'[index % 2]).join(' ') })
add('subject: a double quote', { subject: 'He said "hello" twice' })
add('subject: only a double quote', { subject: '"' })
add('subject: what looks like an encoded word', { subject: '=?UTF-8?Q?not_encoded?= really' })
add('subject: equals signs, question marks and underscores with an accent', { subject: 'a=b? c_d é' })
add('subject: a no-break space', { subject: 'prix\u00a0: 10' })
add('subject: a line separator', { subject: 'one\u2028two' })
add('subject: a zero width space', { subject: 'one\u200btwo' })
add('subject: an accent', { subject: 'Réunion' })
add('subject: more accents than letters', { subject: 'éé a' })
add('subject: as many accents as letters', { subject: 'éa' })
add('subject: Chinese', { subject: '你好，世界' })
add('subject: an emoji', { subject: 'Party 🎉' })
add('subject: emoji only', { subject: '🎉🎉🎉' })
add('subject: 255 units of emoji', { subject: `${'🎉'.repeat(127)}!` })
add('subject: 255 accents', { subject: 'é'.repeat(255) })
add('subject: 255 Chinese characters', { subject: '好'.repeat(255) })
add('subject: 251 characters of four kinds', { subject: 'aé好🎉 '.repeat(42).trim() })
for (const length of [12, 13, 14, 15, 18, 19, 20, 21, 38, 39, 40, 41, 42, 43, 44, 79, 80, 81, 82]) {
  add(`subject: Q encoding, ${length} letters and an accent`, { subject: `${word(length, 'a')}é` })
  add(`subject: Q encoding, an accent every third of ${length} letters`, { subject: repeat(length, (index) => (index % 3 === 2 ? 'é' : 'a')).join('') })
  add(`subject: Q encoding, ${length} letters with spaces and an accent`, { subject: `é${repeat(length, (index) => (index % 5 === 4 ? ' ' : 'b')).join('').trim()}` })
}
for (const length of [6, 7, 8, 9, 10, 11, 14, 15, 16, 20, 21, 22, 23, 29, 30, 31, 32]) {
  add(`subject: B encoding, ${length} Chinese characters`, { subject: '好'.repeat(length) })
  add(`subject: B encoding, ${length} emoji and a letter`, { subject: `a${'🎉'.repeat(length)}` })
  add(`subject: B encoding, ${length} accents`, { subject: 'é'.repeat(length) })
}
add('subject: a space before the end of a Q encoded text', { subject: 'é a' })
add('subject: a Q encoded text cut inside a character', { subject: `${word(37, 'a')}好好` })
add('subject: a Q encoded text cut inside an emoji', { subject: `${word(35, 'a')}🎉🎉 ok` })
add('subject: of a reply', { subject: 'Re: Re: Fwd: [list] Minutes of the meeting of October 7th (final version)' })

// Texts.
const text = (name, value, options) => add(`text: ${name}`, { text: value }, options)
text('one line', 'Hello.')
text('one line that ends with a line feed', 'Hello.\n')
text('one line that ends with a carriage return', 'Hello.\r')
text('one line that ends with both', 'Hello.\r\n')
text('lines that end with both', 'one\r\ntwo\r\n\r\nthree')
text('lines that end with a carriage return', 'one\rtwo\r\rthree')
text('lines that end in every way', 'one\ntwo\r\nthree\rfour\n\rfive\r\r\nsix\n\n')
text('only line feeds', '\n\n\n')
text('only a carriage return', '\r')
text('a line that starts with From', 'Hello,\nFrom here on it is easy.\nFrom: me\n>From there')
text('lines that start with a dot', '.\n..\n.hidden\r\n.\r\n. \n')
text('a dot after a carriage return', 'one\r.two\r.\r')
text('tabs', 'one\ttwo\n\tthree\t\n')
text('spaces at the end of lines', 'one \ntwo  \r\nthree \t\rfour ')
text('a line of 76 characters', word(76))
text('a line of 77 characters', word(77))
text('a line of 77 digits', word(77, '1'))
text('a line of 77 digits and one letter', `${word(76, '1')}a`)
text('a line of 77 punctuation marks', word(77, '.'))
text('a line of 76 characters after a short one', `short\n${word(76)}\nshort`)
text('a line of 77 characters after a short one', `short\n${word(77)}\nshort`)
text('a line of 77 characters that ends with a carriage return', `${word(77)}\rshort`)
text('a line of 1000 characters', word(1000))
text('a line of 1000 characters in words', repeat(140, (index) => word(1 + (index % 9), 'abcdefghi'[index % 9])).join(' '))
text('a long line with punctuation', repeat(40, (index) => `Sentence ${index}, with a comma! Is it long? Yes.`).join(' '))
text('a long line with equals signs', repeat(60, (index) => `key${index}=value${index}`).join('&'))
text('a long line of spaces', ' '.repeat(300))
text('a long line of tabs', `${'\t'.repeat(200)}x`)
text('long lines that end with spaces', `${word(80)} \n${word(74)}  \n${word(75)} \n${word(76)}\t\n`)
for (const length of [70, 71, 72, 73, 74, 75, 76, 77, 78]) {
  text(`${length} letters before an accent`, `${word(length)}é${word(30)}`)
  text(`${length} letters before a Chinese character`, `${word(length)}好${word(30)} ok`)
  text(`${length} letters before an emoji`, `${word(length)}🎉${word(30)} ok`)
  text(`${length} letters before an equals sign`, `${word(length)}=${word(30)}`)
  text(`${length} letters before a space`, `${word(length)} ${word(30)}`)
  text(`${length} letters before a dot`, `${word(length)}.${word(30)}`)
  text(`${length} letters before a line feed`, `${word(length)}\n${word(90)}`)
  text(`${length} letters before a carriage return and a line feed`, `${word(length)}\r\n${word(90)}`)
  text(`${length} letters before a carriage return`, `${word(length)}\r${word(90)}`)
}
text('a line feed near the end of a long line', `${word(60)}\n${word(20)}é${word(60)}`)
text('a line feed then a carriage return near the end of a long line', `${word(60)}\n${word(8)}\r${word(60)}é`)
text('accents', 'Voilà un été très chaud, où l’on boit du café glacé.\nÀ bientôt !')
text('accents on a long line', repeat(30, () => 'Voilà un été très chaud.').join(' '))
text('accents only', 'éèàùç'.repeat(40))
text('more accents than letters', 'ééé ab')
text('as many accents as letters', 'éa éa éa')
text('Chinese', '你好，世界。\n这是一封测试邮件。')
text('Chinese on a long line', '这是一封测试邮件。'.repeat(40))
text('Chinese with many letters', `${'这是'.repeat(10)} ${word(50, 'a')}`)
text('emoji', 'Party 🎉🎉🎉 tonight\n')
text('Arabic', 'مرحبا بالعالم\nهذه رسالة اختبار')
text('every kind on one line', 'aé好🎉 '.repeat(60))
text('a null character', 'one\u0000two')
text('a bell character', 'one\u0007two three four five six')
text('a vertical tab and a form feed', 'one\u000btwo\u000cthree')
text('an escape character among many letters', `${word(40, 'a')} \u001b[0m ${word(40, 'b')}`)
text('a delete character', 'one\u007ftwo')
text('a delete character on a long line', `${word(80)}\u007f${word(80)}`)
text('control characters only', '\u0001\u0002\u0003')
text('a C1 control character', 'one\u0085two')
text('a line separator', 'one\u2028two\u2029three')
text('a byte order mark', '\ufeffHello')
text('an equals sign', 'a=b')
text('what looks like quoted-printable', 'caf=C3=A9 =\r\n=20')
text('what looks like a boundary', '----_NmP-0123456789abcdef-Part_1\n----_NmP-0123456789abcdef-Part_1--\n')
text('what looks like headers', 'Subject: no\nBcc: nobody@example.com\n\nbody')
add('text: 100,000 letters on short lines', { textRepeat: ['The quick brown fox.\n', 4761] })
add('text: 100,000 letters on one line', { textRepeat: ['The quick brown fox jumps. ', 3703] })
add('text: 100,000 accents', { textRepeat: ['é', 100_000] })
add('text: 100,000 units of emoji', { textRepeat: ['🎉', 50_000] })
add('text: 100,000 Chinese characters on lines', { textRepeat: ['这是一封测试邮件。这是一封测试邮件。\r\n', 5000] })
add('text: 100,000 characters of accented words', { textRepeat: ['Voilà un été très chaud, où l’on boit du café glacé. ', 1886] })
add('text: 100,000 digits', { textRepeat: ['0123456789', 10_000] })
add('text: 100,000 line feeds', { textRepeat: ['\n', 100_000] })
add('text: 140,000 letters on short lines', { textRepeat: ['The quick brown fox.\n', 6667] }, { isBeyond: true })

// Attachments.
const attached = (name, files, options) => add(`attachment: ${name}`, { attachments: files }, options)
attached('one', [file('quote.pdf', 'application/pdf', '%PDF-1.4')])
attached('ten', repeat(10, (index) => generatedFile(`file-${index}.bin`, undefined, index + 10, 100 + index)))
for (const length of [1, 2, 3, 4, 56, 57, 58, 113, 114, 115, 171, 1000, 58_368, 58_369, 58_370, 100_000]) {
  attached(`${length} bytes`, [generatedFile('data.bin', undefined, length, length)])
}
attached('every byte', [file('bytes.bin', undefined, Buffer.from(repeat(256, (index) => index)))])
attached('line feeds in a text file', [file('lines.txt', 'text/plain', 'one\ntwo\r\nthree\r')])
const names = [
  ['a plain name', 'report.pdf'],
  ['no extension', 'README'],
  ['only an extension', '.gitignore'],
  ['only a known extension', '.pdf'],
  ['a name that is an extension', 'pdf'],
  ['a name that ends with a dot', 'report.'],
  ['a known name that ends with a dot', 'pdf.'],
  ['two dots', '..'],
  ['three dots', '...'],
  ['two dots and an extension', '..pdf'],
  ['several extensions', 'archive.tar.gz'],
  ['an extension in upper case', 'REPORT.PDF'],
  ['an extension with a Kelvin sign', 'movie.M\u212aV'],
  ['a space in the extension', 'report. pdf'],
  ['a question mark after the extension', 'report.pdf?download'],
  ['a question mark before the extension', 'report?.png'],
  ['an unknown extension', 'data.qwertyuiop'],
  ['the star extension', 'data.*'],
  ['spaces', 'my report.pdf'],
  ['a double quote', 'my "final" report.pdf'],
  ['a semicolon', 'report;v2.pdf'],
  ['a percent sign', '100%.txt'],
  ['an apostrophe', "dave's report.pdf"],
  ['an apostrophe without spaces', "dave's.pdf"],
  ['an apostrophe at the start', "'quoted.pdf"],
  ['an apostrophe at the end', "quoted.pdf'"],
  ['parentheses', 'report(1).pdf'],
  ['an equals sign', 'a=b.txt'],
  ['an at sign', 'me@home.txt'],
  ['a comma and a colon', 'a,b:c.txt'],
  ['angle brackets and square brackets', '<a>[b].txt'],
  ['a question mark', 'what?'],
  ['a hyphen at the start', '-rf.txt'],
  ['an asterisk', 'a*b.txt'],
  ['a tilde and an exclamation mark', '~tmp!.txt'],
  ['accents', 'Devis été.pdf'],
  ['one accent', 'é.pdf'],
  ['Chinese', '报告.pdf'],
  ['an emoji', 'party🎉.png'],
  ['a line separator', 'a\u2028b.txt'],
  ['a no-break space', 'a\u00a0b.txt'],
  ['74 characters', `${word(70)}.txt`],
  ['75 characters', `${word(71)}.txt`],
  ['76 characters', `${word(72)}.txt`],
  ['100 characters', `${word(96)}.txt`],
  ['255 characters', `${word(251)}.txt`],
  ['255 characters with spaces', `${repeat(36, () => word(6)).join(' ')}.txt`],
  ['255 characters with an apostrophe every 50', `${repeat(5, () => `${word(49)}'`).join('')}.txt`],
  ['255 characters with a hyphen every 50', `${repeat(5, () => `-${word(49)}`).join('')}.txt`],
  ['255 accents', `${'é'.repeat(251)}.pdf`],
  ['255 units of emoji', `${'🎉'.repeat(125)}.png`],
  ['255 characters with one accent at the end', `${word(250)}é.txt`],
  ['255 characters with one accent at the start', `é${word(250)}.txt`],
  ['255 characters with an accent every 40', `${repeat(6, () => `${word(40)}é`).join('')}.txt`],
  ['255 characters with an accent every 60', `${repeat(4, () => `${word(60)}é`).join('')}.txt`],
  ['255 characters in words with an accent', `${repeat(30, (index) => (index === 17 ? 'été' : word(7))).join(' ')}.txt`],
  ['an accent after 41 characters', `${word(41)}é.txt`],
  ['an accent after 42 characters', `${word(42)}é.txt`],
  ['an accent after 43 characters', `${word(43)}é.txt`],
  ['an accent after 47 characters and spaces', `${word(20)} ${word(26)}é and more words after it.txt`],
  ['an accent then 100 characters', `é${word(100)}.txt`],
  ['an accent then words', `é ${repeat(12, () => word(8)).join(' ')}.txt`],
  ['an emoji after 45 characters', `${word(45)}🎉🎉.png`],
  ['a double quote in 100 characters', `${word(60)}"${word(35)}.txt`],
]
for (const [what, filename] of names) {
  attached(`a name with ${what}`, [file(filename, undefined, 'content')])
}
attached('a name with accents and a type', [file('Devis été.pdf', 'application/pdf', 'content')])
const types = [
  'application/pdf',
  'APPLICATION/PDF',
  'text/plain',
  'TEXT/HTML',
  'image/png',
  'message/rfc822',
  'Message/Partial',
  'multipart/mixed',
  'MULTIPART/Related',
  'multipart/x-zip',
  'x.y-z+w/a_b.c',
  `${word(100)}/${word(100, 'y')}`,
  '1/2',
]
for (const type of types) {
  attached(`given as ${type.length > 40 ? 'a very long type' : type}`, [file('data.bin', type, 'one\ntwo\r\nthree')])
}
for (const extension of ['eml', 'mht', 'mhtml', 'nws', 'mime', 'gzip', 'ustar', 'zip', 'html', 'txt', 'png', 'ccxml', 'a', 'z']) {
  attached(`found to be a .${extension} file`, [file(`data.${extension}`, undefined, 'one\ntwo\r\nthree')])
}
attached('a message with lines that end in every way', [file('mail.eml', undefined, 'From: a@example.com\n\none\ntwo\r\nthree\rfour\n\n.\n.dot\r\n')])
attached('a message that starts with a line feed', [file('mail.eml', 'message/rfc822', '\nbody')])
attached('a message that ends with a carriage return', [file('mail.eml', 'message/rfc822', 'body\r')])
attached('a message that is not text', [file('mail.eml', 'message/rfc822', Buffer.from(repeat(256, (index) => index)))])
attached('a message that looks like a boundary', [file('mail.eml', 'message/rfc822', '----_NmP-0123456789abcdef-Part_1--\r\n')])
attached('a message of 100 KB', [generatedFile('mail.eml', undefined, 7, 100_000)])
attached('two messages', [file('a.eml', undefined, 'first'), file('b.eml', 'message/rfc822', 'second\n')])
attached('two multiparts', [file('a.gzip', undefined, 'first'), file('b', 'multipart/alternative', 'second\n')])
attached('a multipart with accents in its name', [file('été.gzip', undefined, 'content')])
attached('every kind at once', [
  file('report.pdf', 'application/pdf', '%PDF'),
  file('notes.txt', undefined, 'notes'),
  file('mail.eml', undefined, 'Subject: x\n\nbody'),
  file('archive.gzip', undefined, 'gz'),
  file('Devis été.pdf', undefined, '%PDF'),
  file('README', undefined, 'read me'),
])
add('attachment: with a text that needs quoted-printable', { text: 'Voilà.\n', attachments: [file('a.txt', undefined, 'a')] })
add('attachment: with a text that needs base64', { text: '你好', attachments: [file('a.txt', undefined, 'a')] })
add('attachment: with a text that ends with a carriage return', { text: 'Hello.\r', attachments: [file('a.txt', undefined, 'a')] })
add('attachment: with a text that ends with line feeds', { text: 'Hello.\n\n', attachments: [file('a.txt', undefined, 'a')] })

// The headers that tie a reply to its conversation.
const reply = (name, inReplyTo, references, options) =>
  add(`reply: ${name}`, { inReplyTo, references: references ?? [inReplyTo] }, options)
reply('to a message with a plain ID', '<abc@example.com>')
reply('to a message whose ID has no angle brackets', 'abc@example.com')
reply('to a message whose ID opens an angle bracket', '<abc@example.com')
reply('to a message whose ID closes one', 'abc@example.com>')
reply('to a message whose ID is an angle bracket', '<')
reply('to a message whose ID is a space', ' ')
reply('to a message whose ID has spaces', '<abc def@example.com>')
reply('to a message whose ID has spaces around it', '  <abc@example.com>  ')
reply('to a message with two IDs', '<a@example.com> <b@example.com>')
reply('to a message whose ID has a line feed', '<abc@example.com>\nBcc: evil@example.com')
reply('to a message whose ID has a carriage return and a line feed', '<abc@example.com>\r\nBcc: evil@example.com\r\n\r\nbody')
reply('to a message whose ID has a tab', '<abc\t@example.com>')
reply('to a message whose ID has control characters', '<a\u0000b\u0007c\u001bd\u007fe@example.com>')
reply('to a message whose ID is not ASCII', '<été-好-🎉@例え.jp>')
reply('to a message whose ID has a no-break space', '<abc\u00a0def@example.com>')
reply('to a message whose ID has a line separator', '<abc\u2028def@example.com>')
reply('to a message whose ID looks like an encoded word', '<=?UTF-8?Q?abc?=@example.com>')
reply('to a message whose ID has quotes and commas', '<"a,b";c@example.com>')
reply('to a message whose ID is 70 characters', `<${word(55)}@example.com>`)
reply('to a message whose ID is 2,000 characters', `<${word(1985)}@example.com>`)
reply('to a message whose ID is 2,000 characters in words', `<${repeat(250, () => word(7)).join(' ')}>`)
reply('to a message whose ID is 300 emoji', `<${'🎉'.repeat(300)}@example.com>`)
reply('to a message whose ID has nested angle brackets', '<<a@example.com>>')
reply('to a message whose ID has brackets inside', '<a<b>c@example.com>')
add('reply: with no reference', { inReplyTo: '<abc@example.com>', references: [] }, { isBeyond: true })
add('reply: references only', { references: ['<a@example.com>'] })
add('reply: an empty list of references', { references: [] })
add('reply: 20 references', { references: repeat(20, (index) => `<message-${index}@example.com>`) })
add('reply: 20 long references', { references: repeat(20, (index) => `<${word(60 + index)}@example.com>`) })
add('reply: references of Latin-1 characters', { references: ['<\u00e9t\u00e9@ex\u00e4mple.com>', '<\u00ff\u0080\u0085\u009f@x>'] })
add('reply: references with punctuation', { references: ['<"a,b";c@[127.0.0.1]>', "<a!#$%&'*+-/=?^_`{|}~@x>", '<@>', '<a>'] })
add('reply: references with control characters', { references: ['<a\u0000b@x>', '<a\u001fb@x>', '<a\u007fb@x>', '<\u0001>'] })
add('reply: a reference that control characters make empty', { references: ['<\u0001\u0002>', '<a@x>'] })
add('reply: references that fold at the limit', { references: [`<${word(60)}@x>`, `<${word(72)}@x>`, `<${word(73)}@x>`, `<${word(74)}@x>`, '<a@x>'] })

// Dates.
for (const [what, time] of [
  ['the first millisecond of 1970', 0],
  ['the last millisecond of 1969', -1],
  ['a leap day', Date.UTC(2028, 1, 29, 23, 59, 59, 999)],
  ['new year', Date.UTC(2027, 0, 1, 0, 0, 0, 0)],
  ['a day of one digit', Date.UTC(2026, 2, 5, 4, 3, 2, 1)],
  ['the year 999', Date.UTC(999, 11, 31, 12, 0, 0)],
  ['the year 1', -62135596800000],
  ['the year 0', -62167219200000],
  ['a year before 0', -62198755200000],
  ['the year 12026', Date.UTC(12026, 5, 15, 12, 0, 0)],
  ['the year 200000', Date.UTC(200000, 0, 1)],
  ['the year -200000', Date.UTC(-200000, 0, 1)],
]) {
  add(`date: ${what}`, { date: new Date(time) })
}

// A message ID carries the domain of the sender as it was typed.
add('message ID: of a sender whose domain is not ASCII', { from: '用户@例え.jp' })
add('message ID: of a sender whose domain has an emoji', { from: 'dave@😀.example' })

// Mails the tools never ask for, which nodemailer still builds.
add('beyond: no text', { text: '' }, { isBeyond: true })
add('beyond: no text and one attachment', { text: '', attachments: [file('a.pdf', undefined, 'content')] }, { isBeyond: true })
add('beyond: no text and one message attached', { text: '', attachments: [file('a.eml', undefined, 'content\r')] }, { isBeyond: true })
add('beyond: no text and one multipart attached', { text: '', attachments: [file('a.gzip', undefined, 'content')] }, { isBeyond: true })
add('beyond: no text and two attachments', { text: '', attachments: [file('a.pdf', undefined, 'a'), file('b.gzip', undefined, 'b')] }, { isBeyond: true })
add('beyond: an empty attachment', { attachments: [file('a.pdf', undefined, Buffer.alloc(0))] }, { isBeyond: true })
add('beyond: an empty message attached', { attachments: [file('a.eml', undefined, Buffer.alloc(0))] }, { isBeyond: true })
add('beyond: a subject with a tab and a bell', { subject: 'one\ttwo\u0007three' }, { isBeyond: true })
add('beyond: a subject with a line feed', { subject: 'one\r\ntwo\nthree\rfour' }, { isBeyond: true })
add('beyond: a subject with spaces around it', { subject: '  one  ' }, { isBeyond: true })
add('beyond: a file name with a control character', { attachments: [file('a\u0001b.txt', undefined, 'a')] }, { isBeyond: true })
add('beyond: a file name with a line feed', { attachments: [file('a\r\nb.txt', undefined, 'a')] }, { isBeyond: true })
add('beyond: a file name with a tab and an accent', { attachments: [file('é\tb.txt', undefined, 'a')] }, { isBeyond: true })
add('beyond: a file name with a backslash', { attachments: [file('a\\b.txt', undefined, 'a')] }, { isBeyond: true })
add('beyond: a file name with a slash', { attachments: [file('folder/sub/report.pdf', undefined, 'a')] }, { isBeyond: true })
add('beyond: a file name that ends with slashes', { attachments: [file('report.pdf//', undefined, 'a')] }, { isBeyond: true })
add('beyond: a recipient with spaces', { to: ['da ve@example.com', ' dave@example.com '] }, { isBeyond: true })
add('beyond: a recipient with special characters', { to: ['da"ve@example.com', 'da\\ve@example.com', 'a,b@example.com', 'a;b@example.com', '(a)@example.com', 'a:b@example.com', '[a]@example.com'] }, { isBeyond: true })
add('beyond: a recipient in angle brackets', { to: ['<dave@example.com>', 'Dave <dave@example.org>'] }, { isBeyond: true })
add('beyond: a recipient with two at signs', { to: ['dave@evil.example@example.com'] }, { isBeyond: true })
add('beyond: a recipient without a domain', { to: ['dave', 'dave@', 'da ve'] }, { isBeyond: true })
add('beyond: a recipient that is already quoted', { to: ['"da ve"@example.com', '"da\\"ve"@example.com', '"dave@example.com'] }, { isBeyond: true })
add('beyond: a recipient of control characters only', { to: ['\u0001\u0002', 'dave@example.com'] }, { isBeyond: true })
add('beyond: a domain with a colon', { to: ['dave@example.com:25', 'dave@a:b'] }, { isBeyond: true })
add('beyond: a message ID without angle brackets', { messageId: 'no brackets' }, { isBeyond: true })
add('beyond: an empty ID to answer', { inReplyTo: '', references: [''] }, { isBeyond: true })
add('beyond: a subject of spaces', { subject: '   ' }, { isBeyond: true })
const twoFiles = [file('a.txt', undefined, 'a'), file('b.gzip', undefined, 'b')]
for (const [what, baseBoundary] of [
  ['with a space', 'a b'],
  ['that is not ASCII', 'été'],
  ['with a double quote', 'a"b'],
  ['with an apostrophe', "a'b"],
  ['of 80 characters', word(80)],
  ['with control characters', 'a\r\nb\u0000c\u007fd'],
  ['that is not hexadecimal', 'XYZ_-.'],
]) {
  add(`beyond: a boundary ${what}`, { attachments: twoFiles, baseBoundary }, { isBeyond: true })
}

// A generator of mails over the same inputs.

const LETTERS = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ'
const WORDS = ['the', 'quick', 'brown', 'fox', 'jumps', 'over', 'lazy', 'dog', 'Hello', 'Dave', 'report', 'invoice', 'Re:', 'Fwd:', 'meeting', 'October', '2026', '42', 'a', 'I', 'été', 'café', 'naïve', 'Zürich', 'señor', 'Ελληνικά', 'привет', 'שלום', 'مرحبا', '你好', '日本語', '한국어', '🎉', '👍🏽', '🇫🇷', 'e\u0301', '…', '—', '“quoted”', '"quoted"', "it's", '(note)', 'a=b', '50%', 'x?y', 'a_b', '#1', 'C:\\dir', 'a/b', '<tag>', '[x]', 'a;b', 'a,b', 'me@example.com', '=?UTF-8?Q?x?=']
const SEPARATORS = [' ', ' ', ' ', ' ', '  ', '\u00a0', '-', '', '\u2028', '\u3000', '\u200b']
const anyCharacter = () => {
  for (;;) {
    const kind = below(10)
    const point = kind < 4 ? 0x20 + below(0x5f) : kind < 6 ? 0xa0 + below(0x250) : kind < 8 ? below(0x3000) : kind < 9 ? 0x3000 + below(0xd000 - 0x3000) : 0x10000 + below(0x20000)
    if (point >= 0xd800 && point <= 0xdfff) continue
    return String.fromCodePoint(point)
  }
}
/** Cut to a number of UTF-16 units, without cutting a character in two. */
const cut = (value, units) => {
  let result = ''
  for (const character of value) {
    if (result.length + character.length > units) break
    result += character
  }
  return result
}
const cleanLine = (value) => value.replace(/\p{Cc}/gu, '').trim()

function randomLine(maxUnits) {
  const style = below(10)
  const target = pick([0, 3, 10, 20, 40, 60, 66, 67, 68, 76, 80, 120, 200, 255, 255])
  let line = ''
  while (line.length < target) {
    if (style < 6) line += pick(WORDS) + pick(SEPARATORS)
    else if (style < 8) line += pick(LETTERS).repeat(1 + below(12)) + pick([' ', ' ', '', '-'])
    else line += anyCharacter()
  }
  return cleanLine(cut(line, Math.min(target, maxUnits)))
}

const LOCAL_PARTS = ['dave', 'carol', 'thomas.tastet', 'first.last', 'a', 'x_y', "o'brien", 'user+tag', 'UPPER', 'a.b.c', '.dot', 'dot.', 'a..b', '!#$%&*+-/=?^_`{|}~', '用户', 'dévé', 'ελ', 'da😀ve', 'a\u0001b', 'a\u007f', '\u0085x', 'a\u200bb', '12345', '-', '_']
const DOMAIN_LABELS = ['example', 'mail', 'icloud', 'me', 'sub', 'a', 'x-y', 'UPPER', 'bücher', 'café', 'straße', '例え', 'пример', 'مثال', 'דוגמה', '😀', 'ｅｘ', 'cafe\u0301', 'xn--bcher-kva', 'xn--a', 'xn--9ca', '1', '127', '0x7f', '010', 'a_b', "a'b", 'a~b', 'a^b', 'a|b', 'a%b', 'a/b', 'a?b', 'a#b', 'ex\u00adample', 'ex\u200dample', 'ΟΔΟΣ', 'İ', '-a-', 'a\u0002b', '']
const TOP_LABELS = ['com', 'org', 'example', 'fr', 'co.uk', 'jp', 'рф', '中国', 'COM', 'io', '123', 'x1', 'museum', 'xn--p1ai']
function randomAddress() {
  for (;;) {
    const local = chance(80) ? pick(LOCAL_PARTS.slice(0, 12)) + (chance(40) ? String(below(1000)) : '') : pick(LOCAL_PARTS)
    const labels = repeat(1 + below(3), () => (chance(75) ? pick(DOMAIN_LABELS.slice(0, 8)) : pick(DOMAIN_LABELS)))
    const separator = chance(97) ? '.' : pick(['\u3002', '\uff0e', '\uff61'])
    const address = `${local}@${labels.join(separator)}.${chance(85) ? pick(TOP_LABELS.slice(0, 6)) : pick(TOP_LABELS)}`.trim()
    if (address.length <= 254 && ADDRESS.test(address)) return address
  }
}
/** Addresses that differ in more than the case of their letters, as `uniqueAddresses` of the tools leaves them. */
function randomRecipients(count, taken = []) {
  const seen = new Set(taken.map((address) => address.toLowerCase()))
  const list = []
  for (let attempt = 0; list.length < count && attempt < count * 20; attempt++) {
    const address = randomAddress()
    if (!seen.has(address.toLowerCase())) {
      seen.add(address.toLowerCase())
      list.push(address)
    }
  }
  return list
}
const recipientCount = () => (chance(3) ? pick([20, 50]) : pick([0, 0, 1, 1, 1, 1, 1, 2, 2, 3, 5]))

const LINE_ENDS = ['\n', '\n', '\n', '\r\n', '\r\n', '\r', '\n\n', '\r\n\r\n', '']
function randomText() {
  const style = below(12)
  const lines = chance(4) ? 30 : pick([1, 1, 1, 2, 2, 3, 3, 5, 8])
  let value = ''
  for (let index = 0; index < lines; index++) {
    const target = chance(3) ? 400 : pick([0, 5, 5, 10, 20, 20, 30, 40, 60, 70, 75, 76, 77, 78, 80, 100, 150])
    let line = ''
    while (line.length < target) {
      if (style < 4) line += pick(WORDS.slice(0, 20)) + pick([' ', ' ', ' ', ', ', '. ', '! ', '? ', '\t', '='])
      else if (style < 8) line += pick(WORDS) + pick(SEPARATORS)
      else if (style < 9) line += pick(LETTERS).repeat(1 + below(30)) + pick([' ', '', '.', ','])
      else if (style < 10) line += pick(['0', '1', ' ', '.', '-', '9']).repeat(1 + below(20))
      else if (style < 11) line += anyCharacter()
      else line += pick(['\u0000', '\u0007', '\u000b', '\u001b', '\u007f', 'a', 'b ', 'é', '=', '\t'])
    }
    if (chance(8)) line = pick(['From ', '.', '..', '. ', '>From ', '--']) + line
    if (chance(10)) line += pick([' ', '  ', '\t', ' \t'])
    value += cut(line, target + 8) + (index === lines - 1 && chance(50) ? '' : pick(LINE_ENDS))
  }
  return cut(value, 100_000) || 'x'
}

const EXTENSIONS = ['pdf', 'txt', 'png', 'jpg', 'html', 'docx', 'zip', 'eml', 'gzip', 'csv', 'json', 'PDF', 'tar.gz', 'unknown', 'mht', 'ics', 'mp4', 'a', '']
const MEDIA_TYPES = ['application/pdf', 'text/plain', 'text/html', 'image/png', 'application/octet-stream', 'message/rfc822', 'multipart/mixed', 'TEXT/CSV', 'Message/RFC822', 'Multipart/X-Zip', 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'x/y', 'a-b.c+d/e_f']
function randomFilename() {
  for (;;) {
    const style = below(10)
    let base
    if (style < 5) base = pick(['report', 'notes', 'quote', 'Devis été', 'my file', 'photo-2026', 'données', '报告', 'party🎉', "dave's", 'a"b', 'a;b', '100%', '-x', 'a=b', '(1)', 'README'])
    else if (style < 8) base = randomLine(pick([10, 40, 60, 74, 100, 250]))
    else base = pick(LETTERS).repeat(pick([1, 45, 46, 47, 70, 71, 72, 73, 100, 248])) + pick(['', 'é', ' é', '🎉'])
    const extension = pick(EXTENSIONS)
    const filename = cleanLine(cut(`${base}${chance(85) && extension ? `.${extension}` : ''}`.replace(/[\\/]/g, '-'), 255))
    if (filename.length > 0) return filename
  }
}
function randomContent(isSmall) {
  const length = isSmall ? 1 + below(60) : pick([1, 2, 3, 20, 56, 57, 58, 114, 115, 200, 600])
  if (chance(30)) {
    const lines = ['one', 'two\r', '', 'From a', '.', '..x', 'three\r\r', '\u00e9t\u00e9', 'four']
    return Buffer.from(cut(repeat(1 + Math.floor(length / 5), () => pick(lines)).join('\n'), length) || 'x', 'utf8')
  }
  return generatedBytes(random(), length)
}

const ID_PARTS = ['abc', '123', 'message', 'a.b', 'x-y', 'CAF=x', 'é', '好', '🎉', ' ', '\t', '\n', '\r\n', '<', '>', '"', ',', ';', '\u0000', '\u007f', '\u00a0', '\u2028', '@', '@example.com', '%', '$']
const randomId = () => `<${uuid()}@${pick(['example.com', 'mail.gmail.com', 'icloud.com', 'x'])}>`
function hostileId() {
  const value = repeat(1 + below(8), () => pick(ID_PARTS)).join('')
  return (chance(50) ? `<${value}>` : value) || '<x>'
}
function randomReference() {
  for (;;) {
    const inside = repeat(1 + below(4), () => (chance(60) ? pick(['abc', '123', 'a.b', '@', '@example.com', 'x-y']) : String.fromCharCode(pick([0x21 + below(0x5e), 0xa1 + below(0x5e), below(9), 0x0e + below(0x12), 0x7f, 0x80 + below(0x20)])))).join('')
    const reference = `<${inside}>`
    if (/^<[^<>\s]+>$/.test(reference)) return reference
  }
}

function randomMail() {
  const from = chance(60) ? pick(['thomas@icloud.com', 'thomas@me.com', 'Thomas.Tastet@iCloud.com']) : randomAddress()
  const to = randomRecipients(recipientCount())
  const cc = chance(40) ? randomRecipients(recipientCount(), to) : []
  const bcc = chance(40) ? randomRecipients(recipientCount()) : []
  const attachmentCount = chance(3) ? 10 : pick([0, 0, 0, 0, 0, 1, 1, 1, 2, 3])
  const inReplyTo = chance(35) ? (chance(60) ? randomId() : hostileId()) : undefined
  let references
  if (inReplyTo !== undefined || chance(10)) {
    const fromHeader = repeat(pick([0, 0, 1, 2, 5, 19]), () => (chance(70) ? randomId() : randomReference()))
    references = inReplyTo === undefined ? fromHeader : [...fromHeader, inReplyTo]
  }
  return compose({
    from,
    to,
    cc,
    bcc,
    subject: chance(8) ? '' : randomLine(255),
    text: randomText(),
    attachments: repeat(attachmentCount, () => file(randomFilename(), chance(50) ? undefined : pick(MEDIA_TYPES), randomContent(attachmentCount > 3))),
    inReplyTo,
    references,
    date: new Date(chance(95) ? Date.UTC(1990, 0, 1) + below(50 * 365) * 86_400_000 + below(86_400_000) : (below(2) ? 1 : -1) * below(0x7fffffff) * 100_000),
  })
}

// Addresses on their own: `MimeNode._normalizeAddress`, which `_parseAddresses`
// runs on each address and `_convertAddresses` runs again on the result.

const node = new MimeNode()
function written(address) {
  const once = node._normalizeAddress(address)
  // What is written is UTF-8, where a surrogate without its pair becomes U+FFFD.
  return Buffer.from(once ? node._normalizeAddress(once) : '', 'utf8').toString('utf8')
}

const url = await import('node:url')
const keptCharacters = []
for (let point = 0x80; point <= 0x10ffff; point++) {
  if (point >= 0xd800 && point <= 0xdfff) continue
  const character = String.fromCodePoint(point)
  // The characters a label can hold once it is mapped, left-to-right or right-to-left.
  if (url.domainToASCII(`\u0915${character}\u0915.\uff41`) !== '' || url.domainToASCII(`\u05d0${character}\u05d0.\uff41`) !== '') {
    keptCharacters.push(character)
  }
}
const marks = keptCharacters.filter((character) => /\p{M}/u.test(character))
const rightToLeft = keptCharacters.filter((character) => /[\p{Script=Hebrew}\p{Script=Arabic}\p{Script=Syriac}\p{Script=Thaana}\p{Script=Nko}\p{Script=Adlam}]/u.test(character))
// Characters that Node's host parser treats in a way of its own.
const SPECIAL = [0x2d, 0x31, 0x61, 0x7a, 0x41, 0xad, 0xdf, 0xe9, 0x130, 0x301, 0x308, 0x316, 0x323, 0x327, 0x334, 0x341, 0x345, 0x3c2, 0x3a3, 0x5d0, 0x5b0, 0x627, 0x628, 0x648, 0x661, 0x6f1, 0x6cc, 0x710, 0x7ca, 0x840, 0x870, 0x888, 0x897, 0x8b5, 0x8ca, 0x94d, 0x915, 0xe3a, 0x1100, 0x1161, 0x11a8, 0x11c2, 0x1820, 0x1ea1, 0x1eaf, 0x200b, 0x200c, 0x200d, 0x2060, 0x212a, 0x3002, 0xff0e, 0xff11, 0xff21, 0xac00, 0xac01, 0xfffd, 0xe000, 0x378, 0x10d4a, 0x10d69, 0x105d2, 0x113c2, 0x1611e, 0x16d63, 0x16d67, 0x16d68, 0x1e900, 0x1f600, 0x2f800].map((point) => String.fromCodePoint(point))
const LABELS = ['example', 'xn--bcher-kva', 'xn--a', 'xn--', 'xn---9ca', 'xn--xn---epa', 'xn--ls8h', '0x7f', '010', '1', '256', '4294967295', 'a_b', 'a^b', 'a%b', 'a/b', '', '-', 'XN--MNCHEN-3YA']
function sweptAddress() {
  for (;;) {
    const label = () => {
      const kind = below(10)
      if (kind === 0) return pick(LABELS)
      const draw = () => (kind < 5 ? pick(SPECIAL) : kind < 7 ? pick(keptCharacters) : kind < 8 ? pick(marks) : kind < 9 ? pick(rightToLeft) : anyCharacter())
      return repeat(1 + below(5), draw).join('')
    }
    const local = chance(30) ? repeat(1 + below(3), () => pick(SPECIAL)).join('') : 'u'
    const address = `${local}@${repeat(1 + below(3), label).join('.')}.${chance(50) ? 'example' : label()}`.trim()
    if (address.length <= 254 && ADDRESS.test(address)) return address
  }
}

/** Addresses around one code point, each of which reads one thing off Node's host parser. */
const around = (c) => [
  `u@${c}.com`, `u@a${c}b.com`, `\u00e9@a${c}b.com`, `u@x|.${c}a`, `\u00e9@x|.${c}a`, `${c}@example.com`, `a${c}b@example.com`,
  `u@${c}1.com`, `u@\u05d0${c}.com`, `\u00e9@${c}.com`,
  // With a label that only mapping turns into "a", a label the parser refuses shows.
  `u@\u05d0${c}\u05d0.\uff41`, `u@\u05d0${c}.\uff41`, `u@${c}\u05d0.\uff41`, `u@\u0915${c}.\uff41`, `u@\u05d01${c}\u05d0.\uff41`, `u@\u05d0\u0661${c}\u05d0.\uff41`,
  `u@${c}\u0915\u094d\u200d.\uff41`, `u@\u0915${c}\u200d.\uff41`, `u@\u0915${c}\u200c\u1820.\uff41`, `u@\u1820\u200c${c}.\uff41`,
  // Marks before and after it, and what it could compose with.
  `\u00e9@x${c}\u0301.z`, `\u00e9@x\u0301${c}.z`, `\u00e9@x${c}\u0334.z`, `\u00e9@x\u0e3a${c}.z`, `\u00e9@${c}\u0327.z`, `\u00e9@x${c}${c}.z`, `\u00e9@\u00e9${c}.z`, `\u00e9@\uac00${c}.z`,
]
const SWEEP_BLOCK = 0x1000

// Write everything.

async function writeMails(fileName, mails) {
  const lines = []
  for (const [name, mail, options] of mails) lines.push(await fixture(name, mail, options))
  writeFileSync(join(here, fileName), `${lines.join('\n')}\n`)
  console.log(`${fileName}: ${lines.length} mails`)
}

await writeMails('corpus.jsonl', corpus)
// A random mail is only checked by its digests: the corpus is where a message can be read.
await writeMails('random.jsonl', repeat(2200, (index) => [`random ${index}`, randomMail(), { shownUpTo: 0 }]))

writeFileSync(
  join(here, 'addresses.jsonl'),
  `${repeat(12_000, () => {
    const address = sweptAddress()
    return JSON.stringify([address, written(address)])
  }).join('\n')}\n`
)
console.log('addresses.jsonl: 12000 addresses')

const blocks = []
for (let start = 0; start <= 0x10ffff; start += SWEEP_BLOCK) {
  const hash = crypto.createHash('sha256')
  for (let point = start; point < start + SWEEP_BLOCK; point++) {
    if (point >= 0xd800 && point <= 0xdfff) continue
    for (const address of around(String.fromCodePoint(point))) hash.update(`${written(address)}\n`)
  }
  blocks.push(hash.digest('hex'))
}
writeFileSync(join(here, 'address_sweep.json'), `${JSON.stringify({ block: SWEEP_BLOCK, digests: blocks }, null, 1)}\n`)
console.log(`address_sweep.json: ${blocks.length} blocks of ${SWEEP_BLOCK} code points`)
