import { Readable } from 'node:stream'
import type { HttpContext } from '@adonisjs/core/http'
import Mcp from '#models/mcp'
import {
  BuiltinToolError,
  type BuiltinFile,
  type BuiltinUploadTarget,
} from '#services/builtin/definition'
import { BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE } from '#services/builtin/file_link'
import { limitedPlaces } from '#services/builtin/places'
import { builtinUploadTarget, downloadBuiltinFile } from '#services/builtin/runtime'
import { BuiltinUploadError, saveBuiltinUpload } from '#services/builtin/upload_store'
import { builtinFileRateLimiter, builtinUploadRateLimiter } from '#start/limiter'
import { builtinFileValidator } from '#validators/builtin_files'

const MEDIA_TYPE = /^[\w.+-]+\/[\w.+-]+$/
const INVALID_LINK = 'This link is invalid or has expired.'
const UNAVAILABLE = 'This file is no longer available.'
const UPLOAD_UNAVAILABLE = 'This upload link can no longer be used.'
const HOW_TO_UPLOAD =
  'Send the file itself as the body of the PUT request, for example with: curl -T <file> "<link>"'

/**
 * A download signs in to the provider and keeps the whole file in memory
 * until the client has received it, so an MCP serves only a few at once.
 */
const MAX_CONCURRENT_DOWNLOADS = 3
/** An upload is written to disk as it arrives, and keeps a connection open meanwhile. */
const MAX_CONCURRENT_UPLOADS = 3
const BUSY_RETRY_SECONDS = 5
/** A client that stops reading, or sending, must not keep its place for good. */
const STALLED_CLIENT_MS = 60_000

const startDownload = limitedPlaces(MAX_CONCURRENT_DOWNLOADS)
const startUpload = limitedPlaces(MAX_CONCURRENT_UPLOADS)

/** What the sender of a file is told when it was not kept. */
function uploadRefusal(error: BuiltinUploadError, target: BuiltinUploadTarget) {
  switch (error.reason) {
    case 'taken':
      return {
        status: 409,
        message: 'A file was already sent to this link. Ask for a new link to send another one.',
      }
    case 'empty':
      return { status: 400, message: `The request has no body. ${HOW_TO_UPLOAD}` }
    case 'too_large':
      return {
        status: 413,
        message: `The file is larger than the ${target.maxBytes / 1_000_000} MB this link takes.`,
      }
    case 'full':
      return {
        status: 429,
        message:
          'Too many uploaded files are waiting for this MCP. They are deleted an hour after their upload: try again later.',
      }
  }
}

