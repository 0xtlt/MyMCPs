//! What an account asks for besides its password when someone signs in: a
//! code of an authenticator app or a passkey, and the recovery codes that
//! stand in for either once they are lost.

use rand::RngExt;
use sqlx::sqlite::SqliteExecutor;
use sqlx::{Sqlite, Transaction};

use crate::crypto::{Encryption, sha256_hex, totp};
use crate::models::{Passkey, User};
use crate::time::Timestamp;

/// How many recovery codes an account gets at a time.
pub const RECOVERY_CODES: usize = 8;

/// The letters and digits of a recovery code: none that reads as another.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const RECOVERY_CODE_LENGTH: usize = 10;

model! {
    /// A code that signs an account in once, in place of its authenticator
    /// app or its passkey. Only its hash is kept: the person has the code.
    table = "two_factor_recovery_codes", created_at = true, updated_at = false;
    pub struct RecoveryCode {
        pub user_id: i64,
        pub code_hash: String,
        pub used_at: Option<Timestamp>,
        pub created_at: Timestamp,
    }
}

impl RecoveryCode {
    /// A new code, as it is shown: `abcde-fghjk`.
    fn generate() -> String {
        let mut random = rand::rng();
        let mut code = String::with_capacity(RECOVERY_CODE_LENGTH + 1);
        for position in 0..RECOVERY_CODE_LENGTH {
            if position == RECOVERY_CODE_LENGTH / 2 {
                code.push('-');
            }
            let letter = RECOVERY_ALPHABET[random.random_range(0..RECOVERY_ALPHABET.len())];
            code.push(char::from(letter));
        }
        code
    }

    /// The hash of a code, however it was typed: with or without its dash,
    /// its spaces, in capitals.
    fn hash(code: &str) -> String {
        let typed: String = code
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|character| character.to_ascii_lowercase())
            .collect();
        sha256_hex(&typed)
    }

    /// Give the account new codes, in place of those it had. They are
    /// returned to be shown once: nothing here can show them again.
    pub async fn replace_all(
        transaction: &mut Transaction<'_, Sqlite>,
        user_id: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        Self::delete_all(&mut **transaction, user_id).await?;
        let mut codes = Vec::with_capacity(RECOVERY_CODES);
        for _ in 0..RECOVERY_CODES {
            let code = Self::generate();
            let mut stored = Self {
                user_id,
                code_hash: Self::hash(&code),
                ..Default::default()
            };
            stored.insert(&mut **transaction).await?;
            codes.push(code);
        }
        Ok(codes)
    }

    pub async fn delete_all<'e, E>(db: E, user_id: i64) -> Result<(), sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query("delete from `two_factor_recovery_codes` where `user_id` = ?")
            .bind(user_id)
            .execute(db)
            .await?;
        Ok(())
    }

    /// How many codes the account has not used yet.
    pub async fn remaining<'e, E>(db: E, user_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar(
            "select count(*) from `two_factor_recovery_codes` where `user_id` = ? and `used_at` is null",
        )
        .bind(user_id)
        .fetch_one(db)
        .await
    }

    /// Use a code. `true` once for each code: of two requests that send
    /// the same one, only the first marks it used.
    pub async fn consume<'e, E>(db: E, user_id: i64, code: &str) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let used = sqlx::query(
            "update `two_factor_recovery_codes` set `used_at` = ? \
             where `user_id` = ? and `code_hash` = ? and `used_at` is null",
        )
        .bind(Timestamp::now())
        .bind(user_id)
        .bind(Self::hash(code))
        .execute(db)
        .await?;
        Ok(used.rows_affected() == 1)
    }
}

impl User {
    /// Whether a code of an authenticator app is asked for at sign-in.
    pub fn has_authenticator(&self) -> bool {
        self.two_factor_secret.is_some() && self.two_factor_confirmed_at.is_some()
    }

