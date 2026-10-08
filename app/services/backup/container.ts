import { createCipheriv, createDecipheriv, randomBytes, scrypt } from 'node:crypto'

/**
 * The file format of a backup, version 1. The Rust rewrite of MyMCPs reads
 * and writes the same bytes: nothing here can change without it.
 *
 * A 36-byte header in clear, then the plaintext cut into chunks of 64 KiB,
 * each sealed with AES-256-GCM under a key derived from the password with
 * scrypt. The plaintext is the length of the metadata on four bytes, the
 * metadata (a JSON object), then the SQLite database file.
 */
export const BACKUP_HEADER_BYTES = 36
export const BACKUP_CHUNK_BYTES = 65_536
export const BACKUP_TAG_BYTES = 16
/** The metadata is read into memory before anything is known about the file. */
export const BACKUP_MAX_METADATA_BYTES = 65_536

const BLOCK_BYTES = BACKUP_CHUNK_BYTES + BACKUP_TAG_BYTES
const MAGIC = Buffer.from('MYMCPSBK', 'ascii')
const FORMAT_VERSION = 1
const KEY_DERIVATION_SCRYPT = 1
const SALT_BYTES = 16
const NONCE_PREFIX_BYTES = 7
const KEY_BYTES = 32
const LENGTH_BYTES = 4

/** What a backup is written with. */
const WRITER_LOG_N = 17
/**
 * What a backup may ask of whoever reads it. The import form is reachable
 * without an account, and the derivation takes 128 * N * r bytes of memory:
 * 256 MiB at most with these bounds.
 */
const MIN_LOG_N = 14
const MAX_LOG_N = 18
const SCRYPT_R = 8
const SCRYPT_P = 1

export type BackupParams = {
  /** scrypt's N is two to this power. */
  logN: number
  r: number
  p: number
  salt: Buffer
  noncePrefix: Buffer
}

/**
 * Why a file could not be read: it is not a backup (`not_a_backup`), it asks
 * for something this version does not know (`unsupported`), its first chunk
 * does not open with the password (`wrong_password`), or the rest of it is
 * not what was written (`damaged`).
 */
export type BackupContainerFault = 'not_a_backup' | 'unsupported' | 'wrong_password' | 'damaged'

export class BackupContainerError extends Error {
  constructor(public reason: BackupContainerFault) {
    super(`Backup refused: ${reason}`)
    this.name = 'BackupContainerError'
  }
}

/** The parameters of a new backup: a salt and a nonce prefix of its own. */
export function newBackupParams(): BackupParams {
  return {
    logN: WRITER_LOG_N,
    r: SCRYPT_R,
    p: SCRYPT_P,
    salt: randomBytes(SALT_BYTES),
    noncePrefix: randomBytes(NONCE_PREFIX_BYTES),
  }
}

export function encodeBackupHeader(params: BackupParams) {
  if (params.salt.length !== SALT_BYTES || params.noncePrefix.length !== NONCE_PREFIX_BYTES) {
    throw new Error('A backup takes a 16-byte salt and a 7-byte nonce prefix')
  }
  return Buffer.concat([
    MAGIC,
    Buffer.from([FORMAT_VERSION, KEY_DERIVATION_SCRYPT, params.logN, params.r, params.p]),
    params.salt,
    params.noncePrefix,
  ])
}

/**
 * Read the first bytes of a file as the header of a backup. Throws
 * `BackupContainerError` before any work is spent on the file.
 */
export function parseBackupHeader(header: Uint8Array): BackupParams {
  const bytes = Buffer.from(header.buffer, header.byteOffset, header.byteLength)
  if (bytes.length < BACKUP_HEADER_BYTES || !bytes.subarray(0, MAGIC.length).equals(MAGIC)) {
    throw new BackupContainerError('not_a_backup')
  }

  const [version, keyDerivation, logN, r, p] = bytes.subarray(MAGIC.length, MAGIC.length + 5)
  if (
    version !== FORMAT_VERSION ||
    keyDerivation !== KEY_DERIVATION_SCRYPT ||
    logN < MIN_LOG_N ||
    logN > MAX_LOG_N ||
    r !== SCRYPT_R ||
    p !== SCRYPT_P
  ) {
    throw new BackupContainerError('unsupported')
  }

  const salt = MAGIC.length + 5
  const noncePrefix = salt + SALT_BYTES
  return {
    logN,
    r,
    p,
    salt: Buffer.from(bytes.subarray(salt, noncePrefix)),
    noncePrefix: Buffer.from(bytes.subarray(noncePrefix, BACKUP_HEADER_BYTES)),
  }
}

/**
 * The key of a backup. The password is used as typed, in UTF-8, without
 * Unicode normalization. The derivation runs off the event loop.
 */
export function deriveBackupKey(password: string, params: BackupParams) {
  const N = 2 ** params.logN
  return new Promise<Buffer>((resolve, reject) => {
    scrypt(
      Buffer.from(password, 'utf8'),
      params.salt,
      KEY_BYTES,
      // Node refuses these parameters with its default allowance of 32 MiB.
      { N, r: params.r, p: params.p, maxmem: 256 * N * params.r },
      (error, key) => (error ? reject(error) : resolve(key))
    )
  })
}

/** The size of the file that holds a plaintext of this length. */
export function sealedBackupSize(plaintextBytes: number) {
  return (
    BACKUP_HEADER_BYTES +
    plaintextBytes +
    BACKUP_TAG_BYTES * (Math.floor(plaintextBytes / BACKUP_CHUNK_BYTES) + 1)
  )
}

