//! The built-in Strava MCP: the athlete's profile, activities, segments,
//! routes, clubs and gear, read from the Strava API v3, and a few changes to
//! them once the admin allows write access.
//!
//! [`definition`] is what the registry of built-in MCPs takes. The modules
//! are what it is made of: the [`tools`], the [`validators`] of their
//! arguments and of what Strava answers, the [`api`] client, and how a
//! [`payload`] is shaped for an agent.

pub mod api;
pub mod payload;
pub mod tools;
pub mod validators;

use std::sync::Arc;

use mymcps_builtin::{
    BuiltinMcpDefinition, BuiltinOauthConfig, BuiltinProvider, BuiltinToolContext,
};
use mymcps_vine as vine;

use crate::api::{Params, strava_get};
use crate::tools::strava_tools;

/// Strava's own MCP (mcp.strava.com) only issues tokens to first-party
/// clients, so this one talks to the public API v3 through an API application
/// the admin creates at <https://www.strava.com/settings/api>.
pub fn definition() -> BuiltinMcpDefinition {
    let provider = BuiltinProvider::new(
        "strava",
        "Strava",
        strava_tools(),
        |context: Arc<BuiltinToolContext>| async move {
            strava_get(&context, "/athlete", Params::new()).await?;
            Ok(())
        },
    );
    let client_id_pattern = vine::js::regex(r"^\d+$", "").expect("static regex");

    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://www.strava.com",
            authorize_url: "https://www.strava.com/oauth/authorize",
            token_url: "https://www.strava.com/api/v3/oauth/token",
            // The `_all` scopes add activities and profile data that are visible to
            // Only You, which the athlete can still uncheck on Strava.
            scopes: vec!["read", "read_all", "profile:read_all", "activity:read_all"],
            write_scopes: vec!["activity:write", "profile:write"],
            scope_separator: ",",
            // Always show the consent screen so re-authorizing can restore a
            // permission that was unchecked the first time.
            authorize_params: vec![("approval_prompt", "force")],
            sends_redirect_uri_with_code: false,
            client_id_pattern: Some(client_id_pattern.as_regex().clone()),
            client_id_hint: Some("The Strava Client ID is a number, such as 123456"),
        },
    }
}
