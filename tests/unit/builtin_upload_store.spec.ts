import { randomUUID } from 'node:crypto'
import { mkdir, readdir, readFile, truncate, utimes, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import { test } from '@japa/runner'
import {
  BUILTIN_UPLOAD_MINUTES,
  BuiltinUploadError,
  findBuiltinUpload,
  pruneBuiltinUploads,
  readBuiltinUpload,
  removeBuiltinUploads,
  saveBuiltinUpload,
  startBuiltinUploadSweeper,
  stopBuiltinUploadSweeper,
} from '#services/builtin/upload_store'
import { clearUploads, uploadsDirectory } from '#tests/helpers/icloud_mail'

const MCP = 7
const HOUR_MS = BUILTIN_UPLOAD_MINUTES * 60_000

function target(overrides: { id?: string; maxBytes?: number } = {}) {
  return {
    id: randomUUID(),
    filename: 'report.pdf',
    contentType: 'application/pdf',
    maxBytes: 1_000,
    ...overrides,
  }
}

function body(...pieces: string[]) {
  return Readable.from(pieces.map((piece) => Buffer.from(piece)))
}

async function refusal(save: Promise<unknown>) {
  try {
    await save
    return null
  } catch (error) {
    if (!(error instanceof BuiltinUploadError)) throw error
    return error.reason
  }
}

async function stored(mcpId = MCP) {
  try {
    const names = await readdir(uploadsDirectory(mcpId))
    return names.sort()
  } catch {
    return null
  }
}

async function storedCount(mcpId = MCP) {
  const names = await stored(mcpId)
  return names?.length ?? 0
}

async function text(id: string, mcpId = MCP) {
  const content = await readBuiltinUpload(mcpId, id)
  return content?.toString()
}

/** A file as a finished upload leaves it, without writing its bytes. */
async function waiting(mcpId: number, size: number, expiresAt = Date.now() + HOUR_MS) {
  const id = randomUUID()
  const path = join(uploadsDirectory(mcpId), id)
  await mkdir(uploadsDirectory(mcpId), { recursive: true })
  await writeFile(path, '')
  await truncate(path, size)
  await writeFile(`${path}.json`, JSON.stringify({ id, filename: 'big.bin', size, expiresAt }))
  return id
}

test.group('Built-in uploads: store', (group) => {
  group.each.setup(clearUploads)
  group.each.teardown(clearUploads)

  test('keeps the file a link was made for, and gives it back', async ({ assert }) => {
    const file = target()
    const before = Date.now()
    const upload = await saveBuiltinUpload(MCP, file, body('%PDF-', '1.4\n', '%%EOF\n'))

    assert.deepEqual(
      { ...upload, expiresAt: 0 },
      {
        id: file.id,
        filename: 'report.pdf',
        contentType: 'application/pdf',
        size: 15,
        expiresAt: 0,
      }
    )
    assert.isAtLeast(upload.expiresAt, before + HOUR_MS)
    assert.isAtMost(upload.expiresAt, Date.now() + HOUR_MS)

    assert.deepEqual(await findBuiltinUpload(MCP, file.id), upload)
    assert.equal(await text(file.id), '%PDF-1.4\n%%EOF\n')
    assert.deepEqual(await stored(), [file.id, `${file.id}.json`])

    // Uploads belong to one MCP.
    assert.isNull(await findBuiltinUpload(MCP + 1, file.id))
    assert.isNull(await readBuiltinUpload(MCP + 1, file.id))
    assert.isNull(await findBuiltinUpload(MCP, randomUUID()))
  })

  test('takes one file for each link, also while the first is arriving', async ({ assert }) => {
    const file = target()
    let arrived = () => {}
    const slow = (async function* () {
      yield Buffer.from('first')
      await new Promise<void>((resolve) => {
        arrived = resolve
      })
    })()

    const first = saveBuiltinUpload(MCP, file, slow)
    while ((await storedCount()) === 0) {
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
    // Nothing can be attached before the file is whole.
    assert.isNull(await findBuiltinUpload(MCP, file.id))
    assert.equal(await refusal(saveBuiltinUpload(MCP, file, body('second'))), 'taken')

    arrived()
    await first
    assert.equal(await refusal(saveBuiltinUpload(MCP, file, body('third'))), 'taken')
    assert.equal(await text(file.id), 'first')
  })

  test('keeps nothing of a file that is empty or too large, so the link can be tried again', async ({
    assert,
  }) => {
    const file = target({ maxBytes: 10 })

    assert.equal(await refusal(saveBuiltinUpload(MCP, file, body())), 'empty')
    assert.equal(await refusal(saveBuiltinUpload(MCP, file, body('12345', '678901'))), 'too_large')
    assert.deepEqual(await stored(), [])
    assert.isNull(await findBuiltinUpload(MCP, file.id))

    // A body that breaks off is not a file either.
    const broken = Readable.from(
      (async function* () {
        yield Buffer.from('1234')
        throw new Error('aborted')
      })()
    )
    await assert.rejects(() => saveBuiltinUpload(MCP, file, broken), 'aborted')
    assert.deepEqual(await stored(), [])

    const upload = await saveBuiltinUpload(MCP, file, body('1234567890'))
    assert.equal(upload.size, 10)
  })

  test('only stores under a UUID, for a saved MCP', async ({ assert }) => {
    for (const id of [
      '../escape',
      '..',
      'report.pdf',
      '',
      `${randomUUID()}.json`,
      'A'.repeat(36),
    ]) {
      await assert.rejects(
        () => saveBuiltinUpload(MCP, target({ id }), body('x')),
        'Not an upload id'
      )
      await assert.rejects(() => findBuiltinUpload(MCP, id), 'Not an upload id')
      await assert.rejects(() => readBuiltinUpload(MCP, id), 'Not an upload id')
    }
    for (const mcpId of [0, -1, 1.5, Number.NaN]) {
      await assert.rejects(() => saveBuiltinUpload(mcpId, target(), body('x')))
      await assert.rejects(() => removeBuiltinUploads(mcpId))
    }
    assert.isNull(await stored())
  })

  test('holds at most 50 files and 100 MB for an MCP', async ({ assert }) => {
    for (let count = 0; count < 49; count++) {
      await waiting(MCP, 1)
    }
    await saveBuiltinUpload(MCP, target(), body('fiftieth'))
    assert.equal(await refusal(saveBuiltinUpload(MCP, target(), body('one more'))), 'full')
    // Another MCP has its own room.
    await saveBuiltinUpload(MCP + 1, target(), body('other'))

    await clearUploads()
    for (let count = 0; count < 3; count++) {
      await waiting(MCP, 25_000_000)
    }
    await waiting(MCP, 24_999_990)
    // 10 bytes of room are left: a file is cut off at the first byte too many.
    const large = target({ maxBytes: 20_000_000 })
    assert.equal(await refusal(saveBuiltinUpload(MCP, large, body('12345678901'))), 'full')
    await saveBuiltinUpload(MCP, large, body('1234567890'))
    assert.equal(await refusal(saveBuiltinUpload(MCP, target(), body('x'))), 'full')
  })

  test('stops giving a file back an hour after its upload, and deletes it', async ({ assert }) => {
    const expired = await waiting(MCP, 5, Date.now() - 1)
    const fresh = await waiting(MCP, 5)

    assert.isNull(await findBuiltinUpload(MCP, expired))
    assert.isNotNull(await findBuiltinUpload(MCP, fresh))
    assert.deepEqual(await stored(), [fresh, `${fresh}.json`])

    // An expired file makes room for the next upload without being asked for.
    const other = await waiting(MCP, 5, Date.now() - 1)
    const file = target()
    await saveBuiltinUpload(MCP, file, body('new'))
    assert.notInclude(await stored(), other)
    assert.equal(await storedCount(), 4)
  })

  test('prunes expired files, abandoned uploads, and the directories left empty', async ({
    assert,
  }) => {
    const kept = await waiting(MCP, 5)
    const expired = await waiting(MCP, 5, Date.now() - 1)
    const gone = await waiting(MCP + 1, 5, Date.now() - 1)
    // Uploads that never finished: one still arriving, one left by a server that stopped.
    const arriving = randomUUID()
    const abandoned = randomUUID()
    await writeFile(join(uploadsDirectory(MCP), arriving), 'half')
    await writeFile(join(uploadsDirectory(MCP), abandoned), 'half')
    const longAgo = new Date(Date.now() - HOUR_MS - 1_000)
    await utimes(join(uploadsDirectory(MCP), abandoned), longAgo, longAgo)
    // Not ours to delete.
    await mkdir(join(uploadsDirectory(), 'notes'), { recursive: true })
    await writeFile(join(uploadsDirectory(MCP), 'README'), 'left alone')

    await pruneBuiltinUploads()

    assert.deepEqual(await stored(), ['README', arriving, kept, `${kept}.json`].sort())
    assert.notInclude(await stored(), expired)
    assert.isNull(await stored(MCP + 1), gone)
    assert.includeMembers(await readdir(uploadsDirectory()), [String(MCP), 'notes'])

    // An hour later, everything of ours has expired.
    await pruneBuiltinUploads(Date.now() + HOUR_MS + 1_000)
    assert.deepEqual(await stored(), ['README'])
    assert.equal(await readFile(join(uploadsDirectory(MCP), 'README'), 'utf8'), 'left alone')
  })

  test('sweeps on start for what a previous run left, and deletes the files of an MCP with it', async ({
    assert,
  }) => {
    await waiting(MCP, 5, Date.now() - 1)
    const kept = await waiting(MCP, 5)

    const errors: unknown[] = []
    startBuiltinUploadSweeper((error) => errors.push(error), 60_000)
    try {
      while ((await storedCount()) > 2) {
        await new Promise((resolve) => setTimeout(resolve, 5))
      }
      assert.deepEqual(await stored(), [kept, `${kept}.json`])
      assert.deepEqual(errors, [])
    } finally {
      stopBuiltinUploadSweeper()
    }

    await removeBuiltinUploads(MCP)
    assert.isNull(await stored())
    await removeBuiltinUploads(MCP)
  })
})
