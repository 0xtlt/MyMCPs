// Writes what imapflow 2.2.4 makes of IMAP responses, which the unit tests of
// `src/imap/parse.rs` hold the port to. From the root of the repository:
//
//   node crates/icloud-mail/tests/fixtures/dump-imap-responses.mjs > crates/icloud-mail/tests/fixtures/imap_responses.json
import { pathToFileURL } from 'node:url'

const imapflow = new URL('./node_modules/.pnpm/imapflow@2.2.4/node_modules/imapflow/dist/esm/', pathToFileURL(`${process.cwd()}/`))
const { default: parser } = await import(new URL('handler/imap-parser.js', imapflow).href)
const { formatMessageResponse } = await import(new URL('tools.js', imapflow).href)

/** Responses as a server writes them. A literal follows its `{n}` and a line break. */
const responses = [
  // Status responses and their codes.
  '* OK [CAPABILITY IMAP4rev1 SASL-IR AUTH=ATOKEN AUTH=PLAIN] iCloud IMAP ready',
  '* OK iCloud ready',
  '* PREAUTH [CAPABILITY IMAP4rev1] Logged in as thomas',
  '* BYE Autologout; idle for too long',
  'A1 OK [CAPABILITY XAPPLEPUSHSERVICE IMAP4 IMAP4rev1 ACL QUOTA LITERAL+ NAMESPACE UIDPLUS CHILDREN BINARY UNSELECT SORT ESEARCH ID MOVE SPECIAL-USE LIST-STATUS] user thomas authenticated',
  'A2 NO [AUTHENTICATIONFAILED] Authentication failed.',
  'A3 BAD Parse Error (at 5th argument)',
  '3 NO [TRYCREATE] Mailbox doesn\'t exist: Nope',
  '4 OK [READ-ONLY] EXAMINE completed',
  '5 OK [APPENDUID 1700000000 101] APPEND completed',
  '* OK [COPYUID 1700000000 12,14 1:2] Moved',
  '6 OK [COPYUID 7 3:5 20:22] Done',
  '* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)] Flags permitted.',
  '* OK [PERMANENTFLAGS ()] No permanent flags permitted.',
  '* OK [UIDVALIDITY 1700000000] UIDs valid',
  '* OK [UNSEEN 12] Message 12 is first unseen',
  '* OK [ALERT] System shutdown in 10 minutes [really]',
  '* OK [REFERRAL imap://user;AUTH=*@SERVER2/] Remote Server',
  'A7 NO [OVERQUOTA] Quota exceeded (mailbox for user is full)',
  'A8 BAD Request is throttled. Suggested Backoff Time: 92415 milliseconds',
  'A9 NO Some of the requested messages no longer exist',
  '+ Ready for literal data',
  '+',
  '+ VXNlcm5hbWU6',
  // Mailbox listings.
  '* LIST (\\HasNoChildren) "/" "INBOX"',
  '* LIST (\\HasNoChildren \\Sent) "/" "Sent Messages"',
  '* LIST (\\Noselect \\HasChildren) "/" "Projects"',
  '* LIST (\\HasNoChildren \\Trash) "/" "Deleted Messages"',
  '* LIST () NIL INBOX',
  '* LIST (\\HasNoChildren) "." INBOX.Entw&APw-rfe',
  '* LIST (\\HasNoChildren) "/" "A \\"quoted\\" name \\\\ with a backslash"',
  '* LIST (\\HasNoChildren) "/" {12}\r\nliteral name',
  '* LIST (\\NonExistent \\Subscribed) "/" "Old"',
  '* XLIST (\\HasNoChildren \\Inbox) "/" "Posteingang"',
  '* STATUS "Sent Messages" (MESSAGES 1 UNSEEN 0)',
  '* STATUS INBOX (MESSAGES 231 UIDNEXT 44292 UNSEEN 17)',
  '* NAMESPACE (("" "/")) NIL NIL',
  '* NAMESPACE (("INBOX." ".")) NIL (("#shared." ".")("#public" "/"))',
  '* CAPABILITY IMAP4rev1 LITERAL+ SASL-IR LOGIN-REFERRALS ID ENABLE IDLE AUTH=PLAIN',
  '* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft $Forwarded $MDNSent)',
  '* 3 EXISTS',
  '* 0 RECENT',
  '* 2 EXPUNGE',
  '* SEARCH 11 14',
  '* SEARCH',
  '* ESEARCH (TAG "A5") UID ALL 1:3,5',
  '* ESEARCH (TAG "A6") UID COUNT 0',
  // What a server may write that is not quite IMAP.
  '* LIST (\\Seen\\Flagged) "/" x',
  '* OK Still here',
  '* BAD Could not parse command',
  '* 1 FETCH (FLAGS (\\Seen) UID 5)',
  '* 12 FETCH (UID 14 FLAGS (\\Seen \\Flagged $Junk))',
  '* 1 FETCH (UID 11 RFC822.SIZE 4096 INTERNALDATE "01-Oct-2026 09:31:00 +0000")',
  '* 1 FETCH (UID 11 INTERNALDATE " 1-Oct-2026 11:31:00 +0200")',
  // Envelopes.
  '* 1 FETCH (UID 11 ENVELOPE ("Thu, 01 Oct 2026 09:30:00 +0000" "Lunch on Thursday?" (("Alice Martin" NIL "alice" "example.com")) (("Alice Martin" NIL "alice" "example.com")) (("Alice Martin" NIL "alice" "example.com")) (("Thomas" NIL "thomas" "icloud.com")(NIL NIL "bob" "example.com")) (("Carol" NIL "carol" "example.com")) NIL "<root@example.com>" "<lunch@example.com>"))',
  '* 2 FETCH (UID 12 ENVELOPE ("Fri, 2 Oct 2026 08:00:00 +0200 (CEST)" "=?UTF-8?Q?D=C3=A9jeuner_jeudi_=3F?=" (("=?UTF-8?B?QW5kcsOpIE1hcnRpbg==?=" NIL "andre" "example.com")) NIL NIL ((NIL NIL "thomas" "icloud.com")) NIL NIL NIL "<dejeuner@example.com>"))',
  '* 3 FETCH (UID 13 ENVELOPE (NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL))',
  '* 4 FETCH (UID 14 ENVELOPE ("not a date" "" ((NIL NIL "undisclosed-recipients" NIL)(NIL NIL NIL NIL)) NIL NIL ((NIL NIL "friends" NIL)("Bob" NIL "bob" "example.com")(NIL NIL NIL NIL)) NIL NIL " <a@b> " "  <spaced@example.com>  "))',
  '* 5 FETCH (UID 15 ENVELOPE ("Sat, 03 Oct 2026 15:45:00 -0700" {21}\r\nA "quoted" \\ subject (("\\"Quoted Name\\"" NIL "q" "example.com")("=?ISO-8859-1?Q?Andr=E9?= =?ISO-8859-1?Q?_Martin?=" NIL "andre" "example.com")) NIL NIL NIL NIL NIL NIL {17}\r\n<lit@example.com>))',
  '* 6 FETCH (UID 16 ENVELOPE ("Sun, 4 Oct 26 10:00 GMT" "=?UTF-8?B?8J+YgCBlbW9qaQ==?= =?UTF-8?B?IGFuZCBtb3Jl?=" (("=?utf-8?q?a?= =?iso-8859-1?q?=E9?=" NIL "a" "b.c")) NIL NIL (NIL ("only" NIL "one" "example.com")) NIL NIL NIL NIL))',
  '* 7 FETCH (UID 17 ENVELOPE ("Mon, 05 Oct 2026 00:00:00 +0000" "=?shift_jis?B?k/qWe4zq?= =?unknown-8bit?Q?=E9?= =?utf-8?Q?broken" ((NIL NIL "no-name" "example.com")("" NIL "empty-name" "example.com")("same@example.com" NIL "same" "example.com")) NIL NIL NIL NIL NIL NIL NIL))',
  // Structures.
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("text" "plain" ("charset" "utf-8") NIL NIL "7bit" 4000 82 NIL NIL NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("TEXT" "HTML" ("CHARSET" "UTF-8") NIL NIL "QUOTED-PRINTABLE" 4096 91 NIL NIL NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("text" "plain" NIL NIL NIL "7bit" 12 1))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("text" "html" ("charset" "utf-8") NIL NIL "7bit" 4096 NIL ("inline" NIL) NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ((("text" "plain" ("charset" "utf-8") NIL NIL "quoted-printable" 52 3 NIL NIL NIL NIL)("text" "html" ("charset" "utf-8") NIL NIL "quoted-printable" 120 4 NIL NIL NIL NIL) "alternative" ("boundary" "b2") NIL NIL NIL)("application" "pdf" ("name" "=?UTF-8?Q?Menu_=C3=A9t=C3=A9.pdf?=") NIL NIL "base64" 78000 NIL ("attachment" ("filename" "=?UTF-8?Q?Menu_=C3=A9t=C3=A9.pdf?=")) NIL NIL) "mixed" ("boundary" "b1") NIL NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE (("text" "plain" NIL NIL NIL "7bit" 10 1 NIL NIL NIL NIL)("application" "pdf" NIL NIL NIL "base64" 780 NIL ("ATTACHMENT" ("FILENAME*" "utf-8\'\'Devis%20%C3%A9t%C3%A9.pdf")) NIL NIL)("application" "zip" NIL NIL NIL "base64" 99 NIL ("attachment" ("filename*0*" "iso-8859-1\'fr\'archive%20" "filename*1*" "%E9t%E9" "filename*2" ".zip")) NIL NIL)("image" "png" ("name*0" "long name " "name*1" "in pieces.png") "<cid@x>" "A picture" "base64" 300000 NIL ("inline" ("filename" "utf-8\'\'misplaced%20%C3%A9.png")) NIL NIL) "mixed"))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE (("text" "plain" NIL NIL NIL "7bit" 10 1)("message" "rfc822" ("name" "Fwd.eml") NIL NIL "7bit" 9000 ("Thu, 01 Oct 2026 09:30:00 +0000" "Fwd" (("A" NIL "a" "b.c")) NIL NIL NIL NIL NIL NIL "<f@b.c>") (("text" "plain" NIL NIL NIL "7bit" 500 10)("image" "jpeg" NIL NIL NIL "base64" 100) "mixed") 200 NIL ("attachment" NIL) NIL NIL)("application" "octet-stream" NIL NIL NIL "base64" 5) "mixed" ("boundary" "x") NIL ("en" "fr") "location"))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("application" "octet-stream" ("name" "a b.bin" "x-unix-mode" "0644") NIL NIL "BASE64" 10 "md5md5" ("attachment" ("filename" "a b.bin" "size" "10" "creation-date" "Thu, 01 Oct 2026 09:30:00 GMT")) "en" NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ((NIL NIL NIL NIL NIL NIL 0 0) "mixed"))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("text" "calendar" ("method" "REQUEST" "charset" "utf-8" "name" "invite.ics") NIL NIL "8bit" 2048 40 NIL ("attachment" ("filename" "")) NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE ("application" "pdf" ("name" {11}\r\nliteral.pdf) NIL NIL "base64" 10 NIL NIL NIL NIL))',
  '* 1 FETCH (UID 11 BODYSTRUCTURE NIL ENVELOPE NIL)',
  // Content.
  '* 1 FETCH (UID 11 BODY[HEADER.FIELDS (References)] {34}\r\nReferences: <root@example.com>\r\n\r\n)',
  '* 1 FETCH (UID 11 BODY[HEADER.FIELDS (REFERENCES)] {2}\r\n\r\n)',
  '* 1 FETCH (UID 11 RFC822.SIZE 9000 BODY[1.2.MIME] {31}\r\nContent-Type: text/html\r\n\r\n BODY[1.2]<0> {5}\r\nhello)',
  '* 1 FETCH (UID 11 BODY[TEXT]<65536> {3}\r\nabc BODY[HEADER] {4}\r\nA: b)',
  '* 1 FETCH (UID 11 BODY[2] NIL)',
  '* 1 FETCH (UID 11 BODY[1] "quoted body" BODY[] {0}\r\n)',
  '* 1 FETCH (BODY[1]<0> {4}\r\n\xff\xfe\r\n UID 11)',
  // What cannot be read.
  '* LIST (\\Sent "/" x',
  '* LIST ) x',
  '* SEARCH "unterminated',
  '* 1 FETCH (UID 11 BODY[1] {5}\r\nabc)',
  'A1 OK [x',
  '* X ' + '('.repeat(40),
]

/** `imap-stream.js`: a line that ends in `{n}` goes on after n bytes of literal. */
function frame(wire) {
  const literals = []
  let payload = Buffer.alloc(0)
  let rest = wire
  for (;;) {
    const marker = /\{(\d+)\}\r\n/.exec(rest.toString('latin1'))
    if (!marker) break
    const end = marker.index + marker[0].length
    const size = Number(marker[1])
    payload = Buffer.concat([payload, rest.subarray(0, end)])
    literals.push(rest.subarray(end, end + size))
    rest = rest.subarray(end + size)
  }
  return { payload: Buffer.concat([payload, rest]), literals }
}

function token(value) {
  if (value === null || value === undefined) return null
  if (Array.isArray(value)) return value.map(token)
  const written = { t: value.type }
  if (Buffer.isBuffer(value.value)) written.b64 = value.value.toString('base64')
  else if (value.type === 'LITERAL') written.b64 = ''
  else written.v = value.value
  if (value.section) written.section = value.section.map(token)
  if (value.partial) written.partial = value.partial
  return written
}

const date = (value) => (value instanceof Date ? value.toISOString() : null)
const addresses = (list) => (list ?? []).map(({ name, address }) => ({ name: name || '', address: address || '' }))

function structure(node) {
  return {
    part: node.part ?? null,
    type: node.type,
    parameters: node.parameters ?? {},
    encoding: node.encoding ?? null,
    size: node.size ?? null,
    disposition: node.disposition ?? null,
    dispositionParameters: node.dispositionParameters ?? {},
    childNodes: node.childNodes ? node.childNodes.map(structure) : null,
  }
}

function message(fetched) {
  const { envelope, bodyStructure } = fetched
  return {
    uid: fetched.uid ?? null,
    flags: fetched.flags ? [...fetched.flags].sort() : [],
    size: fetched.size ?? null,
    internalDate: date(fetched.internalDate),
    envelope: envelope
      ? {
          date: date(envelope.date),
          subject: envelope.subject ?? null,
          messageId: envelope.messageId ?? null,
          inReplyTo: envelope.inReplyTo ?? null,
          from: addresses(envelope.from),
          sender: addresses(envelope.sender),
          replyTo: addresses(envelope.replyTo),
          to: addresses(envelope.to),
          cc: addresses(envelope.cc),
          bcc: addresses(envelope.bcc),
        }
      : null,
    bodyStructure: bodyStructure ? structure(bodyStructure) : null,
    headers: fetched.headers ? fetched.headers.toString('base64') : null,
    bodyParts: Object.fromEntries(
      [...(fetched.bodyParts ?? new Map())].map(([key, value]) => [key, value ? value.toString('base64') : null])
    ),
  }
}

const cases = []
for (const response of responses) {
  const { payload, literals } = frame(Buffer.from(response, 'latin1'))
  const written = { payload: payload.toString('base64'), literals: literals.map((literal) => literal.toString('base64')) }
  try {
    const parsed = await parser(payload, { literals: [...literals] })
    written.response = { tag: parsed.tag, command: parsed.command, attributes: (parsed.attributes ?? []).map(token) }
    const isFetch = /^\d+$/.test(parsed.command) && parsed.attributes?.[0]?.value === 'FETCH'
    if (isFetch) written.message = message(await formatMessageResponse(parsed, { path: 'INBOX' }))
  } catch (error) {
    written.error = error.code ?? String(error)
  }
  cases.push(written)
}
process.stdout.write(`${JSON.stringify(cases, null, 1)}\n`)
