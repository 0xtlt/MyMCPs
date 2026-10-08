// Shows what the SMTP transport of nodemailer 10.0.14 writes to a server, by
// sending a few mails to one on localhost that offers this or that extension.
//
//   node crates/icloud-mail/tools/smtp_probe.mjs [path to node_modules/nodemailer]
//
// from the root of a checkout that has the Node version's node_modules.
//
// It prints the commands of each session, and whether the bytes after DATA
// are the `DataStream` of smtp-connection applied to the message built
// without `keepBcc`.
import crypto from 'node:crypto'
import net from 'node:net'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const nodemailerPath = resolve(
  process.argv[2] ??
    'node_modules/.pnpm/nodemailer@10.0.14/node_modules/nodemailer'
)
const library = (file) => import(pathToFileURL(join(nodemailerPath, 'dist/esm', file)).href)
const nodemailer = await library('nodemailer.js')
const { default: MailComposer } = await library('mail-composer/index.js')
const { default: DataStream } = await library('smtp-connection/data-stream.js')

// The same boundary for the message sent and the one it is compared with.
crypto.randomBytes = () => Buffer.from('0123456789abcdef', 'hex')

function fakeServer(extensions, replies = {}) {
  const session = { commands: [], data: null }
  const server = net.createServer((socket) => {
    let buffer = Buffer.alloc(0)
    let inData = false
    socket.write('220 fake ESMTP\r\n')
    socket.on('data', (chunk) => {
      buffer = Buffer.concat([buffer, chunk])
      for (;;) {
        if (inData) {
          const end = buffer.indexOf('\r\n.\r\n')
          // The terminator may also be the very first bytes.
          const at = buffer.subarray(0, 3).toString() === '.\r\n' ? -2 : end
          if (at === -1) return
          session.data = buffer.subarray(0, at + 5)
          buffer = buffer.subarray(at + 5)
          inData = false
          socket.write('250 2.0.0 queued\r\n')
          continue
        }
        const eol = buffer.indexOf('\r\n')
        if (eol === -1) return
        const line = buffer.subarray(0, eol).toString('utf8')
        buffer = buffer.subarray(eol + 2)
        session.commands.push(line)
        const verb = line.split(/[ :]/)[0].toUpperCase()
        if (verb === 'EHLO') socket.write(['250-fake', ...extensions.map((e) => '250-' + e), '250 OK'].join('\r\n') + '\r\n')
        else if (verb === 'MAIL') socket.write((replies.mail || '250 2.1.0 ok') + '\r\n')
        else if (verb === 'RCPT') socket.write((replies.rcpt ? replies.rcpt(line) : '250 2.1.5 ok') + '\r\n')
        else if (verb === 'DATA') { socket.write('354 go\r\n'); inData = true }
        else if (verb === 'QUIT') { socket.write('221 bye\r\n'); socket.end() }
        else socket.write('250 ok\r\n')
      }
    })
    socket.on('error', () => {})
  })
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, session, port: server.address().port })))
}

const recipients = (addresses) => addresses.map((address) => ({ name: '', address }))
const options = (mail) => ({ ...mail, from: { name: '', address: mail.from }, to: recipients(mail.to), cc: recipients(mail.cc), bcc: recipients(mail.bcc), xMailer: false, newline: 'windows', disableFileAccess: true, disableUrlAccess: true })

async function send(mail, extensions, replies) {
  const { server, session, port } = await fakeServer(extensions, replies)
  const transport = nodemailer.createTransport({ host: '127.0.0.1', port, secure: false, ignoreTLS: true, disableFileAccess: true, disableUrlAccess: true })
  let info, error
  try { info = await transport.sendMail(options(mail)) } catch (e) { error = e }
  transport.close()
  await new Promise((r) => server.close(r))
  return { session, info, error }
}

