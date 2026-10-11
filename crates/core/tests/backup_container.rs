//! The file format of a backup, held to the known answers of its
//! specification: the Node app writes and reads the same bytes, so a
//! backup made by either opens in the other.
//!
//! The vectors were produced by the reference implementation of the format
//! (`node:crypto` only). The "database" in them is not SQLite: these tests
//! are about the container.

use std::io::Cursor;

use mymcps_core::backup::container::{
    BLOCK_BYTES, CHUNK_BYTES, HEADER_BYTES, Sealer, TAG_BYTES, plaintext_prefix, read_body,
    sealed_len, write_container,
};
use mymcps_core::backup::{BackupError, Header, Key};
use sha2::{Digest, Sha256};

const PASSWORD: &str = "correct horse battery staple \u{e9}";
const SALT: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const NONCE_PREFIX: [u8; 7] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16];
const LOG_N: u8 = 14;
const DERIVED_KEY: &str = "963b3ddc4558b37ce77c318ff0232b2eca5600ce92cf0b83f942a4d610131c3d";
const METADATA: &str = r#"{"createdAt":"2026-01-02T03:04:05.000Z","appKey":"base64:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;

struct Vector {
    database_bytes: usize,
    file_bytes: usize,
    sha256: &'static str,
    first_64: &'static str,
    last_32: &'static str,
}

const FIRST_64: &str = "4d594d435053424b01010e0801000102030405060708090a0b0c0d0e0f1011121314151610df1cb22898ebf88efb3f79241de61f3fc47da55587b3d4eda832ee";

