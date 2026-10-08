import { createHash } from 'node:crypto'
import { test } from '@japa/runner'
import {
  BACKUP_CHUNK_BYTES,
  BACKUP_HEADER_BYTES,
  BACKUP_TAG_BYTES,
  BackupContainerError,
  deriveBackupKey,
  encodeBackupHeader,
  encodeBackupPreamble,
  newBackupParams,
  openBackup,
  parseBackupHeader,
  sealBackup,
  sealedBackupSize,
  splitBackupPlaintext,
  type BackupContainerFault,
} from '#services/backup/container'
import { openBackupFile, sealBackupFile } from '#tests/helpers/backup'

/**
 * The known answers of the shared specification of the format, section 2.5.
 * The Rust rewrite of MyMCPs has the same ones: a change that breaks them
 * breaks every backup made so far, and the other implementation.
 */
const PASSWORD = 'correct horse battery staple é'
const PARAMS = {
  logN: 14,
  r: 8,
  p: 1,
  salt: Buffer.from('000102030405060708090a0b0c0d0e0f', 'hex'),
  noncePrefix: Buffer.from('10111213141516', 'hex'),
}
const DERIVED = '963b3ddc4558b37ce77c318ff0232b2eca5600ce92cf0b83f942a4d610131c3d'
const METADATA =
  '{"createdAt":"2026-01-02T03:04:05.000Z","appKey":"base64:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}'
const HEADER = '4d594d435053424b01010e0801000102030405060708090a0b0c0d0e0f10111213141516'

const VECTORS = [
  {
    databaseBytes: 70_000,
    fileBytes: 70_175,
    sha256: 'fa2b3fe5ff9d1d308df4cf5e720fa0e602a92c5234b9189ea27ed40c1a7c62d9',
    first64: `${HEADER}10df1cb22898ebf88efb3f79241de61f3fc47da55587b3d4eda832ee`,
    last32: 'd696019d2b50057a88959058e9cc53dc3177e6bac4630e04b55afb89ce8d48e7',
  },
  {
    // The plaintext is exactly two chunks: the last chunk is empty.
    databaseBytes: 130_965,
    fileBytes: 131_156,
    sha256: '69b6226bf803d982b4a7c96eec65314369bc7771f1b26268be8b355883951e3d',
    first64: `${HEADER}10df1cb22898ebf88efb3f79241de61f3fc47da55587b3d4eda832ee`,
    last32: 'adee2a49abf83d37426bd07e70e952289696b7643be8029518e35acb2cde8f38',
  },
  {
    databaseBytes: 0,
    fileBytes: 159,
    sha256: '8194b986383667d1fb93ee25bdffc38ed8d8c6bbe45d265d53484a98caebd5a0',
    first64: `${HEADER}4bb605855dd1f4ef9bc1b805daf8795b25e1781ec976e4b6b4173aef`,
    last32: '3bbdda2ab790645fbf0e7a561290902fb5788ae8b9906b8d4d1fcb26d4063db3',
  },
]

function vectorDatabase(bytes: number) {
  const database = Buffer.alloc(bytes)
  for (let index = 0; index < bytes; index++) database[index] = index % 251
  return database
}

function sealVector(databaseBytes: number) {
  return sealBackupFile({
    password: PASSWORD,
    params: PARAMS,
    metadata: METADATA,
    database: vectorDatabase(databaseBytes),
  })
}

async function refusal(password: string, file: Buffer): Promise<BackupContainerFault | 'opened'> {
  try {
    await openBackupFile(password, file)
    return 'opened'
  } catch (error) {
    if (error instanceof BackupContainerError) return error.reason
    throw error
  }
}

function withByte(file: Buffer, offset: number, change: (byte: number) => number) {
  const changed = Buffer.from(file)
  changed[offset] = change(changed[offset])
  return changed
}

const flipped = (file: Buffer, offset: number) => withByte(file, offset, (byte) => byte ^ 0x01)

