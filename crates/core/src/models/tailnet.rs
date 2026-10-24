use sqlx::sqlite::SqliteExecutor;

use crate::models::Mcp;
use crate::time::Timestamp;

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
