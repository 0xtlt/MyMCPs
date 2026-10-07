import { mkdir, open, readdir, readFile, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import app from '@adonisjs/core/services/app'
import type { BuiltinUploadTarget } from '#services/builtin/definition'

/** How long an uploaded file can be used, counted from its upload. */
export const BUILTIN_UPLOAD_MINUTES = 60
const KEPT_MS = BUILTIN_UPLOAD_MINUTES * 60_000

/** What the files waiting for one MCP may take on the instance's disk. */
const MAX_STORED_BYTES = 100_000_000
const MAX_STORED_FILES = 50

const METADATA = '.json'

/** Upload ids name files on disk, so nothing but a UUID is ever one. */
const UPLOAD_ID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/

export function isBuiltinUploadId(value: string) {
  return UPLOAD_ID.test(value)
}

/** A file an agent sent to an upload link, kept until `expiresAt`. */
export type BuiltinUpload = {
  id: string
  filename: string
  contentType?: string
  size: number
  /** In milliseconds since the epoch. */
  expiresAt: number
}

/**
 * Why a file sent to a valid link was not kept: the link already took a file
 * (`taken`), the request had no body (`empty`), the file is larger than the
 * provider accepts (`too_large`), or too much is already waiting for this MCP
 * (`full`).
 */
export class BuiltinUploadError extends Error {
  constructor(public reason: 'taken' | 'empty' | 'too_large' | 'full') {
    super(`Upload refused: ${reason}`)
    this.name = 'BuiltinUploadError'
  }
}

function uploadsRoot() {
  return app.tmpPath('builtin-uploads')
}

function uploadsRootFor(mcpId: number) {
  if (!Number.isSafeInteger(mcpId) || mcpId <= 0) {
    throw new Error('Uploads belong to a saved MCP')
  }
  return join(uploadsRoot(), String(mcpId))
}

function uploadPath(mcpId: number, id: string) {
  if (!isBuiltinUploadId(id)) {
    throw new Error('Not an upload id')
  }
  return join(uploadsRootFor(mcpId), id)
}

function isMissing(error: unknown) {
  return (error as NodeJS.ErrnoException).code === 'ENOENT'
}

/** What was written once the file had arrived whole. `null` until then. */
async function readMetadata(path: string): Promise<BuiltinUpload | null> {
  try {
    return JSON.parse(await readFile(`${path}${METADATA}`, 'utf8'))
  } catch {
    return null
  }
}

async function discard(path: string) {
  await Promise.all([rm(path, { force: true }), rm(`${path}${METADATA}`, { force: true })])
}

/**
 * Delete the files of one MCP that have expired, and count what is left. A
 * file without metadata is still arriving, or was left by a server that
 * stopped halfway: it is given as long as a finished one.
 */
async function sweep(mcpId: number, now = Date.now()) {
  const directory = uploadsRootFor(mcpId)
  const kept = { files: 0, bytes: 0 }

  let names: string[]
  try {
    names = await readdir(directory)
  } catch (error) {
    if (isMissing(error)) return kept
    throw error
  }

  for (const name of names.filter(isBuiltinUploadId)) {
    const path = join(directory, name)
    try {
      const [metadata, { size, mtimeMs }] = await Promise.all([readMetadata(path), stat(path)])
      if ((metadata?.expiresAt ?? mtimeMs + KEPT_MS) <= now) {
        await discard(path)
      } else {
        kept.files += 1
        kept.bytes += size
      }
    } catch (error) {
      // Removed in the meantime by a failed upload.
      if (!isMissing(error)) throw error
    }
  }
  return kept
}

/**
 * Keep the file sent to an upload link. A link takes one file: the name it
 * is stored under is created only if nothing has it yet, which also turns
 * away a second request sent while the first is still arriving. Throws
 * `BuiltinUploadError` when the file is not kept, and leaves nothing behind.
 */
export async function saveBuiltinUpload(
  mcpId: number,
  target: BuiltinUploadTarget,
  body: AsyncIterable<Uint8Array>
): Promise<BuiltinUpload> {
  const path = uploadPath(mcpId, target.id)
  await mkdir(uploadsRootFor(mcpId), { recursive: true, mode: 0o700 })

  const stored = await sweep(mcpId)
  const room = MAX_STORED_BYTES - stored.bytes
  if (stored.files >= MAX_STORED_FILES || room <= 0) {
    throw new BuiltinUploadError('full')
  }

  let file
  try {
    file = await open(path, 'wx', 0o600)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'EEXIST') {
      throw new BuiltinUploadError('taken')
    }
    throw error
  }

  try {
    let size = 0
    try {
      for await (const chunk of body) {
        size += chunk.length
        if (size > target.maxBytes) throw new BuiltinUploadError('too_large')
        if (size > room) throw new BuiltinUploadError('full')
        await file.write(chunk)
      }
    } finally {
      await file.close()
    }
    if (size === 0) throw new BuiltinUploadError('empty')

    const upload: BuiltinUpload = {
      id: target.id,
      filename: target.filename,
      contentType: target.contentType,
      size,
      expiresAt: Date.now() + KEPT_MS,
    }
    await writeFile(`${path}${METADATA}`, JSON.stringify(upload), { flag: 'wx', mode: 0o600 })
    return upload
  } catch (error) {
    // Nothing is kept of a failed upload, so its link can be tried again.
    await discard(path)
    throw error
  }
}

/** The file uploaded under `id`. `null` when none arrived whole, or it has expired. */
export async function findBuiltinUpload(mcpId: number, id: string): Promise<BuiltinUpload | null> {
  const path = uploadPath(mcpId, id)
  const upload = await readMetadata(path)
  if (!upload) return null

  if (upload.expiresAt <= Date.now()) {
    await discard(path)
    return null
  }
  return upload
}

/** The bytes of an uploaded file. `null` when it was deleted in the meantime. */
export async function readBuiltinUpload(mcpId: number, id: string) {
  try {
    return await readFile(uploadPath(mcpId, id))
  } catch (error) {
    if (isMissing(error)) return null
    throw error
  }
}

/** Delete every file uploaded for an MCP. Called when the MCP is deleted. */
export async function removeBuiltinUploads(mcpId: number) {
  await rm(uploadsRootFor(mcpId), { recursive: true, force: true })
}

/**
 * Delete the expired files of every MCP. A file is never served past its
 * expiry, but only this removes the ones nobody asks for again.
 */
export async function pruneBuiltinUploads(now = Date.now()) {
  let directories: string[]
  try {
    directories = await readdir(uploadsRoot())
  } catch (error) {
    if (isMissing(error)) return
    throw error
  }

  for (const name of directories.filter((directory) => /^[1-9]\d*$/.test(directory))) {
    const { files } = await sweep(Number(name), now)
    if (files === 0) {
      // Not empty when an upload has just started: it stays for the next pass.
      await rmdir(join(uploadsRoot(), name)).catch(() => {})
    }
  }
}

let sweeper: NodeJS.Timeout | null = null

/** Prune now, for what a previous run left behind, then every few minutes. */
export function startBuiltinUploadSweeper(onError: (error: unknown) => void, everyMs = 5 * 60_000) {
  stopBuiltinUploadSweeper()
  const run = () => void pruneBuiltinUploads().catch(onError)
  run()
  sweeper = setInterval(run, everyMs)
  // The server must be able to stop while the timer is pending.
  sweeper.unref()
}

export function stopBuiltinUploadSweeper() {
  if (sweeper) clearInterval(sweeper)
  sweeper = null
}
