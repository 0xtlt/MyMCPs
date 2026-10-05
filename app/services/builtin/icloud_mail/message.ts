import type { FetchMessageObject, MessageAddressObject, MessageStructureObject } from 'imapflow'

const MAX_REFERENCES = 20

/**
 * How much of what a sender controls is repeated to the agent, so that one
 * message cannot fill its context. A field that was cut is flagged with
 * `<field>_truncated`.
 */
const MAX_LISTED_ADDRESSES = 50
const MAX_ADDRESS_CHARS = 320
/** RFC 5322 allows 998 characters on a line. */
const MAX_HEADER_CHARS = 998
const MAX_NAME_CHARS = 255
export const MAX_LISTED_ATTACHMENTS = 100

/**
 * One bare address. Display names and the characters an address parser could
 * read as a second recipient are refused. The domain is matched up to its
 * first dot that is not its first character, so that there is one way to read
 * it: a message can name thousands of addresses written to be slow to refuse.
 */
const ADDRESS_PATTERN =
  /^[^\s@<>(),;:"\\[\]]+@[^\s@<>(),;:"\\[\]][^\s@<>(),;:"\\[\].]*\.[^\s@<>(),;:"\\[\]]+$/

/**
 * Invisible characters newsletters repeat to pad the preview line of a mail
 * client: three or more, with spaces in between. What follows the third is
 * matched by a single class, which keeps a run of any length off the regex stack.
 */
const PREVIEW_PADDING =
  /(?:[\u00ad\u034f\u200b-\u200d\u2007\ufeff][ \u00a0]*){3}[\u00ad\u034f\u200b-\u200d\u2007\ufeff \u00a0]*/g

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

/** Sender-controlled text, cut to `max` characters. */
function clip(text: string, max: number) {
  return text.length > max ? `${text.slice(0, max)}…` : text
}

/** `<field>_truncated` for each field that was cut. */
function truncated(fields: Record<string, boolean>) {
  return Object.fromEntries(
    Object.entries(fields)
      .filter(([, isCut]) => isCut)
      .map(([field]) => [`${field}_truncated`, true])
  )
}

/** The addresses of one header, and whether some were left out or shortened. */
function listAddresses(list: MessageAddressObject[] | undefined) {
  const all = (list ?? []).map(formatAddress).filter(Boolean)
  const listed = all
    .slice(0, MAX_LISTED_ADDRESSES)
    .map((address) => clip(address, MAX_ADDRESS_CHARS))
  return {
    listed,
    isCut: all.length > listed.length || listed.some((address, index) => address !== all[index]),
  }
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
    filename: clip(filenameOf(part) ?? 'untitled', MAX_NAME_CHARS),
    content_type: clip(part.type, MAX_NAME_CHARS),
    size: fileSize(part),
  }))
}

/** What an agent needs to pick a message out of a list. */
export function messageSummary(message: FetchMessageObject) {
  const { envelope = {}, flags = new Set<string>() } = message
  const subject = envelope.subject ?? ''
  const from = listAddresses(envelope.from)
  const to = listAddresses(envelope.to)
  return {
    uid: message.uid,
    subject: clip(subject, MAX_HEADER_CHARS),
    from: from.listed.join(', '),
    to: to.listed,
    date: isoDate(envelope.date) ?? isoDate(message.internalDate),
    unread: !flags.has('\\Seen'),
    flagged: flags.has('\\Flagged'),
    answered: flags.has('\\Answered'),
    attachments: attachmentsOf(message.bodyStructure).length,
    ...truncated({
      subject: subject.length > MAX_HEADER_CHARS,
      from: from.isCut,
      to: to.isCut,
    }),
  }
}

/** The headers of one message, without the fields that are empty. */
export function messageHeaders(message: FetchMessageObject) {
  const { envelope = {} } = message
  const { attachments, ...summary } = messageSummary(message)
  const replyAddresses = listAddresses(envelope.replyTo)
  const replyAddress = replyAddresses.listed.join(', ')
  const hasReplyAddress = replyAddress !== '' && replyAddress !== summary.from
  const cc = listAddresses(envelope.cc)
  const bcc = listAddresses(envelope.bcc)
  const messageId = envelope.messageId ?? ''

  return {
    ...summary,
    ...(hasReplyAddress ? { reply_to: replyAddress } : {}),
    ...(cc.listed.length > 0 ? { cc: cc.listed } : {}),
    ...(bcc.listed.length > 0 ? { bcc: bcc.listed } : {}),
    ...(messageId ? { message_id: clip(messageId, MAX_HEADER_CHARS) } : {}),
    ...truncated({
      reply_to: hasReplyAddress && replyAddresses.isCut,
      cc: cc.isCut,
      bcc: bcc.isCut,
      message_id: messageId.length > MAX_HEADER_CHARS,
    }),
  }
}

/**
 * Normalize line endings and drop the padding and blank runs that only cost
 * tokens. The text is written by a stranger, so every pattern must stay
 * linear: the lookbehind lets a run of spaces be tried once, from its start,
 * instead of once from each of its characters.
 */
export function tidyText(text: string) {
  return text
    .replace(/\r\n?/g, '\n')
    .replace(PREVIEW_PADDING, '')
    .replace(/(?<![ \t])[ \t]+\n/g, '\n')
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
  // A set, because the sender decides how long both lists are.
  const addressed = new Set(to.map((address) => address.toLowerCase()))
  const cc = everyone.filter((address) => !isOwn(address) && !addressed.has(address.toLowerCase()))

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
