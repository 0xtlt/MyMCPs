//! The file a backup travels in, version 1.
//!
//! ```text
//! header, 36 bytes, not encrypted
//!   0   8  magic "MYMCPSBK"
//!   8   1  format version, 1
//!   9   1  key derivation, 1 = scrypt
//!   10  1  scrypt log2(N)
//!   11  1  scrypt r
//!   12  1  scrypt p
//!   13  16 salt
//!   29  7  nonce prefix
//! body
//!   the plaintext cut into chunks of 65,536 bytes, each written as its
//!   AES-256-GCM ciphertext followed by its 16-byte tag
//! ```
//!
//! The key is `scrypt(password, salt, N, r, p, 32 bytes)`. The nonce of
//! chunk `i` is the nonce prefix, `i` as four big-endian bytes, then `1` for
//! the final chunk and `0` for the others; every chunk authenticates the
//! header. There are `len / 65536` full chunks, then one final chunk with
//! the rest, written even when it is empty: a file cut anywhere ends on a
//! block that was not sealed as the final one, and never verifies.
//!
//! The plaintext is the length of the metadata on four big-endian bytes,
//! the metadata (a JSON object), then the SQLite database file.
//!
//! The Node app writes and reads the same file: nothing here may change
//! without a new format version.

use std::io::{self, Read, Write};

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};

use super::BackupError;

pub const MAGIC: &[u8; 8] = b"MYMCPSBK";
pub const FORMAT_VERSION: u8 = 1;
/// The only key derivation of version 1.
pub const KDF_SCRYPT: u8 = 1;

pub const HEADER_BYTES: usize = 36;
pub const SALT_BYTES: usize = 16;
pub const NONCE_PREFIX_BYTES: usize = 7;
/// How much plaintext a chunk holds. Only the final one may hold less.
pub const CHUNK_BYTES: usize = 65_536;
pub const TAG_BYTES: usize = 16;
/// A chunk as the file holds it: its ciphertext and its tag.
pub const BLOCK_BYTES: usize = CHUNK_BYTES + TAG_BYTES;
pub const MAX_METADATA_BYTES: usize = 65_536;

/// What a writer asks of scrypt: 128 MiB and a fraction of a second.
pub const WRITER_LOG_N: u8 = 17;
/// What a reader accepts. The import form is reachable without an account,
/// and `log2(N)` decides how much memory a derivation takes (`128 * N * r`
/// bytes: 256 MiB at 18).
pub const MIN_LOG_N: u8 = 14;
pub const MAX_LOG_N: u8 = 18;
const SCRYPT_R: u8 = 8;
const SCRYPT_P: u8 = 1;

/// The first bytes of a backup: how its key is derived, and what makes its
/// nonces unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    log_n: u8,
    salt: [u8; SALT_BYTES],
    nonce_prefix: [u8; NONCE_PREFIX_BYTES],
}

impl Header {
    /// The header of a new backup: the parameters of a writer, a salt and a
    /// nonce prefix drawn at random.
    pub fn generate() -> Self {
        Self {
            log_n: WRITER_LOG_N,
            salt: rand::random(),
            nonce_prefix: rand::random(),
        }
    }

    /// A header with a chosen cost, salt and nonce prefix, to write the same
    /// file twice: the known-answer vectors of the format, and tests that
    /// cannot afford the cost of a real export. `None` when no reader would
    /// accept `log_n`.
    ///
    /// Never for a backup that leaves the instance: a salt or a nonce prefix
    /// used twice with the same password breaks the encryption.
    pub fn from_parts(
        log_n: u8,
        salt: [u8; SALT_BYTES],
        nonce_prefix: [u8; NONCE_PREFIX_BYTES],
    ) -> Option<Self> {
        (MIN_LOG_N..=MAX_LOG_N).contains(&log_n).then_some(Self {
            log_n,
            salt,
            nonce_prefix,
        })
    }

