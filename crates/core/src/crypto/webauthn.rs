//! What a relying party checks of a passkey (W3C Web Authentication,
//! level 2, sections 7.1 and 7.2): that the browser answered the challenge
//! of this server, on this origin, with the person present and verified,
//! and, when they sign in, that the answer is signed by the key they
//! registered.
//!
//! Attestation is not asked for and not read: which make of authenticator
//! holds the key changes nothing here.

use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents,
    UnparsedPublicKey,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use super::constant_time_eq;

/// The signature algorithms a passkey may use, as COSE numbers them, in the
/// order they are offered: ES256, EdDSA, RS256.
pub const ALGORITHMS: [i64; 3] = [-7, -8, -257];

/// The longest credential id an authenticator may give (section 4).
const MAX_CREDENTIAL_ID: usize = 1023;
/// How deep the CBOR of an authenticator may nest.
const MAX_DEPTH: usize = 8;

const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_BACKUP_ELIGIBLE: u8 = 0x08;
const FLAG_BACKED_UP: u8 = 0x10;
const FLAG_ATTESTED_CREDENTIAL: u8 = 0x40;
const FLAG_EXTENSIONS: u8 = 0x80;

/// Why an answer of a browser is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebauthnError {
    #[error("the answer of the browser cannot be read")]
    Malformed,
    #[error("the answer is not one to this request")]
    Challenge,
    #[error("the answer was made for another address than the one of this instance")]
    Origin,
    #[error("the person was not verified by their device")]
    UserVerification,
    #[error("the key uses an algorithm that is not supported")]
    Algorithm,
    #[error("the signature does not match the passkey")]
    Signature,
}

/// The instance, as a passkey knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    /// Its host name. A passkey only answers under it.
    pub id: String,
    /// Its scheme, host and port, as the browser states them.
    pub origin: String,
}

impl RelyingParty {
    /// The relying party of an instance served at this address.
    pub fn from_url(app_url: &str) -> Option<Self> {
        let url = Url::parse(app_url).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        Some(Self {
            id: url.host_str()?.to_string(),
            origin: url.origin().ascii_serialization(),
        })
    }
}

/// A passkey that was just created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub credential_id: Vec<u8>,
    /// The public key, in the COSE encoding the authenticator gave it in.
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    /// Whether the key is kept by a service that syncs it between devices.
    pub backed_up: bool,
}

/// A passkey that just answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assertion {
    pub sign_count: u32,
    pub backed_up: bool,
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(rename = "crossOrigin", default)]
    cross_origin: bool,
}

/// Sections 7.1 and 7.2, the steps on `clientDataJSON`: what the browser
/// says it did, for which challenge, on which page.
fn check_client_data(
    rp: &RelyingParty,
    kind: &str,
    challenge: &[u8],
    client_data_json: &[u8],
) -> Result<(), WebauthnError> {
    let data: ClientData =
        serde_json::from_slice(client_data_json).map_err(|_| WebauthnError::Malformed)?;
    if data.kind != kind {
        return Err(WebauthnError::Malformed);
    }
    let answered = URL_SAFE_NO_PAD
        .decode(data.challenge.as_bytes())
        .map_err(|_| WebauthnError::Challenge)?;
    if challenge.is_empty() || !constant_time_eq(&answered, challenge) {
        return Err(WebauthnError::Challenge);
    }
    // A page of this origin framed by another site is not this instance.
    if data.origin != rp.origin || data.cross_origin {
        return Err(WebauthnError::Origin);
    }
    Ok(())
}

/// The fixed part of authenticator data (section 6.1), once checked.
struct AuthenticatorData<'a> {
    flags: u8,
    sign_count: u32,
    /// What follows: the attested credential, then the extensions.
    rest: &'a [u8],
}

impl AuthenticatorData<'_> {
    fn backed_up(&self) -> bool {
        self.flags & FLAG_BACKED_UP != 0
    }
}

fn check_authenticator_data<'a>(
    rp: &RelyingParty,
    data: &'a [u8],
) -> Result<AuthenticatorData<'a>, WebauthnError> {
    if data.len() < 37 {
        return Err(WebauthnError::Malformed);
    }
    let (rp_id_hash, rest) = data.split_at(32);
    if !constant_time_eq(rp_id_hash, &Sha256::digest(rp.id.as_bytes())) {
        return Err(WebauthnError::Origin);
    }
    let flags = rest[0];
    // Present, and verified: by a fingerprint, a face or the PIN of the key.
    if flags & FLAG_USER_PRESENT == 0 || flags & FLAG_USER_VERIFIED == 0 {
        return Err(WebauthnError::UserVerification);
    }
    // A key cannot be backed up without being eligible to be.
    if flags & FLAG_BACKED_UP != 0 && flags & FLAG_BACKUP_ELIGIBLE == 0 {
        return Err(WebauthnError::Malformed);
    }
    let sign_count = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]);
    Ok(AuthenticatorData {
        flags,
        sign_count,
        rest: &rest[5..],
    })
}

