//! The built-in Google Search Console MCP.
//!
//! It reads how a site does in Google Search (clicks, impressions, positions),
//! its sitemaps, and how Google indexed one of its pages, from the
//! [Search Console API](https://developers.google.com/webmaster-tools/v1/api_reference_index).
//! It signs in like the Google Ads MCP, through an OAuth client the admin
//! creates in their own Google Cloud project, so one client can serve both.
//!
//! [`definition`] is what the gateway registers. `docs/google-search-console.md`
//! is the guide for whoever sets the MCP up.

use std::sync::Arc;

use mymcps_builtin::{
    BuiltinMcpDefinition, BuiltinOauthConfig, BuiltinProvider, BuiltinSettingField,
    BuiltinToolContext,
};
use mymcps_vine as vine;

pub mod api;
#[cfg(feature = "test-util")]
pub mod testing;
pub mod tools;
pub mod validators;

use api::{SearchConsoleRequest, search_console_request};
use validators::normalize_site_url;

/// The scope that reads every property the connected account can open.
pub const READ_SCOPE: &str = "https://www.googleapis.com/auth/webmasters.readonly";
/// The scope that also submits and deletes sitemaps. It covers reading too.
pub const WRITE_SCOPE: &str = "https://www.googleapis.com/auth/webmasters";

/// The key of the setting that lists the properties agents may use.
pub const SITE_URLS_SETTING: &str = "siteUrls";

const MAX_ALLOWED_SITES: usize = 50;

fn pattern(expression: &str) -> regex::Regex {
    vine::js::regex(expression, "")
        .expect("static regex")
        .as_regex()
        .clone()
}

/// Stored as the properties alone, separated by spaces, each written the way
/// Google writes it. What is not a property is kept as written, for the
/// pattern to refuse.
fn normalize_site_urls(value: &str) -> String {
    value
        .split(|character: char| {
            vine::js::is_whitespace(character) || matches!(character, ',' | ';')
        })
        .filter(|site| !site.is_empty())
        .map(|site| normalize_site_url(site).unwrap_or_else(|| site.to_owned()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The properties agents may use. Left blank, every property the sign-in opens.
fn settings() -> Vec<BuiltinSettingField> {
    let site = r"(sc-domain:[a-z0-9.-]+|https?://[^\s?#@]+/)";
    vec![BuiltinSettingField {
        key: SITE_URLS_SETTING,
        required: false,
        pattern: pattern(&format!("^{site}( {site}){{0,{}}}$", MAX_ALLOWED_SITES - 1)),
        hint: format!(
            "Enter up to {MAX_ALLOWED_SITES} Search Console properties, such as sc-domain:example.com or https://www.example.com/, separated by commas"
        ),
        normalize: Some(normalize_site_urls),
    }]
}

/// The built-in Google Search Console MCP: its sign-in, its setting and its tools.
pub fn definition() -> BuiltinMcpDefinition {
    let provider = BuiltinProvider::new(
        "google-search-console",
        "Google Search Console",
        tools::search_console_tools(),
        |context: Arc<BuiltinToolContext>| async move {
            search_console_request(
                &context,
                "/webmasters/v3/sites",
                SearchConsoleRequest::get(),
            )
            .await?;
            Ok(())
        },
    )
    .settings(settings());

    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://accounts.google.com",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scopes: vec![READ_SCOPE],
            write_scopes: vec![WRITE_SCOPE],
            scope_separator: " ",
            // Google only issues a refresh token for offline access, and only issues
            // one again to an account that already consented when asked to consent.
            authorize_params: vec![("access_type", "offline"), ("prompt", "consent")],
            sends_redirect_uri_with_code: true,
            client_id_pattern: Some(pattern(r"^[\w-]+\.apps\.googleusercontent\.com$")),
            client_id_hint: Some("The Google Client ID ends in .apps.googleusercontent.com"),
            // The same Google Cloud OAuth client as Google Ads, saved once on the
            // Credentials page and shared with the other Google MCPs of the team.
            credential_provider: Some("google"),
        },
    }
}
