import { randomUUID } from 'node:crypto'
import { rm } from 'node:fs/promises'
import { Readable } from 'node:stream'
import app from '@adonisjs/core/services/app'
import type {
  FetchQueryObject,
  MessageEnvelopeObject,
  MessageStructureObject,
  SearchObject,
} from 'imapflow'
import type { SendMailOptions } from 'nodemailer'
import Mcp from '#models/mcp'
import { icloudMailServers, type ImapClient } from '#services/builtin/icloud_mail/connection'
import { saveBuiltinUpload } from '#services/builtin/upload_store'
import McpSecretStore from '#services/mcp_secret_store'
import { createMcp } from '#tests/helpers/factories'

export const icloudMailSignIn = {
  username: 'thomas@icloud.com',
  password: 'abcd-efgh-ijkl-mnop',
}

export const ICLOUD_MAIL_PERMISSIONS = ['read', 'draft', 'send', 'organize']

/** The decoded bytes of the attachment of message 11, as a real server delivers them. */
export const ICLOUD_MAIL_ATTACHMENT = '%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n'

type FakeMessage = {
  uid: number
  flags: string[]
  envelope: MessageEnvelopeObject
  bodyStructure: MessageStructureObject
  /** Decoded content by part number, as a real server returns it once downloaded. */
  parts: Record<string, string>
  references?: string
}

type FakeMailbox = {
  path: string
  specialUse?: string
  flags?: string[]
  messages: FakeMessage[]
  appended: Array<{ raw: string; flags: string[] }>
}

type MockOptions = {
  rejectSignIn?: boolean
  /** The only IMAP username the account takes. It takes any by default. */
  imapUsername?: string
  failAppend?: boolean
  smtp?: (mail: SendMailOptions) => Promise<{ rejected: string[] }>
}

/** Shapes follow what imapflow parses from a real server's ENVELOPE and BODYSTRUCTURE. */
function mailboxes(): FakeMailbox[] {
  return [
    {
      path: 'INBOX',
      specialUse: '\\Inbox',
      appended: [],
      messages: [
        {
          uid: 11,
          flags: [],
          envelope: {
            date: new Date('2026-10-01T09:30:00Z'),
            subject: 'Lunch on Thursday?',
            messageId: '<lunch@example.com>',
            from: [{ name: 'Alice Martin', address: 'alice@example.com' }],
            replyTo: [{ name: 'Alice Martin', address: 'alice@example.com' }],
            to: [{ name: 'Thomas', address: 'thomas@icloud.com' }, { address: 'bob@example.com' }],
            cc: [{ name: 'Carol', address: 'carol@example.com' }],
          },
          bodyStructure: {
            type: 'multipart/mixed',
            childNodes: [
              {
                part: '1',
                type: 'multipart/alternative',
                childNodes: [
                  { part: '1.1', type: 'text/plain', encoding: 'quoted-printable', size: 52 },
                  { part: '1.2', type: 'text/html', encoding: 'quoted-printable', size: 120 },
                ],
              },
              {
                part: '2',
                type: 'application/pdf',
                encoding: 'base64',
                size: 78000,
                disposition: 'attachment',
                dispositionParameters: { filename: 'Menu été.pdf' },
              },
            ],
          },
          parts: {
            '1.1': 'Hi Thomas,\r\n\r\nAre you free on Thursday?\r\n\r\nAlice',
            '1.2': '<p>Hi Thomas,</p><p>Are you free on <b>Thursday</b>?</p><p>Alice</p>',
            '2': ICLOUD_MAIL_ATTACHMENT,
          },
          references: '<root@example.com>',
        },
        {
          uid: 12,
          flags: ['\\Seen'],
          envelope: {
            date: new Date('2026-10-02T06:00:00Z'),
            subject: 'Autumn sale',
            messageId: '<sale@shop.example>',
            from: [{ name: 'Shop', address: 'news@shop.example' }],
            to: [{ address: 'thomas@icloud.com' }],
          },
          bodyStructure: { type: 'text/html', encoding: 'quoted-printable', size: 4096 },
          parts: {
            '1': [
              '<html><head><style>p { color: red }</style></head><body>',
              '<p>Hello‌ ‌ ‌ ‌ </p>',
              '<img src="https://shop.example/pixel.gif">',
              '<p><a href="https://shop.example/deals">See the deals</a></p>',
              '<p><a href="https://shop.example">https://shop.example</a></p>',
              '</body></html>',
            ].join(''),
          },
        },
        {
          uid: 14,
          flags: ['\\Seen', '\\Flagged'],
          envelope: {
            date: new Date('2026-10-03T15:45:00Z'),
            subject: 'Re: Invoice 42',
            messageId: '<invoice@example.com>',
            from: [{ address: 'andre@example.com' }],
            to: [{ address: 'thomas@icloud.com' }],
          },
          bodyStructure: { type: 'text/plain', encoding: '7bit', size: 4000 },
          parts: { '1': `Paid today.\r\n\r\n${'Thanks. '.repeat(400)}` },
        },
      ],
    },
    {
      path: 'Sent Messages',
      specialUse: '\\Sent',
      appended: [],
      messages: [
        {
          uid: 5,
          flags: ['\\Seen'],
          envelope: {
            date: new Date('2026-09-30T08:00:00Z'),
            subject: 'Quote',
            messageId: '<quote@icloud.com>',
            from: [{ address: 'thomas@icloud.com' }],
            to: [{ name: 'Dave', address: 'dave@example.com' }],
          },
          bodyStructure: { type: 'text/plain', encoding: '7bit', size: 30 },
          parts: { '1': 'Here is the quote.' },
        },
      ],
    },
    { path: 'Drafts', specialUse: '\\Drafts', messages: [], appended: [] },
    { path: 'Deleted Messages', specialUse: '\\Trash', messages: [], appended: [] },
    {
      path: 'Archive',
      specialUse: '\\Archive',
      appended: [],
      messages: [
        {
          uid: 3,
          flags: ['\\Seen'],
          envelope: {
            date: new Date('2026-09-20T10:00:00Z'),
            subject: 'Website enquiry',
            messageId: '<enquiry@example.com>',
            from: [{ name: 'Erin', address: 'erin@example.com' }],
            to: [{ address: 'Hello@Thomas.example' }],
          },
          bodyStructure: { type: 'text/plain', encoding: '7bit', size: 40 },
          parts: { '1': 'Do you take new projects?' },
        },
      ],
    },
    { path: 'Projects', flags: ['\\Noselect'], messages: [], appended: [] },
  ]
}