/// The extensions, when the flags announce some: one CBOR map, and nothing
/// after it.
fn check_extensions(flags: u8, rest: &[u8]) -> Result<(), WebauthnError> {
    let mut reader = Reader::new(rest);
    if flags & FLAG_EXTENSIONS != 0 && !matches!(reader.value(0)?, Cbor::Map(_)) {
        return Err(WebauthnError::Malformed);
    }
    if reader.is_empty() {
        Ok(())
    } else {
        Err(WebauthnError::Malformed)
    }
}

/// Check the answer to `navigator.credentials.create()` (section 7.1).
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: &[u8],
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<Registration, WebauthnError> {
    check_client_data(rp, "webauthn.create", challenge, client_data_json)?;

    let mut reader = Reader::new(attestation_object);
    let Cbor::Map(entries) = reader.value(0)? else {
        return Err(WebauthnError::Malformed);
    };
    if !reader.is_empty() {
        return Err(WebauthnError::Malformed);
    }
    let authenticator_data = entries
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Cbor::Text("authData"), Cbor::Bytes(bytes)) => Some(*bytes),
            _ => None,
        })
        .ok_or(WebauthnError::Malformed)?;

    let data = check_authenticator_data(rp, authenticator_data)?;
    if data.flags & FLAG_ATTESTED_CREDENTIAL == 0 || data.rest.len() < 18 {
        return Err(WebauthnError::Malformed);
    }
    // The AAGUID (16 bytes) says which make of authenticator: not read.
    let id_length = usize::from(u16::from_be_bytes([data.rest[16], data.rest[17]]));
    let after_length = &data.rest[18..];
    if id_length == 0 || id_length > MAX_CREDENTIAL_ID || after_length.len() < id_length {
        return Err(WebauthnError::Malformed);
    }
    let (credential_id, key_and_extensions) = after_length.split_at(id_length);

    let mut reader = Reader::new(key_and_extensions);
    let key = reader.value(0)?;
    let public_key = &key_and_extensions[..reader.position];
    PublicKey::from_cose(&key)?;
    check_extensions(data.flags, &key_and_extensions[reader.position..])?;

    Ok(Registration {
        credential_id: credential_id.to_vec(),
        public_key: public_key.to_vec(),
        sign_count: data.sign_count,
        backed_up: data.backed_up(),
    })
}

/// Check the answer to `navigator.credentials.get()` (section 7.2) against
/// the public key the passkey was registered with.
pub fn verify_assertion(
    rp: &RelyingParty,
    challenge: &[u8],
    public_key: &[u8],
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
) -> Result<Assertion, WebauthnError> {
    check_client_data(rp, "webauthn.get", challenge, client_data_json)?;
    let data = check_authenticator_data(rp, authenticator_data)?;
    if data.flags & FLAG_ATTESTED_CREDENTIAL != 0 {
        return Err(WebauthnError::Malformed);
    }
    check_extensions(data.flags, data.rest)?;

    let mut reader = Reader::new(public_key);
    let key = PublicKey::from_cose(&reader.value(0)?)?;
    let mut signed = authenticator_data.to_vec();
    signed.extend_from_slice(&Sha256::digest(client_data_json));
    key.verify(&signed, signature)?;

    Ok(Assertion {
        sign_count: data.sign_count,
        backed_up: data.backed_up(),
    })
}

/// A public key of one of [`ALGORITHMS`], read from its COSE encoding
/// (RFC 9052, section 7, and RFC 9053).
enum PublicKey<'a> {
    /// ECDSA on P-256 with SHA-256: the point, uncompressed.
    Es256([u8; 65]),
    Ed25519(&'a [u8]),
    /// RSASSA-PKCS1-v1_5 with SHA-256: the modulus and the exponent.
    Rs256 {
        n: &'a [u8],
        e: &'a [u8],
    },
}

