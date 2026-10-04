import { convert } from 'html-to-text'
import type { FetchMessageObject, MessageAddressObject, MessageStructureObject } from 'imapflow'

const MAX_REFERENCES = 20

/**
 * One bare address. Display names and the characters an address parser could
 * read as a second recipient are refused.
 */
const ADDRESS_PATTERN = /^[^\s@<>(),;:"\\[\]]+@[^\s@<>(),;:"\\[\]]+\.[^\s@<>(),;:"\\[\]]+$/

/** Invisible characters newsletters repeat to pad the preview line of a mail client. */
const PREVIEW_PADDING = /(?:[\u00ad\u034f\u200b-\u200d\u2007\ufeff][ \u00a0]*){3,}/g

export function isAddress(value: string) {
  return value.length <= 254 && ADDRESS_PATTERN.test(value)
}

/** Keep the first spelling of each address. */
export function uniqueAddresses(addresses: string[]) {
  const seen = new Set<string>()
  return addresses.filter((address) => {
    const key = address.toLowerCase()
    return !seen.has(key) && Boolean(seen.add(key))
  })
}

function formatAddress({ name, address }: MessageAddressObject) {
  if (!address) return name ?? ''
  return name && name !== address ? `${name} <${address}>` : address
}

function formatAddresses(list: MessageAddressObject[] | undefined) {
  return (list ?? []).map(formatAddress).filter(Boolean)
}

function isoDate(value: Date | string | undefined) {
  const date = value instanceof Date ? value : new Date(value ?? Number.NaN)
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString()
}

/**
 * Every part that carries content of its own. An attached message counts as
 * one part: its inner parts are not this message's body.
 */
function contentParts(node: MessageStructureObject): MessageStructureObject[] {
  return node.type.startsWith('multipart/') ? (node.childNodes ?? []).flatMap(contentParts) : [node]
}

function filenameOf(part: MessageStructureObject) {
  return part.dispositionParameters?.filename ?? part.parameters?.name
}

function isAttachment(part: MessageStructureObject) {
  return part.disposition === 'attachment' || Boolean(filenameOf(part))
}

/** The part to read as the message text: plain text when there is one, else HTML. */
export function bodyPart(structure: MessageStructureObject) {
  const texts = contentParts(structure).filter((part) => !isAttachment(part))
  const part =
    texts.find(({ type }) => type === 'text/plain') ??
    texts.find(({ type }) => type === 'text/html')
  // A message made of a single part has no part number: its body is part 1.
  return part ? { id: part.part ?? '1', isHtml: part.type === 'text/html' } : null
}

/** Base64 spends 78 bytes, line break included, on every 57 bytes of the file. */
function fileSize({ size = 0, encoding }: MessageStructureObject) {
  return encoding === 'base64' ? Math.round((size * 57) / 78) : size
}

/** `part` identifies an attachment within its message. `size` is approximate. */
export function attachmentsOf(structure: MessageStructureObject | undefined) {
  return (structure ? contentParts(structure) : []).filter(isAttachment).map((part) => ({
    part: part.part ?? '1',
    filename: filenameOf(part) ?? 'untitled',
    content_type: part.type,
    size: fileSize(part),
  }))
}

/** What an agent needs to pick a message out of a list. */
export function messageSummary(message: FetchMessageObject) {
  const { envelope = {}, flags = new Set<string>() } = message
  return {
    uid: message.uid,
    subject: envelope.subject ?? '',
    from: formatAddresses(envelope.from).join(', '),
    to: formatAddresses(envelope.to),
    date: isoDate(envelope.date) ?? isoDate(message.internalDate),
    unread: !flags.has('\\Seen'),
    flagged: flags.has('\\Flagged'),
    answered: flags.has('\\Answered'),
    attachments: attachmentsOf(message.bodyStructure).length,
  }
}

/** The headers of one message, without the fields that are empty. */
export function messageHeaders(message: FetchMessageObject) {
  const { envelope = {} } = message
  const { attachments, ...summary } = messageSummary(message)
  const replyAddress = formatAddresses(envelope.replyTo).join(', ')
  const cc = formatAddresses(envelope.cc)
  const bcc = formatAddresses(envelope.bcc)

  return {
    ...summary,
    ...(replyAddress && replyAddress !== summary.from ? { reply_to: replyAddress } : {}),
    ...(cc.length > 0 ? { cc } : {}),
    ...(bcc.length > 0 ? { bcc } : {}),
    ...(envelope.messageId ? { message_id: envelope.messageId } : {}),
  }
}

export function htmlToText(html: string) {
  return convert(html, {
    wordwrap: false,
    selectors: [
      { selector: 'a', options: { hideLinkHrefIfSameAsText: true } },
      { selector: 'img', format: 'skip' },
    ],
  })
}

/** Normalize line endings and drop the padding and blank runs that only cost tokens. */
export function tidyText(text: string) {
  return text
    .replace(/\r\n?/g, '\n')
    .replace(PREVIEW_PADDING, '')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

/**
 * Sender, recipients, subject, and threading headers of a reply to
 * `original`, which must be fetched with its `References` header: the
 * envelope does not carry it. `ownAddresses` are the account's own.
 */
export function replyTo(original: FetchMessageObject, ownAddresses: string[], replyAll: boolean) {
  const { envelope = {} } = original
  const own = (address: string) =>
    ownAddresses.find((candidate) => candidate.toLowerCase() === address.toLowerCase())
  const isOwn = (address: string) => own(address) !== undefined
  // These come from the message itself, so they are checked like agent input.
  const addresses = (list: MessageAddressObject[] | undefined) =>
    uniqueAddresses((list ?? []).map(({ address }) => address ?? '').filter(isAddress))

  const author = addresses(envelope.replyTo?.length ? envelope.replyTo : envelope.from)
  // Replying to a message you sent continues with the people it was sent to.
  const to = author.every(isOwn) ? addresses(envelope.to) : author
  const everyone = replyAll ? addresses([...(envelope.to ?? []), ...(envelope.cc ?? [])]) : []
  const cc = everyone.filter(
    (address) =>
      !isOwn(address) && !to.some((other) => other.toLowerCase() === address.toLowerCase())
  )

  const subject = envelope.subject ?? ''
  const references = original.headers?.toString('latin1').match(/<[^<>\s]+>/g) ?? []
  return {
    // Answer from the address the message was written to, like a mail app,
    // and follow up on a sent message from the address that sent it.
    from: [...addresses([...(envelope.to ?? []), ...(envelope.cc ?? [])]), ...author]
      .map(own)
      .find(Boolean),
    to,
    cc,
    subject: /^re:/i.test(subject) ? subject : `Re: ${subject}`.trim(),
    inReplyTo: envelope.messageId,
    references: [...references, envelope.messageId]
      .filter((id): id is string => Boolean(id))
      .slice(-MAX_REFERENCES),
  }
}
