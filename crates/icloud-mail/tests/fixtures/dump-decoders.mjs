// Writes what the libraries imapflow 2.2.4 decodes mail with make of a corpus,
// which the unit tests of `src/imap/decode.rs` hold the port to. From the root
// of the repository:
//
//   node crates/icloud-mail/tests/fixtures/dump-decoders.mjs > crates/icloud-mail/tests/fixtures/decoders.json
import { createRequire } from 'node:module'

const pnpm = `${process.cwd()}/node_modules/.pnpm`
const require = createRequire(`${pnpm}/imapflow@2.2.4/node_modules/imapflow/`)
const libmime = require('libmime')
const libqp = require('libqp')
const libbase64 = require('libbase64')
const iconv = require('iconv-lite')
const Headers = require('@zone-eu/mailsplit').Headers

/** The generator of the Node tests: the same texts at every run. */
let seed = 42
const random = (below) => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff
  return seed % below
}
const pick = (alphabet) => alphabet[random(alphabet.length)]
const text = (alphabet, max) => Array.from({ length: random(max) }, () => pick(alphabet)).join('')
const b64 = (value) => Buffer.from(value, 'latin1').toString('base64')

const wordPieces = [
  '=?UTF-8?Q?', '=?utf-8?B?', '=?ISO-8859-1?Q?', '=?iso-8859-1*fr?q?', '=?windows-1252?B?', '=?x-unknown?Q?', '=?utf8?b?', '?=', '?= ', '?=\r\n ',
  'Menu', '_', '=C3=A9', '=C3', '=A9', '=E9', '=', ' ', 'w6k=', 'w6k', 'TWVudQ==', '?', 'été', '"', 'a b', '=?', '=5F', '=3d', '\t',
]
const words = [
  'Lunch on Thursday?',
  '=?UTF-8?Q?Menu_=C3=A9t=C3=A9?=',
  '=?utf-8?B?TWVudSDDqXTDqQ==?=',
  '=?ISO-8859-1?Q?Andr=E9?= Martin',
  '=?UTF-8?B?w6k=?= =?utf8?b?w6k=?=',
  '=?UTF-8?Q?=C3?=\r\n =?UTF-8?Q?=A9t=C3=A9?=',
  '=?UTF-8?Q?a?= =?ISO-8859-1?Q?=E9?= b',
  '=?UTF-8?Q?a?= b =?UTF-8?Q?c?=',
  '=?UTF-8?Q?a?==?UTF-8?B?Yg==?=',
  '=?UTF-8?B?YQ==?= =?UTF-8?Q?b?= =?UTF-8?Q?c?=',
  '=?shift_jis?B?k/qWe4zq?=',
  '=?iso-2022-jp?B?GyRCRnxLXDhsGyhC?=',
  '=?gb2312?B?1tDOxA==?= =?big5?B?pKSk5Q==?= =?koi8-r?Q?=F0=D2=C9=D7=C5=D4?=',
  '=?us-ascii?Q?caf=E9?= =?ascii?Q?caf=E9?= =?latin1?Q?caf=E9?= =?l1?Q?caf=E9?= =?win1252?Q?=93x=94?=',
  '=?UTF-8?Q?=F0=9F=98=80?= =?UTF-8?Q?=F0=9F?= =?UTF-8?Q?=98=80?=',
  '=?UTF-8?Q?a=20b= 3Dc?=',
  '=?UTF-8?X?abc?= =?UTF-8?Q??= =?UTF-8?B??=',
  ...Array.from({ length: 300 }, () => text(wordPieces, 10)),
]