impl<'a> PublicKey<'a> {
    fn from_cose(key: &Cbor<'a>) -> Result<Self, WebauthnError> {
        let Cbor::Map(entries) = key else {
            return Err(WebauthnError::Malformed);
        };
        let field = |label: i64| {
            entries
                .iter()
                .find_map(|(key, value)| (key.integer() == Some(label)).then_some(value))
        };
        let integer = |label: i64| field(label).and_then(Cbor::integer);
        let bytes = |label: i64| match field(label) {
            Some(Cbor::Bytes(bytes)) => Ok(*bytes),
            _ => Err(WebauthnError::Malformed),
        };
        // 1 is the key type, 3 the algorithm, -1 the curve; -1, -2 and -3
        // hold the numbers of the key.
        match (integer(1), integer(3)) {
            (Some(2), Some(-7)) if integer(-1) == Some(1) => {
                let (x, y) = (bytes(-2)?, bytes(-3)?);
                if x.len() != 32 || y.len() != 32 {
                    return Err(WebauthnError::Malformed);
                }
                let mut point = [4u8; 65];
                point[1..33].copy_from_slice(x);
                point[33..].copy_from_slice(y);
                Ok(Self::Es256(point))
            }
            (Some(1), Some(-8)) if integer(-1) == Some(6) => {
                let x = bytes(-2)?;
                if x.len() != 32 {
                    return Err(WebauthnError::Malformed);
                }
                Ok(Self::Ed25519(x))
            }
            (Some(3), Some(-257)) => {
                let (n, e) = (bytes(-1)?, bytes(-2)?);
                if !(256..=1024).contains(&n.len()) || e.is_empty() || e.len() > 8 {
                    return Err(WebauthnError::Malformed);
                }
                Ok(Self::Rs256 { n, e })
            }
            (Some(_), Some(_)) => Err(WebauthnError::Algorithm),
            _ => Err(WebauthnError::Malformed),
        }
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), WebauthnError> {
        let verified = match self {
            Self::Es256(point) => {
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(message, signature)
            }
            Self::Ed25519(key) => UnparsedPublicKey::new(&ED25519, key).verify(message, signature),
            Self::Rs256 { n, e } => RsaPublicKeyComponents { n, e }.verify(
                &RSA_PKCS1_2048_8192_SHA256,
                message,
                signature,
            ),
        };
        verified.map_err(|_| WebauthnError::Signature)
    }
}

/// A CBOR value (RFC 8949), as far as an authenticator writes one: every
/// length is stated up front.
#[derive(Debug, Clone, PartialEq)]
enum Cbor<'a> {
    Unsigned(u64),
    /// The number `-1 - n`.
    Negative(u64),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Cbor<'a>>),
    Map(Vec<(Cbor<'a>, Cbor<'a>)>),
    /// A boolean, null, a float: read past, never looked at.
    Other,
}

impl Cbor<'_> {
    fn integer(&self) -> Option<i64> {
        match self {
            Self::Unsigned(value) => i64::try_from(*value).ok(),
            Self::Negative(value) => i64::try_from(*value).ok().map(|value| -1 - value),
            _ => None,
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], WebauthnError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(WebauthnError::Malformed)?;
        let taken = &self.bytes[self.position..end];
        self.position = end;
        Ok(taken)
    }

    /// The major type of the next item and its argument.
    fn head(&mut self) -> Result<(u8, u8, u64), WebauthnError> {
        let first = self.take(1)?[0];
        let (major, info) = (first >> 5, first & 0x1f);
        let argument = match info {
            0..=23 => u64::from(info),
            24..=27 => {
                let bytes = self.take(1 << (info - 24))?;
                bytes
                    .iter()
                    .fold(0u64, |value, byte| (value << 8) | u64::from(*byte))
            }
            // Reserved values, and lengths that are not stated up front.
            _ => return Err(WebauthnError::Malformed),
        };
        Ok((major, info, argument))
    }

    /// How many items a container announces. Each takes a byte at least:
    /// one that announces more than is left is refused before anything is
    /// allocated for it.
    fn count(&self, announced: u64) -> Result<usize, WebauthnError> {
        usize::try_from(announced)
            .ok()
            .filter(|count| *count <= self.bytes.len() - self.position)
            .ok_or(WebauthnError::Malformed)
    }

    fn value(&mut self, depth: usize) -> Result<Cbor<'a>, WebauthnError> {
        if depth > MAX_DEPTH {
            return Err(WebauthnError::Malformed);
        }
        let (major, info, argument) = self.head()?;
        Ok(match major {
            0 => Cbor::Unsigned(argument),
            1 => Cbor::Negative(argument),
            2 => Cbor::Bytes(self.take(self.count(argument)?)?),
            3 => {
                let bytes = self.take(self.count(argument)?)?;
                Cbor::Text(std::str::from_utf8(bytes).map_err(|_| WebauthnError::Malformed)?)
            }
            4 => {
                let count = self.count(argument)?;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.value(depth + 1)?);
                }
                Cbor::Array(items)
            }
            5 => {
                let count = self.count(argument)?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    entries.push((self.value(depth + 1)?, self.value(depth + 1)?));
                }
                Cbor::Map(entries)
            }
            // A tag: the value it is put on.
            6 => self.value(depth + 1)?,
            // A simple value in the byte that follows must not repeat one
            // that fits in the first (RFC 8949, section 3.3).
            _ if info == 24 && argument < 32 => return Err(WebauthnError::Malformed),
            _ => Cbor::Other,
        })
    }
}

