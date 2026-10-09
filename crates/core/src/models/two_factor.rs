use sqlx::sqlite::SqliteExecutor;

use crate::time::Timestamp;

model! {
    /// The authenticator app of a user: at most one per account.
    table = "user_totp_secrets", created_at = true, updated_at = true;
    pub struct UserTotpSecret {
        pub user_id: i64,
        /// The base32 secret, encrypted with the application key.
        pub secret: String,
        /// Unset until the user proves the app was set up by entering a code:
        /// an unconfirmed secret is not asked for at sign-in.
        pub confirmed_at: Option<Timestamp>,
        /// The last time step a code was accepted for, so that a code is
        /// accepted only once.
        pub last_used_step: Option<i64>,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

impl UserTotpSecret {
    pub async fn for_user<'e, E>(db: E, user_id: i64) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `user_totp_secrets` where `user_id` = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await
    }

    pub fn is_confirmed(&self) -> bool {
        self.confirmed_at.is_some()
    }

    /// Record that a code of `step` was accepted. False when a code of this
    /// step or a later one already was: the code is being replayed. One
    /// statement, so two requests racing with the same code cannot both win.
    pub async fn consume_step<'e, E>(&mut self, db: E, step: i64) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let now = Timestamp::now();
        let updated = sqlx::query(
            "update `user_totp_secrets` set `last_used_step` = ?, `updated_at` = ? \
             where `id` = ? and (`last_used_step` is null or `last_used_step` < ?)",
        )
        .bind(step)
        .bind(now)
        .bind(self.id)
        .bind(step)
        .execute(db)
        .await?
        .rows_affected();
        if updated == 0 {
            return Ok(false);
        }
        self.last_used_step = Some(step);
        self.updated_at = Some(now);
        self.remember();
        Ok(true)
    }
}

model! {
    /// A single-use code that stands in for the second step when the
    /// authenticator app is lost. Only its SHA-256 is stored.
    table = "user_recovery_codes", created_at = true, updated_at = false;
    pub struct UserRecoveryCode {
        pub user_id: i64,
        pub code_hash: String,
        pub used_at: Option<Timestamp>,
        pub created_at: Timestamp,
    }
}

impl UserRecoveryCode {
    /// Replace every code of the user with these hashes.
    pub async fn replace_all(
        connection: &mut sqlx::SqliteConnection,
        user_id: i64,
        hashes: &[String],
    ) -> Result<(), sqlx::Error> {
        Self::delete_all(&mut *connection, user_id).await?;
        for hash in hashes {
            Self {
                user_id,
                code_hash: hash.clone(),
                ..Default::default()
            }
            .insert(&mut *connection)
            .await?;
        }
        Ok(())
    }

    pub async fn delete_all<'e, E>(db: E, user_id: i64) -> Result<(), sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query("delete from `user_recovery_codes` where `user_id` = ?")
            .bind(user_id)
            .execute(db)
            .await?;
        Ok(())
    }

    /// How many codes the user has left.
    pub async fn remaining<'e, E>(db: E, user_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar(
            "select count(*) from `user_recovery_codes` where `user_id` = ? and `used_at` is null",
        )
        .bind(user_id)
        .fetch_one(db)
        .await
    }

    /// Spend the code with this hash. False when the user has no such unused
    /// code. One statement, so a code cannot be spent twice.
    pub async fn redeem<'e, E>(db: E, user_id: i64, code_hash: &str) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let updated = sqlx::query(
            "update `user_recovery_codes` set `used_at` = ? \
             where `user_id` = ? and `code_hash` = ? and `used_at` is null",
        )
        .bind(Timestamp::now())
        .bind(user_id)
        .bind(code_hash)
        .execute(db)
        .await?
        .rows_affected();
        Ok(updated > 0)
    }
}

model! {
    /// A WebAuthn credential that signs the user in on its own.
    table = "user_passkeys", created_at = true, updated_at = true;
    pub struct UserPasskey {
        pub user_id: i64,
        /// What the user called it, to tell their passkeys apart.
        pub name: String,
        /// The credential id, base64url: how the browser names the passkey.
        pub credential_id: String,
        /// The WebAuthn user handle, a UUID shared by every passkey of the
        /// user: a discoverable passkey returns it at sign-in.
        pub user_handle: String,
        /// The credential as `webauthn-rs` serialises it: the public key and
        /// the signature counter. Public data, stored as is.
        pub passkey: String,
        pub last_used_at: Option<Timestamp>,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

impl UserPasskey {
    /// The passkeys of the user, oldest first.
    pub async fn for_user<'e, E>(db: E, user_id: i64) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `user_passkeys` where `user_id` = ? order by `id`")
            .bind(user_id)
            .fetch_all(db)
            .await
    }

    /// The passkey `id` when it belongs to the user.
    pub async fn find_for_user<'e, E>(
        db: E,
        user_id: i64,
        id: i64,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `user_passkeys` where `id` = ? and `user_id` = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(db)
            .await
    }

    pub async fn find_by_credential_id<'e, E>(
        db: E,
        credential_id: &str,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `user_passkeys` where `credential_id` = ?")
            .bind(credential_id)
            .fetch_optional(db)
            .await
    }
}
