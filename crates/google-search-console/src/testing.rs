//! A fake Search Console API for tests, behind the `test-util` feature.
//!
//! ```
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! use mymcps_builtin::BuiltinEnv;
//! use mymcps_core::TestCore;
//! use mymcps_google_search_console::testing::{FakeSearchConsole, SITE, search_console_context};
//! use serde_json::{Map, json};
//!
//! let google = FakeSearchConsole::new();
//! let core = TestCore::new().await;
//! let env = BuiltinEnv::new(core.core.clone()).with_fetcher(google.fetcher());
//! let context = search_console_context(env, &[]);
//!
//! let definition = mymcps_google_search_console::definition();
//! let mymcps_builtin::BuiltinMcpDefinition::Oauth { provider, .. } = &definition else { unreachable!() };
//! let mut arguments = Map::new();
//! arguments.insert("site_url".into(), json!(SITE));
//! let listed = provider.tool("list_sitemaps").unwrap().run(arguments, context).await.unwrap();
//!
//! assert_eq!(listed["sitemaps"][0]["path"], "https://www.example.com/sitemap.xml");
//! assert_eq!(google.requests()[0].url.path(), "/webmasters/v3/sites/sc-domain%3Aexample.com/sitemaps");
//! # });
//! ```

use std::sync::{Arc, Mutex, PoisonError};

use http::{HeaderMap, Method, StatusCode};
use mymcps_builtin::{BuiltinEnv, BuiltinToolContext};
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use serde_json::{Value, json};
use url::Url;

use crate::{READ_SCOPE, WRITE_SCOPE};

/// The domain property the default answers are about.
pub const SITE: &str = "sc-domain:example.com";
/// A URL-prefix property the connected account also opens.
pub const PREFIX_SITE: &str = "https://blog.example.com/";
pub const ACCESS_TOKEN: &str = "google-access-token";

const ORIGIN: &str = "https://searchconsole.googleapis.com";

/// A request the fake received.
#[derive(Debug, Clone)]
pub struct SearchConsoleTestRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub json: Option<Value>,
}

impl SearchConsoleTestRequest {
    fn of(request: &SentRequest) -> Self {
        let body = request.text();
        Self {
            method: request.method.clone(),
            url: request.url.clone(),
            headers: request.headers.clone(),
            json: (!body.is_empty()).then(|| serde_json::from_str(&body).unwrap_or(Value::Null)),
        }
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }
}

/// A JSON answer of Google.
pub fn google_json(body: &Value, status: u16) -> CannedResponse {
    CannedResponse::json(StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body)
}

/// A refusal as Google APIs word it. `reason` goes in both places Google puts
/// one: `errors` for the old reasons, `details` for the new ones.
pub fn google_failure(status: u16, code: &str, reason: &str, message: &str) -> CannedResponse {
    google_json(
        &json!({
            "error": {
                "code": status,
                "message": message,
                "status": code,
                "errors": [{ "message": message, "domain": "global", "reason": reason }],
                "details": [{ "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": reason, "domain": "googleapis.com" }],
            },
        }),
        status,
    )
}

/// Answers shaped like the real Search Console API's.
pub mod fixtures {
    use serde_json::{Value, json};

    use super::{PREFIX_SITE, SITE};

    pub fn sites() -> Value {
        json!({ "siteEntry": [
            { "siteUrl": SITE, "permissionLevel": "siteOwner" },
            { "siteUrl": PREFIX_SITE, "permissionLevel": "siteFullUser" },
        ] })
    }

    pub fn sitemap() -> Value {
        json!({
            "path": "https://www.example.com/sitemap.xml",
            "lastSubmitted": "2026-10-01T08:00:00.000Z",
            "isPending": false,
            "isSitemapsIndex": false,
            "type": "sitemap",
            "lastDownloaded": "2026-11-08T03:12:45.000Z",
            "warnings": "1",
            "errors": "0",
            "contents": [{ "type": "web", "submitted": "1234", "indexed": "0" }],
        })
    }

    pub fn rows() -> Value {
        json!({
            "rows": [
                { "keys": ["running shoes", "https://www.example.com/shoes"], "clicks": 120.0, "impressions": 4000.0, "ctr": 0.03, "position": 4.2567 },
                { "keys": ["trail shoes", "https://www.example.com/trail"], "clicks": 7.0, "impressions": 900.0, "ctr": 0.0077777, "position": 11.04 },
            ],
            "responseAggregationType": "byPage",
        })
    }

