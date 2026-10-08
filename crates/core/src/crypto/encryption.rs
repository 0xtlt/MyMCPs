//! The `aes256gcm` driver of `@boringnode/encryption`, as AdonisJS 7
//! configured it for this app (`config/encryption.ts`, driver id `gcm`).
//!
//! A value is `gcm.v1:<ciphertext>.<iv>.<tag>`, each part base64url without
//! padding. The plaintext is the JSON `{"message": ..., "expiryDate"?: ...}`
//! and an optional purpose is bound as additional authenticated data. The key
//! is HKDF-SHA256 of the SHA-256 of `APP_KEY`, taken as the string it is
//! (the `base64:` prefix is not decoded).

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, SecondsFormat, Utc};
use hkdf::Hkdf;
use rand::RngExt;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const DRIVER_ID: &str = "gcm";
const VERSION_PREFIX: &str = "v1:";
const IV_LENGTH: usize = 12;
const TAG_LENGTH: usize = 16;

#[derive(Clone)]
pub struct Encryption {
    /// Key of values written before the driver derived one with HKDF.
    legacy_key: [u8; 32],
    key: [u8; 32],
    /// Signs links and cookies. Separate from the encryption key.
    signing_key: [u8; 32],
}

impl std::fmt::Debug for Encryption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Encryption(..)")
    }
}

fn derive(crypto_key: &[u8; 32], info: &str) -> [u8; 32] {
    let mut derived = [0u8; 32];
    // 32 bytes is always a valid HKDF-SHA256 output length.
    Hkdf::<Sha256>::new(None, crypto_key)
        .expand(info.as_bytes(), &mut derived)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    derived
}

/// base64url without padding, refusing what Node's decoder would silently
/// accept: other alphabets, padding, and non-canonical trailing bits.
fn decode_base64url(encoded: &str) -> Option<Vec<u8>> {
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    (URL_SAFE_NO_PAD.encode(&decoded) == encoded).then_some(decoded)
}

impl Encryption {
    pub fn new(app_key: &str) -> Self {
        let legacy_key: [u8; 32] = Sha256::digest(app_key.as_bytes()).into();
        Self {
            key: derive(&legacy_key, &format!("aes-256-gcm:v1:{DRIVER_ID}")),
            signing_key: derive(&legacy_key, "mymcps:signing:v1"),
            legacy_key,
        }
    }

    /// The key for HMAC signatures (signed links, session cookies).
    pub fn signing_key(&self) -> &[u8; 32] {
        &self.signing_key
    }

    /// Encrypt a string, as `encryption.encrypt(value)` did.
    pub fn encrypt(&self, value: &str) -> String {
        self.encrypt_value(&Value::String(value.to_string()), None, None)
    }

    /// Decrypt a string written by [`Self::encrypt`]. `None` when the value
    /// is not one, was tampered with, or was encrypted with another key.
    pub fn decrypt(&self, value: &str) -> Option<String> {
        match self.decrypt_value(value, None)? {
            Value::String(message) => Some(message),
            _ => None,
        }
    }

    /// Encrypt any JSON value, optionally bound to a purpose and to an expiry.
    pub fn encrypt_value(
        &self,
        value: &Value,
        purpose: Option<&str>,
        expires_at: Option<DateTime<Utc>>,
    ) -> String {
        let mut message = Map::new();
        message.insert("message".into(), value.clone());
        if let Some(expires_at) = expires_at {
            message.insert(
                "expiryDate".into(),
                Value::String(expires_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            );
        }
        let plaintext = Value::Object(message).to_string();

        let mut iv = [0u8; IV_LENGTH];
        rand::rng().fill(&mut iv);
        let cipher = Aes256Gcm::new(&self.key.into());
        let sealed = cipher
            .encrypt(
                &iv.into(),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: purpose.unwrap_or("").as_bytes(),
                },
            )
            .expect("AES-GCM encryption of an in-memory buffer cannot fail");
        let (ciphertext, tag) = sealed.split_at(sealed.len() - TAG_LENGTH);

        format!(
            "{DRIVER_ID}.{VERSION_PREFIX}{}.{}.{}",
            URL_SAFE_NO_PAD.encode(ciphertext),
            URL_SAFE_NO_PAD.encode(iv),
            URL_SAFE_NO_PAD.encode(tag)
        )
    }

    /// Decrypt a value written by [`Self::encrypt_value`] with the same
    /// purpose. `None` once it has expired, like any value that does not decrypt.
    pub fn decrypt_value(&self, value: &str, purpose: Option<&str>) -> Option<Value> {
        let parts: Vec<&str> = value.split('.').collect();
        let [id, cipher_encoded, iv_encoded, tag_encoded] = parts.as_slice() else {
            return None;
        };
        if *id != DRIVER_ID
            || cipher_encoded.is_empty()
            || iv_encoded.is_empty()
            || tag_encoded.is_empty()
        {
            return None;
        }

        let (key, encoded_ciphertext) = match cipher_encoded.strip_prefix(VERSION_PREFIX) {
            Some(current) => (&self.key, current),
            None => (&self.legacy_key, *cipher_encoded),
        };
        let mut sealed = decode_base64url(encoded_ciphertext)?;
        let iv: [u8; IV_LENGTH] = decode_base64url(iv_encoded)?.try_into().ok()?;
        let tag = decode_base64url(tag_encoded).filter(|tag| tag.len() == TAG_LENGTH)?;
        sealed.extend_from_slice(&tag);

        let plaintext = Aes256Gcm::new(key.into())
            .decrypt(
                &iv.into(),
                Payload {
                    msg: &sealed,
                    aad: purpose.unwrap_or("").as_bytes(),
                },
            )
            .ok()?;
        verify_message(&plaintext)
    }
}

/// `MessageBuilder.verify` of `@poppinss/utils`: the message must be there
/// and truthy, carry no purpose of its own, and not be past its expiry.
fn verify_message(plaintext: &[u8]) -> Option<Value> {
    let Value::Object(mut parsed) = serde_json::from_slice(plaintext).ok()? else {
        return None;
    };
    let message = parsed.remove("message")?;
    let falsy = match &message {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(_) | Value::Object(_) => false,
    };
    if falsy
        || parsed
            .get("purpose")
            .is_some_and(|purpose| !purpose.is_null())
    {
        return None;
    }

    match parsed.get("expiryDate") {
        None | Some(Value::Null) => {}
        Some(Value::String(expiry)) if expiry.is_empty() => {}
        Some(Value::String(expiry)) => {
            let expiry = DateTime::parse_from_rfc3339(expiry).ok()?;
            if expiry < Utc::now() {
                return None;
            }
        }
        Some(_) => return None,
    }
    Some(message)
}
