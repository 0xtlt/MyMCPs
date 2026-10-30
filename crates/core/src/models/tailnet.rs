use sqlx::SqlitePool;
use sqlx::sqlite::SqliteExecutor;

use crate::models::Mcp;
use crate::time::Timestamp;

/// Where the node of a tailnet is, against the tailnet it was first found
/// running in. See [`Tailnet::settle_identity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailnetIdentity {
    /// The tailnet it was first found in.
    Same,
    /// It runs for the first time: this tailnet is now the one its MCPs are for.
    Bound,
    /// Another tailnet. The credentials of its MCPs, entered for the first
    /// one, were cleared, and this one is remembered instead.
    Changed {
        /// How many MCPs were signed out.
        mcps: u64,
    },
}

/// Why an MCP lost its credentials when its tailnet changed, for the MCP's
/// page.
pub const SIGNED_OUT_FOR_ANOTHER_TAILNET: &str = "The tailnet of this MCP was signed in to another Tailscale tailnet, so its saved credentials and OAuth sign-in were cleared. Enter them again, or connect it again";

model! {
    /// A Tailscale tailnet a team connects, for its HTTP MCPs to reach
    /// upstreams that only answer inside it. The server joins it as one
    /// node of its own, called `hostname`. The name is unique in the team.
    table = "tailnets", created_at = true, updated_at = true;
    pub struct Tailnet {
        /// The team the tailnet belongs to, and that alone may use it.
        pub team_id: i64,
        /// What the team calls it.
        pub name: String,
        /// The control server to join, for a Headscale or another server
        /// that is not Tailscale's own. `None` for Tailscale's.
        pub control_url: Option<String>,
        /// The name of the node this server is in the tailnet.
        pub hostname: String,
        /// The tailnet the node was first found running in, by its MagicDNS
        /// suffix (`tail1234.ts.net`), in lowercase. `None` until then. The
        /// credentials of its MCPs were entered for that tailnet: see
        /// [`Tailnet::settle_identity`].
        pub magic_dns_suffix: Option<String>,
        pub created_by: i64,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

#[cfg(feature = "test-util")]
impl Tailnet {
    /// A tailnet row of `team_id` called `name`, as if connected earlier: its
    /// node is `mymcps-<name>`.
    pub fn for_tests(team_id: i64, created_by: i64, name: &str) -> Self {
        Self {
            team_id,
            name: name.into(),
            hostname: format!("mymcps-{}", name.to_lowercase()),
            created_by,
            ..Default::default()
        }
    }

    /// [`for_tests`](Self::for_tests), saved.
    pub async fn create_for_tests<'e, E>(db: E, team_id: i64, created_by: i64, name: &str) -> Self
    where
        E: SqliteExecutor<'e>,
    {
        let mut tailnet = Self::for_tests(team_id, created_by, name);
        tailnet.insert(db).await.unwrap();
        tailnet
    }
}

impl Tailnet {
    /// The tailnets of a team, by name.
    pub async fn of_team<'e, E>(db: E, team_id: i64) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `tailnets` where `team_id` = ? order by `name` collate nocase asc, `id` asc",
        )
        .bind(team_id)
        .fetch_all(db)
        .await
    }

    /// A tailnet, only when it belongs to `team_id`: a request of one team
    /// never reaches the tailnet of another by its id.
    pub async fn find_in_team<'e, E>(
        db: E,
        team_id: i64,
        id: i64,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `tailnets` where `team_id` = ? and `id` = ?")
            .bind(team_id)
            .bind(id)
            .fetch_optional(db)
            .await
    }

    /// The MCPs that reach their upstream through this tailnet, by name.
    pub async fn mcps<'e, E>(&self, db: E) -> Result<Vec<Mcp>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `mcps` where `tailnet_id` = ? order by `name` collate nocase asc, `id` asc",
        )
        .bind(self.id)
        .fetch_all(db)
        .await
    }

    /// How many MCPs use this tailnet.
    pub async fn mcp_count<'e, E>(&self, db: E) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `mcps` where `tailnet_id` = ?")
            .bind(self.id)
            .fetch_one(db)
            .await
    }

    /// Check that the node of this tailnet runs in the tailnet it was first
    /// found running in, given the MagicDNS suffix the daemon reports now,
    /// and remember it the first time.
    ///
    /// The node of a row can join another tailnet: a sign-in again with
    /// another account, or a backup restored without the node's keys and
    /// signed in again elsewhere. The same names and subnet addresses then
    /// reach other machines, which the credentials of its MCPs were never
    /// entered for. So the new tailnet is remembered, and in the same
    /// transaction every MCP on it loses its secrets and its OAuth sign-in,
    /// gets a new sign-in generation (a sign-in or renewal in flight is
    /// dropped) and says why in its error.
    ///
    /// Compares with `self` first: a tailnet that did not move costs no write.
    pub async fn settle_identity(
        &self,
        db: &SqlitePool,
        magic_dns_suffix: &str,
    ) -> Result<TailnetIdentity, sqlx::Error> {
        let current = magic_dns_suffix.trim_matches('.').to_ascii_lowercase();
        let mut saved = self.magic_dns_suffix.clone();
        // Another request may settle the same move first: then this one reads
        // what it left, which is the same tailnet. Three turns are plenty.
        for _ in 0..3 {
            if saved.as_deref() == Some(current.as_str()) {
                return Ok(TailnetIdentity::Same);
            }
            let mut transaction = db.begin().await?;
            // The tailnet moves only from what was read: the move and the
            // clearing of its MCPs are one, or nothing.
            let moved = sqlx::query(
                "update `tailnets` set `magic_dns_suffix` = ?, `updated_at` = ? where `id` = ? and `magic_dns_suffix` is ?",
            )
            .bind(&current)
            .bind(Timestamp::now())
            .bind(self.id)
            .bind(&saved)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
            if moved == 0 {
                transaction.rollback().await?;
                saved = sqlx::query_scalar::<_, Option<String>>(
                    "select `magic_dns_suffix` from `tailnets` where `id` = ?",
                )
                .bind(self.id)
                .fetch_optional(db)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?;
                continue;
            }
            if saved.is_none() {
                transaction.commit().await?;
                return Ok(TailnetIdentity::Bound);
            }
            let mcps = sign_out_mcps(&mut transaction, self.id).await?;
            transaction.commit().await?;
            return Ok(TailnetIdentity::Changed { mcps });
        }
        Err(sqlx::Error::Protocol(
            "the tailnet of a node kept changing while it was checked".to_owned(),
        ))
    }

    /// How many tailnets a team has.
    pub async fn count_in_team<'e, E>(db: E, team_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `tailnets` where `team_id` = ?")
            .bind(team_id)
            .fetch_one(db)
            .await
    }
}

