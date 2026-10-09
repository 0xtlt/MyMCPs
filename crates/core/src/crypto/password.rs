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
const MAX_MEMORY_BYTES: u128 = 33_554_432;
/// How much more work than a hash of this app a stored hash may ask for. The
/// hashes of an imported backup are whatever its author wrote, and checking
/// one is what anyone who sends a password to the sign-in form asks for.
const MAX_WORK_FACTOR: u128 = 16;

fn derive(password: &str, salt: &[u8], n: u32, r: u32, p: u32, length: usize) -> Option<Vec<u8>> {
    if n < 2 || !n.is_power_of_two() || r == 0 || p == 0 {
        return None;
    }
    let (cost, block, parallel) = (u128::from(n), u128::from(r), u128::from(p));
    // The memory scrypt needs, as OpenSSL checks it against `maxmem` for
    // Node: the blocks of each of the `p` mixes, and the table of one.
    if 128 * block * parallel + 128 * block * (cost + 2) > MAX_MEMORY_BYTES {
        return None;
    }
    if cost * block * parallel
        > MAX_WORK_FACTOR * u128::from(COST) * u128::from(BLOCK_SIZE) * u128::from(PARALLELIZATION)
    {
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn checks_a_password_against_its_own_hashes() {
        let stored = hash_password("correct horse");
        assert!(stored.starts_with("$scrypt$n=16384,r=8,p=1$"));
        assert!(verify_password(&stored, "correct horse"));
        assert!(!verify_password(&stored, "wrong horse"));
    }

    #[test]
    fn never_computes_a_hash_that_asks_for_more_than_a_sign_in_may_cost() {
        let stored = hash_password("correct horse");
        let (_, salt_and_hash) = stored.split_at("$scrypt$n=16384,r=8,p=1".len());
        let started = Instant::now();
        for parameters in [
            // Sixty-five thousand mixes: an hour of one core.
            "n=16384,r=8,p=65536",
            // A hundred gigabytes of blocks.
            "n=2,r=1,p=1073741823",
            // More memory than Node allowed, which the table alone asks for.
            "n=32768,r=8,p=1",
            // Within the memory, and twenty times the work.
            "n=16384,r=8,p=20",
            "n=4294967295,r=4294967295,p=4294967295",
        ] {
            let stored = format!("$scrypt${parameters}{salt_and_hash}");
            assert!(!verify_password(&stored, "correct horse"), "{parameters}");
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
