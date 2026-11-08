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
        /// Names the sign-in this MCP holds, and the destination it is for.
        /// It is replaced whenever the MCP is moved or its sign-in is
        /// cleared ([`Mcp::renew_oauth_generation`]). A sign-in that is
        /// started carries it, and its callback is refused once it is no
        /// longer the MCP's: a destination that was left and come back to is
        /// not the one the sign-in began on.
        pub oauth_generation: i64,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    oauth_generation: i64,
}

impl McpOauthBasis {
    /// The `and` parts of a `where` clause that holds only for a row that is
    /// this basis. `is` compares a missing value to a missing value.
    fn push_conditions(&self, query: &mut sqlx::QueryBuilder<sqlx::Sqlite>) {
        query.push(" and `transport` = ");
        query.push_bind(self.transport);
        query.push(" and `auth_type` = ");
        query.push_bind(self.auth_type);
        query.push(" and `oauth_generation` = ");
        query.push_bind(self.oauth_generation);
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

/// What a test of an MCP reached it with: its destination and its sign-in
/// ([`McpOauthBasis`]), and what else it connects with (its package, its
/// secrets, the account of a built-in provider). A result is saved with
/// [`Mcp::save_probe_result`] only while the row still is this: a test that
/// ran on what was edited, signed in again or signed out meanwhile says
/// nothing of the MCP as it is now.
///
/// Two equal bases connect the same way: an edit that changes it leaves the
/// MCP untested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProbeBasis {
    oauth: McpOauthBasis,
    npm_package: Option<String>,
    npm_version: Option<String>,
    npm_args: Option<String>,
    npm_env: Option<String>,
    auth_bearer: Option<String>,
    auth_header_name: Option<String>,
    auth_header_value: Option<String>,
    builtin_username: Option<String>,
    builtin_password: Option<String>,
    builtin_settings: Option<String>,
}

impl McpProbeBasis {
    /// The tokens the test used in the end. A test that had to renew the
    /// access token saved the new one itself, under the sign-in it started
    /// from, and read the row again: what it holds after is what it used.
    /// Everything else stays as the test started.
    pub fn after_renewal(mut self, mcp: &Mcp) -> Self {
        self.oauth.oauth_access_token = mcp.oauth_access_token.clone();
        self.oauth.oauth_refresh_token = mcp.oauth_refresh_token.clone();
        self
    }

    fn push_conditions(&self, query: &mut sqlx::QueryBuilder<sqlx::Sqlite>) {
        self.oauth.push_conditions(query);
        for (column, value) in [
            ("npm_package", &self.npm_package),
            ("npm_version", &self.npm_version),
            ("npm_args", &self.npm_args),
            ("npm_env", &self.npm_env),
            ("auth_bearer", &self.auth_bearer),
            ("auth_header_name", &self.auth_header_name),
            ("auth_header_value", &self.auth_header_value),
            ("builtin_username", &self.builtin_username),
            ("builtin_password", &self.builtin_password),
            ("builtin_settings", &self.builtin_settings),
        ] {
            query.push(format!(" and `{column}` is "));
            query.push_bind(value.clone());
        }
    }
}

/// The `last_error` of an MCP that waits for an OAuth sign-in it never had.
pub const AWAITING_SIGN_IN_ERROR: &str = "OAuth authorization required";

/// Whether an MCP can be used, as far as is known: the one meaning of its
/// `status`, `last_error` and `oauth_required` columns together. They are
/// written only through [`Mcp::set_health`], so that no combination of them
/// means two things at once, and read through [`Mcp::health`].
///
/// | health             | `status` | `last_error`             | `oauth_required` |
/// |--------------------|----------|--------------------------|------------------|
/// | `Untested`         | draft    | none                     | false            |
/// | `AwaitingSignIn`   | draft    | [`AWAITING_SIGN_IN_ERROR`] | true           |
/// | `SignInRejected`   | error    | the reason               | true             |
/// | `Ready`            | ready    | none                     | false            |
/// | `Failing`          | error    | the reason               | false            |
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpHealth {
    /// Not tested since what it connects to was last set.
    Untested,
    /// Waits for an OAuth sign-in: it holds none. Connect gives it one.
    AwaitingSignIn,
    /// Its OAuth sign-in was refused or taken away: connect it again.
    SignInRejected(String),
    /// Its last test reached it.
    Ready,
    /// Its last test failed, for a reason a sign-in does not repair.
    Failing(String),
}

impl McpHealth {
    /// Whether someone has to act before the MCP can be used: sign it in,
    /// fix it, or test it.
    pub fn needs_attention(&self) -> bool {
        *self != Self::Ready
    }

