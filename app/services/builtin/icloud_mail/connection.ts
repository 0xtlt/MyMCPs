import { ImapFlow, type ImapFlowError } from 'imapflow'
import { createTransport, type NodemailerError, type SendMailOptions } from 'nodemailer'
import {
  BuiltinAuthorizationError,
  BuiltinToolError,
  type BuiltinPasswordContext,
} from '#services/builtin/definition'

const CONNECT_TIMEOUT_MS = 20_000
const SOCKET_TIMEOUT_MS = 60_000

const SIGN_IN_REJECTED =
  'iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.'

/** Socket and connection failures that happen before iCloud answers a command. */
const UNREACHABLE_CODES = new Set([
  'CONNECT_TIMEOUT',
  'GREETING_TIMEOUT',
  'UPGRADE_TIMEOUT',
  'ETIMEOUT',
  'ETIMEDOUT',
  'NoConnection',
  'EConnectionClosed',
  'ClosedAfterConnectTLS',
  'ClosedAfterConnectText',
  'ENOTFOUND',
  'EAI_AGAIN',
  'ECONNREFUSED',
  'ECONNRESET',
  'EHOSTUNREACH',
  'ENETUNREACH',
  'EPIPE',
])

export type ImapClient = Pick<
  ImapFlow,
  | 'on'
  | 'connect'
  | 'logout'
  | 'close'
  | 'mailbox'
  | 'list'
  | 'getMailboxLock'
  | 'search'
  | 'fetchAll'
  | 'fetchOne'
  | 'download'
  | 'messageFlagsAdd'
  | 'messageFlagsRemove'
  | 'messageMove'
  | 'append'
>

export type SmtpClient = {
  sendMail: (mail: SendMailOptions) => Promise<{ rejected: string[] }>
  close: () => void
}

/**
 * The only two hosts the saved password is ever sent to. Tests swap these
 * factories for fakes.
 */
export const icloudMailServers = {
  /** The IMAP username that worked for each address, so the other form is not tried again. */
  imapUsernames: new Map<string, string>(),
  imap: ({ username, password }: BuiltinPasswordContext): ImapClient =>
    new ImapFlow({
      host: 'imap.mail.me.com',
      port: 993,
      secure: true,
      auth: { user: username, pass: password },
      // The default logger prints every command and response, mail included.
      logger: false,
      disableAutoIdle: true,
      connectionTimeout: CONNECT_TIMEOUT_MS,
      greetingTimeout: CONNECT_TIMEOUT_MS,
      socketTimeout: SOCKET_TIMEOUT_MS,
    }),
  smtp: ({ username, password }: BuiltinPasswordContext): SmtpClient =>
    createTransport({
      host: 'smtp.mail.me.com',
      port: 587,
      secure: false,
      requireTLS: true,
      auth: { user: username, pass: password },
      // Mail is built from agent input, which must never name a file or URL to attach.
      disableFileAccess: true,
      disableUrlAccess: true,
      connectionTimeout: CONNECT_TIMEOUT_MS,
      greetingTimeout: CONNECT_TIMEOUT_MS,
      socketTimeout: SOCKET_TIMEOUT_MS,
    }),
}

function imapFailure(error: unknown) {
  if (error instanceof BuiltinToolError || !(error instanceof Error)) {
    return error
  }

  const failure: ImapFlowError = error
  if (failure.authenticationFailed) {
    return new BuiltinAuthorizationError(SIGN_IN_REJECTED, { cause: error })
  }
  if (failure.code === 'ETHROTTLE') {
    return new BuiltinToolError(
      'iCloud Mail is limiting requests for this account. Try again in a few minutes.',
      { cause: error }
    )
  }
  if (failure.code && UNREACHABLE_CODES.has(failure.code)) {
    return new BuiltinToolError('Could not reach iCloud Mail. Try again.', { cause: error })
  }
  if (failure.responseText) {
    return new BuiltinToolError(
      `iCloud Mail refused the request: ${failure.responseText.slice(0, 200)}`,
      { cause: error }
    )
  }
  return error
}

