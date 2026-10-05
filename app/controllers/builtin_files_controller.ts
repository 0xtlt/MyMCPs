import { Readable } from 'node:stream'
import type { HttpContext } from '@adonisjs/core/http'
import Mcp from '#models/mcp'
import { BuiltinToolError, type BuiltinFile } from '#services/builtin/definition'
import { BUILTIN_FILE_PURPOSE, decodeFileReference } from '#services/builtin/file_link'
import { downloadBuiltinFile } from '#services/builtin/runtime'
import { builtinFileRateLimiter } from '#start/limiter'

const MEDIA_TYPE = /^[\w.+-]+\/[\w.+-]+$/
const UNAVAILABLE = 'This file is no longer available.'

/**
 * A download signs in to the provider and keeps the whole file in memory
 * until the client has received it, so an MCP serves only a few at once.
 */
const MAX_CONCURRENT_DOWNLOADS = 3
const BUSY_RETRY_SECONDS = 5
/** A client that stops reading must not keep its place for good. */
const STALLED_CLIENT_MS = 60_000

const downloads = new Map<number, number>()

/** Take one of the MCP's places. Returns how to give it back, or `null` when all are taken. */
function startDownload(mcpId: number) {
  const running = downloads.get(mcpId) ?? 0
  if (running >= MAX_CONCURRENT_DOWNLOADS) return null

  downloads.set(mcpId, running + 1)
  return () => {
    const left = (downloads.get(mcpId) ?? 1) - 1
    if (left > 0) downloads.set(mcpId, left)
    else downloads.delete(mcpId)
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
  async show({ request, response, params }: HttpContext) {
    // Checked before anything is counted: only the holder of a link can use up downloads.
    if (!request.hasValidSignature(BUILTIN_FILE_PURPOSE)) {
      return response.status(403).send('This link is invalid or has expired.')
    }

    // Counted for each MCP and address, so one client cannot use up the downloads of another.
    const client = `builtin-file:${params.id}:${request.ip()}`
    if (!(await builtinFileRateLimiter.attempt(client, () => true))) {
      response.header('Retry-After', await builtinFileRateLimiter.availableIn(client))
      return response.status(429).send('Too many downloads. Try again later.')
    }

    const mcp = await Mcp.find(params.id)
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
      file = await downloadBuiltinFile(mcp, decodeFileReference(params.reference))
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
}