    /// Whether it waits for an OAuth sign-in, first or again.
    pub fn awaits_sign_in(&self) -> bool {
        matches!(self, Self::AwaitingSignIn | Self::SignInRejected(_))
    }

    /// Whether its last test failed for a reason a sign-in does not repair.
    pub fn is_failing(&self) -> bool {
        matches!(self, Self::Failing(_))
    }

    /// The `status` column it is saved as.
    pub fn status(&self) -> McpStatus {
        self.columns().0
    }

    /// Why it cannot be used, when a reason was saved.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::SignInRejected(reason) | Self::Failing(reason) => Some(reason),
            Self::AwaitingSignIn => Some(AWAITING_SIGN_IN_ERROR),
            Self::Untested | Self::Ready => None,
        }
    }

    fn columns(&self) -> (McpStatus, Option<String>, bool) {
        match self {
            Self::Untested => (McpStatus::Draft, None, false),
            Self::AwaitingSignIn => (
                McpStatus::Draft,
                Some(AWAITING_SIGN_IN_ERROR.to_owned()),
                true,
            ),
            Self::SignInRejected(reason) => (McpStatus::Error, Some(reason.clone()), true),
            Self::Ready => (McpStatus::Ready, None, false),
            Self::Failing(reason) => (McpStatus::Error, Some(reason.clone()), false),
        }
    }
}

/// What a test of an MCP found, for [`Mcp::mark_probed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// It answered.
    Reached,
    /// It refused for want of an OAuth sign-in, which Connect can give it.
    /// `held_token`: it was sent an access token, which it refused.
    SignInRefused { held_token: bool, reason: String },
    /// It failed for another reason.
    Failed(String),
}

impl Mcp {
    /// Whether it can be used, read from its columns. A row a version before
    /// [`McpHealth`] left contradictory reads as what it waits for: a sign-in
    /// that is required comes before a status that says otherwise.
    pub fn health(&self) -> McpHealth {
        let reason = || self.last_error.clone().filter(|reason| !reason.is_empty());
        match (self.oauth_required, self.status) {
            (true, McpStatus::Error) => McpHealth::SignInRejected(
                reason().unwrap_or_else(|| AWAITING_SIGN_IN_ERROR.to_owned()),
            ),
            (true, _) => McpHealth::AwaitingSignIn,
            (false, McpStatus::Ready) => McpHealth::Ready,
            (false, McpStatus::Error) => {
                McpHealth::Failing(reason().unwrap_or_else(|| "Connection failed".to_owned()))
            }
            (false, McpStatus::Draft) => McpHealth::Untested,
        }
    }

    /// Write `status`, `last_error` and `oauth_required` together, as
    /// `health` means them. The only writer of these columns.
    pub fn set_health(&mut self, health: McpHealth) {
        let (status, last_error, oauth_required) = health.columns();
        self.status = status;
        self.last_error = last_error;
        self.oauth_required = oauth_required;
    }

    /// What it connects to or with changed: what was known of it no longer
    /// holds.
    pub fn mark_untested(&mut self) {
        self.set_health(McpHealth::Untested);
    }

    /// Its OAuth tokens were dropped for a new sign-in: it waits for it.
    pub fn mark_awaiting_sign_in(&mut self) {
        self.set_health(McpHealth::AwaitingSignIn);
    }

    /// Save what a test found.
    pub fn mark_probed(&mut self, outcome: ProbeOutcome) {
        self.set_health(match outcome {
            ProbeOutcome::Reached => McpHealth::Ready,
            ProbeOutcome::SignInRefused {
                held_token: false, ..
            } => McpHealth::AwaitingSignIn,
            ProbeOutcome::SignInRefused {
                held_token: true,
                reason,
            } => McpHealth::SignInRejected(reason),
            ProbeOutcome::Failed(reason) => McpHealth::Failing(reason),
        });
    }

    /// Whether it signs in with OAuth and holds a token of that sign-in.
    pub fn holds_oauth_sign_in(&self) -> bool {
        let set = |value: &Option<String>| value.as_deref().is_some_and(|value| !value.is_empty());
        self.auth_type == McpAuthType::Auto
            && (set(&self.oauth_access_token) || set(&self.oauth_refresh_token))
    }