/** Name the download after the file, in ASCII for old clients and in full for the others. */
function attachmentDisposition(filename: string) {
  // A lone surrogate cannot be percent-encoded.
  const name = filename.toWellFormed().replace(/[\p{Cc}"\\/]/gu, '_')
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
  async show({ request, response }: HttpContext) {
    // Checked before anything is counted: only the holder of a link can use up downloads.
    if (!request.hasValidSignature(BUILTIN_FILE_PURPOSE)) {
      return response.status(403).send(INVALID_LINK)
    }

    const [malformed, link] = await request.tryValidateUsing(builtinFileValidator)
    if (malformed) {
      return response.status(404).send(UNAVAILABLE)
    }
    const { id, reference } = link.params

    // Counted for each MCP and address, so one client cannot use up the downloads of another.
    const client = `builtin-file:${id}:${request.ip()}`
    if (!(await builtinFileRateLimiter.attempt(client, () => true))) {
      response.header('Retry-After', await builtinFileRateLimiter.availableIn(client))
      return response.status(429).send('Too many downloads. Try again later.')
    }

    const mcp = await Mcp.find(id)
    if (!mcp || !mcp.enabled || mcp.transport !== 'builtin') {
      return response.status(404).send(UNAVAILABLE)
    }

    const finishDownload = startDownload(mcp.id)
    if (!finishDownload) {
      response.header('Retry-After', BUSY_RETRY_SECONDS)
      return response.status(429).send('Too many downloads at once. Try again in a few seconds.')
    }

    let file: BuiltinFile
    try {
      file = await downloadBuiltinFile(mcp, reference)
    } catch (error) {
      finishDownload()
      if (!(error instanceof BuiltinToolError)) throw error
      return response.status(404).send(UNAVAILABLE)
    }
    // Kept for as long as the file is in memory: until the client has it, or
    // has gone. A client that left earlier did not stop the download above.
    response.onFinish(finishDownload)

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
    response.header(
      'Content-Length',
      file.content.reduce((size, chunk) => size + chunk.length, 0)
    )
    response.response.setTimeout(STALLED_CLIENT_MS)
    return response.stream(Readable.from(file.content, { objectMode: false }))
  }

  /**
   * Keep the file sent to a temporary link that a built-in tool handed out,
   * for the tool to use afterwards. As for a download, the signature is the
   * credential and the MCP is checked again. The body is the file whatever
   * its content type: it is not parsed, and goes to disk as it arrives.
   */
  async store({ request, response }: HttpContext) {
    // Checked before anything is counted or read: only the holder of a link can send a file.
    if (!request.hasValidSignature(BUILTIN_UPLOAD_PURPOSE)) {
      return response.status(403).send(INVALID_LINK)
    }

    const [malformed, link] = await request.tryValidateUsing(builtinFileValidator)
    if (malformed) {
      return response.status(404).send(UPLOAD_UNAVAILABLE)
    }
    const { id, reference } = link.params

    const client = `builtin-upload:${id}:${request.ip()}`
    if (!(await builtinUploadRateLimiter.attempt(client, () => true))) {
      response.header('Retry-After', await builtinUploadRateLimiter.availableIn(client))
      return response.status(429).send('Too many uploads. Try again later.')
    }

    const mcp = await Mcp.find(id)
    if (!mcp || !mcp.enabled || mcp.transport !== 'builtin') {
      return response.status(404).send(UPLOAD_UNAVAILABLE)
    }

    let target: BuiltinUploadTarget
    try {
      target = await builtinUploadTarget(mcp, reference)
    } catch (error) {
      if (!(error instanceof BuiltinToolError)) throw error
      return response.status(404).send(UPLOAD_UNAVAILABLE)
    }

    // A form would be stored with its boundaries and field headers around the file.
    if (/^multipart\//i.test(request.header('content-type') ?? '')) {
      return response.status(415).send(`A form cannot be stored as a file. ${HOW_TO_UPLOAD}`)
    }
    // Most clients say how much they are about to send.
    if (Number(request.header('content-length')) > target.maxBytes) {
      const tooLarge = uploadRefusal(new BuiltinUploadError('too_large'), target)
      return response.status(tooLarge.status).send(tooLarge.message)
    }

    const finishUpload = startUpload(mcp.id)
    if (!finishUpload) {
      response.header('Retry-After', BUSY_RETRY_SECONDS)
      return response.status(429).send('Too many uploads at once. Try again in a few seconds.')
    }

    const body = request.request
    body.setTimeout(STALLED_CLIENT_MS)
    try {
      // Reading stops at the first byte too many, which must not close the
      // connection the answer is sent on.
      const upload = await saveBuiltinUpload(
        mcp.id,
        target,
        body.iterator({ destroyOnReturn: false })
      )
      return response.status(201).json({
        upload_id: upload.id,
        filename: upload.filename,
        size: upload.size,
        expires_at: new Date(upload.expiresAt).toISOString(),
      })
    } catch (error) {
      if (error instanceof BuiltinUploadError) {
        // What the client is still sending is read and dropped, like any body nobody asked for.
        body.resume()
        const refusal = uploadRefusal(error, target)
        return response.status(refusal.status).send(refusal.message)
      }
      // The client hung up halfway: nothing was kept, and nobody is left to answer.
      if (body.destroyed) return
      throw error
    } finally {
      finishUpload()
    }
  }
}
