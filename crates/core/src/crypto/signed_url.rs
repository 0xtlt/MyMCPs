//! Links that carry their own credential: a signature over the path, a
//! purpose and an expiry. Whoever holds the link may use it until then.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::Encryption;
use super::tokens::constant_time_eq;

fn mac(encryption: &Encryption, path: &str, purpose: &str, expires_at_ms: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(encryption.signing_key())
        .expect("HMAC-SHA256 accepts a key of any length");
    mac.update(format!("{purpose}\n{path}\n{expires_at_ms}").as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// The `signature` query parameter for a path of this site. The purpose
/// keeps a signature issued for one thing from opening another.
pub fn sign_path(
    encryption: &Encryption,
    path: &str,
    purpose: &str,
    expires_at: DateTime<Utc>,
) -> String {
    let expires_at_ms = expires_at.timestamp_millis();
    format!(
        "{expires_at_ms}.{}",
        mac(encryption, path, purpose, expires_at_ms)
    )
}

/// Whether `signature` was issued for this path and purpose and has not expired.
pub fn verify_signed_path(
    encryption: &Encryption,
    path: &str,
    purpose: &str,
    signature: &str,
) -> bool {
    let Some((expires_at_ms, signed)) = signature.split_once('.') else {
        return false;
    };
    let Ok(expires_at_ms) = expires_at_ms.parse::<i64>() else {
        return false;
    };
    let expected = mac(encryption, path, purpose, expires_at_ms);
    constant_time_eq(expected.as_bytes(), signed.as_bytes())
        && expires_at_ms > Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::config::TEST_APP_KEY;

    #[test]
    fn a_signature_opens_one_path_for_one_purpose_until_it_expires() {
        let encryption = Encryption::new(TEST_APP_KEY);
        let later = Utc::now() + Duration::minutes(15);
        let signature = sign_path(&encryption, "/files/7/abc", "builtin_file", later);

        assert!(verify_signed_path(
            &encryption,
            "/files/7/abc",
            "builtin_file",
            &signature
        ));
        assert!(!verify_signed_path(
            &encryption,
            "/files/8/abc",
            "builtin_file",
            &signature
        ));
        assert!(!verify_signed_path(
            &encryption,
            "/uploads/7/abc",
            "builtin_file",
            &signature
        ));
        assert!(!verify_signed_path(
            &encryption,
            "/files/7/abc",
            "builtin_upload",
            &signature
        ));
        assert!(!verify_signed_path(
            &Encryption::new("another-key-of-16-chars"),
            "/files/7/abc",
            "builtin_file",
            &signature
        ));

        // The expiry is signed too: pushing it back breaks the signature.
        let (expiry, mac) = signature.split_once('.').unwrap();
        let extended = format!("{}.{mac}", expiry.parse::<i64>().unwrap() + 60_000);
        assert!(!verify_signed_path(
            &encryption,
            "/files/7/abc",
            "builtin_file",
            &extended
        ));

        let expired = sign_path(
            &encryption,
            "/files/7/abc",
            "builtin_file",
            Utc::now() - Duration::seconds(1),
        );
        assert!(!verify_signed_path(
            &encryption,
            "/files/7/abc",
            "builtin_file",
            &expired
        ));
        for malformed in ["", "abc", ".", "123", "x.y"] {
            assert!(!verify_signed_path(
                &encryption,
                "/files/7/abc",
                "builtin_file",
                malformed
            ));
        }
    }
}