test.group('backup container: known answers', () => {
  test('derives the key of the specification', async ({ assert }) => {
    const key = await deriveBackupKey(PASSWORD, PARAMS)
    assert.equal(key.toString('hex'), DERIVED)
  })

  test('writes the header of the specification', ({ assert }) => {
    const header = encodeBackupHeader(PARAMS)
    assert.lengthOf(header, BACKUP_HEADER_BYTES)
    assert.equal(header.toString('hex'), HEADER)
    assert.deepEqual(parseBackupHeader(header), PARAMS)
  })

  for (const vector of VECTORS) {
    test(`writes the file of a ${vector.databaseBytes}-byte database, and reads it back`, async ({
      assert,
    }) => {
      const database = vectorDatabase(vector.databaseBytes)
      const file = await sealVector(vector.databaseBytes)

      assert.lengthOf(file, vector.fileBytes)
      assert.equal(sealedBackupSize(4 + Buffer.byteLength(METADATA) + database.length), file.length)
      assert.equal(file.subarray(0, 64).toString('hex'), vector.first64)
      assert.equal(file.subarray(file.length - 32).toString('hex'), vector.last32)
      assert.equal(createHash('sha256').update(file).digest('hex'), vector.sha256)

      const opened = await openBackupFile(PASSWORD, file)
      assert.equal(opened.metadata, METADATA)
      assert.isTrue(opened.database.equals(database))
    })
  }

  test('writes and reads the same bytes however the content arrives', async ({ assert }) => {
    const [vector] = VECTORS
    const key = await deriveBackupKey(PASSWORD, PARAMS)
    const plaintext = Buffer.concat([
      encodeBackupPreamble(METADATA),
      vectorDatabase(vector.databaseBytes),
    ])

    // Sizes that fall on no chunk boundary, on both sides.
    const pieces = (content: Buffer, size: number) =>
      Array.from({ length: Math.ceil(content.length / size) }, (_, index) =>
        content.subarray(index * size, (index + 1) * size)
      )

    const written: Buffer[] = []
    for await (const part of sealBackup(key, PARAMS, pieces(plaintext, 7001))) written.push(part)
    const file = Buffer.concat(written)
    assert.equal(createHash('sha256').update(file).digest('hex'), vector.sha256)

    const database: Buffer[] = []
    const metadata = await splitBackupPlaintext(
      openBackup(
        key,
        file.subarray(0, BACKUP_HEADER_BYTES),
        pieces(file.subarray(BACKUP_HEADER_BYTES), 4099)
      ),
      async (chunk) => database.push(chunk)
    )
    assert.equal(metadata.toString(), METADATA)
    assert.isTrue(Buffer.concat(database).equals(vectorDatabase(vector.databaseBytes)))
  })

  test('gives every new backup a salt and a nonce prefix of its own', ({ assert }) => {
    const first = newBackupParams()
    const second = newBackupParams()

    assert.deepInclude(first, { logN: 17, r: 8, p: 1 })
    assert.lengthOf(first.salt, 16)
    assert.lengthOf(first.noncePrefix, 7)
    assert.isFalse(first.salt.equals(second.salt))
    assert.isFalse(first.noncePrefix.equals(second.noncePrefix))
  })
})

