use sqlx::sqlite::SqliteExecutor;

use crate::time::Timestamp;

/// Most proxies a team may have.
pub const MAX_PROXIES_PER_TEAM: usize = 5;

string_enum! {
    /// What the proxy connects through.
    pub enum ProxyKind {
        /// A Tailscale node that MyMCPs runs, in userspace.
        #[default]
        Tailscale => "tailscale",
    }
}

model! {
    /// A way to reach an MCP that is not on the public internet. An HTTP MCP
    /// that names a proxy sends all its traffic through it. Columns marked
    /// encrypted hold ciphertext: see [`crate::secrets`].
    table = "proxies", created_at = true, updated_at = true;
    pub struct Proxy {
        /// The team the proxy belongs to. Only its MCPs use it.
        pub team_id: i64,
        pub name: String,
        pub kind: ProxyKind,
        /// The name of the node in the tailnet.
        pub tailscale_hostname: String,
        /// Encrypted. The key the node logs in with, when it has one.
        pub tailscale_auth_key: Option<String>,
        pub created_by: Option<i64>,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

impl Proxy {
    /// The proxies of a team, by name.
    pub async fn for_team<'e, E>(db: E, team_id: i64) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select * from `proxies` where `team_id` = ? order by `name` collate nocase asc, `id` asc",
        )
        .bind(team_id)
        .fetch_all(db)
        .await
    }

    /// Every proxy of the instance, oldest first.
    pub async fn all<'e, E>(db: E) -> Result<Vec<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `proxies` order by `id` asc")
            .fetch_all(db)
            .await
    }

    /// How many proxies a team has.
    pub async fn count_for_team<'e, E>(db: E, team_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `proxies` where `team_id` = ?")
            .bind(team_id)
            .fetch_one(db)
            .await
    }

    /// How many proxies the instance has.
    pub async fn count<'e, E>(db: E) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `proxies`")
            .fetch_one(db)
            .await
    }

    /// The id and name of the MCPs that use this proxy, by name.
    pub async fn mcps<'e, E>(&self, db: E) -> Result<Vec<(i64, String)>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(
            "select `id`, `name` from `mcps` where `proxy_id` = ? order by `name` collate nocase asc, `id` asc",
        )
        .bind(self.id)
        .fetch_all(db)
        .await
    }
}