    /// Whether the password alone no longer signs this account in: it has
    /// an authenticator app or a passkey.
    pub async fn is_protected<'e, E>(&self, db: E) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        if self.has_authenticator() {
            return Ok(true);
        }
        Ok(Passkey::count_of_user(db, self.id).await? > 0)
    }

    /// Ask for a code of the app that holds `secret` from now on. The step
    /// of the code that proved the app has it is recorded with it, so that
    /// this code does not sign anyone in afterwards.
    pub async fn enable_authenticator(
        &mut self,
        transaction: &mut Transaction<'_, Sqlite>,
        encryption: &Encryption,
        secret: &str,
        step: i64,
    ) -> Result<(), sqlx::Error> {
        self.two_factor_secret = Some(encryption.encrypt(secret));
        self.two_factor_confirmed_at = Some(Timestamp::now());
        self.two_factor_last_step = Some(step);
        self.save(&mut **transaction).await
    }

    /// Stop asking for a code, and forget the secret.
    pub async fn disable_authenticator(
        &mut self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        self.two_factor_secret = None;
        self.two_factor_confirmed_at = None;
        self.two_factor_last_step = None;
        self.save(&mut **transaction).await
    }

    /// Whether `code` is one the authenticator app of this account shows
    /// now, and was not used yet. It is used by being accepted: of two
    /// requests that send the same code, only the first is told `true`.
    pub async fn accept_authenticator_code<'e, E>(
        &self,
        db: E,
        encryption: &Encryption,
        code: &str,
        unix_seconds: i64,
    ) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        if !self.has_authenticator() {
            return Ok(false);
        }
        let secret = self
            .two_factor_secret
            .as_deref()
            .and_then(|stored| encryption.decrypt(stored));
        let Some(step) = secret.and_then(|secret| {
            totp::verify(&secret, code, unix_seconds, self.two_factor_last_step)
        }) else {
            return Ok(false);
        };
        let written = sqlx::query(
            "update `users` set `two_factor_last_step` = ? \
             where `id` = ? and (`two_factor_last_step` is null or `two_factor_last_step` < ?)",
        )
        .bind(step)
        .bind(self.id)
        .bind(step)
        .execute(db)
        .await?;
        Ok(written.rows_affected() == 1)
    }

    /// Once an authenticator app or a passkey was removed: an account that
    /// has neither left signs in with its password alone, and its recovery
    /// codes stand in for nothing.
    pub async fn settle_recovery_codes(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        if !self.is_protected(&mut **transaction).await? {
            RecoveryCode::delete_all(&mut **transaction, self.id).await?;
        }
        Ok(())
    }

    /// Take away everything the account asks for besides its password: its
    /// authenticator app, its passkeys, its recovery codes. For someone
    /// who lost them all, by someone who runs the instance.
    pub async fn remove_protection(
        &mut self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        self.disable_authenticator(transaction).await?;
        sqlx::query("delete from `passkeys` where `user_id` = ?")
            .bind(self.id)
            .execute(&mut **transaction)
            .await?;
        RecoveryCode::delete_all(&mut **transaction, self.id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_code_is_read_however_it_is_typed() {
        let code = RecoveryCode::generate();
        assert_eq!(code.len(), RECOVERY_CODE_LENGTH + 1);
        assert_eq!(code.chars().nth(5), Some('-'));
        assert!(
            code.bytes()
                .all(|byte| byte == b'-' || RECOVERY_ALPHABET.contains(&byte))
        );
        assert_ne!(code, RecoveryCode::generate());

        let stored = RecoveryCode::hash(&code);
        for typed in [
            code.to_uppercase(),
            code.replace('-', ""),
            format!("  {} ", code.replace('-', " ")),
        ] {
            assert_eq!(RecoveryCode::hash(&typed), stored, "{typed}");
        }
        assert_ne!(RecoveryCode::hash("abcde-fghjk"), stored);
    }
}