    /// Empty every column of its OAuth sign-in: the client, the provider and
    /// the tokens. Its health is the caller's to set.
    pub fn clear_oauth_connection(&mut self) {
        self.oauth_authorize_url = None;
        self.oauth_token_url = None;
        self.oauth_scopes = None;
        self.oauth_client_id = None;
        self.oauth_client_secret = None;
        self.oauth_access_token = None;
        self.oauth_refresh_token = None;
        self.oauth_token_expires_at = None;
        self.oauth_issuer = None;
        self.oauth_resource = None;
        self.oauth_redirect_uri = None;
        self.oauth_client_auth_method = None;
        self.oauth_token_type = None;
    }

    /// Forget the secrets and the OAuth sign-in it was given, for `reason`,
    /// and name a new sign-in, so that one in flight is dropped. One that
    /// signed in with OAuth waits to be connected again; another fails with
    /// `reason` until its secrets are entered again.
    pub fn sign_out(&mut self, reason: &str) {
        let held = self.holds_oauth_sign_in();
        self.renew_oauth_generation();
        self.auth_bearer = None;
        self.auth_header_value = None;
        self.clear_oauth_connection();
        self.set_health(if held {
            McpHealth::SignInRejected(reason.to_owned())
        } else {
            McpHealth::Failing(reason.to_owned())
        });
    }

    /// What a test that starts now starts from.
    pub fn probe_basis(&self) -> McpProbeBasis {
        McpProbeBasis {
            oauth: self.oauth_basis(),
            npm_package: self.npm_package.clone(),
            npm_version: self.npm_version.clone(),
            npm_args: self.npm_args.clone(),
            npm_env: self.npm_env.clone(),
            auth_bearer: self.auth_bearer.clone(),
            auth_header_name: self.auth_header_name.clone(),
            auth_header_value: self.auth_header_value.clone(),
            builtin_username: self.builtin_username.clone(),
            builtin_password: self.builtin_password.clone(),
            builtin_settings: self.builtin_settings.clone(),
        }
    }

