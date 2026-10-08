//! Password hashes in the PHC format of the AdonisJS `scrypt` driver:
//! `$scrypt$n=16384,r=8,p=1$<salt>$<hash>`, salt and hash in base64 without
//! padding, 16 and 64 bytes.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use scrypt::Params;

use super::tokens::constant_time_eq;

const COST: u32 = 16_384;
const BLOCK_SIZE: u32 = 8;
const PARALLELIZATION: u32 = 1;
const SALT_BYTES: usize = 16;
const KEY_BYTES: usize = 64;
/// Node's `maxmem` for the driver: a hash asking for more is never computed.
const MAX_MEMORY_BYTES: u64 = 33_554_432;

fn derive(password: &str, salt: &[u8], n: u32, r: u32, p: u32, length: usize) -> Option<Vec<u8>> {
    if n < 2 || !n.is_power_of_two() || r == 0 || p == 0 {
        return None;
    }
    // The memory scrypt needs, as Node checks it against `maxmem`.
    if 128 * u64::from(n) * u64::from(r) > MAX_MEMORY_BYTES {
        return None;
    }
    let params = Params::new(n.trailing_zeros() as u8, r, p).ok()?;
    let mut output = vec![0u8; length];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut output).ok()?;
    Some(output)
}

/// Hash a password. CPU-bound for tens of milliseconds: call it from
/// `tokio::task::spawn_blocking` in request handlers.
pub fn hash_password(password: &str) -> String {
    let salt: [u8; SALT_BYTES] = rand::random();
    let hash = derive(
        password,
        &salt,
        COST,
        BLOCK_SIZE,
        PARALLELIZATION,
        KEY_BYTES,
    )
    .expect("the configured scrypt parameters are valid");
    format!(
        "$scrypt$n={COST},r={BLOCK_SIZE},p={PARALLELIZATION}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(hash)
    )
}

/// Whether the password matches a stored hash. False for anything that is
/// not a scrypt hash in the expected format. CPU-bound like [`hash_password`].
pub fn verify_password(stored: &str, password: &str) -> bool {
    let Some(phc) = parse_phc(stored) else {
        return false;
    };
    match derive(password, &phc.salt, phc.n, phc.r, phc.p, phc.hash.len()) {
        Some(computed) => constant_time_eq(&computed, &phc.hash),
        None => false,
    }
}

struct Phc {
    n: u32,
    r: u32,
    p: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

fn parse_phc(stored: &str) -> Option<Phc> {
    let mut parts = stored.split('$');
    if !parts.next()?.is_empty() || parts.next()? != "scrypt" {
        return None;
    }
    let (mut n, mut r, mut p) = (None, None, None);
    for param in parts.next()?.split(',') {
        let (name, value) = param.split_once('=')?;
        let value: u32 = value.parse().ok()?;
        match name {
            "n" => n = Some(value),
            "r" => r = Some(value),
            "p" => p = Some(value),
            _ => return None,
        }
    }
    let salt = STANDARD_NO_PAD.decode(parts.next()?).ok()?;
    let hash = STANDARD_NO_PAD.decode(parts.next()?).ok()?;
    if parts.next().is_some()
        || !(8..=1024).contains(&salt.len())
        || !(64..=128).contains(&hash.len())
    {
        return None;
    }
    Some(Phc {
        n: n?,
        r: r?,
        p: p?,
        salt,
        hash,
    })
}
