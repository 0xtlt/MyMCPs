use sqlx::SqlitePool;
use sqlx::sqlite::SqliteExecutor;

use crate::crypto::Encryption;
use crate::models::Mcp;
use crate::secrets;
use crate::time::Timestamp;

model! {
    /// An OAuth client a team registered with a provider, saved once and
    /// chosen by each of its built-in MCPs that signs in with that
    /// provider: one Google Cloud client serves Google Ads and Google Search
    /// Console. The name is unique in the team, and so is the Client ID of a
    /// provider.
    ///
    /// An MCP keeps a copy of the client in its `oauth_client_id` and
    /// `oauth_client_secret` columns, which every OAuth step reads and
    /// compares: [`ProviderCredential::save_to_mcps`] rewrites the copies.
    table = "provider_credentials", created_at = true, updated_at = true;
    pub struct ProviderCredential {
        /// The team the credential belongs to, and that alone may use it.
        pub team_id: i64,
        /// Who issued it, such as `google`.
        pub provider: String,
        /// What the team calls it.
        pub name: String,
        pub client_id: String,
        /// Encrypted.
        pub client_secret: String,
        pub created_by: i64,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

/// What a change of a credential did to the MCPs that use it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialChange {
    /// MCPs whose sign-in was dropped: a new Client ID does not renew the
    /// tokens of the old one, so they wait to be connected again.
    pub signed_out: u64,
    /// MCPs given the new Client Secret. Their sign-in still holds.
    pub updated: u64,
}

impl ProviderCredential {
    /// The credentials of a team, by name. `provider`: only that provider's.
    pub async fn of_team<'e, E>(
        db: E,
        team_id: i64,
        provider: Option<&str>,
    ) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `provider_credentials` where `team_id` = ? and (? is null or `provider` = ?) \
             order by `name` collate nocase asc, `id` asc",
        )
        .bind(team_id)
        .bind(provider)
        .bind(provider)
        .fetch_all(db)
        .await
    }

    /// A credential, only when it belongs to `team_id`: a request of one
    /// team never reaches the credential of another by its id.
    pub async fn find_in_team<'e, E>(
        db: E,
        team_id: i64,
        id: i64,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `provider_credentials` where `team_id` = ? and `id` = ?")
            .bind(team_id)
            .bind(id)
            .fetch_optional(db)
            .await
    }

    /// The credential of a team that holds this Client ID of `provider`.
    pub async fn find_by_client_id<'e, E>(
        db: E,
        team_id: i64,
        provider: &str,
        client_id: &str,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `provider_credentials` where `team_id` = ? and `provider` = ? and `client_id` = ?",
        )
        .bind(team_id)
        .bind(provider)
        .bind(client_id)
        .fetch_optional(db)
        .await
    }

    /// The MCPs that sign in with this credential, by name.
    pub async fn mcps<'e, E>(&self, db: E) -> Result<Vec<Mcp>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `mcps` where `provider_credential_id` = ? order by `name` collate nocase asc, `id` asc",
        )
        .bind(self.id)
        .fetch_all(db)
        .await
    }

    /// The Client Secret, decrypted.
    pub fn secret(&self, encryption: &Encryption) -> Option<String> {
        secrets::decrypt_secret(encryption, Some(&self.client_secret))
    }

    /// Give `mcp` a copy of this client. `true` when its Client ID changed:
    /// its tokens were issued to another client, so its sign-in is dropped
    /// and a sign-in in flight with it. A new secret alone keeps them.
    pub fn copy_to(&self, mcp: &mut Mcp, encryption: &Encryption) -> bool {
        let renamed = mcp.oauth_client_id.as_deref() != Some(self.client_id.as_str());
        if renamed {
            mcp.renew_oauth_generation();
            mcp.clear_oauth_connection();
            mcp.mark_awaiting_sign_in();
        }
        mcp.provider_credential_id = Some(self.id);
        mcp.oauth_client_id = Some(self.client_id.clone());
        mcp.oauth_client_secret = secrets::encrypt_secret_keeping(
            encryption,
            mcp.oauth_client_secret.as_deref(),
            self.secret(encryption).as_deref(),
        );
        renamed
    }

    /// Save the credential and rewrite the copy each of its MCPs holds, in
    /// one transaction. The credential is written first, which takes the
    /// write lock: the MCPs read after are the latest, and none can change
    /// between their read and their write.
    pub async fn save_to_mcps(
        &mut self,
        db: &SqlitePool,
        encryption: &Encryption,
    ) -> Result<CredentialChange, sqlx::Error> {
        let mut transaction = db.begin().await?;
        // Written even when nothing changed, for the lock.
        self.updated_at = Some(Timestamp::now());
        self.save(&mut *transaction).await?;
        let mcps: Vec<Mcp> =
            sqlx::query_as("select * from `mcps` where `provider_credential_id` = ?")
                .bind(self.id)
                .fetch_all(&mut *transaction)
                .await?;
        let mut change = CredentialChange::default();
        for mut mcp in mcps {
            let before = mcp.oauth_client_secret.clone();
            if self.copy_to(&mut mcp, encryption) {
                change.signed_out += 1;
            } else if mcp.oauth_client_secret != before {
                change.updated += 1;
            }
            mcp.save(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(change)
    }

    /// Delete the credential, only while no MCP uses it. `false`: one does.
    pub async fn delete_unused<'e, E>(&self, db: E) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let deleted = sqlx::query(
            "delete from `provider_credentials` where `id` = ? and not exists \
             (select 1 from `mcps` where `mcps`.`provider_credential_id` = `provider_credentials`.`id`)",
        )
        .bind(self.id)
        .execute(db)
        .await?
        .rows_affected();
        Ok(deleted > 0)
    }
}
