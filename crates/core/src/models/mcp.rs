use std::time::Duration;

use serde_json::Value;

use crate::crypto::Encryption;
use crate::secrets::{self, EnvironmentError};
use crate::time::Timestamp;

string_enum! {
    pub enum McpTransport {
        #[default]
        Http => "http",
        Npm => "npm",
        Builtin => "builtin",
    }
}

/// How long the gateway waits for the answer to a tool call of an MCP that
/// sets no timeout of its own.
pub const DEFAULT_TOOL_TIMEOUT_SECONDS: i64 = 60;

impl McpTransport {
    /// The longest an MCP of this transport may wait for the answer to a
    /// tool call, in seconds. `None` for a built-in MCP: it runs inside
    /// MyMCPs, and each request to its provider has a timeout of its own.
    ///
    /// An HTTP MCP only holds a connection open while it waits. It stops at
    /// 5 minutes, the idle limit of the HTTP client (`IDLE_TIMEOUT` in
    /// `mymcps-net`), which cuts a response that stays silent longer. An npm
    /// MCP is a process this server runs in its sandbox for the whole call,
    /// with its memory and its files: it waits less.
    pub const fn max_tool_timeout_seconds(self) -> Option<i64> {
        match self {
            Self::Http => Some(300),
            Self::Npm => Some(120),
            Self::Builtin => None,
        }
    }
}

string_enum! {
    pub enum McpAuthType {
        #[default]
        Auto => "auto",
        Bearer => "bearer",
        Header => "header",
    }
}

string_enum! {
    pub enum McpStatus {
        #[default]
        Draft => "draft",
        Ready => "ready",
        Error => "error",
    }
}

model! {
    /// An MCP server the gateway exposes: reached over HTTP, run from an npm
    /// package by Node.js in a sandbox, or implemented by MyMCPs itself. Columns
    /// marked encrypted hold ciphertext: see [`crate::secrets`].
    table = "mcps", created_at = true, updated_at = true;
    pub struct Mcp {
        pub name: String,
        pub slug: String,
        pub description: Option<String>,
        pub transport: McpTransport,
        pub http_url: Option<String>,
        pub npm_package: Option<String>,
        pub npm_version: Option<String>,
        /// JSON array of strings. Read it with [`Mcp::npm_args_list`].
        pub npm_args: Option<String>,
        pub auth_type: McpAuthType,
        /// Encrypted.
        pub auth_bearer: Option<String>,
        pub auth_header_name: Option<String>,
        /// Encrypted.
        pub auth_header_value: Option<String>,
        pub oauth_authorize_url: Option<String>,
        pub oauth_token_url: Option<String>,
        pub oauth_scopes: Option<String>,
        pub oauth_client_id: Option<String>,
        /// Encrypted.
        pub oauth_client_secret: Option<String>,
        /// Encrypted.
        pub oauth_access_token: Option<String>,
        /// Encrypted.
        pub oauth_refresh_token: Option<String>,
        pub oauth_token_expires_at: Option<Timestamp>,
        pub status: McpStatus,
        pub last_error: Option<String>,
        pub enabled: bool,
        pub created_by: i64,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
        pub oauth_issuer: Option<String>,
        pub oauth_resource: Option<String>,
        pub oauth_redirect_uri: Option<String>,
        pub oauth_client_auth_method: Option<String>,
        pub oauth_token_type: Option<String>,
        /// JSON object of variable name to encrypted value.
        pub npm_env: Option<String>,
        pub oauth_required: bool,
        pub builtin_key: Option<String>,
        pub builtin_write_enabled: bool,
        pub builtin_username: Option<String>,
        /// Encrypted.
        pub builtin_password: Option<String>,
        pub builtin_permissions: Option<String>,
        pub builtin_aliases: Option<String>,
        /// JSON object of tool name to "ask" or "auto". A tool it does not
        /// name follows its default.
        pub tool_approvals: Option<String>,
        /// JSON object of setting key to encrypted value: what a built-in
        /// MCP needs beyond its sign-in.
        pub builtin_settings: Option<String>,
        /// The team the MCP belongs to. Its slug is unique there.
        pub team_id: i64,
        /// How long a tool call of an HTTP or npm MCP may wait for its
        /// answer, in seconds. `None` waits [`DEFAULT_TOOL_TIMEOUT_SECONDS`].
        /// Read it with [`Mcp::tool_timeout`].
        pub tool_timeout_seconds: Option<i64>,
        /// The tailnet of its team that an HTTP MCP reaches its upstream
        /// through. `None`: the direct network.
        pub tailnet_id: Option<i64>,
    }
}

/// What a step of an OAuth flow started from: where the MCP goes (its
/// transport, URL, network and built-in provider) and the sign-in it holds
/// (the client, the provider and the tokens, as saved: ciphertext is compared
/// as stored). A step that makes requests before it saves what they returned
/// saves it with [`Mcp::save_if_basis_holds`], which writes only while the row
/// is still exactly this.
///
/// That catches a change the step did not see, however far it went: another
/// URL or network, a sign-in that was cleared or made again, and a change
/// that was then undone, which leaves the sign-in cleared.
#[derive(Debug, Clone)]
pub struct McpOauthBasis {
    transport: McpTransport,
    http_url: Option<String>,
    tailnet_id: Option<i64>,
    builtin_key: Option<String>,
    auth_type: McpAuthType,
    oauth_issuer: Option<String>,
    oauth_resource: Option<String>,
    oauth_redirect_uri: Option<String>,
    oauth_authorize_url: Option<String>,
    oauth_token_url: Option<String>,
    oauth_client_id: Option<String>,
    oauth_client_secret: Option<String>,
    oauth_client_auth_method: Option<String>,
    oauth_access_token: Option<String>,
    oauth_refresh_token: Option<String>,
}

