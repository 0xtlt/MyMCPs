import type { HttpContext } from '@adonisjs/core/http'
import Mcp from '#models/mcp'
import { BuiltinToolError, type BuiltinFile } from '#services/builtin/definition'
import { BUILTIN_FILE_PURPOSE, decodeFileReference } from '#services/builtin/file_link'
import { downloadBuiltinFile } from '#services/builtin/runtime'
import { builtinFileRateLimiter } from '#start/limiter'

const MEDIA_TYPE = /^[\w.+-]+\/[\w.+-]+$/
const UNAVAILABLE = 'This file is no longer available.'

/** Name the download after the file, in ASCII for old clients and in full for the others. */
function attachmentDisposition(filename: string) {
  const name = filename.replace(/[\p{Cc}"\\/]/gu, '_')
  const ascii = name.replace(/[^\x20-\x7e]/g, '_')
  const encoded = encodeURIComponent(name).replace(
    /['()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`
  )
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encoded}`
}

export default class BuiltinFilesController {
  /**
   * Serve the file behind a temporary link that a built-in tool handed out.
   * There is no session and no access token: the signature is the credential,
   * so the MCP is checked again in case it changed since the link was made.
   */
  async show({ request, response, params }: HttpContext) {
    if (!(await builtinFileRateLimiter.attempt(`builtin-file:${request.ip()}`, () => true))) {
      return response.status(429).send('Too many downloads. Try again later.')
    }
    if (!request.hasValidSignature(BUILTIN_FILE_PURPOSE)) {
      return response.status(403).send('This link is invalid or has expired.')
    }

    const mcp = await Mcp.find(params.id)
    if (!mcp || !mcp.enabled || mcp.transport !== 'builtin') {
      return response.status(404).send(UNAVAILABLE)
    }

    let file: BuiltinFile
    try {
      file = await downloadBuiltinFile(mcp, decodeFileReference(params.reference))
    } catch (error) {
      if (!(error instanceof BuiltinToolError)) throw error
      return response.status(404).send(UNAVAILABLE)
    }

    // The file was written by a stranger. A browser must save it, never
    // render it on this origin.
    response.header(
      'Content-Type',
      MEDIA_TYPE.test(file.contentType) ? file.contentType : 'application/octet-stream'
    )
    response.header('Content-Disposition', attachmentDisposition(file.filename))
    response.header('X-Content-Type-Options', 'nosniff')
    response.header('Content-Security-Policy', "sandbox; default-src 'none'")
    response.header('Cache-Control', 'private, no-store')
    return response.send(file.content)
  }
}
