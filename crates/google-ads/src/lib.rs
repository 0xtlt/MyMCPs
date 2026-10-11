//! The built-in Google Ads MCP.
//!
//! Google has no MCP for Google Ads that a self-hosted gateway can sign in to,
//! so this one talks to the Google Ads API through an OAuth client the admin
//! creates in their own Google Cloud project. What the project may reach
//! (test accounts only, or production ones) is its access level at Google:
//! developer tokens, which used to carry it, were retired in September 2026.
//!
//! [`definition`] is what the gateway registers. `docs/google-ads.md` is the
//! guide for whoever sets the MCP up.
//!
//! | TypeScript | Rust |
//! |---|---|
//! | `app/services/builtin/google_ads/index.ts` | this file |
//! | `…/google_ads/api.ts` | [`api`] |
//! | `…/google_ads/format.ts` | [`mod@format`] |
//! | `…/google_ads/lookup.ts` | [`lookup`] |
//! | `…/google_ads/read_tools.ts` | [`read_tools`] |
//! | `…/google_ads/write_tools.ts` | [`write_tools`] |
//! | `…/google_ads/images.ts` | [`images`] |
//! | `app/validators/builtin_google_ads.ts` | [`validators`] |
//! | `tests/helpers/google_ads.ts` | `testing`, with the `test-util` feature |
//!
//! # Rows
//!
//! Google answers a report with rows of JSON, which the TypeScript reads as
//! `any`. The tools here read them through the private `js` module, which
//! does to a value what JavaScript does: `Number(x ?? 0)`, `String(x)`,
//! `x || null`, and no key at all for an `undefined`. A tool builds its
//! answer with `js::Object`, key by key in the order the TypeScript writes
//! them, so that it answers the same JSON.

use std::sync::{Arc, LazyLock};

use mymcps_builtin::{
    BuiltinMcpDefinition, BuiltinOauthConfig, BuiltinProvider, BuiltinSettingField,
    BuiltinToolContext,
};
use mymcps_vine as vine;

pub mod api;
pub mod format;
pub mod images;
mod js;
pub mod lookup;
pub mod read_tools;
#[cfg(feature = "test-util")]
pub mod testing;
pub mod validators;
pub mod write_tools;

use api::{GoogleAdsRequest, google_ads_request};
use format::customer_number;

const MAX_ALLOWED_ACCOUNTS: usize = 50;

fn pattern(expression: &str) -> regex::Regex {
    vine::js::regex(expression, "")
        .expect("static regex")
        .as_regex()
        .clone()
}

/// With or without its dashes, as Google Ads shows it.
static CUSTOMER_ID: LazyLock<vine::JsRegex> =
    LazyLock::new(|| vine::js::regex(r"^\d{3}-?\d{3}-?\d{4}$", "").expect("static regex"));

/// Stored as the IDs alone, separated by spaces. What is not an ID is
/// kept as written, for the pattern to refuse.
fn normalize_customer_ids(value: &str) -> String {
    value
        .split(|character: char| {
            vine::js::is_whitespace(character) || matches!(character, ',' | ';')
        })
        .filter(|id| !id.is_empty())
        .map(|id| {
            if CUSTOMER_ID.test(id) {
                customer_number(id)
            } else {
                id.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the admin enters besides the OAuth client: the manager account the
/// sign-in acts through, and the accounts agents may use.
fn settings() -> Vec<BuiltinSettingField> {
    vec![
        BuiltinSettingField {
            key: "loginCustomerId",
            required: false,
            pattern: pattern(r"^\d{10}$"),
            hint: "Enter the ID of the manager account, such as 123-456-7890".to_owned(),
            normalize: Some(customer_number),
        },
        BuiltinSettingField {
            key: "customerIds",
            required: false,
            pattern: pattern(&format!(
                r"^\d{{10}}( \d{{10}}){{0,{}}}$",
                MAX_ALLOWED_ACCOUNTS - 1
            )),
            hint: format!(
                "Enter up to {MAX_ALLOWED_ACCOUNTS} Google Ads account IDs, such as 123-456-7890, separated by commas"
            ),
            normalize: Some(normalize_customer_ids),
        },
    ]
}

/// The built-in Google Ads MCP: its sign-in, its settings and its tools.
/// (`googleAdsMcp`)
pub fn definition() -> BuiltinMcpDefinition {
    let mut tools = read_tools::google_ads_read_tools();
    tools.extend(write_tools::google_ads_write_tools());

    let provider = BuiltinProvider::new(
        "google-ads",
        "Google Ads",
        tools,
        |context: Arc<BuiltinToolContext>| async move {
            google_ads_request(
                &context,
                "/customers:listAccessibleCustomers",
                GoogleAdsRequest::get(),
            )
            .await?;
            Ok(())
        },
    )
    .settings(settings())
    .upload(write_tools::image_upload);

    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://accounts.google.com",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            // The one scope of the Google Ads API. It reads and writes: what agents
            // may change is decided in MyMCPs, by write access and tool approvals.
            scopes: vec!["https://www.googleapis.com/auth/adwords"],
            write_scopes: Vec::new(),
            scope_separator: " ",
            // Google only issues a refresh token for offline access, and only issues
            // one again to an account that already consented when asked to consent.
            authorize_params: vec![("access_type", "offline"), ("prompt", "consent")],
            sends_redirect_uri_with_code: true,
            client_id_pattern: Some(pattern(r"^[\w-]+\.apps\.googleusercontent\.com$")),
            client_id_hint: Some("The Google Client ID ends in .apps.googleusercontent.com"),
        },
    }
}
