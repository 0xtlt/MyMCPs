import { randomUUID } from 'node:crypto'
import { readFile, readdir, rm, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from '@japa/runner'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'

/**
 * Files in the system tmp directory that hold the uploaded content. The body
 * parser would name them with a UUID, so they are found by what they contain.
 */
async function uploadedCopies(content: Buffer, ignore: Set<string>) {
  const copies: string[] = []

  for (const name of await readdir(tmpdir())) {
    const path = join(tmpdir(), name)
    try {
      const { size } = await stat(path)
      const stored = !ignore.has(name) && size === content.length ? await readFile(path) : null
      if (stored?.equals(content)) {
        copies.push(path)
      }
    } catch {
      // Other processes create and remove their own files in the meantime.
    }
  }

  return copies
}

test.group('hardening: multipart uploads', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('does not write an anonymous multipart upload to disk', async ({
    client,
    assert,
    cleanup,
  }) => {
    await createAdmin({ email: 'admin@example.com' })
    const content = Buffer.from(`mymcps-upload-${randomUUID()}`.repeat(64))
    const existing = new Set(await readdir(tmpdir()))
    cleanup(async () => {
      const copies = await uploadedCopies(content, existing)
      await Promise.all(copies.map((path) => rm(path, { force: true })))
    })

    const response = await client
      .post('/login')
      .withCsrfToken()
      .redirects(0)
      .file('attachment', content, { filename: 'attachment.bin' })
      .fields({ email: 'admin@example.com', password: 'password123' })

    assert.deepEqual(await uploadedCopies(content, existing), [])

    // The body is not parsed at all, so the login form sees no credentials.
    response.assertStatus(302)
    assert.property(response.flashMessage('inputErrorsBag'), 'email')
    response.assertSessionMissing('auth_web')
  })
})
