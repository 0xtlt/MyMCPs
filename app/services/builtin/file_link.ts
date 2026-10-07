import { signedUrlFor } from '@adonisjs/core/services/url_builder'
import { BuiltinToolError } from '#services/builtin/definition'
import { requirePublicAppUrl } from '#services/public_url'

/** Keeps a signature issued for anything else from opening a file. */
export const BUILTIN_FILE_PURPOSE = 'builtin_file'

/** A link that downloads a file must not be one that stores a file, nor the reverse. */
export const BUILTIN_UPLOAD_PURPOSE = 'builtin_upload'

function signedLink(
  route: 'builtin.file' | 'builtin.upload',
  purpose: string,
  mcpId: number,
  reference: unknown,
  expiresInMs: number
) {
  let appUrl: string
  try {
    appUrl = requirePublicAppUrl()
  } catch (error) {
    throw new BuiltinToolError(
      'File links need the public address of this MyMCPs instance. An administrator must set APP_URL.',
      { cause: error }
    )
  }

  return signedUrlFor(
    route,
    { id: mcpId, reference: Buffer.from(JSON.stringify(reference)).toString('base64url') },
    { expiresIn: expiresInMs, purpose, prefixUrl: appUrl }
  )
}

/**
 * A temporary link to a file of a built-in MCP, for whoever holds it: the
 * agent, or the person the agent gives it to. `reference` tells the provider
 * which file to serve and cannot be changed without breaking the signature.
 */
export function builtinFileUrl(mcpId: number, reference: unknown, expiresInMs: number) {
  return signedLink('builtin.file', BUILTIN_FILE_PURPOSE, mcpId, reference, expiresInMs)
}

/**
 * A temporary link that takes one file for a built-in MCP, from whoever holds
 * it. `reference` tells the provider what the file is for, and is signed like
 * the reference of a download.
 */
export function builtinUploadUrl(mcpId: number, reference: unknown, expiresInMs: number) {
  return signedLink('builtin.upload', BUILTIN_UPLOAD_PURPOSE, mcpId, reference, expiresInMs)
}

/** `undefined` when the path segment is not a reference encoded above. */
export function decodeFileReference(value: string): unknown {
  try {
    return JSON.parse(Buffer.from(value, 'base64url').toString('utf8'))
  } catch {
    return undefined
  }
}