    /// Save what changed in this value, the result of a test, if the row
    /// is still `basis`, in one `update` statement. `false`: the row was
    /// edited, signed in or out, or deleted since the test started, and
    /// nothing was written; this value keeps its unsaved changes.
    pub async fn save_probe_result<'e, E>(
        &mut self,
        db: E,
        basis: &McpProbeBasis,
    ) -> Result<bool, sqlx::Error>
    where
        E: sqlx::sqlite::SqliteExecutor<'e>,
    {
        // Written even when it found what was read: the check is the
        // write, and a row that changed meanwhile may say otherwise.
        self.write_over(&["status", "last_error", "oauth_required"]);
        self.save_if(db, |query| basis.push_conditions(query)).await
    }

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
            oauth_generation: self.oauth_generation,
        }
    }

    /// Name a new sign-in: the one that was started before is no longer this
    /// MCP's. The value is drawn rather than counted, so that two edits that
    /// each read the row and each clear it cannot end on the same one.
    pub fn renew_oauth_generation(&mut self) {
        self.oauth_generation = (rand::random::<u64>() >> 12) as i64;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(mcp: &Mcp) -> (McpStatus, Option<&str>, bool) {
        (mcp.status, mcp.last_error.as_deref(), mcp.oauth_required)
    }

    #[test]
    fn every_health_reads_back_as_it_was_set() {
        for health in [
            McpHealth::Untested,
            McpHealth::AwaitingSignIn,
            McpHealth::SignInRejected("token expired".into()),
            McpHealth::Ready,
            McpHealth::Failing("refused".into()),
        ] {
            let mut mcp = Mcp {
                status: McpStatus::Error,
                last_error: Some("stale".into()),
                oauth_required: true,
                ..Default::default()
            };
            mcp.set_health(health.clone());
            assert_eq!(mcp.health(), health);
            assert_eq!(mcp.status, health.status());
        }
    }

    #[test]
    fn a_probe_sets_the_three_columns_together() {
        let mut mcp = Mcp {
            status: McpStatus::Error,
            last_error: Some("stale".into()),
            oauth_required: true,
            ..Default::default()
        };
        mcp.mark_probed(ProbeOutcome::Reached);
        assert_eq!(columns(&mcp), (McpStatus::Ready, None, false));

        mcp.mark_probed(ProbeOutcome::SignInRefused {
            held_token: false,
            reason: "401".into(),
        });
        assert_eq!(
            columns(&mcp),
            (McpStatus::Draft, Some(AWAITING_SIGN_IN_ERROR), true)
        );
        assert!(mcp.health().awaits_sign_in());

        mcp.mark_probed(ProbeOutcome::SignInRefused {
            held_token: true,
            reason: "token rejected".into(),
        });
        assert_eq!(
            columns(&mcp),
            (McpStatus::Error, Some("token rejected"), true)
        );
        assert!(mcp.health().awaits_sign_in());
        assert!(!mcp.health().is_failing());

        mcp.mark_probed(ProbeOutcome::Failed("refused".into()));
        assert_eq!(columns(&mcp), (McpStatus::Error, Some("refused"), false));
        assert!(mcp.health().is_failing());

        mcp.mark_untested();
        assert_eq!(columns(&mcp), (McpStatus::Draft, None, false));
        assert_eq!(mcp.health(), McpHealth::Untested);
        assert!(mcp.health().needs_attention());
    }

    #[test]
    fn a_contradictory_row_reads_as_the_sign_in_it_waits_for() {
        // What forgetting the tokens of a ready MCP used to leave.
        let mcp = Mcp {
            status: McpStatus::Ready,
            oauth_required: true,
            ..Default::default()
        };
        assert_eq!(mcp.health(), McpHealth::AwaitingSignIn);
        assert!(mcp.health().needs_attention());
        let failing = Mcp {
            status: McpStatus::Error,
            last_error: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            failing.health(),
            McpHealth::Failing("Connection failed".into())
        );
    }

    #[test]
    fn signing_out_waits_for_a_sign_in_only_where_one_was_held() {
        let mut signed_in = Mcp {
            auth_type: McpAuthType::Auto,
            oauth_access_token: Some("access-ciphertext".into()),
            oauth_client_id: Some("client".into()),
            auth_bearer: Some("bearer-ciphertext".into()),
            status: McpStatus::Ready,
            ..Default::default()
        };
        let generation = signed_in.oauth_generation;
        signed_in.sign_out("moved");
        assert_eq!(
            signed_in.health(),
            McpHealth::SignInRejected("moved".into())
        );
        assert_eq!(signed_in.oauth_access_token, None);
        assert_eq!(signed_in.oauth_client_id, None);
        assert_eq!(signed_in.auth_bearer, None);
        assert_ne!(signed_in.oauth_generation, generation);

        let mut bearer = Mcp {
            auth_type: McpAuthType::Bearer,
            auth_bearer: Some("bearer-ciphertext".into()),
            status: McpStatus::Ready,
            ..Default::default()
        };
        bearer.sign_out("moved");
        assert_eq!(bearer.health(), McpHealth::Failing("moved".into()));
        assert_eq!(bearer.auth_bearer, None);
    }

    #[test]
    fn a_probe_basis_follows_what_the_mcp_connects_with() {
        let mcp = Mcp {
            http_url: Some("https://mcp.example/mcp".into()),
            oauth_access_token: Some("old".into()),
            ..Default::default()
        };
        let basis = mcp.probe_basis();

        let mut renamed = mcp.clone();
        renamed.name = "Renamed".into();
        renamed.enabled = !renamed.enabled;
        renamed.set_health(McpHealth::Ready);
        assert_eq!(renamed.probe_basis(), basis);

        for change in [
            (|mcp: &mut Mcp| mcp.http_url = Some("https://other.example/mcp".into()))
                as fn(&mut Mcp),
            |mcp| mcp.auth_bearer = Some("secret".into()),
            |mcp| mcp.npm_version = Some("2.0.0".into()),
            |mcp| mcp.tailnet_id = Some(7),
            |mcp| mcp.renew_oauth_generation(),
        ] {
            let mut changed = mcp.clone();
            change(&mut changed);
            assert_ne!(changed.probe_basis(), basis);
        }

        // The tokens a renewal saved during the test are those it used.
        let mut renewed = mcp.clone();
        renewed.oauth_access_token = Some("new".into());
        assert_ne!(renewed.probe_basis(), basis);
        assert_eq!(basis.after_renewal(&renewed), renewed.probe_basis());
    }
}
