use sqlx::sqlite::SqliteExecutor;

use crate::crypto::webauthn::Assertion;
use crate::time::Timestamp;

model! {
    /// A key an authenticator holds for an account: a phone, a computer, a
    /// password manager, a security key. The private half never leaves it;
    /// this is the public half, which checks what it signs.
    table = "passkeys", created_at = true, updated_at = false;
    pub struct Passkey {
        pub user_id: i64,
        /// What the person calls it.
        pub name: String,
        /// The id the authenticator gave the key, in base64url.
        pub credential_id: String,
        /// The public key in COSE, in base64url.
        pub public_key: String,
        /// How many times the authenticator says it signed. Zero for the
        /// many that do not count.
        pub sign_count: i64,
        /// Whether a service keeps the key and syncs it between devices.
        pub backed_up: bool,
        pub created_at: Timestamp,
        pub last_used_at: Option<Timestamp>,
    }
}

impl Passkey {
    /// The passkeys of an account, oldest first.
    pub async fn of_user<'e, E>(db: E, user_id: i64) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `passkeys` where `user_id` = ? order by `id` asc")
            .bind(user_id)
            .fetch_all(db)
            .await
    }

    pub async fn count_of_user<'e, E>(db: E, user_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `passkeys` where `user_id` = ?")
            .bind(user_id)
            .fetch_one(db)
            .await
    }

    pub async fn find_by_credential_id<'e, E>(
        db: E,
        credential_id: &str,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `passkeys` where `credential_id` = ?")
            .bind(credential_id)
            .fetch_optional(db)
            .await
    }

    /// Record that the passkey just signed. `false`, and nothing written,
    /// when its counter did not move forward although it counts: the key
    /// was copied, and one of the two copies is not the person's.
    pub async fn record_use<'e, E>(&self, db: E, assertion: Assertion) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let sign_count = i64::from(assertion.sign_count);
        let written = sqlx::query(
            "update `passkeys` set `sign_count` = ?, `backed_up` = ?, `last_used_at` = ? \
             where `id` = ? and (`sign_count` < ? or (`sign_count` = 0 and ? = 0))",
        )
        .bind(sign_count)
        .bind(assertion.backed_up)
        .bind(Timestamp::now())
        .bind(self.id)
        .bind(sign_count)
        .bind(sign_count)
        .execute(db)
        .await?;
        Ok(written.rows_affected() == 1)
    }
}