function contains(value: string | undefined, text: string | undefined) {
  return text === undefined || (value ?? '').toLowerCase().includes(text.toLowerCase())
}

/** The criteria the tests filter on. Dates and body text are only recorded. */
function matches(criteria: SearchObject) {
  const uids = criteria.uid === undefined ? null : String(criteria.uid).split(',').map(Number)
  return ({ uid, flags, envelope }: FakeMessage) =>
    (uids === null || uids.includes(uid)) &&
    (criteria.seen === undefined || flags.includes('\\Seen') === criteria.seen) &&
    (criteria.flagged === undefined || flags.includes('\\Flagged') === criteria.flagged) &&
    contains(envelope.subject, criteria.subject) &&
    contains(
      (envelope.from ?? []).map(({ name, address }) => `${name} ${address}`).join(' '),
      criteria.from
    )
}

/**
 * Replace iCloud's IMAP and SMTP servers with an in-memory account. A mailbox
 * selected read-only refuses changes, like the real one.
 */
export function mockIcloudMail(options: MockOptions = {}) {
  const original = { ...icloudMailServers }
  icloudMailServers.imapUsernames.clear()
  const account = mailboxes()
  const calls = {
    signIns: [] as Array<{ username: string; password: string }>,
    logouts: 0,
    locks: [] as Array<{ path: string; readOnly: boolean }>,
    searches: [] as SearchObject[],
    fetches: [] as Array<{ range: unknown; byUid: boolean }>,
    downloads: [] as Array<{ uid: number; part: string | undefined; maxBytes: number }>,
    sent: [] as SendMailOptions[],
  }
  const mailbox = (path: string) => account.find((candidate) => candidate.path === path)!

  icloudMailServers.imap = ({ username, password }) => {
    let selected: FakeMailbox | null = null
    let isReadOnly = false
    const fetched = (message: FakeMessage, index: number, query: FetchQueryObject) => ({
      seq: index + 1,
      uid: message.uid,
      flags: new Set(message.flags),
      envelope: message.envelope,
      bodyStructure: message.bodyStructure,
      headers: query.headers
        ? Buffer.from(message.references ? `References: ${message.references}\r\n\r\n` : '\r\n')
        : undefined,
    })
    const changeFlags = (uids: number[], change: (message: FakeMessage) => void) => {
      if (isReadOnly) throw new Error('The mailbox was selected read-only')
      selected!.messages.filter(({ uid }) => uids.includes(uid)).forEach(change)
      return true
    }

    const client = {
      mailbox: false as false | { path: string; exists: number },
      on: () => client,
      async connect() {
        calls.signIns.push({ username, password })
        if (options.rejectSignIn || (options.imapUsername ?? username) !== username) {
          throw Object.assign(new Error('Command failed'), {
            authenticationFailed: true,
            responseText: 'Authentication failed.',
          })
        }
      },
      async logout() {
        calls.logouts += 1
      },
      close() {},
      async list(listOptions?: { statusQuery?: unknown }) {
        return account.map(({ path, specialUse, flags, messages }) => ({
          path,
          specialUse,
          flags: new Set(flags ?? []),
          status: listOptions?.statusQuery
            ? {
                path,
                messages: messages.length,
                unseen: messages.filter((message) => !message.flags.includes('\\Seen')).length,
              }
            : undefined,
        }))
      },
      async getMailboxLock(path: string, lockOptions?: { readOnly?: boolean }) {
        const found = account.find((candidate) => candidate.path === path)
        if (!found || found.flags?.includes('\\Noselect')) {
          throw Object.assign(new Error('Command failed'), { mailboxMissing: true })
        }
        selected = found
        isReadOnly = Boolean(lockOptions?.readOnly)
        client.mailbox = { path, exists: found.messages.length }
        calls.locks.push({ path, readOnly: isReadOnly })
        return { path, release() {} }
      },
      async search(criteria: SearchObject) {
        calls.searches.push(criteria)
        return selected!.messages.filter(matches(criteria)).map(({ uid }) => uid)
      },
      async fetchAll(range: string | number[], query: FetchQueryObject, fetchOptions?: object) {
        calls.fetches.push({ range, byUid: Boolean((fetchOptions as { uid?: boolean })?.uid) })
        const all = selected!.messages.map((message, index) => fetched(message, index, query))
        if (Array.isArray(range)) {
          return all.filter(({ uid }) => range.includes(uid))
        }
        const [first, last] = range.split(':').map(Number)
        return all.slice(first - 1, last)
      },
      async fetchOne(uid: string, query: FetchQueryObject) {
        const index = selected!.messages.findIndex((message) => message.uid === Number(uid))
        return index < 0 ? false : fetched(selected!.messages[index], index, query)
      },
      async download(uid: string, part: string | undefined, download: { maxBytes: number }) {
        calls.downloads.push({ uid: Number(uid), part, maxBytes: download.maxBytes })
        const content = selected!.messages.find((message) => message.uid === Number(uid))?.parts[
          part ?? ''
        ]
        return content === undefined
          ? {}
          : {
              meta: {},
              content: Readable.from([Buffer.from(content).subarray(0, download.maxBytes)]),
            }
      },
      async messageFlagsAdd(uids: number[], flags: string[]) {
        return changeFlags(uids, (message) => {
          message.flags = [...new Set([...message.flags, ...flags])]
        })
      },
      async messageFlagsRemove(uids: number[], flags: string[]) {
        return changeFlags(uids, (message) => {
          message.flags = message.flags.filter((flag) => !flags.includes(flag))
        })
      },
      async messageMove(uids: number[], destination: string) {
        const target = account.find((candidate) => candidate.path === destination)
        if (!target || isReadOnly) return false

        const uidMap = new Map<number, number>()
        for (const message of selected!.messages.filter(({ uid }) => uids.includes(uid))) {
          const uid = target.messages.length + 1
          uidMap.set(message.uid, uid)
          target.messages.push({ ...message, uid })
        }
        selected!.messages = selected!.messages.filter(({ uid }) => !uids.includes(uid))
        return { path: selected!.path, destination, uidMap }
      },
      async append(path: string, raw: Buffer, flags: string[]) {
        if (options.failAppend) throw new Error('APPEND failed')
        const target = mailbox(path)
        target.appended.push({ raw: raw.toString(), flags })
        return { destination: path, uid: 100 + target.appended.length }
      },
    }
    return client as unknown as ImapClient
  }

  icloudMailServers.smtp = () => ({
    async sendMail(mail) {
      calls.sent.push(mail)
      return options.smtp ? options.smtp(mail) : { rejected: [] }
    },
    close() {},
  })

  return {
    ...calls,
    get logouts() {
      return calls.logouts
    },
    mailbox,
    restore: () => {
      Object.assign(icloudMailServers, original)
    },
  }
}