test.group('backup container: refusals', () => {
  test('refuses another password', async ({ assert }) => {
    const file = await sealVector(70_000)

    assert.equal(await refusal(`${PASSWORD} `, file), 'wrong_password')
    // The password is used as typed: the same letter written another way is another password.
    assert.equal(await refusal('correct horse battery staple é', file), 'wrong_password')
  })

  test('refuses what is not a backup', async ({ assert }) => {
    const file = await sealVector(0)

    assert.equal(await refusal(PASSWORD, flipped(file, 0)), 'not_a_backup')
    assert.equal(await refusal(PASSWORD, flipped(file, 7)), 'not_a_backup')
    assert.equal(
      await refusal(PASSWORD, Buffer.from('SQLite format 3\0'.repeat(8))),
      'not_a_backup'
    )
    // Shorter than a header.
    assert.equal(await refusal(PASSWORD, file.subarray(0, BACKUP_HEADER_BYTES - 1)), 'not_a_backup')
    assert.equal(await refusal(PASSWORD, Buffer.alloc(0)), 'not_a_backup')
  })

  test('refuses a version, a key derivation and parameters it does not know', async ({
    assert,
  }) => {
    const file = await sealVector(0)
    const unknown: Array<[string, number, number]> = [
      ['version 2', 8, 2],
      ['version 0', 8, 0],
      ['another key derivation', 9, 2],
      ['log2(N) 13', 10, 13],
      ['log2(N) 19', 10, 19],
      ['log2(N) 255', 10, 255],
      ['r 4', 11, 4],
      ['r 16', 11, 16],
      ['p 2', 12, 2],
    ]

    for (const [what, offset, value] of unknown) {
      const changed = withByte(file, offset, () => value)
      assert.equal(await refusal(PASSWORD, changed), 'unsupported', what)
      assert.throws(() => parseBackupHeader(changed.subarray(0, BACKUP_HEADER_BYTES)))
    }

    // The bounds themselves are accepted: the file then fails on its key, not on its header.
    for (const logN of [14, 18]) {
      const header = withByte(file, 10, () => logN).subarray(0, BACKUP_HEADER_BYTES)
      assert.equal(parseBackupHeader(header).logN, logN)
    }
  })

  test('refuses a file whose header was changed, whatever the byte', async ({ assert }) => {
    const file = await sealVector(0)

    for (let offset = 0; offset < BACKUP_HEADER_BYTES; offset++) {
      const reason = await refusal(PASSWORD, flipped(file, offset))
      assert.notEqual(reason, 'opened', `byte ${offset}`)
    }
    // The salt and the nonce prefix are read as they are: only the chunks tell.
    assert.equal(await refusal(PASSWORD, flipped(file, 13)), 'wrong_password')
    assert.equal(await refusal(PASSWORD, flipped(file, 35)), 'wrong_password')
    // So is a cost the reader accepts, which every chunk is bound to.
    assert.equal(
      await refusal(
        PASSWORD,
        withByte(file, 10, () => 15)
      ),
      'wrong_password'
    )
  }).timeout(20_000)

  test('refuses a file whose chunks were changed', async ({ assert }) => {
    // Two full chunks and an empty last one.
    const file = await sealVector(130_965)
    const block = BACKUP_CHUNK_BYTES + BACKUP_TAG_BYTES
    const second = BACKUP_HEADER_BYTES + block

    // The first chunk is where a wrong password shows, so that is what it is taken for.
    assert.equal(await refusal(PASSWORD, flipped(file, BACKUP_HEADER_BYTES)), 'wrong_password')
    assert.equal(await refusal(PASSWORD, flipped(file, second - 1)), 'wrong_password')
    assert.equal(await refusal(PASSWORD, flipped(file, second)), 'damaged')
    assert.equal(await refusal(PASSWORD, flipped(file, second + 1000)), 'damaged')
    // The tag of the empty last chunk.
    assert.equal(await refusal(PASSWORD, flipped(file, file.length - 1)), 'damaged')

    // Chunks in another order.
    const swapped = Buffer.concat([
      file.subarray(0, BACKUP_HEADER_BYTES),
      file.subarray(second, second + block),
      file.subarray(BACKUP_HEADER_BYTES, second),
      file.subarray(second + block),
    ])
    assert.equal(await refusal(PASSWORD, swapped), 'wrong_password')
  })

  test('refuses a file cut short, at a chunk boundary or inside a chunk', async ({ assert }) => {
    const file = await sealVector(130_965)
    const block = BACKUP_CHUNK_BYTES + BACKUP_TAG_BYTES
    const cut = (bytes: number) => refusal(PASSWORD, file.subarray(0, bytes))

    // Without its empty last chunk: the second chunk is not the last one.
    assert.equal(await cut(file.length - BACKUP_TAG_BYTES), 'damaged')
    // Inside the tag of the last chunk, and inside the second chunk.
    assert.equal(await cut(file.length - 1), 'damaged')
    assert.equal(await cut(BACKUP_HEADER_BYTES + block + 1000), 'damaged')
    assert.equal(await cut(BACKUP_HEADER_BYTES + block + 1), 'damaged')
    // After the first chunk, and inside it: what is left does not open at all.
    assert.equal(await cut(BACKUP_HEADER_BYTES + block), 'wrong_password')
    assert.equal(await cut(BACKUP_HEADER_BYTES + 1000), 'wrong_password')
    // Not even the tag of one chunk.
    assert.equal(await cut(BACKUP_HEADER_BYTES + BACKUP_TAG_BYTES - 1), 'damaged')
    assert.equal(await cut(BACKUP_HEADER_BYTES), 'damaged')
  })

  test('refuses a file with bytes after its last chunk', async ({ assert }) => {
    const file = await sealVector(130_965)
    const block = BACKUP_CHUNK_BYTES + BACKUP_TAG_BYTES

    for (const extra of [1, BACKUP_TAG_BYTES, block, block + 1]) {
      const longer = Buffer.concat([file, Buffer.alloc(extra)])
      assert.equal(await refusal(PASSWORD, longer), 'damaged', `${extra} more bytes`)
    }
    // A second copy of the last chunk.
    const repeated = Buffer.concat([file, file.subarray(file.length - BACKUP_TAG_BYTES)])
    assert.equal(await refusal(PASSWORD, repeated), 'damaged')
  })

  test('refuses a content that is not metadata followed by a database', async ({ assert }) => {
    const key = await deriveBackupKey(PASSWORD, PARAMS)
    const header = encodeBackupHeader(PARAMS)

    const split = async (plaintext: Buffer) => {
      const written: Buffer[] = []
      for await (const part of sealBackup(key, PARAMS, [plaintext])) written.push(part)
      const body = Buffer.concat(written).subarray(BACKUP_HEADER_BYTES)
      try {
        await splitBackupPlaintext(openBackup(key, header, [body]), async () => {})
        return 'opened'
      } catch (error) {
        if (error instanceof BackupContainerError) return error.reason
        throw error
      }
    }
    const length = (bytes: number) => {
      const encoded = Buffer.alloc(4)
      encoded.writeUInt32BE(bytes)
      return encoded
    }

    assert.equal(await split(Buffer.alloc(0)), 'damaged')
    assert.equal(await split(Buffer.alloc(3)), 'damaged')
    // No metadata at all, more than it may take, and less than it announces.
    assert.equal(await split(Buffer.concat([length(0), Buffer.from('{}')])), 'damaged')
    assert.equal(await split(Buffer.concat([length(65_537), Buffer.alloc(70_000)])), 'damaged')
    assert.equal(await split(Buffer.concat([length(10), Buffer.from('{}')])), 'damaged')
    assert.equal(await split(Buffer.concat([length(2), Buffer.from('{}')])), 'opened')

    assert.throws(() => encodeBackupPreamble(''))
    assert.throws(() => encodeBackupPreamble('x'.repeat(65_537)))
    assert.lengthOf(encodeBackupPreamble('x'.repeat(65_536)), 65_540)
  })
})
