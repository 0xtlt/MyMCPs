//! The Search Console API, called as the connected Google sign-in: requests,
//! what a refusal says, and which properties the MCP may use.

use std::time::Duration;

use http::Method;
use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinToolContext};
use mymcps_net::{FetchError, FetchRequest, FetchResponse, UpstreamResponseLimits};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::SITE_URLS_SETTING;
use crate::validators::FAILURE_VALIDATOR;

const SEARCH_CONSOLE_ORIGIN: &str = "https://searchconsole.googleapis.com";

/// A report over a large property, or the inspection of a page, takes Google a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Most of Google's own explanation that is passed on, in characters.
const MAX_MESSAGE_CHARS: usize = 500;

/// A property or a sitemap address as one segment of a path: `https://example.com/`
/// is `https%3A%2F%2Fexample.com%2F`.
pub fn path_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// The path of something that belongs to a property, such as
/// `/webmasters/v3/sites/sc-domain%3Aexample.com/sitemaps`.
pub fn site_path(site: &str, rest: &str) -> String {
    format!("/webmasters/v3/sites/{}{rest}", path_segment(site))
}

/// The `error` of a refusal, as `FAILURE_VALIDATOR` leaves it.
#[derive(Debug, Default, Deserialize)]
struct Failure {
    message: Option<String>,
    status: Option<String>,
    #[serde(default)]
    errors: Vec<Reason>,
    #[serde(default)]
    details: Vec<Reason>,
}

#[derive(Debug, Deserialize)]
struct FailureBody {
    error: Failure,
}

#[derive(Debug, Deserialize)]
struct Reason {
    reason: Option<String>,
}

impl Failure {
    fn has(&self, wanted: &[&str]) -> bool {
        let status = self.status.as_deref();
        let reasons = self
            .errors
            .iter()
            .chain(&self.details)
            .filter_map(|reason| reason.reason.as_deref());
        status
            .into_iter()
            .chain(reasons)
            .any(|reason| wanted.contains(&reason))
    }
}

/// What to do about a refusal that the setup, not the call, has to fix.
fn setup_hint(failure: &Failure, status: u16) -> Option<&'static str> {
    if failure.has(&["SERVICE_DISABLED", "accessNotConfigured"]) {
        return Some(
            "The Google Search Console API is not enabled in the Google Cloud project of the OAuth client. Enable it in the Google Cloud console, then try again in a few minutes.",
        );
    }
    if failure.has(&["ACCESS_TOKEN_SCOPE_INSUFFICIENT", "insufficientPermissions"]) {
        return Some(
            "The saved authorization does not allow this. For a write tool, an administrator must turn on write access from the MCPs page in MyMCPs, then re-authorize the MCP.",
        );
    }
    if failure.has(&[
        "RESOURCE_EXHAUSTED",
        "rateLimitExceeded",
        "quotaExceeded",
        "userRateLimitExceeded",
    ]) || status == 429
    {
        return Some(
            "Search Console quota reached for now. Wait a few minutes before trying again; URL inspections are limited to 2,000 a day for each property.",
        );
    }
    if failure.has(&["PERMISSION_DENIED", "forbidden"]) || status == 403 {
        return Some(
            "The connected Google account is not a user of this property, or not one with enough permission. Add it under Settings, Users and permissions, in Search Console, or use a property list_sites returns.",
        );
    }
    None
}

async fn search_console_failure(response: &mut FetchResponse) -> BuiltinError {
    let status = response.status().as_u16();
    // A body that is not JSON, or that was cut at its limit, explains nothing.
    let body: Option<Value> = response.json().await.ok();
    let failure = FAILURE_VALIDATOR
        .validate_as::<FailureBody>(body.as_ref())
        .map(|failure| failure.error)
        .unwrap_or_default();

    if status == 401 || failure.status.as_deref() == Some("UNAUTHENTICATED") {
        return BuiltinError::authorization(
            "Google rejected the saved authorization. Re-authorize this MCP in MyMCPs.",
        );
    }

    let message = failure
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(MAX_MESSAGE_CHARS).collect::<String>())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let hint = setup_hint(&failure, status)
        .map(|hint| format!(" {hint}"))
        .unwrap_or_default();
    BuiltinError::tool(format!(
        "Google Search Console refused the request: {message}{hint}"
    ))
}

/// How to call the Search Console API.
#[derive(Debug, Clone)]
pub struct SearchConsoleRequest {
    pub method: Method,
    /// Sent as JSON.
    pub body: Option<Value>,
    /// Whether a timeout may have left a change behind.
    pub changes: bool,
}

impl SearchConsoleRequest {
    pub fn get() -> Self {
        Self {
            method: Method::GET,
            body: None,
            changes: false,
        }
    }

    /// A `POST` of this JSON, which reads and changes nothing.
    pub fn post(body: Value) -> Self {
        Self {
            method: Method::POST,
            body: Some(body),
            changes: false,
        }
    }

    /// A request without a body that changes the property, such as a `PUT` or a `DELETE`.
    pub fn change(method: Method) -> Self {
        Self {
            method,
            body: None,
            changes: true,
        }
    }
}

/// Call the Search Console API as the connected Google sign-in. `path` starts
/// with a slash and may hold a query. An empty answer is `null`.
pub async fn search_console_request(
    context: &BuiltinToolContext,
    path: &str,
    request: SearchConsoleRequest,
) -> BuiltinResult<Value> {
    let SearchConsoleRequest {
        method,
        body,
        changes,
    } = request;
    let url =
        Url::parse(&format!("{SEARCH_CONSOLE_ORIGIN}{path}")).map_err(BuiltinError::internal)?;

    let mut fetch = FetchRequest::new(method, url)
        .header("Accept", "application/json")?
        .header("Authorization", &format!("Bearer {}", context.access_token))?;
    if let Some(body) = &body {
        fetch = fetch
            .header("Content-Type", "application/json")?
            .body(serde_json::to_string(body)?);
    }

    let sent = context.env.fetcher.fetch_with_same_origin_redirects(
        fetch.timeout(REQUEST_TIMEOUT),
        "Google Search Console API",
        UpstreamResponseLimits::default(),
    );
    let mut response = match sent.await {
        Err(FetchError::Timeout) => {
            return Err(BuiltinError::tool(if changes {
                "Google Search Console did not respond in time. The change may still have been applied, so check before retrying."
            } else {
                "Google Search Console did not respond in time. Try again, with a shorter period or a lower row_limit."
            }));
        }
        response => response?,
    };

    if !response.ok() {
        return Err(search_console_failure(&mut response).await);
    }
    let text = response.text().await?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&text)?)
}

/// The properties the MCP may use: every one the sign-in opens, unless the
/// administrator listed some. `None` when all are allowed.
pub fn allowed_sites(context: &BuiltinToolContext) -> Option<Vec<&str>> {
    let listed = context
        .settings
        .get(SITE_URLS_SETTING)
        .filter(|listed| !listed.is_empty())?;
    Some(listed.split(' ').collect())
}

/// The property a tool was asked to act on, once it is known to be one the
/// MCP may use. `site` is already written as Google writes it.
pub fn site_of<'a>(context: &BuiltinToolContext, site: &'a str) -> BuiltinResult<&'a str> {
    if let Some(allowed) = allowed_sites(context)
        && !allowed.contains(&site)
    {
        return Err(BuiltinError::tool(format!(
            "This MCP may not use the Search Console property {site}. It is limited to: {}. An administrator can change that from the MCPs page in MyMCPs.",
            allowed.join(", "),
        )));
    }
    Ok(site)
}