/** Where the files uploaded for an MCP are kept. Tests look at it to see what is left. */
export function uploadsDirectory(mcpId?: number) {
  return app.tmpPath('builtin-uploads', ...(mcpId === undefined ? [] : [String(mcpId)]))
}

/** Rolled-back tests reuse MCP ids, so what one uploaded must not be found by the next. */
export function clearUploads() {
  return rm(uploadsDirectory(), { recursive: true, force: true })
}

/** Keep a file the way its upload link would, for the tests that are not about the link. */
export async function uploadFile(
  mcpId: number,
  filename: string,
  content: string | Buffer,
  contentType?: string
) {
  const id = randomUUID()
  await saveBuiltinUpload(
    mcpId,
    { id, filename, contentType, maxBytes: 20_000_000 },
    Readable.from([Buffer.from(content)])
  )
  return id
}

/** A built-in iCloud Mail MCP as the setup form leaves it. */
export async function createIcloudMailMcp(
  createdBy: number,
  options: { name?: string; permissions?: string[]; aliases?: string[] } = {}
) {
  const mcp = await createMcp(createdBy, {
    name: options.name ?? 'iCloud Mail',
    transport: 'builtin',
    builtinKey: 'icloud-mail',
  })
  mcp.builtinUsername = icloudMailSignIn.username
  mcp.builtinPassword = McpSecretStore.encrypt(icloudMailSignIn.password)
  mcp.builtinPermissions = (options.permissions ?? ['read']).join(' ')
  mcp.builtinAliases = options.aliases?.join(' ') ?? null
  await mcp.save()
  return Mcp.findOrFail(mcp.id)
}