impl McpOauthBasis {
    /// The `and` parts of a `where` clause that holds only for a row that is
    /// this basis. `is` compares a missing value to a missing value.
    fn push_conditions(&self, query: &mut sqlx::QueryBuilder<sqlx::Sqlite>) {
        query.push(" and `transport` = ");
        query.push_bind(self.transport);
        query.push(" and `auth_type` = ");
        query.push_bind(self.auth_type);
        query.push(" and `tailnet_id` is ");
        query.push_bind(self.tailnet_id);
        for (column, value) in [
            ("http_url", &self.http_url),
            ("builtin_key", &self.builtin_key),
            ("oauth_issuer", &self.oauth_issuer),
            ("oauth_resource", &self.oauth_resource),
            ("oauth_redirect_uri", &self.oauth_redirect_uri),
            ("oauth_authorize_url", &self.oauth_authorize_url),
            ("oauth_token_url", &self.oauth_token_url),
            ("oauth_client_id", &self.oauth_client_id),
            ("oauth_client_secret", &self.oauth_client_secret),
            ("oauth_client_auth_method", &self.oauth_client_auth_method),
            ("oauth_access_token", &self.oauth_access_token),
            ("oauth_refresh_token", &self.oauth_refresh_token),
        ] {
            query.push(format!(" and `{column}` is "));
            query.push_bind(value.clone());
        }
    }
}

impl Mcp {
    /// What an OAuth step that starts now starts from.
    pub fn oauth_basis(&self) -> McpOauthBasis {
        McpOauthBasis {
            transport: self.transport,
            http_url: self.http_url.clone(),
            tailnet_id: self.tailnet_id,
            builtin_key: self.builtin_key.clone(),
            auth_type: self.auth_type,
            oauth_issuer: self.oauth_issuer.clone(),
            oauth_resource: self.oauth_resource.clone(),
            oauth_redirect_uri: self.oauth_redirect_uri.clone(),
            oauth_authorize_url: self.oauth_authorize_url.clone(),
            oauth_token_url: self.oauth_token_url.clone(),
            oauth_client_id: self.oauth_client_id.clone(),
            oauth_client_secret: self.oauth_client_secret.clone(),
            oauth_client_auth_method: self.oauth_client_auth_method.clone(),
            oauth_access_token: self.oauth_access_token.clone(),
            oauth_refresh_token: self.oauth_refresh_token.clone(),
        }
    }

    /// Save what changed in this value if the row is still `basis`, in one
    /// `update` statement: a row that was edited meanwhile (or deleted) is
    /// not written, not even in part, and `false` says so. This value then
    /// keeps its unsaved changes: drop it.
    pub async fn save_if_basis_holds<'e, E>(
        &mut self,
        db: E,
        basis: &McpOauthBasis,
    ) -> Result<bool, sqlx::Error>
    where
        E: sqlx::sqlite::SqliteExecutor<'e>,
    {
        self.save_if(db, |query| basis.push_conditions(query)).await
    }

    /// How long a tool call of this MCP waits for its answer: the timeout
    /// saved for it, or [`DEFAULT_TOOL_TIMEOUT_SECONDS`], and never more
    /// than its transport allows.
    pub fn tool_timeout(&self) -> Duration {
        let max = self
            .transport
            .max_tool_timeout_seconds()
            .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECONDS);
        let seconds = self
            .tool_timeout_seconds
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECONDS)
            .min(max);
        Duration::from_secs(seconds.unsigned_abs())
    }

    /// The npm args, which the column stores as JSON text.
    pub fn npm_args_list(&self) -> Vec<String> {
        let Some(Ok(Value::Array(parts))) = self
            .npm_args
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(serde_json::from_str::<Value>)
        else {
            return Vec::new();
        };
        parts
            .into_iter()
            .map(|part| match part {
                Value::String(text) => text,
                other => other.to_string(),
            })
            .collect()
    }

    pub fn set_npm_args_list(&mut self, args: &[String]) {
        self.npm_args = (!args.is_empty()).then(|| Value::from(args.to_vec()).to_string());
    }

    pub fn npm_env_names(&self) -> Vec<String> {
        secrets::environment_names(self.npm_env.as_deref())
    }

    /// The process environment of an npm MCP, decrypted, in the order saved.
    pub fn npm_environment(
        &self,
        encryption: &Encryption,
    ) -> Result<Vec<(String, String)>, EnvironmentError> {
        secrets::decrypt_environment(encryption, self.npm_env.as_deref())
    }

    pub fn slugify(name: &str) -> String {
        let lowered = name.to_lowercase();
        let mut slug = String::new();
        let mut pending_dash = false;
        for character in lowered.trim().chars() {
            if character.is_ascii_lowercase() || character.is_ascii_digit() {
                if pending_dash && !slug.is_empty() {
                    slug.push('-');
                }
                pending_dash = false;
                slug.push(character);
            } else {
                pending_dash = true;
            }
        }
        // Cut after trimming the dashes, as the Node app did: a slug may end
        // with a dash when the cut falls on one.
        let slug: String = slug.chars().take(80).collect();
        if slug.is_empty() {
            "mcp".to_string()
        } else {
            slug
        }
    }
}
