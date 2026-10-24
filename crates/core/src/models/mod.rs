//! The tables of the database, one struct each.
//!
//! Load a row with `find` or with `sqlx::query_as::<_, Model>("select * ...")`,
//! change its fields, and `save` it: only the columns that changed are
//! written. `insert` stores a new row built with `..Default::default()`.

mod access_token;
mod approval_request;
mod instance_setting;
mod invite;
mod mcp;
mod mcp_call_log;
mod oauth;
mod passkey;
mod tailnet;
mod team;
mod two_factor;
mod user;

pub use access_token::{AccessToken, ScopeMode, TokenSource};
pub use approval_request::{ApprovalDecision, ApprovalRequest, ApprovalState};
pub use instance_setting::{GatewayToolMode, InstanceSetting, McpLogLevel};
pub use invite::Invite;
pub use mcp::{
    DEFAULT_TOOL_TIMEOUT_SECONDS, Mcp, McpAuthType, McpOauthBasis, McpStatus, McpTransport,
};
pub use mcp_call_log::{CallErrorCategory, CallOutcome, McpCallLog};
pub use oauth::{OauthAuthorizationCode, OauthClient};
pub use passkey::Passkey;
pub use tailnet::Tailnet;
pub use team::{AccountRemovalRefusal, Membership, Team, TeamMember, TeamRole};
pub use two_factor::{RECOVERY_CODES, RecoveryCode};
pub use user::User;

/// The cron expression npm MCPs are refreshed on until the admin sets another.
pub const DEFAULT_MCP_AUTO_UPDATE_CRON: &str = "0 2 * * *";
