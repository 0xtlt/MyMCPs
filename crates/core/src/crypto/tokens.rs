use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// SHA-256 in lowercase hex: how access tokens, refresh tokens, authorization
/// codes and client secrets are stored.
pub fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random_bytes(length: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; length];
    rand::rng().fill(bytes.as_mut_slice());
    bytes
}

/// `randomBytes(length).toString('base64url')`.
pub fn random_base64url(length: usize) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(length))
}

/// `randomBytes(length).toString('hex')`.
pub fn random_hex(length: usize) -> String {
    random_bytes(length)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.ct_eq(right).into()
}
