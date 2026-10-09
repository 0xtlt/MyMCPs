use sqlx::{Acquire, Sqlite};

use crate::models::DEFAULT_MCP_AUTO_UPDATE_CRON;
use crate::time::Timestamp;

string_enum! {
    pub enum McpLogLevel {
        Off => "off",
        #[default]
        Metadata => "metadata",
        Arguments => "arguments",
        Responses => "responses",
    }
}

string_enum! {
    pub enum GatewayToolMode {
        #[default]
        Eager => "eager",
        Lazy => "lazy",
    }
}

model! {
    table = "instance_settings", created_at = true, updated_at = true;
    pub struct InstanceSetting {
        pub mcp_log_level: McpLogLevel,
        pub mcp_log_retention_days: i64,
        pub updated_by: Option<i64>,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
        pub gateway_tool_mode: GatewayToolMode,
        pub mcp_auto_update_enabled: bool,
        pub mcp_auto_update_cron: String,
    }
}

impl InstanceSetting {
    /// The one row of settings, created with the defaults when missing.
    ///
    /// Reading it takes no write lock: it is read on every request of the
    /// gateway, and SQLite has one writer at a time.
    pub async fn current<'a, A>(db: A) -> Result<Self, sqlx::Error>
    where
        A: Acquire<'a, Database = Sqlite>,
    {
        let mut connection = db.acquire().await?;
        let existing = sqlx::query_as("select * from `instance_settings` where `id` = 1")
            .fetch_optional(&mut *connection)
            .await?;
        if let Some(settings) = existing {
            return Ok(settings);
        }
        // One statement, so that two first requests cannot both insert the row.
        sqlx::query_as(
            "insert into `instance_settings` (`id`, `gateway_tool_mode`, `mcp_log_level`, `mcp_log_retention_days`, `mcp_auto_update_enabled`, `mcp_auto_update_cron`, `updated_by`, `created_at`, `updated_at`) \
             values (1, 'eager', 'metadata', 14, 0, ?, null, ?, ?) \
             on conflict (`id`) do update set `id` = `id` returning *",
        )
        .bind(DEFAULT_MCP_AUTO_UPDATE_CRON)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .fetch_one(&mut *connection)
        .await
    }
}