    pub fn inspection() -> Value {
        json!({ "inspectionResult": {
            "inspectionResultLink": "https://search.google.com/search-console/inspect?resource_id=sc-domain:example.com&id=abc",
            "indexStatusResult": {
                "verdict": "PASS",
                "coverageState": "Submitted and indexed",
                "robotsTxtState": "ALLOWED",
                "indexingState": "INDEXING_ALLOWED",
                "lastCrawlTime": "2026-11-07T10:00:00Z",
                "pageFetchState": "SUCCESSFUL",
                "googleCanonical": "https://www.example.com/shoes",
                "userCanonical": "https://www.example.com/shoes",
                "sitemap": ["https://www.example.com/sitemap.xml"],
                "referringUrls": ["https://www.example.com/"],
                "crawledAs": "MOBILE",
            },
            "mobileUsabilityResult": { "verdict": "VERDICT_UNSPECIFIED" },
            "richResultsResult": {
                "verdict": "PASS",
                "detectedItems": [
                    { "richResultType": "Product snippets", "items": [{ "name": "Shoe" }] },
                    { "richResultType": "Breadcrumbs", "items": [] },
                    { "richResultType": "Product snippets", "items": [] },
                ],
            },
        } })
    }
}

/// What Google answers when a test has nothing to say about a request.
fn default_answer(request: &SearchConsoleTestRequest) -> CannedResponse {
    let path = request.url.path();
    let method = &request.method;
    if path == "/webmasters/v3/sites" {
        return google_json(&fixtures::sites(), 200);
    }
    if path == "/v1/urlInspection/index:inspect" {
        return google_json(&fixtures::inspection(), 200);
    }
    if path.ends_with("/searchAnalytics/query") {
        return google_json(&fixtures::rows(), 200);
    }
    if path.ends_with("/sitemaps") {
        return google_json(&json!({ "sitemap": [fixtures::sitemap()] }), 200);
    }
    if path.contains("/sitemaps/") {
        return match *method {
            Method::GET => google_json(&fixtures::sitemap(), 200),
            _ => CannedResponse::new(StatusCode::NO_CONTENT),
        };
    }
    google_failure(404, "NOT_FOUND", "notFound", "Not found")
}

type Requests = Arc<Mutex<Vec<SearchConsoleTestRequest>>>;

/// A fake Search Console API. It answers the requests of the fetcher it hands
/// out, remembers them, and lets any other request fail as one the network
/// could not carry.
#[derive(Clone)]
pub struct FakeSearchConsole {
    requests: Requests,
    fetcher: Fetcher,
}

impl Default for FakeSearchConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSearchConsole {
    /// A Search Console that gives its default answers.
    pub fn new() -> Self {
        Self::responding(|_| None)
    }

    /// `respond` handles the cases a test cares about and returns `None` to
    /// fall back to the default answers.
    pub fn responding(
        respond: impl Fn(&SearchConsoleTestRequest) -> Option<CannedResponse> + Send + Sync + 'static,
    ) -> Self {
        let requests = Requests::default();
        let fetcher = Fetcher::offline().answering({
            let requests = requests.clone();
            move |sent| {
                if sent.url.origin().ascii_serialization() != ORIGIN {
                    return None;
                }
                let request = SearchConsoleTestRequest::of(sent);
                requests
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(request.clone());
                Some(respond(&request).unwrap_or_else(|| default_answer(&request)))
            }
        });
        Self { requests, fetcher }
    }

    /// What reaches this Google instead of the network: give it to `BuiltinEnv::with_fetcher`.
    pub fn fetcher(&self) -> Fetcher {
        self.fetcher.clone()
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<SearchConsoleTestRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What a tool of a connected Search Console MCP runs with, signed in with
/// write access. `settings` is what the admin entered, such as
/// `("siteUrls", "sc-domain:example.com")`.
pub fn search_console_context(
    env: BuiltinEnv,
    settings: &[(&str, &str)],
) -> Arc<BuiltinToolContext> {
    Arc::new(BuiltinToolContext {
        env,
        mcp_id: 1,
        access_token: ACCESS_TOKEN.to_owned(),
        granted_scopes: Some(vec![READ_SCOPE.to_owned(), WRITE_SCOPE.to_owned()]),
        settings: settings
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    })
}
