//! What the tests of the MCPs page share: an upstream whose requests are
//! answered by a closure (as the TypeScript tests replaced `fetch`), name
//! resolution without the network, and a few helpers to read what a
//! response and the database hold. After `crates/upstream/tests/support`.
#![allow(dead_code)]

use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use http::{HeaderMap, StatusCode};
use mymcps_core::models::Mcp;
use mymcps_net::{AddressGuard, CannedResponse, Fetcher, Resolver, SentRequest};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::{Value, json};
use url::Url;

pub const CALLBACK: &str = "http://localhost:3333/mcps/oauth/callback";

/// Every name resolves to a public address: the mocked providers have no DNS
/// records, and no test may depend on the network.
struct EveryNameIsPublic;

#[async_trait::async_trait]
impl Resolver for EveryNameIsPublic {
    async fn lookup(&self, _hostname: &str) -> io::Result<Vec<IpAddr>> {
        Ok(vec!["203.0.113.10".parse().unwrap()])
    }
}

pub fn public_names() -> AddressGuard {
    AddressGuard::new(Arc::new(EveryNameIsPublic))
}

/// One request the gateway sent.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub url: String,
    pub body: String,
    pub headers: HeaderMap,
}

impl Call {
    fn of(request: &SentRequest) -> Self {
        Self {
            method: request.method.to_string(),
            url: request.url.to_string(),
            body: request.text(),
            headers: request.headers.clone(),
        }
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    }

    pub fn parsed(&self) -> Url {
        Url::parse(&self.url).unwrap()
    }

    pub fn origin(&self) -> String {
        self.parsed().origin().ascii_serialization()
    }

    pub fn hostname(&self) -> String {
        self.parsed().host_str().unwrap().to_owned()
    }

    pub fn path(&self) -> String {
        self.parsed().path().to_owned()
    }

    /// The body as JSON, `null` when it is not.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// A parameter of a form body.
    pub fn form(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    /// Every parameter of a form body, in the order sent.
    pub fn form_pairs(&self) -> Vec<(String, String)> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }
}

/// Every request sent so far, in order.
#[derive(Debug, Clone, Default)]
pub struct Calls(Arc<Mutex<Vec<Call>>>);

impl Calls {
    pub fn all(&self) -> Vec<Call> {
        self.0.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn with_path(&self, path: &str) -> Vec<Call> {
        self.all()
            .into_iter()
            .filter(|call| call.path() == path)
            .collect()
    }

    /// `METHOD url` of every request.
    pub fn lines(&self) -> Vec<String> {
        self.all()
            .iter()
            .map(|call| format!("{} {}", call.method, call.url))
            .collect()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// A fetcher that never reaches the network: `answer` is the response to
/// every request, which is recorded first.
pub fn mock_fetch(
    answer: impl Fn(&Call) -> CannedResponse + Send + Sync + 'static,
) -> (Fetcher, Calls) {
    let calls = Calls::default();
    let fetcher = Fetcher::offline().answering({
        let calls = calls.clone();
        move |request| {
            let call = Call::of(request);
            calls.0.lock().unwrap().push(call.clone());
            Some(answer(&call))
        }
    });
    (fetcher, calls)
}

pub fn json_response(body: Value) -> CannedResponse {
    CannedResponse::json(StatusCode::OK, &body)
}

pub fn json_status(status: u16, body: Value) -> CannedResponse {
    CannedResponse::json(StatusCode::from_u16(status).unwrap(), &body)
}

pub fn status(status: u16) -> CannedResponse {
    CannedResponse::new(StatusCode::from_u16(status).unwrap())
}

pub fn not_found() -> CannedResponse {
    status(404).body("not found")
}

/// What an MCP server without a stream of its own answers: the handshake,
/// then `tools` for `tools/list`.
pub fn mcp_answer(call: &Call, tools: &Value) -> CannedResponse {
    if call.method != "POST" {
        return status(405);
    }
    let message = call.json();
    let answer = |result: Value| {
        json_response(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }))
    };
    match message["method"].as_str() {
        Some("initialize") => answer(json!({
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "test", "version": "1.0.0" },
        })),
        Some("tools/list") => answer(json!({ "tools": tools })),
        _ => status(202),
    }
}

/// The metadata of a complete OAuth provider at `issuer`.
pub fn authorization_server(issuer: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
    })
}

pub fn registered_client(client_id: &str) -> Value {
    json!({
        "client_id": client_id,
        "redirect_uris": [CALLBACK],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}

/// The app around an upstream whose requests are answered by `answer`, with
/// every discovered name resolving to a public address.
pub async fn app_answering(
    answer: impl Fn(&Call) -> CannedResponse + Send + Sync + 'static,
) -> (TestApp, Calls) {
    let (fetcher, calls) = mock_fetch(answer);
    let app =
        TestApp::with_upstream(|upstream| upstream.fetcher(fetcher).address_guard(public_names()))
            .await;
    (app, calls)
}

/// The app around an upstream that answers every MCP request like a server
/// without tools, and knows no other endpoint.
pub async fn app_with_mcp_servers() -> (TestApp, Calls) {
    app_answering(|call| mcp_answer(call, &json!([]))).await
}

/// Send the request as the page's script sends an async form.
pub fn from_script(request: TestRequest<'_>) -> TestRequest<'_> {
    request
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
}

pub fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND, "{}", response.text());
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

/// The row as the database holds it now.
pub async fn find_mcp(app: &TestApp, id: i64) -> Mcp {
    Mcp::find(&*app.core.db, id).await.unwrap().unwrap()
}

pub async fn mcp_by_slug(app: &TestApp, slug: &str) -> Option<Mcp> {
    sqlx::query_as("select * from `mcps` where `slug` = ?")
        .bind(slug)
        .fetch_optional(&*app.core.db)
        .await
        .unwrap()
}

pub async fn count_mcps(app: &TestApp) -> i64 {
    sqlx::query_scalar("select count(*) from `mcps`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

pub fn decrypt(app: &TestApp, column: &Option<String>) -> Option<String> {
    app.core.decrypt_secret(column.as_deref())
}

pub fn encrypt(app: &TestApp, value: &str) -> Option<String> {
    app.core.encrypt_secret(Some(value))
}

/// The saved environment of an npm MCP, decrypted, in the order saved.
pub fn environment(app: &TestApp, mcp: &Mcp) -> Vec<(String, String)> {
    mcp.npm_environment(&app.core.encryption).unwrap()
}

/// A parameter of the query of a URL.
pub fn query(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The text between two markers of a page, to look at one part of it.
pub fn between<'a>(page: &'a str, start: &str, end: &str) -> &'a str {
    let from = page
        .find(start)
        .unwrap_or_else(|| panic!("{start} is not in the page"));
    let rest = &page[from..];
    let to = rest[start.len()..]
        .find(end)
        .map_or(rest.len(), |position| position + start.len());
    &rest[..to]
}
