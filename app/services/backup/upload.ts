import { open } from 'node:fs/promises'
import { join } from 'node:path'
import { defineConfig } from '@adonisjs/core/bodyparser'
import BodyParserMiddleware from '@adonisjs/core/bodyparser_middleware'
import type { HttpContext } from '@adonisjs/core/http'
import { BackupImportError } from '#services/backup/import'
import { backupRuntime } from '#services/backup/runtime'

/** A client that stops sending must not keep the one place there is for an import. */
const STALLED_CLIENT_MS = 60_000

/** What a form adds around its file: boundaries, the headers of its parts, the password. */
const FORM_OVERHEAD_BYTES = 64 * 1024

/**
 * The parser of the import form, and of nothing else: the app does not read
 * multipart bodies (see `config/bodyparser.ts`). It only finds the parts;
 * the file part is written by `receiveBackupUpload`, into the directory of
 * the import. Fields are trimmed, and empty ones are null, as in every form.
 */
const importFormParser = new BodyParserMiddleware(
  defineConfig({
    allowedMethods: ['POST'],
    form: { types: [] },
    json: { types: [] },
    raw: { types: [] },
    multipart: {
      types: ['multipart/form-data'],
      autoProcess: false,
      // The form has one field beside its file.
      maxFields: 8,
      fieldsLimit: '16kb',
    },
  })
)

export type BackupUpload = {
  /** The `password` field, as the form parser normalized it. */
  password: unknown
  /** The file of the `backup` field. `null` when the form had none. */
  file: { path: string; size: number } | null
}

/**
 * Read the import form. The file goes to disk as it arrives, into
 * `workspace`, and is never held in memory. To call once the request has
 * passed every check: this is where an anonymous request starts to cost.
 * Throws `BackupImportError` for a file over the limit.
 */
export async function receiveBackupUpload(ctx: HttpContext, workspace: string) {
  const { request } = ctx

  // Most clients say how much they are about to send: nothing of it is kept then.
  const announced = Number(request.header('content-length'))
  if (announced > backupRuntime.maxFileBytes + FORM_OVERHEAD_BYTES) {
    throw new BackupImportError('too_large')
  }

  await importFormParser.handle(ctx, async () => {})
  if (request.bodyType !== 'multipart') {
    return { password: request.input('password'), file: null } satisfies BackupUpload
  }

  const path = join(workspace, 'upload.mymcps')
  let taken = false
  const received: { size: number | null } = { size: null }

  request.multipart.onFile('backup', { deferValidations: true }, async (part) => {
    // The form sends one file. Whatever else is named like it is read and dropped.
    if (taken) {
      part.resume()
      return
    }
    taken = true

    try {
      const file = await open(path, 'wx', 0o600)
      let written = 0
      try {
        // Reading stops at the first byte too many, which must not close the
        // connection the answer is sent on.
        for await (const chunk of part.iterator({ destroyOnReturn: false })) {
          written += chunk.length
          if (written > backupRuntime.maxFileBytes) {
            throw new BackupImportError('too_large')
          }
          await file.write(chunk)
        }
      } finally {
        await file.close()
      }
      received.size = written
    } catch (error) {
      // What the client is still sending is read and dropped, like any body nobody asked for.
      part.resume()
      request.multipart.abort(error)
    }
  })

  request.request.setTimeout(STALLED_CLIENT_MS)
  try {
    await request.multipart.process()
  } finally {
    // The import itself can take longer than that without a byte on the connection.
    request.request.setTimeout(0)
  }

  return {
    password: request.input('password'),
    file: received.size === null ? null : { path, size: received.size },
  } satisfies BackupUpload
}
