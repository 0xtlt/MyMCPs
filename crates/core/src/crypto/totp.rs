//! Time-based one-time passwords as authenticator apps compute them
//! (RFC 6238 over RFC 4226): HMAC-SHA1, a step of 30 seconds, 6 digits.
//! The apps agree on these values and little else, so none is a setting.

use aws_lc_rs::hmac;
use rand::RngExt;
use url::Url;

/// How long a code is the current one.
pub const STEP_SECONDS: i64 = 30;
/// How many digits a code has.
pub const DIGITS: usize = 6;
/// The size RFC 4226 recommends for a secret: that of a SHA-1 digest.
const SECRET_BYTES: usize = 20;
/// How many steps before and after the current one are accepted: the clock
/// of a phone drifts, and a code takes time to type.
const WINDOW: i64 = 1;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new secret, in the base32 an authenticator app takes it in (RFC 4648,
/// without padding).
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rng().fill(bytes.as_mut_slice());
    encode_base32(&bytes)
}

fn encode_base32(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        text.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    text
}

/// The bytes of a base32 secret. Spaces, dashes, padding and lowercase are
/// what people and apps add to one: they are read through.
fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for character in text.chars() {
        if matches!(character, ' ' | '-' | '=') {
            continue;
        }
        let upper = character.to_ascii_uppercase();
        let value = BASE32
            .iter()
            .position(|letter| char::from(*letter) == upper)?;
        buffer = ((buffer << 5) | value as u32) & 0xffff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// The step the clock is in.
pub fn step_at(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(STEP_SECONDS)
}

/// The code of a step (RFC 4226, section 5.3).
fn code_at(secret: &[u8], step: i64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let digest = hmac::sign(&key, &step.to_be_bytes());
    let digest = digest.as_ref();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:0DIGITS$}", binary % 10u32.pow(DIGITS as u32))
}

/// The code an authenticator app shows for this secret right now.
pub fn current_code(secret: &str, unix_seconds: i64) -> Option<String> {
    Some(code_at(&decode_base32(secret)?, step_at(unix_seconds)))
}

/// The step this code is the code of, when it is one that may still be
/// used: the current step, the one before or the one after, and never a
/// step at or before `last_step`, the last one a code was accepted for.
/// The caller stores the step it gets, so that no code works twice.
pub fn verify(secret: &str, code: &str, unix_seconds: i64, last_step: Option<i64>) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let secret = decode_base32(secret)?;
    let current = step_at(unix_seconds);
    let mut accepted = None;
    // Every step of the window is computed, whichever matches.
    for step in (current - WINDOW)..=(current + WINDOW) {
        let matches = super::constant_time_eq(code_at(&secret, step).as_bytes(), code.as_bytes());
        if matches && last_step.is_none_or(|last| step > last) && accepted.is_none() {
            accepted = Some(step);
        }
    }
    accepted
}

/// What the QR code of a secret says: the `otpauth` address authenticator
/// apps read, with the name of the instance and the account it is for.
pub fn provisioning_uri(issuer: &str, account: &str, secret: &str) -> String {
    let mut uri = Url::parse("otpauth://totp/").expect("a constant address");
    if let Ok(mut segments) = uri.path_segments_mut() {
        segments.pop_if_empty().push(&format!("{issuer}:{account}"));
    }
    uri.query_pairs_mut()
        .append_pair("secret", secret)
        .append_pair("issuer", issuer)
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", &DIGITS.to_string())
        .append_pair("period", &STEP_SECONDS.to_string());
    uri.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secret of the test vectors of RFC 6238, appendix B: the ASCII
    /// text `12345678901234567890`.
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn computes_the_codes_of_rfc_6238() {
        // The RFC lists 8 digits; these are their last 6.
        for (seconds, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(current_code(RFC_SECRET, seconds).as_deref(), Some(code));
        }
    }

    #[test]
    fn encodes_and_reads_base32() {
        assert_eq!(encode_base32(b"12345678901234567890"), RFC_SECRET);
        assert_eq!(encode_base32(b"fo"), "MZXQ");
        assert_eq!(decode_base32("MZXW6YTBOI").as_deref(), Some(&b"foobar"[..]));
        // As an app or a person writes it.
        assert_eq!(
            decode_base32("gezd gnbv-gy3t qojq gezd gnbv gy3t qojq").as_deref(),
            Some(&b"12345678901234567890"[..])
        );
        assert_eq!(decode_base32("MZXW6==="), decode_base32("MZXW6"));
        for invalid in ["", "   ", "MZXW1", "é"] {
            assert_eq!(decode_base32(invalid), None, "{invalid}");
        }

        let secret = generate_secret();
        assert_eq!(secret.len(), 32);
        assert_eq!(decode_base32(&secret).map(|bytes| bytes.len()), Some(20));
        assert_ne!(secret, generate_secret());
    }

    #[test]
    fn accepts_the_current_code_and_its_neighbours_once() {
        let now = 1_111_111_111;
        let step = step_at(now);
        let code = |step: i64| code_at(&decode_base32(RFC_SECRET).unwrap(), step);

        assert_eq!(verify(RFC_SECRET, &code(step), now, None), Some(step));
        assert_eq!(
            verify(RFC_SECRET, &code(step - 1), now, None),
            Some(step - 1)
        );
        assert_eq!(
            verify(RFC_SECRET, &code(step + 1), now, None),
            Some(step + 1)
        );
        assert_eq!(verify(RFC_SECRET, &code(step - 2), now, None), None);
        assert_eq!(verify(RFC_SECRET, &code(step + 2), now, None), None);
        // As it is read aloud or pasted.
        assert_eq!(verify(RFC_SECRET, " 050 471 ", now, None), Some(step));

        // A code is used once, and so is every code before it.
        assert_eq!(verify(RFC_SECRET, &code(step), now, Some(step)), None);
        assert_eq!(verify(RFC_SECRET, &code(step - 1), now, Some(step)), None);
        assert_eq!(
            verify(RFC_SECRET, &code(step + 1), now, Some(step)),
            Some(step + 1)
        );
        assert_eq!(
            verify(RFC_SECRET, &code(step), now, Some(step - 1)),
            Some(step)
        );

        for invalid in ["", "05047", "0504711", "05047a", "０５０４７１"] {
            assert_eq!(verify(RFC_SECRET, invalid, now, None), None, "{invalid}");
        }
        assert_eq!(verify("not base32!", &code(step), now, None), None);
    }

    #[test]
    fn writes_the_address_an_authenticator_app_reads() {
        assert_eq!(
            provisioning_uri("MyMCPs", "ada+test@example.com", "MZXW6YTBOI"),
            "otpauth://totp/MyMCPs:ada+test@example.com?secret=MZXW6YTBOI&issuer=MyMCPs&algorithm=SHA1&digits=6&period=30"
        );
        // A name with a space or a slash stays one path segment.
        let uri = provisioning_uri("My MCPs", "a/b@example.com", "MZXW6YTBOI");
        assert!(
            uri.starts_with("otpauth://totp/My%20MCPs:a%2Fb@example.com?"),
            "{uri}"
        );
    }
}