/// An authenticator in software, and the encodings it answers in: for the
/// tests of what registers a passkey and of what signs in with one.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::{Digest, Sha256};

    use super::{FLAG_ATTESTED_CREDENTIAL, FLAG_USER_PRESENT, FLAG_USER_VERIFIED, RelyingParty};

    pub(super) fn head(major: u8, value: u64) -> Vec<u8> {
        let major = major << 5;
        match value {
            0..=23 => vec![major | value as u8],
            24..=0xff => vec![major | 24, value as u8],
            0x100..=0xffff => [vec![major | 25], (value as u16).to_be_bytes().to_vec()].concat(),
            _ => [vec![major | 26], (value as u32).to_be_bytes().to_vec()].concat(),
        }
    }

    pub(super) fn integer(value: i64) -> Vec<u8> {
        if value >= 0 {
            head(0, value as u64)
        } else {
            head(1, (-1 - value) as u64)
        }
    }

    pub(super) fn bytes(value: &[u8]) -> Vec<u8> {
        [head(2, value.len() as u64), value.to_vec()].concat()
    }

    pub(super) fn text(value: &str) -> Vec<u8> {
        [head(3, value.len() as u64), value.as_bytes().to_vec()].concat()
    }

    pub(super) fn map(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut encoded = head(5, entries.len() as u64);
        for (key, value) in entries {
            encoded.extend_from_slice(key);
            encoded.extend_from_slice(value);
        }
        encoded
    }

    pub(super) fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
        serde_json::json!({
            "type": kind,
            "challenge": URL_SAFE_NO_PAD.encode(challenge),
            "origin": origin,
            "crossOrigin": false,
        })
        .to_string()
        .into_bytes()
    }

    pub(super) fn authenticator_data(
        rp_id: &str,
        flags: u8,
        sign_count: u32,
        rest: &[u8],
    ) -> Vec<u8> {
        let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
        data.push(flags);
        data.extend_from_slice(&sign_count.to_be_bytes());
        data.extend_from_slice(rest);
        data
    }

    pub(super) const REGISTERED: u8 =
        FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_ATTESTED_CREDENTIAL;
    pub(super) const ANSWERED: u8 = FLAG_USER_PRESENT | FLAG_USER_VERIFIED;

    pub(super) fn attested(credential_id: &[u8], cose_key: &[u8]) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        data.extend_from_slice(credential_id);
        data.extend_from_slice(cose_key);
        data
    }

    pub(super) fn attestation(authenticator_data: &[u8]) -> Vec<u8> {
        map(&[
            (text("fmt"), text("none")),
            (text("attStmt"), map(&[])),
            (text("authData"), bytes(authenticator_data)),
        ])
    }

    /// What `navigator.credentials.create()` answers.
    pub struct Created {
        pub client_data_json: Vec<u8>,
        pub attestation_object: Vec<u8>,
    }

    /// What `navigator.credentials.get()` answers.
    pub struct Answered {
        pub client_data_json: Vec<u8>,
        pub authenticator_data: Vec<u8>,
        pub signature: Vec<u8>,
    }

    /// An authenticator that holds one ES256 passkey, and verifies whoever
    /// asks.
    pub struct SoftwareAuthenticator {
        key: EcdsaKeyPair,
        pub credential_id: Vec<u8>,
        /// How many times it signed. It only says so when `counts`.
        pub sign_count: u32,
        pub counts: bool,
    }

    impl SoftwareAuthenticator {
        pub fn new(credential_id: &[u8]) -> Self {
            Self {
                key: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).expect("a new key"),
                credential_id: credential_id.to_vec(),
                sign_count: 0,
                counts: false,
            }
        }

        /// The id of the passkey as a browser sends it.
        pub fn id(&self) -> String {
            URL_SAFE_NO_PAD.encode(&self.credential_id)
        }

        fn cose_key(&self) -> Vec<u8> {
            let point = self.key.public_key().as_ref();
            map(&[
                (integer(1), integer(2)),
                (integer(3), integer(-7)),
                (integer(-1), integer(1)),
                (integer(-2), bytes(&point[1..33])),
                (integer(-3), bytes(&point[33..])),
            ])
        }

        /// Make the passkey for this relying party, on a page of `origin`.
        pub fn create(&self, rp: &RelyingParty, origin: &str, challenge: &[u8]) -> Created {
            let data = authenticator_data(
                &rp.id,
                REGISTERED,
                0,
                &attested(&self.credential_id, &self.cose_key()),
            );
            Created {
                client_data_json: client_data("webauthn.create", challenge, origin),
                attestation_object: attestation(&data),
            }
        }

        /// Sign in to this relying party, on a page of `origin`.
        pub fn get(&mut self, rp: &RelyingParty, origin: &str, challenge: &[u8]) -> Answered {
            self.sign_count += 1;
            let counted = if self.counts { self.sign_count } else { 0 };
            let client_data_json = client_data("webauthn.get", challenge, origin);
            let authenticator_data = authenticator_data(&rp.id, ANSWERED, counted, &[]);
            let mut signed = authenticator_data.clone();
            signed.extend_from_slice(&Sha256::digest(&client_data_json));
            let signature = self
                .key
                .sign(&SystemRandom::new(), &signed)
                .expect("a signature")
                .as_ref()
                .to_vec();
            Answered {
                client_data_json,
                authenticator_data,
                signature,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::rsa::KeySize;
    use aws_lc_rs::signature::{
        ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair, RSA_PKCS1_SHA256,
        RsaKeyPair,
    };

    use super::testing::*;
    use super::*;

    const CHALLENGE: &[u8] = b"a challenge of this server";

    fn rp() -> RelyingParty {
        RelyingParty::from_url("https://mcp.example.com").unwrap()
    }

    /// The modulus and the exponent of an `RSAPublicKey` in DER (RFC 8017,
    /// A.1.1), without the zero byte that keeps them positive.
    fn rsa_numbers(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
        /// The content of the element at the start, and what follows it.
        fn element(der: &[u8]) -> (&[u8], &[u8]) {
            let (length, header) = match der[1] {
                short if short < 0x80 => (usize::from(short), 2),
                0x81 => (usize::from(der[2]), 3),
                _ => (usize::from(u16::from_be_bytes([der[2], der[3]])), 4),
            };
            der[header..].split_at(length)
        }
        let (sequence, _) = element(der);
        let (n, rest) = element(sequence);
        let (e, _) = element(rest);
        let unsigned = |number: &[u8]| number.iter().copied().skip_while(|b| *b == 0).collect();
        (unsigned(n), unsigned(e))
    }

    /// An authenticator in software: one key, and what it signs with it.
    enum Authenticator {
        Es256(EcdsaKeyPair),
        Ed25519(Ed25519KeyPair),
        Rs256(RsaKeyPair),
    }

    impl Authenticator {
        fn es256() -> Self {
            Self::Es256(EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap())
        }

        fn cose_key(&self) -> Vec<u8> {
            match self {
                Self::Es256(pair) => {
                    let point = pair.public_key().as_ref();
                    map(&[
                        (integer(1), integer(2)),
                        (integer(3), integer(-7)),
                        (integer(-1), integer(1)),
                        (integer(-2), bytes(&point[1..33])),
                        (integer(-3), bytes(&point[33..])),
                    ])
                }
                Self::Ed25519(pair) => map(&[
                    (integer(1), integer(1)),
                    (integer(3), integer(-8)),
                    (integer(-1), integer(6)),
                    (integer(-2), bytes(pair.public_key().as_ref())),
                ]),
                Self::Rs256(pair) => {
                    let (n, e) = rsa_numbers(pair.public_key().as_ref());
                    map(&[
                        (integer(1), integer(3)),
                        (integer(3), integer(-257)),
                        (integer(-1), bytes(&n)),
                        (integer(-2), bytes(&e)),
                    ])
                }
            }
        }

        fn sign(&self, message: &[u8]) -> Vec<u8> {
            let random = SystemRandom::new();
            match self {
                Self::Es256(pair) => pair.sign(&random, message).unwrap().as_ref().to_vec(),
                Self::Ed25519(pair) => pair.sign(message).as_ref().to_vec(),
                Self::Rs256(pair) => {
                    let mut signature = vec![0; pair.public_modulus_len()];
                    pair.sign(&RSA_PKCS1_SHA256, &random, message, &mut signature)
                        .unwrap();
                    signature
                }
            }
        }
    }

    fn register(authenticator: &Authenticator, flags: u8) -> Result<Registration, WebauthnError> {
        let data = authenticator_data(
            "mcp.example.com",
            flags,
            0,
            &attested(b"credential-1", &authenticator.cose_key()),
        );
        verify_registration(
            &rp(),
            CHALLENGE,
            &client_data("webauthn.create", CHALLENGE, "https://mcp.example.com"),
            &attestation(&data),
        )
    }

    /// What the authenticator answers when asked to sign in.
    fn answer(authenticator: &Authenticator, sign_count: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let client = client_data("webauthn.get", CHALLENGE, "https://mcp.example.com");
        let data = authenticator_data("mcp.example.com", ANSWERED, sign_count, &[]);
        let mut signed = data.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        let signature = authenticator.sign(&signed);
        (client, data, signature)
    }

    #[test]
    fn names_the_relying_party_of_an_address() {
        assert_eq!(
            RelyingParty::from_url("https://mcp.example.com/"),
            Some(RelyingParty {
                id: "mcp.example.com".into(),
                origin: "https://mcp.example.com".into()
            })
        );
        assert_eq!(
            RelyingParty::from_url("http://localhost:3333"),
            Some(RelyingParty {
                id: "localhost".into(),
                origin: "http://localhost:3333".into()
            })
        );
        // The default port of a scheme is not part of an origin.
        assert_eq!(
            RelyingParty::from_url("https://mcp.example.com:443/app").map(|rp| rp.origin),
            Some("https://mcp.example.com".into())
        );
        for invalid in [
            "",
            "mcp.example.com",
            "ftp://mcp.example.com",
            "data:text/plain,x",
        ] {
            assert_eq!(RelyingParty::from_url(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn registers_a_passkey_and_signs_in_with_it_for_each_algorithm() {
        let authenticators = [
            Authenticator::es256(),
            Authenticator::Ed25519(Ed25519KeyPair::generate().unwrap()),
            Authenticator::Rs256(RsaKeyPair::generate(KeySize::Rsa2048).unwrap()),
        ];
        for authenticator in &authenticators {
            let registered = register(
                authenticator,
                REGISTERED | FLAG_BACKUP_ELIGIBLE | FLAG_BACKED_UP,
            )
            .unwrap();
            assert_eq!(registered.credential_id, b"credential-1");
            assert_eq!(registered.public_key, authenticator.cose_key());
            assert_eq!(registered.sign_count, 0);
            assert!(registered.backed_up);

            let (client, data, signature) = answer(authenticator, 7);
            let assertion = verify_assertion(
                &rp(),
                CHALLENGE,
                &registered.public_key,
                &client,
                &data,
                &signature,
            );
            assert_eq!(
                assertion,
                Ok(Assertion {
                    sign_count: 7,
                    backed_up: false
                })
            );

            // The key of another authenticator did not sign this.
            let other = Authenticator::es256().cose_key();
            assert_eq!(
                verify_assertion(&rp(), CHALLENGE, &other, &client, &data, &signature),
                Err(WebauthnError::Signature)
            );
        }
    }

    #[test]
    fn refuses_an_answer_that_was_changed_after_it_was_signed() {
        let authenticator = Authenticator::es256();
        let key = authenticator.cose_key();
        let (client, data, signature) = answer(&authenticator, 1);
        let verify = |client: &[u8], data: &[u8], signature: &[u8]| {
            verify_assertion(&rp(), CHALLENGE, &key, client, data, signature)
        };
        assert!(verify(&client, &data, &signature).is_ok());

        let mut counted = data.clone();
        counted[36] ^= 1;
        assert_eq!(
            verify(&client, &counted, &signature),
            Err(WebauthnError::Signature)
        );
        let mut forged = signature.clone();
        let last = forged.len() - 1;
        forged[last] ^= 1;
        assert_eq!(
            verify(&client, &data, &forged),
            Err(WebauthnError::Signature)
        );
        assert_eq!(verify(&client, &data, &[]), Err(WebauthnError::Signature));
        // The same fields in another order are another text.
        let reordered = format!(
            "{{\"origin\":\"https://mcp.example.com\",\"type\":\"webauthn.get\",\"challenge\":\"{}\"}}",
            URL_SAFE_NO_PAD.encode(CHALLENGE)
        );
        assert_eq!(
            verify(reordered.as_bytes(), &data, &signature),
            Err(WebauthnError::Signature)
        );
    }

    #[test]
    fn refuses_an_answer_to_another_request() {
        let authenticator = Authenticator::es256();
        let key = authenticator.cose_key();
        let sign = |client: &[u8], data: &[u8]| {
            let mut signed = data.to_vec();
            signed.extend_from_slice(&Sha256::digest(client));
            authenticator.sign(&signed)
        };
        let data = authenticator_data("mcp.example.com", ANSWERED, 1, &[]);
        let check = |client: &[u8], data: &[u8]| {
            verify_assertion(&rp(), CHALLENGE, &key, client, data, &sign(client, data))
        };
        let origin = "https://mcp.example.com";

        for (client, error) in [
            (
                client_data("webauthn.get", b"another challenge", origin),
                WebauthnError::Challenge,
            ),
            (
                client_data("webauthn.get", b"", origin),
                WebauthnError::Challenge,
            ),
            (
                client_data("webauthn.create", CHALLENGE, origin),
                WebauthnError::Malformed,
            ),
            (
                client_data("webauthn.get", CHALLENGE, "https://evil.example.com"),
                WebauthnError::Origin,
            ),
            (
                client_data("webauthn.get", CHALLENGE, "http://mcp.example.com"),
                WebauthnError::Origin,
            ),
            (
                client_data("webauthn.get", CHALLENGE, "https://mcp.example.com:8443"),
                WebauthnError::Origin,
            ),
            (b"not json".to_vec(), WebauthnError::Malformed),
        ] {
            assert_eq!(check(&client, &data), Err(error));
        }
        let framed = serde_json::json!({
            "type": "webauthn.get",
            "challenge": URL_SAFE_NO_PAD.encode(CHALLENGE),
            "origin": origin,
            "crossOrigin": true,
        })
        .to_string();
        assert_eq!(check(framed.as_bytes(), &data), Err(WebauthnError::Origin));
        // An empty challenge is never the one of a request.
        let empty = client_data("webauthn.get", b"", origin);
        assert_eq!(
            verify_assertion(&rp(), b"", &key, &empty, &data, &sign(&empty, &data)),
            Err(WebauthnError::Challenge)
        );

        let client = client_data("webauthn.get", CHALLENGE, origin);
        for (data, error) in [
            (
                authenticator_data("evil.example.com", ANSWERED, 1, &[]),
                WebauthnError::Origin,
            ),
            (
                authenticator_data("mcp.example.com", FLAG_USER_PRESENT, 1, &[]),
                WebauthnError::UserVerification,
            ),
            (
                authenticator_data("mcp.example.com", FLAG_USER_VERIFIED, 1, &[]),
                WebauthnError::UserVerification,
            ),
            (
                authenticator_data("mcp.example.com", ANSWERED | FLAG_BACKED_UP, 1, &[]),
                WebauthnError::Malformed,
            ),
            (
                authenticator_data("mcp.example.com", ANSWERED, 1, b"trailing"),
                WebauthnError::Malformed,
            ),
            (
                authenticator_data("mcp.example.com", ANSWERED | FLAG_EXTENSIONS, 1, &[]),
                WebauthnError::Malformed,
            ),
            (vec![0; 36], WebauthnError::Malformed),
        ] {
            assert_eq!(check(&client, &data), Err(error));
        }
        // Extensions are read past.
        let extended = authenticator_data(
            "mcp.example.com",
            ANSWERED | FLAG_EXTENSIONS,
            1,
            &map(&[(text("credProps"), map(&[(text("rk"), vec![0xf5])]))]),
        );
        assert!(check(&client, &extended).is_ok());
    }

    #[test]
    fn refuses_a_registration_that_is_not_one_of_this_instance() {
        let authenticator = Authenticator::es256();
        assert!(register(&authenticator, REGISTERED).is_ok());
        assert_eq!(
            register(&authenticator, FLAG_USER_PRESENT | FLAG_ATTESTED_CREDENTIAL),
            Err(WebauthnError::UserVerification)
        );
        assert_eq!(
            register(&authenticator, ANSWERED),
            Err(WebauthnError::Malformed)
        );

        let key = authenticator.cose_key();
        let create = client_data("webauthn.create", CHALLENGE, "https://mcp.example.com");
        let with = |client: &[u8], attestation: &[u8]| {
            verify_registration(&rp(), CHALLENGE, client, attestation)
        };
        let data = |rest: &[u8]| authenticator_data("mcp.example.com", REGISTERED, 0, rest);

        assert_eq!(
            with(
                &client_data("webauthn.get", CHALLENGE, "https://mcp.example.com"),
                &attestation(&data(&attested(b"id", &key)))
            ),
            Err(WebauthnError::Malformed)
        );
        for (attestation, error) in [
            (
                attestation(&data(&attested(b"", &key))),
                WebauthnError::Malformed,
            ),
            (
                attestation(&data(&attested(&[7; MAX_CREDENTIAL_ID + 1], &key))),
                WebauthnError::Malformed,
            ),
            (
                attestation(&data(&[attested(b"id", &key), b"more".to_vec()].concat())),
                WebauthnError::Malformed,
            ),
            // The id says it is longer than what follows.
            (
                attestation(&data(&[vec![0; 16], vec![0xff, 0xff]].concat())),
                WebauthnError::Malformed,
            ),
            (
                map(&[(text("fmt"), text("none"))]),
                WebauthnError::Malformed,
            ),
            (
                [attestation(&data(&attested(b"id", &key))), vec![0]].concat(),
                WebauthnError::Malformed,
            ),
            (b"not cbor".to_vec(), WebauthnError::Malformed),
            (Vec::new(), WebauthnError::Malformed),
        ] {
            assert_eq!(with(&create, &attestation), Err(error));
        }
        // The longest id an authenticator may give.
        let longest = attestation(&data(&attested(&[7; MAX_CREDENTIAL_ID], &key)));
        assert_eq!(
            with(&create, &longest).map(|registered| registered.credential_id.len()),
            Ok(MAX_CREDENTIAL_ID)
        );
        // A statement about the authenticator is read past, whatever it holds.
        let stated = map(&[
            (text("fmt"), text("packed")),
            (
                text("attStmt"),
                map(&[
                    (text("alg"), integer(-7)),
                    (text("sig"), bytes(&[1, 2, 3])),
                    (text("x5c"), [head(4, 1), bytes(&[4; 300])].concat()),
                ]),
            ),
            (text("authData"), bytes(&data(&attested(b"id", &key)))),
        ]);
        assert!(with(&create, &stated).is_ok());
    }

    #[test]
    fn refuses_a_key_it_cannot_check_a_signature_with() {
        let point = [9u8; 32];
        let es256 = |alg: i64, curve: i64, x: &[u8]| {
            map(&[
                (integer(1), integer(2)),
                (integer(3), integer(alg)),
                (integer(-1), integer(curve)),
                (integer(-2), bytes(x)),
                (integer(-3), bytes(&point)),
            ])
        };
        let read = |key: &[u8]| {
            let mut reader = Reader::new(key);
            reader
                .value(0)
                .and_then(|key| PublicKey::from_cose(&key).map(|_| ()))
        };
        assert_eq!(read(&es256(-7, 1, &point)), Ok(()));
        // ES384, and ES256 on another curve.
        assert_eq!(read(&es256(-35, 2, &point)), Err(WebauthnError::Algorithm));
        assert_eq!(read(&es256(-7, 2, &point)), Err(WebauthnError::Algorithm));
        assert_eq!(
            read(&es256(-7, 1, &point[..31])),
            Err(WebauthnError::Malformed)
        );
        assert_eq!(
            read(&map(&[
                (integer(1), integer(3)),
                (integer(3), integer(-257))
            ])),
            Err(WebauthnError::Malformed)
        );
        // A modulus of 1024 bits.
        assert_eq!(
            read(&map(&[
                (integer(1), integer(3)),
                (integer(3), integer(-257)),
                (integer(-1), bytes(&[0xff; 128])),
                (integer(-2), bytes(&[1, 0, 1])),
            ])),
            Err(WebauthnError::Malformed)
        );
        assert_eq!(read(&map(&[])), Err(WebauthnError::Malformed));
        assert_eq!(read(&integer(1)), Err(WebauthnError::Malformed));
    }

    #[test]
    fn reads_the_cbor_an_authenticator_writes_and_no_other() {
        fn read(bytes: &[u8]) -> Result<Cbor<'_>, WebauthnError> {
            Reader::new(bytes).value(0)
        }
        assert_eq!(read(&integer(23)), Ok(Cbor::Unsigned(23)));
        assert_eq!(
            read(&integer(-257)).map(|value| value.integer()),
            Ok(Some(-257))
        );
        assert_eq!(
            read(&integer(70_000)).map(|value| value.integer()),
            Ok(Some(70_000))
        );
        assert_eq!(read(&text("fmt")), Ok(Cbor::Text("fmt")));
        assert_eq!(
            read(&[head(4, 2), integer(1), bytes(b"x")].concat()),
            Ok(Cbor::Array(vec![Cbor::Unsigned(1), Cbor::Bytes(b"x")]))
        );
        // A tag, a boolean, a float.
        assert_eq!(
            read(&[vec![0xc1], integer(5)].concat()),
            Ok(Cbor::Unsigned(5))
        );
        assert_eq!(read(&[0xf5]), Ok(Cbor::Other));
        assert_eq!(read(&[0xfb, 0, 0, 0, 0, 0, 0, 0, 0]), Ok(Cbor::Other));

        for refused in [
            &[][..],
            // Lengths that are not stated up front, and their end mark.
            &[0x5f],
            &[0x9f],
            &[0xbf],
            &[0xff],
            // A reserved value.
            &[0x1c],
            // A length longer than what is left.
            &[0x58, 200, 1],
            &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            &[0xa2, 1, 2],
            // Text that is not UTF-8.
            &[0x61, 0xff],
            &[0xf8, 0x10],
        ] {
            assert_eq!(read(refused), Err(WebauthnError::Malformed), "{refused:x?}");
        }
        let nested = [vec![0x81; MAX_DEPTH + 1], integer(1)].concat();
        assert_eq!(read(&nested), Err(WebauthnError::Malformed));
        let nested = [vec![0x81; MAX_DEPTH], integer(1)].concat();
        assert!(read(&nested).is_ok());
    }
}