/**
 * The start of the plaintext: the length of the metadata, then the metadata.
 * The database follows.
 */
export function encodeBackupPreamble(metadata: string) {
  const bytes = Buffer.from(metadata, 'utf8')
  if (bytes.length < 1 || bytes.length > BACKUP_MAX_METADATA_BYTES) {
    throw new Error('The metadata of a backup takes 1 to 65,536 bytes')
  }
  const length = Buffer.alloc(LENGTH_BYTES)
  length.writeUInt32BE(bytes.length)
  return Buffer.concat([length, bytes])
}

/**
 * The nonce of a chunk names its place and says whether it is the last one,
 * so chunks cannot be reordered and a file cut short never opens.
 */
function chunkNonce(prefix: Buffer, index: number, last: boolean) {
  if (index > 0xffffffff) {
    throw new Error('A backup holds at most 2^32 chunks')
  }
  const nonce = Buffer.alloc(12)
  prefix.copy(nonce, 0)
  nonce.writeUInt32BE(index, NONCE_PREFIX_BYTES)
  nonce[11] = last ? 1 : 0
  return nonce
}

/**
 * Write a backup: the header, then every chunk as its ciphertext followed by
 * its tag. The plaintext is read as it comes and never held whole.
 */
export async function* sealBackup(
  key: Buffer,
  params: BackupParams,
  plaintext: Iterable<Uint8Array> | AsyncIterable<Uint8Array>
): AsyncGenerator<Buffer> {
  const header = encodeBackupHeader(params)
  yield header

  const seal = (chunk: Buffer, index: number, last: boolean) => {
    const cipher = createCipheriv('aes-256-gcm', key, chunkNonce(params.noncePrefix, index, last))
    cipher.setAAD(header)
    return Buffer.concat([cipher.update(chunk), cipher.final(), cipher.getAuthTag()])
  }

  let pending: Buffer = Buffer.alloc(0)
  let index = 0
  for await (const piece of plaintext) {
    pending = Buffer.concat([pending, piece])
    while (pending.length >= BACKUP_CHUNK_BYTES) {
      yield seal(pending.subarray(0, BACKUP_CHUNK_BYTES), index, false)
      index += 1
      pending = pending.subarray(BACKUP_CHUNK_BYTES)
    }
  }
  // The last chunk is always written, even empty: it is what says the file is whole.
  yield seal(pending, index, true)
}

/**
 * Read the body of a backup, the bytes after its header, and yield its
 * plaintext chunk by chunk. Each chunk is verified before it is handed out;
 * that the file is whole is only known once the last one has been. Throws
 * `BackupContainerError`.
 */
export async function* openBackup(
  key: Buffer,
  header: Uint8Array,
  body: Iterable<Uint8Array> | AsyncIterable<Uint8Array>
): AsyncGenerator<Buffer> {
  const { noncePrefix } = parseBackupHeader(header)

  const open = (block: Buffer, index: number, last: boolean) => {
    const sealed = block.length - BACKUP_TAG_BYTES
    const decipher = createDecipheriv('aes-256-gcm', key, chunkNonce(noncePrefix, index, last))
    decipher.setAAD(header)
    decipher.setAuthTag(block.subarray(sealed))
    try {
      return Buffer.concat([decipher.update(block.subarray(0, sealed)), decipher.final()])
    } catch {
      // Nothing tells a wrong key from a changed file. The first chunk is
      // where a wrong password shows.
      throw new BackupContainerError(index === 0 ? 'wrong_password' : 'damaged')
    }
  }

  let pending: Buffer = Buffer.alloc(0)
  let index = 0
  for await (const piece of body) {
    pending = Buffer.concat([pending, piece])
    // A block is the last one only when nothing follows it.
    while (pending.length > BLOCK_BYTES) {
      yield open(pending.subarray(0, BLOCK_BYTES), index, false)
      index += 1
      pending = pending.subarray(BLOCK_BYTES)
    }
  }

  if (pending.length < BACKUP_TAG_BYTES) {
    throw new BackupContainerError('damaged')
  }
  yield open(pending, index, true)
}

/**
 * Split the plaintext of a backup: the metadata is returned, as the bytes
 * that were written, and the database goes to `writeDatabase` as it comes.
 * Throws `BackupContainerError` when the plaintext is not made that way.
 */
export async function splitBackupPlaintext(
  plaintext: AsyncIterable<Buffer>,
  writeDatabase: (chunk: Buffer) => Promise<unknown>
) {
  let head: Buffer = Buffer.alloc(0)
  let metadata: Buffer | null = null

  for await (const chunk of plaintext) {
    if (metadata) {
      await writeDatabase(chunk)
      continue
    }

    head = Buffer.concat([head, chunk])
    if (head.length < LENGTH_BYTES) continue

    const length = head.readUInt32BE(0)
    if (length < 1 || length > BACKUP_MAX_METADATA_BYTES) {
      throw new BackupContainerError('damaged')
    }
    if (head.length < LENGTH_BYTES + length) continue

    metadata = Buffer.from(head.subarray(LENGTH_BYTES, LENGTH_BYTES + length))
    const database = head.subarray(LENGTH_BYTES + length)
    if (database.length > 0) await writeDatabase(database)
  }

  if (!metadata) {
    throw new BackupContainerError('damaged')
  }
  return metadata
}