const base = {
  from: 'thomas@icloud.com', to: ['dave@example.com'], cc: ['carol@example.com'], bcc: ['boss@example.com'],
  subject: 'Quote for October', text: 'Hello Dave,\n\nHere is the quote.\n.\n..leading dots\r\nlone\rcr and .dot\r.after cr\nend',
  attachments: [], inReplyTo: undefined, references: undefined, messageId: '<5f1c9f0e-1b1a-4b6e-9c57-000000000000@icloud.com>', date: new Date(Date.UTC(2026, 9, 7, 8, 9, 10, 123)),
}
const build = async (mail, keepBcc) => { const m = new MailComposer(options(mail)).compile(); m.keepBcc = keepBcc; return m.build() }
const dotStuff = (message) => new Promise((resolve) => { const s = new DataStream(); const chunks = []; s.on('data', (c) => chunks.push(c)); s.on('end', () => resolve(Buffer.concat(chunks))); s.end(message) })

for (const [label, mail, extensions, replies] of [
  ['plain, all extensions', base, ['PIPELINING', 'SIZE 28311552', '8BITMIME', 'SMTPUTF8', 'DSN', 'ENHANCEDSTATUSCODES']],
  ['plain, no extensions', base, []],
  ['utf8 text, 8BITMIME offered', { ...base, text: 'héllo wörld' }, ['8BITMIME', 'SIZE 100']],
  ['unicode recipient, SMTPUTF8 offered', { ...base, to: ['用户@例え.jp', 'dave@bücher.example'] }, ['SMTPUTF8', '8BITMIME']],
  ['unicode recipient, SMTPUTF8 not offered', { ...base, to: ['用户@例え.jp'] }, ['8BITMIME']],
  ['unicode sender', { ...base, from: 'thömas@icloud.com' }, ['SMTPUTF8']],
  ['quoted local part', { ...base, to: ['a..b@example.com', 'x\u0001y@example.com'] }, ['SMTPUTF8']],
  ['no recipients', { ...base, to: [], cc: [], bcc: [] }, ['SMTPUTF8']],
  ['only bcc', { ...base, to: [], cc: [] }, []],
  ['duplicates', { ...base, to: ['a@example.com', 'A@example.com', 'a@EXAMPLE.com'], cc: ['b@example.com'], bcc: ['a@example.com', 'b@example.com'] }, ['PIPELINING']],
  ['one rejected', { ...base, to: ['good@example.com', 'bad@example.com'] }, ['PIPELINING'], { rcpt: (line) => (line.includes('bad@') ? '550 5.1.1 no such user' : '250 ok') }],
  ['all rejected', base, [], { rcpt: () => '550 5.1.1 no' }],
  ['big size vs SIZE', { ...base, attachments: [{ filename: 'a.bin', contentType: undefined, content: Buffer.alloc(3000, 65) }] }, ['SIZE 1000']],
]) {
  const { session, info, error } = await send(mail, extensions, replies)
  console.log('== ' + label)
  for (const command of session.commands) console.log('   C: ' + JSON.stringify(command))
  if (error) console.log('   error:', error.code, error.message, error.command || '')
  if (info) console.log('   accepted', JSON.stringify(info.accepted), 'rejected', JSON.stringify(info.rejected), 'envelope', JSON.stringify(info.envelope))
  if (session.data) {
    const dropped = await build(mail, false)
    const kept = await build(mail, true)
    const expected = await dotStuff(dropped)
    console.log('   DATA bytes', session.data.length, '== DataStream(build(keepBcc=false)):', session.data.equals(expected), '| unstuffed == build(false):', session.data.equals(Buffer.concat([dropped, Buffer.from('.\r\n')])), '| has Bcc:', session.data.includes('Bcc:'), '| kept has Bcc:', kept.includes('Bcc:'))
    if (label.startsWith('plain, all')) console.log(JSON.stringify(session.data.toString('latin1')))
  }
}
// streamTransport
{
  const t = nodemailer.createTransport({ streamTransport: true, newline: 'windows', buffer: true })
  const info = await t.sendMail(options(base))
  console.log('streamTransport == build(keepBcc=true):', info.message.equals(await build(base, true)), ' == build(false):', info.message.equals(await build(base, false)), JSON.stringify(info.envelope))
}
