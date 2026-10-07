/**
 * Vine schemas for the built-in iCloud Mail MCP: the arguments of its tools,
 * and what its attachment and upload links refer to. A schema lists the arguments in the
 * order they are checked: of several wrong ones, the agent is told about the
 * first.
 */
import type { BaseLiteralType } from '@vinejs/vine'
import type { BuiltinPasswordContext } from '#services/builtin/definition'
import { isAddress } from '#services/builtin/icloud_mail/message'
import { isBuiltinUploadId } from '#services/builtin/upload_store'
import {
  blankAsMissing,
  boolean,
  integer,
  isBlank,
  isoDate,
  line,
  listLength,
  mediaType,
  pattern,
  text,
  toolVine,
  uploadedFileName,
  VineArgument,
} from '#validators/builtin_tools'

/** The bounds the tools advertise, and the schemas below enforce. */
export const ICLOUD_MAIL_LIMITS = {
  pageSize: 50,
  textChars: 100_000,
  uids: 100,
  recipients: 50,
  subjectLength: 255,
  linkMinutes: 60,
  attachments: 10,
  /**
   * Apple carries 20 MB of files in a message. Encoded for mail they take a
   * third more, which fits in the 27 MB its server accepts.
   */
  attachmentBytes: 20_000_000,
  filenameLength: 255,
} as const

const MAX_UID = 4_294_967_295
const MAX_MAILBOX_LENGTH = 255
const MAX_SEARCH_LENGTH = 200

/** Left out, it is the inbox. */
const mailbox = () => line(MAX_MAILBOX_LENGTH).optional()

const uid = () => integer({ min: 1, max: MAX_UID })

/** Anything but a list counts as an empty one, which gets the same answer. */
const uids = () =>
  toolVine
    .array(uid())
    .parse((value) => (Array.isArray(value) ? value : []))
    .use(
      listLength({
        min: 1,
        max: ICLOUD_MAIL_LIMITS.uids,
        sentence: `{{ field }} must be a list of 1 to ${ICLOUD_MAIL_LIMITS.uids} message UIDs`,
      })
    )

const part = () =>
  pattern(
    /^\d{1,3}(\.\d{1,3}){0,9}$/,
    'the part of an attachment, such as 2, as returned by get_message'
  )

const searchText = () => line(MAX_SEARCH_LENGTH).optional()

/** What the recipient sees the file as. It never names a file on the instance. */
const fileName = () => uploadedFileName(ICLOUD_MAIL_LIMITS.filenameLength)

const ATTACHMENTS = `{{ field }} must be a list of at most ${ICLOUD_MAIL_LIMITS.attachments} upload IDs, as returned by create_upload_link`

const uploadIdRule = toolVine.createRule(
  (value, _options, field) => {
    if (!isBuiltinUploadId(value as string)) {
      field.report(ATTACHMENTS, 'uploadId', field)
    }
  },
  {
    toJSONSchema: (schema) => {
      schema.type = 'string'
    },
  }
)

/** One uploaded file. What is not text is refused like a wrong ID. */
const uploadId = () =>
  new VineArgument<string>(uploadIdRule()).parse((value) =>
    typeof value === 'string' ? value.trim().toLowerCase() : ''
  )

/** A single ID is a list of one. */
const attachments = () =>
  toolVine
    .array(uploadId())
    .parse((value) => (isBlank(value) ? undefined : Array.isArray(value) ? value : [value]))
    .use(listLength({ max: ICLOUD_MAIL_LIMITS.attachments, sentence: ATTACHMENTS }))
    .optional()

const ADDRESSES = `{{ field }} must be a list of at most ${ICLOUD_MAIL_LIMITS.recipients} email addresses such as name@example.com, without display names`

const addressRule = toolVine.createRule(
  (value, _options, field) => {
    if (!isAddress(value as string)) {
      field.report(ADDRESSES, 'address', field)
    }
  },
  {
    toJSONSchema: (schema) => {
      schema.type = 'string'
    },
  }
)

/** One address of a list. What is not text is refused like a wrong address. */
const address = () =>
  new VineArgument<string>(addressRule()).parse((value) =>
    typeof value === 'string' ? value.trim() : ''
  )

/** A single address is a list of one. */
const addresses = () =>
  toolVine
    .array(address())
    .parse((value) => (isBlank(value) ? undefined : Array.isArray(value) ? value : [value]))
    .use(listLength({ max: ICLOUD_MAIL_LIMITS.recipients, sentence: ADDRESSES }))
    .optional()