const qpPieces = ['=C3=A9', '=\r\n', '=\n', '=', ' ', '\t', '\r\n', '\n', '\r', 'abc', '=3D', '=3d', '=ZZ', '= ', '=\t\r\n', 'é', '=0', '=A']
const base64Pieces = ['aGVsbG8=', 'aGVs', 'bG8', '=', '==', '\r\n', ' ', 'w6k', 'a', '-_', '!', '+/', 'QUJD', 'Zm9v']
const flowedPieces = ['A line ', 'that goes on', ' ', '\r\n', '\n', '-- ', '> quoted ', ' From', '\r', 'x', '  ']
const names = ['Sent Messages', 'Entwürfe', 'R&D', '日本語', 'Envoyés/été 😀', '&', 'a&b&c', 'Café & Thé', '~peter/mail/台北/日本語', 'tab\there', '']
const charsets = ['utf-8', 'UTF8', 'us-ascii', 'ascii', 'latin1', 'ISO-8859-1', 'iso8859-15', 'windows-1252', 'cp1252', 'win-1251', 'koi8-r', 'shift_jis', 'euc-jp', 'iso-2022-jp', 'gbk', 'gb2312', 'big5', 'euc-kr', 'x-unknown', '', 'l2', '866']
const bytes = ['caf\xe9', 'caf\xc3\xa9', '\x93quoted\x94', '\x93\xfa\x96\x7b\x8c\xea', '\x1b$BF|K\\8l\x1b(B', '\xd6\xd0\xce\xc4', 'plain', '\xf0\xd2\xc9\xd7\xc5\xd4', 'h\x00i\x00']
const headers = [
  'text/plain; charset="ISO-8859-1"; Format=Flowed; delsp=yes',
  'attachment; filename="a; b.pdf"',
  'Quoted-Printable (comment)',
  'text/html;charset=utf-8',
  ' multipart/mixed ; boundary = "x y" ; ',
  'inline; filename=plain.txt; size=12',
  'text/plain; charset="a\\"b"; x',
  '',
]
const headerBlocks = [
  'Content-Type: text/plain;\r\n\tcharset="ISO-8859-1"; Format=Flowed\r\nContent-Transfer-Encoding: Quoted-Printable (comment)\r\n\r\n',
  'content-type:text/html\ncontent-transfer-encoding:  BASE64  \n\n',
  'X-Other: 1\r\nContent-Type: text/plain;\r\n charset=utf-8\r\n  ; format=flowed\r\nContent-Disposition: attachment;\r\n filename="caf\xe9.txt"\r\n',
  'Content-Disposition: inline; filename="caf\xc3\xa9.txt"\r\nContent-Type: application/pdf\r\n\r\n',
  '',
]

const fixture = {
  words: words.map((word) => [word, libmime.decodeWords(word)]),
  quotedPrintable: Array.from({ length: 300 }, () => text(qpPieces, 12)).map((value) => [b64(value), libqp.decode(Buffer.from(value, 'latin1')).toString('base64')]),
  base64: Array.from({ length: 300 }, () => text(base64Pieces, 8)).map((value) => [value, libbase64.decode(value).toString('base64')]),
  flowed: Array.from({ length: 300 }, () => [text(flowedPieces, 12), random(2) === 1]).map(([value, delSp]) => [b64(value), delSp, b64(libmime.decodeFlowed(value, delSp))]),
  utf7: names.map((name) => [name, iconv.encode(name, 'utf-7-imap').toString(), iconv.decode(iconv.encode(name, 'utf-7-imap'), 'utf-7-imap')]),
  // Half a pair of surrogates cannot be written in JSON for Rust to read: what Japanese decoders make of bytes that are not Japanese.
  charsets: charsets.flatMap((charset) => bytes.map((value) => [charset, b64(value), libmime.decodeWord(charset || 'x', 'B', b64(value))])).filter(([, , decoded]) => decoded.isWellFormed()),
  headerValues: headers.map((header) => {
    const { value, params } = libmime.parseHeaderValue(header)
    return [header, value || '', params.charset ?? null, params.format ?? null, params.filename ?? null]
  }),
  headerBlocks: headerBlocks.map((block) => {
    const parsed = new Headers(Buffer.from(block, 'latin1'))
    return [b64(block), parsed.getFirst('Content-Type'), parsed.getFirst('content-transfer-encoding'), parsed.getFirst('Content-Disposition')]
  }),
}
process.stdout.write(`${JSON.stringify(fixture)}\n`)