    /// The header a file starts with. A file shorter than a header, or one
    /// that starts with anything else than the magic, is not a backup; a
    /// version, a key derivation or scrypt parameters this version does not
    /// read were written by a newer one.
    pub fn parse(bytes: &[u8]) -> Result<Self, BackupError> {
        let Some(header) = bytes.first_chunk::<HEADER_BYTES>() else {
            return Err(BackupError::NotABackup);
        };
        if &header[..8] != MAGIC {
            return Err(BackupError::NotABackup);
        }
        let [version, kdf, log_n, r, p] =
            [header[8], header[9], header[10], header[11], header[12]];
        if version != FORMAT_VERSION
            || kdf != KDF_SCRYPT
            || !(MIN_LOG_N..=MAX_LOG_N).contains(&log_n)
            || r != SCRYPT_R
            || p != SCRYPT_P
        {
            return Err(BackupError::NewerVersion);
        }
        let (Ok(salt), Ok(nonce_prefix)) = (header[13..29].try_into(), header[29..36].try_into())
        else {
            return Err(BackupError::NotABackup);
        };
        Ok(Self {
            log_n,
            salt,
            nonce_prefix,
        })
    }

    /// The header read from the start of a file.
    pub fn read(input: impl Read) -> Result<Self, BackupError> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES);
        input.take(HEADER_BYTES as u64).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }

    pub fn to_bytes(&self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0; HEADER_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8] = FORMAT_VERSION;
        bytes[9] = KDF_SCRYPT;
        bytes[10] = self.log_n;
        bytes[11] = SCRYPT_R;
        bytes[12] = SCRYPT_P;
        bytes[13..29].copy_from_slice(&self.salt);
        bytes[29..36].copy_from_slice(&self.nonce_prefix);
        bytes
    }

    pub fn log_n(&self) -> u8 {
        self.log_n
    }
}

/// The key of one backup.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Key(..)")
    }
}

impl Key {
    /// Derive the key from the password, taken as the UTF-8 bytes it is,
    /// without Unicode normalization.
    ///
    /// Holds a core and `128 * N * 8` bytes (128 MiB for a new backup) for
    /// a fraction of a second: never call it on an async thread. A server
    /// goes through [`super::KeyDerivations`], which also runs one at a time.
    pub fn derive(password: &str, header: &Header) -> Self {
        let params = scrypt::Params::new(header.log_n, u32::from(SCRYPT_R), u32::from(SCRYPT_P))
            .expect("a header only holds scrypt parameters that are valid");
        let mut key = <[u8; 32]>::default();
        scrypt::scrypt(password.as_bytes(), &header.salt, &params, &mut key)
            .expect("32 bytes is a valid scrypt output length");
        Self(key)
    }

