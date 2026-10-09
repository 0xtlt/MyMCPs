//! Two-step verification: the authenticator app (TOTP, RFC 6238), the
//! recovery codes, and what an account has turned on.
//!
//! An account is protected as soon as it has a passkey or a confirmed
//! authenticator app: a password alone then no longer opens a session. The
//! recovery codes exist while it is protected, and go with the last factor.

use rand::RngExt;
use totp_rs::{Builder, Secret, Totp};

use crate::crypto::sha256_hex;
use crate::db::Db;
use crate::models::{User, UserRecoveryCode, UserTotpSecret};

/// The name authenticator apps show above the code.
pub const ISSUER: &str = "MyMCPs";

/// How many recovery codes a user receives at a time.
pub const RECOVERY_CODE_COUNT: usize = 10;

/// 32 symbols, so that a random byte masked to five bits picks one without
/// bias. No `i`, `l`, `o` or `u`: they read as `1`, `1`, `0` and `v`.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Sixteen symbols of five bits: 80 bits of entropy per code.
const RECOVERY_CODE_LENGTH: usize = 16;

/// The secret of an authenticator app, with the standard settings every app
/// understands: SHA-1, six digits, thirty seconds, and one step of clock
/// drift either way.
pub struct TotpKey {
    totp: Totp,
}

impl TotpKey {
    /// A new random 160-bit secret, the size RFC 4226 recommends.
    pub fn generate(account: &str) -> Self {
        let mut bytes = [0u8; 20];
        rand::rng().fill(&mut bytes);
        Self::from_bytes(bytes.to_vec(), account).expect("a 160-bit secret is valid")
    }

    /// The key of a stored base32 secret. `None` when it does not decode.
    pub fn from_base32(secret: &str, account: &str) -> Option<Self> {
        let bytes = Secret::try_from_base32(secret).ok()?.to_vec();
        Self::from_bytes(bytes, account)
    }

    fn from_bytes(bytes: Vec<u8>, account: &str) -> Option<Self> {
        // The otpauth label separates the issuer from the account with `:`.
        let account = account.replace(':', " ");
        Builder::new()
            .with_secret(bytes)
            .with_account_name(account)
            .with_issuer(Some(ISSUER))
            .build()
            .ok()
            .map(|totp| Self { totp })
    }

    /// The secret as the user types it into an app that cannot scan.
    pub fn base32(&self) -> String {
        self.totp.secret().to_base32()
    }

    /// The `otpauth://` URL the QR code holds.
    pub fn otpauth_url(&self) -> String {
        self.totp
            .to_url()
            .expect("the account name has no colon and is set")
    }

    /// The time step `code` is valid for at `unix_seconds`, if any. The
    /// caller must still refuse a step already used.
    pub fn matching_step(&self, code: &str, unix_seconds: u64) -> Option<i64> {
        let code = normalize_totp_code(code)?;
        self.totp
            .check(&code, unix_seconds)
            .and_then(|step| i64::try_from(step).ok())
    }

    /// The code the app shows at `unix_seconds`.
    pub fn code_at(&self, unix_seconds: u64) -> String {
        self.totp.generate(unix_seconds).to_string()
    }
}

/// The six digits of a code typed as `123456`, `123 456` or `123-456`.
pub fn normalize_totp_code(input: &str) -> Option<String> {
    let digits: String = input
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect();
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(digits)
}

/// The seconds since the epoch, the clock TOTP counts with.
pub fn unix_now() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

/// A fresh set of recovery codes, formatted `xxxx-xxxx-xxxx-xxxx`.
pub fn generate_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let mut bytes = [0u8; RECOVERY_CODE_LENGTH];
            rand::rng().fill(&mut bytes);
            let symbols: Vec<char> = bytes
                .iter()
                .map(|byte| char::from(RECOVERY_ALPHABET[usize::from(byte & 31)]))
                .collect();
            symbols
                .chunks(4)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect()
}

/// A recovery code as typed, reduced to the form that is hashed: lowercase,
/// without separators, and with the look-alike letters read as digits.
pub fn normalize_recovery_code(input: &str) -> String {
    input
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .map(|character| match character.to_ascii_lowercase() {
            'o' => '0',
            'i' | 'l' => '1',
            other => other,
        })
        .collect()
}

/// What is stored for a recovery code.
pub fn hash_recovery_code(input: &str) -> String {
    sha256_hex(&normalize_recovery_code(input))
}

/// The second steps an account has turned on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TwoFactorStatus {
    pub passkeys: i64,
    /// Whether a confirmed authenticator app is set up.
    pub totp: bool,
    /// Unused recovery codes.
    pub recovery_codes: i64,
}

impl TwoFactorStatus {
    pub async fn of(db: &Db, user_id: i64) -> Result<Self, sqlx::Error> {
        let passkeys: i64 =
            sqlx::query_scalar("select count(*) from `user_passkeys` where `user_id` = ?")
                .bind(user_id)
                .fetch_one(&**db)
                .await?;
        let totp = UserTotpSecret::for_user(&**db, user_id)
            .await?
            .is_some_and(|secret| secret.is_confirmed());
        let recovery_codes = UserRecoveryCode::remaining(&**db, user_id).await?;
        Ok(Self {
            passkeys,
            totp,
            recovery_codes,
        })
    }