/// Clears the secrets and the OAuth sign-in of every MCP on tailnet `id`:
/// they were entered for the tailnet it was before.
async fn sign_out_mcps(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
) -> Result<u64, sqlx::Error> {
    // `random() & (2^52 - 1)`: a new generation, as `renew_oauth_generation`.
    // An MCP that held OAuth tokens is left waiting for its account, as
    // `forget_oauth_tokens` leaves it, so Connect stays available: the
    // right-hand sides read the row as it was before this update.
    let mcps = sqlx::query(
        "update `mcps` set `auth_bearer` = null, `auth_header_value` = null, \
         `oauth_authorize_url` = null, `oauth_token_url` = null, `oauth_scopes` = null, \
         `oauth_client_id` = null, `oauth_client_secret` = null, \
         `oauth_access_token` = null, `oauth_refresh_token` = null, \
         `oauth_token_expires_at` = null, `oauth_issuer` = null, `oauth_resource` = null, \
         `oauth_redirect_uri` = null, `oauth_client_auth_method` = null, \
         `oauth_token_type` = null, \
         `oauth_required` = (`auth_type` = 'auto' and \
             (coalesce(`oauth_access_token`, '') != '' or coalesce(`oauth_refresh_token`, '') != '')), \
         `oauth_generation` = (random() & 4503599627370495), \
         `status` = 'error', `last_error` = ?, `updated_at` = ? \
         where `tailnet_id` = ?",
    )
    .bind(SIGNED_OUT_FOR_ANOTHER_TAILNET)
    .bind(Timestamp::now())
    .bind(id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(mcps)
}