const VECTORS: &[Vector] = &[
    Vector {
        database_bytes: 70_000,
        file_bytes: 70_175,
        sha256: "fa2b3fe5ff9d1d308df4cf5e720fa0e602a92c5234b9189ea27ed40c1a7c62d9",
        first_64: FIRST_64,
        last_32: "d696019d2b50057a88959058e9cc53dc3177e6bac4630e04b55afb89ce8d48e7",
    },
    // The plaintext is exactly two chunks: the final chunk is empty.
    Vector {
        database_bytes: 130_965,
        file_bytes: 131_156,
        sha256: "69b6226bf803d982b4a7c96eec65314369bc7771f1b26268be8b355883951e3d",
        first_64: FIRST_64,
        last_32: "adee2a49abf83d37426bd07e70e952289696b7643be8029518e35acb2cde8f38",
    },
    Vector {
        database_bytes: 0,
        file_bytes: 159,
        sha256: "8194b986383667d1fb93ee25bdffc38ed8d8c6bbe45d265d53484a98caebd5a0",
        first_64: "4d594d435053424b01010e0801000102030405060708090a0b0c0d0e0f101112131415164bb605855dd1f4ef9bc1b805daf8795b25e1781ec976e4b6b4173aef",
        last_32: "3bbdda2ab790645fbf0e7a561290902fb5788ae8b9906b8d4d1fcb26d4063db3",
    },
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `n` bytes where byte `i` is `i % 251`.
fn database(bytes: usize) -> Vec<u8> {
    (0..bytes).map(|index| (index % 251) as u8).collect()
}

fn header() -> Header {
    Header::from_parts(LOG_N, SALT, NONCE_PREFIX).unwrap()
}

fn seal(key: &Key, database: &[u8]) -> Vec<u8> {
    let mut file = Vec::new();
    let written = write_container(
        key,
        &header(),
        METADATA.as_bytes(),
        Cursor::new(database),
        &mut file,
    )
    .unwrap();
    assert_eq!(written, file.len() as u64);
    file
}

/// Read a file with a key that was already derived.
fn open_with(key: &Key, file: &[u8]) -> Result<(Vec<u8>, Vec<u8>), BackupError> {
    let header = Header::parse(file)?;
    let mut database = Vec::new();
    let body = &file[HEADER_BYTES..];
    let metadata = read_body(key, &header, body, body.len() as u64, &mut database)?;
    Ok((metadata, database))
}

/// Read a file as an import does: the header, the key, then the body.
fn open(password: &str, file: &[u8]) -> Result<(Vec<u8>, Vec<u8>), BackupError> {
    let header = Header::read(file)?;
    open_with(&Key::derive(password, &header), file)
}

#[test]
fn derives_the_key_of_the_specification() {
    assert_eq!(METADATA.len(), 103);
    assert_eq!(PASSWORD.as_bytes()[PASSWORD.len() - 2..], [0xc3, 0xa9]);
    let key = Key::derive(PASSWORD, &header());
    assert_eq!(hex(key.as_bytes()), DERIVED_KEY);
    // No Unicode normalization: the same letter written as two code points
    // is another password.
    let decomposed = "correct horse battery staple e\u{301}";
    assert_ne!(
        hex(Key::derive(decomposed, &header()).as_bytes()),
        DERIVED_KEY
    );
}

#[test]
fn writes_and_reads_the_known_answer_vectors_byte_for_byte() {
    let key = Key::derive(PASSWORD, &header());
    for vector in VECTORS {
        let database = database(vector.database_bytes);
        let file = seal(&key, &database);
        let name = vector.database_bytes;

        assert_eq!(file.len(), vector.file_bytes, "{name}");
        assert_eq!(
            sealed_len((4 + METADATA.len() + database.len()) as u64),
            vector.file_bytes as u64,
            "{name}"
        );
        assert_eq!(hex(&file[..64]), vector.first_64, "{name}");
        assert_eq!(hex(&file[file.len() - 32..]), vector.last_32, "{name}");
        assert_eq!(hex(&Sha256::digest(&file)), vector.sha256, "{name}");

        let (metadata, read) = open(PASSWORD, &file).unwrap();
        assert_eq!(metadata, METADATA.as_bytes(), "{name}");
        assert_eq!(read, database, "{name}");
    }
}

#[test]
fn a_header_says_how_to_derive_the_key() {
    let bytes = header().to_bytes();
    assert_eq!(bytes.len(), 36);
    assert_eq!(hex(&bytes), FIRST_64[..72]);
    assert_eq!(Header::parse(&bytes).unwrap(), header());
    assert_eq!(header().log_n(), 14);

    // A writer asks for more than the vectors do, and never draws the same
    // salt or nonce prefix twice.
    let (first, second) = (Header::generate(), Header::generate());
    assert_eq!(first.log_n(), 17);
    assert_eq!(first.to_bytes()[..13], *b"MYMCPSBK\x01\x01\x11\x08\x01");
    assert_ne!(first.to_bytes()[13..], second.to_bytes()[13..]);

    for accepted in 14..=18 {
        assert!(Header::from_parts(accepted, SALT, NONCE_PREFIX).is_some());
    }
    for refused in [0, 13, 19, 255] {
        assert!(Header::from_parts(refused, SALT, NONCE_PREFIX).is_none());
    }
}

#[test]
fn refuses_what_is_not_a_backup_and_what_a_newer_version_wrote() {
    let key = Key::derive(PASSWORD, &header());
    let file = seal(&key, &database(1_000));
    let with = |offset: usize, value: u8| {
        let mut changed = file.clone();
        changed[offset] = value;
        changed
    };

    // Wrong magic, or shorter than a header.
    for not_a_backup in [
        with(0, b'm'),
        with(7, b'X'),
        b"SQLite format 3\0 and whatever follows it in a database file".to_vec(),
        file[..HEADER_BYTES - 1].to_vec(),
        b"MYMCPSBK".to_vec(),
        Vec::new(),
    ] {
        assert!(
            matches!(open(PASSWORD, &not_a_backup), Err(BackupError::NotABackup)),
            "{} bytes",
            not_a_backup.len()
        );
    }

    // Version 2, another key derivation, log2(N) 13 and 19, r 4, p 2: none
    // of them costs a derivation.
    for (offset, value) in [
        (8, 2),
        (8, 0),
        (9, 2),
        (10, 13),
        (10, 19),
        (10, 255),
        (11, 4),
        (12, 2),
    ] {
        assert!(
            matches!(
                open(PASSWORD, &with(offset, value)),
                Err(BackupError::NewerVersion)
            ),
            "byte {offset} = {value}"
        );
    }
}

#[test]
fn refuses_a_wrong_password_and_a_header_that_was_altered() {
    let key = Key::derive(PASSWORD, &header());
    let file = seal(&key, &database(70_000));

    for wrong in [
        "correct horse battery staple e",
        "Correct horse battery staple \u{e9}",
        "correct horse battery staple \u{e9} ",
        "",
    ] {
        assert!(
            matches!(open(wrong, &file), Err(BackupError::WrongPassword)),
            "{wrong:?}"
        );
    }

    // The header is authenticated with every chunk: a flipped bit in the
    // salt gives another key, one in the nonce prefix other nonces, and
    // another cost a key that opens nothing.
    for offset in [10, 13, 28, 29, 35] {
        let mut changed = file.clone();
        changed[offset] ^= 0x01;
        assert!(
            matches!(open(PASSWORD, &changed), Err(BackupError::WrongPassword)),
            "byte {offset}"
        );
    }
}

#[test]
fn refuses_a_body_that_was_altered_cut_or_extended() {
    let key = Key::derive(PASSWORD, &header());
    // Three chunks: two full ones and the final one.
    let database = database(2 * CHUNK_BYTES + 5_000);
    let file = seal(&key, &database);
    assert_eq!(
        file.len(),
        HEADER_BYTES + 2 * BLOCK_BYTES + 5_107 + TAG_BYTES
    );
    assert_eq!(open_with(&key, &file).unwrap().1, database);
    let second_chunk = HEADER_BYTES + BLOCK_BYTES;
    let final_chunk = HEADER_BYTES + 2 * BLOCK_BYTES;

    let flipped = |offset: usize| {
        let mut changed = file.clone();
        changed[offset] ^= 0x80;
        changed
    };
    // In the first chunk, a damaged file cannot be told from a wrong password.
    for offset in [HEADER_BYTES, HEADER_BYTES + 40_000, second_chunk - 1] {
        assert!(
            matches!(
                open_with(&key, &flipped(offset)),
                Err(BackupError::WrongPassword)
            ),
            "byte {offset}"
        );
    }
    // Later, the password was right: the file is damaged.
    for offset in [
        second_chunk,
        second_chunk + 123,
        final_chunk - 1,
        final_chunk,
        file.len() - 1,
    ] {
        assert!(
            matches!(open_with(&key, &flipped(offset)), Err(BackupError::Damaged)),
            "byte {offset}"
        );
    }

    // Cut at a chunk boundary: the last block was not sealed as the final one.
    assert!(matches!(
        open_with(&key, &file[..final_chunk]),
        Err(BackupError::Damaged)
    ));
    assert!(matches!(
        open_with(&key, &file[..second_chunk]),
        Err(BackupError::WrongPassword)
    ));
    // Cut inside a chunk, inside a tag, and down to less than a tag.
    for length in [
        file.len() - 1,
        final_chunk + TAG_BYTES,
        final_chunk + 1,
        second_chunk + 30_000,
    ] {
        assert!(
            matches!(open_with(&key, &file[..length]), Err(BackupError::Damaged)),
            "{length} bytes"
        );
    }
    assert!(matches!(
        open_with(&key, &file[..HEADER_BYTES + 30_000]),
        Err(BackupError::WrongPassword)
    ));
    // A header and nothing that could be a chunk: the file ends early.
    for length in [HEADER_BYTES, HEADER_BYTES + TAG_BYTES - 1] {
        assert!(
            matches!(open_with(&key, &file[..length]), Err(BackupError::Damaged)),
            "{length} bytes"
        );
    }

    // Bytes appended: one, a tag's worth, a whole block, and a second copy
    // of the final chunk.
    for extra in [
        vec![0],
        vec![0; TAG_BYTES],
        vec![0; BLOCK_BYTES],
        file[final_chunk..].to_vec(),
    ] {
        let mut extended = file.clone();
        extended.extend_from_slice(&extra);
        assert!(
            matches!(open_with(&key, &extended), Err(BackupError::Damaged)),
            "{} bytes appended",
            extra.len()
        );
    }

    // Chunks that changed places, and a file whose chunks are those of
    // another one made with the same password and header.
    let mut swapped = file[..HEADER_BYTES].to_vec();
    swapped.extend_from_slice(&file[second_chunk..final_chunk]);
    swapped.extend_from_slice(&file[HEADER_BYTES..second_chunk]);
    swapped.extend_from_slice(&file[final_chunk..]);
    assert!(matches!(
        open_with(&key, &swapped),
        Err(BackupError::WrongPassword)
    ));
}

#[test]
fn reads_a_final_chunk_of_any_size_and_refuses_a_plaintext_without_metadata() {
    let key = Key::derive(PASSWORD, &header());
    let seal_chunks = |chunks: &[&[u8]]| {
        let mut sealer = Sealer::new(&key, &header());
        let mut file = header().to_bytes().to_vec();
        for (index, chunk) in chunks.iter().enumerate() {
            file.extend_from_slice(&sealer.seal(chunk, index + 1 == chunks.len()).unwrap());
        }
        file
    };

    // A writer follows a full chunk with an empty final one. A reader also
    // takes a final chunk that is full.
    let mut plaintext = plaintext_prefix(METADATA.as_bytes()).unwrap();
    plaintext.extend_from_slice(&database(CHUNK_BYTES - plaintext.len()));
    assert_eq!(plaintext.len(), CHUNK_BYTES);
    let file = seal_chunks(&[&plaintext]);
    assert_eq!(file.len(), HEADER_BYTES + BLOCK_BYTES);
    let (metadata, read) = open_with(&key, &file).unwrap();
    assert_eq!(metadata, METADATA.as_bytes());
    assert_eq!(read.len(), CHUNK_BYTES - 4 - METADATA.len());

    // The same plaintext as a writer cuts it.
    let file = seal_chunks(&[&plaintext, &[]]);
    assert_eq!(file.len() as u64, sealed_len(CHUNK_BYTES as u64));
    assert_eq!(open_with(&key, &file).unwrap().1, read);

    // What decrypts but does not start with metadata: no plaintext at all,
    // a length of zero, a length past the limit, metadata cut short.
    for damaged in [
        &b""[..],
        &[0, 0, 0],
        &[0, 0, 0, 0, b'{', b'}'],
        &[0, 1, 0, 1, b'{', b'}'],
        &[0, 0, 0, 103, b'{', b'}'],
    ] {
        assert!(
            matches!(
                open_with(&key, &seal_chunks(&[damaged])),
                Err(BackupError::Damaged)
            ),
            "{damaged:?}"
        );
    }

    // A sealer refuses what no reader could follow.
    let mut sealer = Sealer::new(&key, &header());
    assert!(sealer.seal(&[1, 2, 3], false).is_err());
    assert!(sealer.seal(&vec![0; CHUNK_BYTES + 1], true).is_err());
    sealer.seal(&[], true).unwrap();
    assert!(sealer.seal(&[], true).is_err());
}