    /// Whether a password alone no longer opens a session.
    pub fn is_enabled(&self) -> bool {
        self.passkeys > 0 || self.totp
    }
}

/// What [`reset_two_factor`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResetSummary {
    pub passkeys: u64,
    pub totp: bool,
    pub recovery_codes: u64,
}

/// Remove every passkey, authenticator app and recovery code of the user,
/// and sign the account out everywhere: the way back in for a user locked
/// out of their account. One transaction.
pub async fn reset_two_factor(db: &Db, user: &mut User) -> Result<ResetSummary, sqlx::Error> {
    let mut transaction = db.begin().await?;
    let passkeys = sqlx::query("delete from `user_passkeys` where `user_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    let totp = sqlx::query("delete from `user_totp_secrets` where `user_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
        > 0;
    let recovery_codes = sqlx::query("delete from `user_recovery_codes` where `user_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    user.invalidate_sessions(&mut *transaction).await?;
    sqlx::query("delete from `remember_me_tokens` where `tokenable_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ResetSummary {
        passkeys,
        totp,
        recovery_codes,
    })
}

/// Delete the recovery codes when the account has no factor left to
/// recover. Call it in the transaction that removed a factor.
pub async fn forget_recovery_codes_if_unprotected(
    connection: &mut sqlx::SqliteConnection,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    let factors: i64 = sqlx::query_scalar(
        "select (select count(*) from `user_passkeys` where `user_id` = ?) \
         + (select count(*) from `user_totp_secrets` where `user_id` = ? and `confirmed_at` is not null)",
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_one(&mut *connection)
    .await?;
    if factors == 0 {
        UserRecoveryCode::delete_all(&mut *connection, user_id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_round_trips_through_base32() {
        let key = TotpKey::generate("ada@example.com");
        let again = TotpKey::from_base32(&key.base32(), "ada@example.com").unwrap();
        assert_eq!(key.code_at(1_000_000), again.code_at(1_000_000));
        assert_eq!(key.base32().len(), 32);
    }

    #[test]
    fn the_rfc_6238_vector_matches() {
        // RFC 6238, appendix B: the SHA-1 secret is "12345678901234567890".
        let key = TotpKey::from_bytes(b"12345678901234567890".to_vec(), "test").unwrap();
        assert_eq!(key.code_at(59), "287082");
        assert_eq!(key.code_at(1_111_111_109), "081804");
        assert_eq!(key.matching_step("287082", 59), Some(1));
    }

    #[test]
    fn a_code_is_valid_one_step_either_way() {
        let key = TotpKey::generate("ada@example.com");
        let now = 1_700_000_000;
        let code = key.code_at(now);
        let step = i64::try_from(now / 30).unwrap();
        assert_eq!(key.matching_step(&code, now), Some(step));
        assert_eq!(key.matching_step(&code, now + 30), Some(step));
        assert_eq!(key.matching_step(&code, now - 30), Some(step));
        assert_eq!(key.matching_step(&code, now + 90), None);
    }

    #[test]
    fn codes_are_read_with_spaces_and_dashes() {
        assert_eq!(normalize_totp_code(" 123 456 "), Some("123456".into()));
        assert_eq!(normalize_totp_code("123-456"), Some("123456".into()));
        assert_eq!(normalize_totp_code("12345"), None);
        assert_eq!(normalize_totp_code("12345a"), None);
        assert_eq!(normalize_totp_code("+12345"), None);
    }

    #[test]
    fn the_otpauth_url_names_the_issuer_and_the_account() {
        let key = TotpKey::generate("ada:lovelace@example.com");
        let url = key.otpauth_url();
        assert!(url.starts_with("otpauth://totp/MyMCPs:ada%20lovelace%40example.com?"));
        assert!(url.contains(&format!("secret={}", key.base32())));
        assert!(url.contains("issuer=MyMCPs"));
    }

    #[test]
    fn recovery_codes_are_distinct_and_formatted() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), RECOVERY_CODE_COUNT);
        for code in &codes {
            assert_eq!(code.len(), 19);
            assert_eq!(code.matches('-').count(), 3);
            assert!(
                code.bytes()
                    .all(|byte| byte == b'-' || RECOVERY_ALPHABET.contains(&byte))
            );
        }
    }

    #[test]
    fn recovery_codes_forgive_case_separators_and_look_alikes() {
        let hash = hash_recovery_code("ab01-cd23-ef45-gh67");
        assert_eq!(hash_recovery_code("AB01CD23EF45GH67"), hash);
        assert_eq!(hash_recovery_code("abO1 cd23 ef45 gh67"), hash);
        assert_eq!(hash_recovery_code("ab0l-cd23-ef45-gh67"), hash);
        assert_ne!(hash_recovery_code("ab01-cd23-ef45-gh68"), hash);
    }
}