type Senders = Pick<BuiltinPasswordContext, 'username' | 'aliases'>

/** One of the account's own addresses, in the spelling the administrator saved. */
const ownAddressRule = toolVine.createRule((value, _options, field) => {
  const { username, aliases } = field.meta as Senders
  const allowed = [username, ...aliases]
  const saved = allowed.find(
    (candidate) => candidate.toLowerCase() === (value as string).toLowerCase()
  )
  if (!saved) {
    field.report(
      '{{ field }} must be one of the sender addresses allowed for this MCP: {{ allowed }}',
      'ownAddress',
      field,
      { allowed: allowed.join(', ') }
    )
    return
  }
  field.mutate(saved, field)
})

/** For an argument that may only be left out when `other` is given. */
const requiredUnlessRule = toolVine.createRule<{ other: string; sentence?: string }>(
  (value, { other, sentence }, field) => {
    if (isBlank(value) && isBlank(field.parent[other])) {
      field.report(
        sentence ?? '{{ field }} is required unless {{ other }} is set',
        'requiredUnless',
        field,
        { other }
      )
    }
  },
  { implicit: true }
)

/** An argument that says more about `other`, and is not looked at without it. */
function onlyWith<Schema extends BaseLiteralType<any, any, any>>(other: string, schema: Schema) {
  const { parse } = schema.options
  return schema.parse((value, context) =>
    isBlank(context.parent[other]) ? undefined : parse ? parse(value, context) : value
  )
}

export const listMessagesValidator = toolVine.create({
  mailbox: mailbox(),
  page: integer({ min: 1 }).optional(),
  per_page: integer({ min: 1, max: ICLOUD_MAIL_LIMITS.pageSize }).optional(),
  from: searchText(),
  to: searchText(),
  subject: searchText(),
  text: searchText(),
  since: isoDate().optional(),
  before: isoDate().optional(),
  unread: boolean().optional(),
  flagged: boolean().optional(),
})

export const getMessageValidator = toolVine.create({
  mailbox: mailbox(),
  uid: uid(),
  max_chars: integer({ min: 500, max: ICLOUD_MAIL_LIMITS.textChars }).optional(),
})

export const getAttachmentLinkValidator = toolVine.create({
  mailbox: mailbox(),
  uid: uid(),
  part: part(),
  expires_in_minutes: integer({ min: 1, max: ICLOUD_MAIL_LIMITS.linkMinutes }).optional(),
})

/** What get_attachment_link puts in a link, and gets back when the link is opened. */
export const attachmentReferenceValidator = toolVine.create({
  mailbox: mailbox(),
  uid: uid(),
  part: part(),
})

export const createUploadLinkValidator = toolVine.create({
  filename: fileName(),
  content_type: mediaType().optional(),
  expires_in_minutes: integer({ min: 1, max: ICLOUD_MAIL_LIMITS.linkMinutes }).optional(),
})

/** What create_upload_link puts in a link, and gets back when a file is sent to it. */
export const uploadReferenceValidator = toolVine.create({
  upload: uploadId(),
  filename: fileName(),
  content_type: mediaType().optional(),
})

/** A message to draft or to send. The sign-in says which addresses it may come from. */
export const compositionValidator = toolVine.withMetaData<Senders>().create({
  reply_to_uid: uid().optional(),
  subject: line(ICLOUD_MAIL_LIMITS.subjectLength)
    .optional()
    .use(requiredUnlessRule({ other: 'reply_to_uid' })),
  from: line(254).use(ownAddressRule()).optional(),
  to: addresses(),
  cc: addresses(),
  bcc: addresses(),
  text: text(ICLOUD_MAIL_LIMITS.textChars).parse(blankAsMissing),
  attachments: attachments(),
  reply_to_mailbox: onlyWith('reply_to_uid', line(MAX_MAILBOX_LENGTH)).optional(),
  reply_all: onlyWith('reply_to_uid', boolean()).optional(),
})

export const markMessagesValidator = toolVine.create({
  mailbox: mailbox(),
  uids: uids(),
  unread: boolean().optional(),
  flagged: boolean()
    .optional()
    .use(requiredUnlessRule({ other: 'unread', sentence: 'Set unread, flagged, or both' })),
})

export const moveMessagesValidator = toolVine.create({
  mailbox: mailbox(),
  uids: uids(),
  destination: line(MAX_MAILBOX_LENGTH),
})