    /// The key itself, for the known-answer test of the derivation.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

fn nonce(prefix: &[u8; NONCE_PREFIX_BYTES], counter: u32, last: bool) -> [u8; 12] {
    let mut nonce = <[u8; 12]>::default();
    nonce[..NONCE_PREFIX_BYTES].copy_from_slice(prefix);
    nonce[NONCE_PREFIX_BYTES..11].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = u8::from(last);
    nonce
}

fn misuse(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// The size of the file that holds `plaintext_len` bytes of plaintext.
pub fn sealed_len(plaintext_len: u64) -> u64 {
    let chunks = plaintext_len / CHUNK_BYTES as u64 + 1;
    HEADER_BYTES as u64 + plaintext_len + TAG_BYTES as u64 * chunks
}

/// What the plaintext starts with: the length of the metadata, then the
/// metadata.
pub fn plaintext_prefix(metadata: &[u8]) -> io::Result<Vec<u8>> {
    if metadata.is_empty() || metadata.len() > MAX_METADATA_BYTES {
        return Err(misuse("the metadata of a backup takes 1 to 65,536 bytes"));
    }
    let mut prefix = Vec::with_capacity(4 + metadata.len());
    prefix.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    prefix.extend_from_slice(metadata);
    Ok(prefix)
}

/// Encrypts the chunks of one backup, in order.
pub struct Sealer {
    cipher: Aes256Gcm,
    header: [u8; HEADER_BYTES],
    nonce_prefix: [u8; NONCE_PREFIX_BYTES],
    chunks: u64,
    finished: bool,
}

impl Sealer {
    pub fn new(key: &Key, header: &Header) -> Self {
        Self {
            cipher: Aes256Gcm::new(&key.0.into()),
            header: header.to_bytes(),
            nonce_prefix: header.nonce_prefix,
            chunks: 0,
            finished: false,
        }
    }

    /// The next chunk as the file holds it: its ciphertext, then its tag.
    /// Every chunk is full but the final one, which may be empty.
    pub fn seal(&mut self, chunk: &[u8], last: bool) -> io::Result<Vec<u8>> {
        if self.finished {
            return Err(misuse("the final chunk of this backup was already sealed"));
        }
        if chunk.len() > CHUNK_BYTES || (!last && chunk.len() != CHUNK_BYTES) {
            return Err(misuse("only the final chunk of a backup may be short"));
        }
        // A nonce is never used twice: the counter does not wrap.
        let counter =
            u32::try_from(self.chunks).map_err(|_| misuse("a backup holds at most 2^32 chunks"))?;
        let sealed = self
            .cipher
            .encrypt(
                &nonce(&self.nonce_prefix, counter, last).into(),
                Payload {
                    msg: chunk,
                    aad: &self.header,
                },
            )
            .map_err(|_| io::Error::other("a chunk of the backup could not be encrypted"))?;
        self.chunks += 1;
        self.finished = last;
        Ok(sealed)
    }
}

/// Fill `chunk` with the next [`CHUNK_BYTES`] of `reader`, or with what is
/// left of it.
fn read_chunk(reader: &mut impl Read, chunk: &mut Vec<u8>) -> io::Result<()> {
    chunk.resize(CHUNK_BYTES, 0);
    let mut filled = 0;
    while filled < CHUNK_BYTES {
        match reader.read(&mut chunk[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    chunk.truncate(filled);
    Ok(())
}

/// Write a whole backup: the header, then `metadata` and `database` as the
/// encrypted body. Returns the size of what was written.
///
/// The export of the server does not go through here: it streams the same
/// bytes from [`super::Export`]. This is the format in one function, for
/// tests and tools: with a [`Header::from_parts`] it writes the same file
/// every time.
pub fn write_container(
    key: &Key,
    header: &Header,
    metadata: &[u8],
    database: impl Read,
    mut output: impl Write,
) -> io::Result<u64> {
    let mut plaintext = io::Cursor::new(plaintext_prefix(metadata)?).chain(database);
    let mut sealer = Sealer::new(key, header);
    output.write_all(&header.to_bytes())?;
    let mut written = HEADER_BYTES as u64;

    let mut chunk = Vec::with_capacity(CHUNK_BYTES);
    loop {
        read_chunk(&mut plaintext, &mut chunk)?;
        let last = chunk.len() < CHUNK_BYTES;
        let sealed = sealer.seal(&chunk, last)?;
        output.write_all(&sealed)?;
        written += sealed.len() as u64;
        if last {
            break;
        }
    }
    output.flush()?;
    Ok(written)
}

/// Sorts decrypted plaintext into the metadata, kept here, and the
/// database, written out as it comes.
#[derive(Default)]
struct Plaintext {
    /// The four bytes of the length, then the metadata, as far as they were read.
    head: Vec<u8>,
    metadata_len: Option<usize>,
}

impl Plaintext {
    fn take(&mut self, mut chunk: &[u8], database: &mut impl Write) -> Result<(), BackupError> {
        let metadata_len = match self.metadata_len {
            Some(length) => length,
            None => {
                let taken = (4 - self.head.len()).min(chunk.len());
                self.head.extend_from_slice(&chunk[..taken]);
                chunk = &chunk[taken..];
                let Some(length) = self.head.first_chunk::<4>() else {
                    return Ok(());
                };
                let length = u32::from_be_bytes(*length) as usize;
                if !(1..=MAX_METADATA_BYTES).contains(&length) {
                    return Err(BackupError::Damaged);
                }
                self.metadata_len = Some(length);
                length
            }
        };
        let missing = (4 + metadata_len).saturating_sub(self.head.len());
        let taken = missing.min(chunk.len());
        self.head.extend_from_slice(&chunk[..taken]);
        let database_bytes = &chunk[taken..];
        if !database_bytes.is_empty() {
            database.write_all(database_bytes)?;
        }
        Ok(())
    }

    fn into_metadata(mut self) -> Result<Vec<u8>, BackupError> {
        match self.metadata_len {
            Some(length) if self.head.len() == 4 + length => Ok(self.head.split_off(4)),
            _ => Err(BackupError::Damaged),
        }
    }
}

/// Decrypt the body of a backup: `input` is at the first byte after the
/// header and holds `body_len` more. The database is written to `database`
/// and the metadata is returned, as the bytes the file holds.
///
/// The last block of the file is taken as the final chunk. A first chunk
/// that does not verify is most often a wrong password
/// ([`BackupError::WrongPassword`]); a later one, or a file that ends
/// early, is a damaged file ([`BackupError::Damaged`]), as is a plaintext
/// that does not start with metadata.
pub fn read_body(
    key: &Key,
    header: &Header,
    mut input: impl Read,
    body_len: u64,
    mut database: impl Write,
) -> Result<Vec<u8>, BackupError> {
    if body_len < TAG_BYTES as u64 {
        return Err(BackupError::Damaged);
    }
    let cipher = Aes256Gcm::new(&key.0.into());
    let authenticated = header.to_bytes();
    let blocks = body_len.div_ceil(BLOCK_BYTES as u64);

    let mut plaintext = Plaintext::default();
    let mut block = vec![0; BLOCK_BYTES];
    for index in 0..blocks {
        let last = index + 1 == blocks;
        let size = if last {
            (body_len - index * BLOCK_BYTES as u64) as usize
        } else {
            BLOCK_BYTES
        };
        // What is left after the last full block is too short to be a chunk.
        if size < TAG_BYTES {
            return Err(BackupError::Damaged);
        }
        let counter = u32::try_from(index).map_err(|_| BackupError::Damaged)?;
        input
            .read_exact(&mut block[..size])
            .map_err(|error| match error.kind() {
                io::ErrorKind::UnexpectedEof => BackupError::Damaged,
                _ => BackupError::Io(error),
            })?;
        let chunk = cipher
            .decrypt(
                &nonce(&header.nonce_prefix, counter, last).into(),
                Payload {
                    msg: &block[..size],
                    aad: &authenticated,
                },
            )
            .map_err(|_| {
                if index == 0 {
                    BackupError::WrongPassword
                } else {
                    BackupError::Damaged
                }
            })?;
        plaintext.take(&chunk, &mut database)?;
    }
    database.flush()?;
    plaintext.into_metadata()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_bytes_of_a_file_before_writing_it() {
        // Header, the plaintext, and one tag for each chunk: the final chunk
        // is there even when it is empty.
        assert_eq!(sealed_len(0), 36 + 16);
        assert_eq!(sealed_len(1), 36 + 1 + 16);
        assert_eq!(sealed_len(65_535), 36 + 65_535 + 16);
        assert_eq!(sealed_len(65_536), 36 + 65_536 + 32);
        assert_eq!(sealed_len(131_072), 36 + 131_072 + 48);
        assert_eq!(sealed_len(70_107), 70_175);
    }

    #[test]
    fn splits_the_plaintext_wherever_the_chunks_fall() {
        let metadata = b"{\"a\":1}";
        let mut whole = plaintext_prefix(metadata).unwrap();
        whole.extend_from_slice(b"database bytes");

        for piece in [1, 2, 3, 5, 11, whole.len()] {
            let mut plaintext = Plaintext::default();
            let mut database = Vec::new();
            for chunk in whole.chunks(piece) {
                plaintext.take(chunk, &mut database).unwrap();
            }
            assert_eq!(plaintext.into_metadata().unwrap(), metadata, "{piece}");
            assert_eq!(database, b"database bytes", "{piece}");
        }

        // A length of zero, one past the limit, and a plaintext that ends
        // before its metadata does.
        for damaged in [
            &[0, 0, 0, 0, b'{'][..],
            &[0, 1, 0, 1, b'{'][..],
            &[0, 0, 0, 9, b'{', b'}'][..],
            &[0, 0][..],
        ] {
            let mut plaintext = Plaintext::default();
            let outcome = plaintext
                .take(damaged, &mut Vec::new())
                .and_then(|()| plaintext.into_metadata());
            assert!(matches!(outcome, Err(BackupError::Damaged)), "{damaged:?}");
        }
    }

    #[test]
    fn refuses_metadata_no_reader_would_take() {
        assert!(plaintext_prefix(b"").is_err());
        assert!(plaintext_prefix(&vec![b' '; MAX_METADATA_BYTES + 1]).is_err());
        assert_eq!(
            plaintext_prefix(&vec![b' '; MAX_METADATA_BYTES]).unwrap()[..4],
            [0, 1, 0, 0]
        );
    }
}
