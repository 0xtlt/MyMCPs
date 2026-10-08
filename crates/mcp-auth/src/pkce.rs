//! PKCE (RFC 7636), as the `pkce-challenge` package the SDK uses generates it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// The unreserved characters a code verifier is made of.
const MASK: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

/// The length `pkce-challenge` defaults to, which is the one the SDK asks for.
const VERIFIER_LENGTH: usize = 43;

/// A code verifier and the `S256` challenge derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceChallenge {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generates a PKCE challenge pair.
pub fn pkce_challenge() -> PkceChallenge {
    let code_verifier = random(VERIFIER_LENGTH);
    let code_challenge = generate_challenge(&code_verifier);
    PkceChallenge {
        code_verifier,
        code_challenge,
    }
}

/// The `S256` code challenge of a code verifier: its SHA-256, in base64url
/// without padding.
pub fn generate_challenge(code_verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
}

/// A cryptographically strong random string of `size` characters of the mask.
fn random(size: usize) -> String {
    // Bytes at or past this would make the first characters of the mask more
    // likely than the last ones.
    let even_distribution_cutoff = 256 - 256 % MASK.len();
    let mut result = String::with_capacity(size);
    while result.len() < size {
        let mut bytes = vec![0u8; size - result.len()];
        rand::fill(&mut bytes[..]);
        for byte in bytes {
            if usize::from(byte) < even_distribution_cutoff {
                result.push(char::from(MASK[usize::from(byte) % MASK.len()]));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifier_is_43_unreserved_characters_and_its_challenge_follows() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let pair = pkce_challenge();
            assert_eq!(pair.code_verifier.len(), 43);
            assert!(pair.code_verifier.bytes().all(|byte| MASK.contains(&byte)));
            assert_eq!(pair.code_challenge, generate_challenge(&pair.code_verifier));
            assert_eq!(pair.code_challenge.len(), 43);
            assert!(seen.insert(pair.code_verifier));
        }
    }

    #[test]
    fn computes_the_challenge_of_rfc_7636() {
        // Appendix B of RFC 7636.
        assert_eq!(
            generate_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