/**
 * Apple documents the name before the @ as the usual IMAP username and the
 * full address as the one to try when it fails, without saying which accounts
 * take which. Try the address as entered, then the name alone.
 */
function imapUsernames(address: string) {
  const remembered = icloudMailServers.imapUsernames.get(address)
  return remembered ? [remembered] : [address, address.split('@')[0]]
}

async function signInToImap(
  signIn: BuiltinPasswordContext,
  usernames = imapUsernames(signIn.username)
): Promise<ImapClient> {
  const [username, ...alternatives] = usernames
  const client = icloudMailServers.imap({ ...signIn, username })
  // A dropped socket is reported as an 'error' event, which stops the process
  // when nothing listens. The pending command rejects with the same error.
  client.on('error', () => {})

  try {
    await client.connect()
  } catch (error) {
    client.close()
    if (alternatives.length === 0 || !(error as ImapFlowError).authenticationFailed) {
      throw error
    }
    return signInToImap(signIn, alternatives)
  }
  icloudMailServers.imapUsernames.set(signIn.username, username)
  return client
}

/** Sign in to iCloud Mail over IMAP for the duration of `use`. */
export async function withImap<Result>(
  signIn: BuiltinPasswordContext,
  use: (client: ImapClient) => Promise<Result>
): Promise<Result> {
  let client: ImapClient | undefined
  try {
    client = await signInToImap(signIn)
    return await use(client)
  } catch (error) {
    throw imapFailure(error)
  } finally {
    await client?.logout().catch(() => {})
    client?.close()
  }
}

/** Select `path` for the duration of `use`. Read-only access never changes a flag. */
export async function withMailbox<Result>(
  client: ImapClient,
  path: string,
  access: 'read' | 'write',
  use: () => Promise<Result>
): Promise<Result> {
  let lock
  try {
    lock = await client.getMailboxLock(path, { readOnly: access === 'read' })
  } catch (error) {
    if ((error as ImapFlowError).mailboxMissing) {
      throw new BuiltinToolError(
        `Mailbox "${path}" does not exist. Call list_mailboxes for the exact paths.`,
        { cause: error }
      )
    }
    throw error
  }

  try {
    return await use()
  } finally {
    lock.release()
  }
}

/**
 * Explain a failed SMTP delivery. Once the connection is open, a socket error
 * does not say whether iCloud had already accepted the message.
 */
function smtpFailure(error: unknown) {
  if (!(error instanceof Error)) {
    return error
  }

  const failure: NodemailerError = error
  const reply = failure.response ? `: ${failure.response.slice(0, 200)}` : ''
  if (failure.code === 'EAUTH') {
    return new BuiltinAuthorizationError(SIGN_IN_REJECTED, { cause: error })
  }
  if (failure.code === 'EENVELOPE') {
    return new BuiltinToolError(`iCloud Mail rejected the recipients${reply}`, { cause: error })
  }
  if (failure.code === 'EMESSAGE') {
    return new BuiltinToolError(`iCloud Mail refused the message${reply}`, { cause: error })
  }
  if (failure.code === 'EDNS') {
    return new BuiltinToolError('Could not reach iCloud Mail. Nothing was sent. Try again.', {
      cause: error,
    })
  }
  if (failure.code && ['ETIMEDOUT', 'ECONNECTION', 'ESOCKET', 'ETLS'].includes(failure.code)) {
    return new BuiltinToolError(
      'iCloud Mail did not confirm the message, so it may or may not have been sent. Check with the user before sending it again.',
      { cause: error }
    )
  }
  return error
}

/** Deliver one message through iCloud's SMTP server. Returns the recipients it refused. */
export async function sendThroughSmtp(signIn: BuiltinPasswordContext, mail: SendMailOptions) {
  const client = icloudMailServers.smtp(signIn)
  try {
    const { rejected } = await client.sendMail(mail)
    return rejected
  } catch (error) {
    throw smtpFailure(error)
  } finally {
    client.close()
  }
}
