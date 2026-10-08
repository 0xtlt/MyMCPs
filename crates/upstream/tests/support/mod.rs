//! What the tests of this crate share: a session in memory, name resolution
//! without the network, providers and MCP servers that answer from a closure
//! (as the TypeScript tests replaced `fetch`), and rows to test with.
#![allow(dead_code)]

pub mod providers;

use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use http::{HeaderMap, StatusCode};
use mymcps_builtin::BuiltinRegistry;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport, User, UserRole};
use mymcps_core::{Core, TestCore};
use mymcps_net::{AddressGuard, CannedResponse, Fetcher, Resolver, SentRequest, StaticResolver};
use mymcps_upstream::{OauthSessionStore, Upstream};
use serde_json::{Map, Value, json};
use url::Url;

pub const CALLBACK: &str = "http://localhost:3333/mcps/oauth/callback";

/// The browser session of a test: values by key, the oldest first.
#[derive(Debug, Default)]
pub struct MemorySession {
    values: Mutex<Map<String, Value>>,
}

impl MemorySession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, key: &str, value: Value) {
        self.values.lock().unwrap().insert(key.to_owned(), value);
    }

    pub fn value(&self, key: &str) -> Option<Value> {
        self.values.lock().unwrap().get(key).cloned()
    }

    pub fn has(&self, key: &str) -> bool {
        self.values.lock().unwrap().contains_key(key)
    }

    /// The keys of the pending authorizations, the oldest first.
    pub fn pending(&self) -> Vec<String> {
        self.keys()
            .into_iter()
            .filter(|key| key.starts_with("mcp_oauth:"))
            .collect()
    }

    /// What the pending authorizations weigh in the session.
    pub fn pending_bytes(&self) -> usize {
        self.values
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with("mcp_oauth:"))
            .map(|(key, value)| key.len() + value.to_string().len())
            .sum()
    }
}

impl OauthSessionStore for MemorySession {
    fn get(&self, key: &str) -> Option<Value> {
        self.value(key)
    }

    fn put(&self, key: &str, value: Value) {
        self.set(key, value);
    }

    fn forget(&self, key: &str) {
        self.values.lock().unwrap().shift_remove(key);
    }

    fn keys(&self) -> Vec<String> {
        self.values.lock().unwrap().keys().cloned().collect()
    }
}

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

/// Name resolution without the network: only the names a test declares resolve.
pub fn resolve_names(names: &[(&str, &[&str])]) -> (AddressGuard, Arc<StaticResolver>) {
    let mut resolver = StaticResolver::new();
    for (name, addresses) in names {
        let addresses: Vec<IpAddr> = addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        resolver = resolver.with(name, &addresses);
    }
    let resolver = Arc::new(resolver);
    (AddressGuard::new(resolver.clone()), resolver)
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

    /// The host, with its port when the URL names one.
    pub fn host(&self) -> String {
        let url = self.parsed();
        match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap()),
            None => url.host_str().unwrap().to_owned(),
        }
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

    pub fn to(&self, url: &str) -> Vec<Call> {
        self.all()
            .into_iter()
            .filter(|call| call.url == url)
            .collect()
    }

    pub fn with_path(&self, path: &str) -> Vec<Call> {
        self.all()
            .into_iter()
            .filter(|call| call.path() == path)
            .collect()
    }

    pub fn posts(&self) -> Vec<Call> {
        self.all()
            .into_iter()
            .filter(|call| call.method == "POST")
            .collect()
    }

    pub fn hosts(&self) -> Vec<String> {
        self.all().iter().map(Call::host).collect()
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
/// then `tools` for `tools/list` and `result` for a call.
pub fn mcp_answer(call: &Call, tools: &Value, result: &Value) -> CannedResponse {
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
            "serverInfo": { "name": "test", "version": "1.0" },
        })),
        Some("tools/list") => answer(json!({ "tools": tools })),
        Some("tools/call") => answer(result.clone()),
        _ => status(202),
    }
}

/// The metadata of a complete provider at `issuer`.
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

/// `authorization_server`, with some members replaced.
pub fn authorization_server_with(issuer: &str, overrides: Value) -> Value {
    let mut metadata = authorization_server(issuer);
    for (name, value) in overrides.as_object().unwrap() {
        metadata[name] = value.clone();
    }
    metadata
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

/// An upstream on the database of `core` whose requests are answered by
/// `answer`, with every discovered name resolving to a public address.
pub fn upstream(
    core: &TestCore,
    answer: impl Fn(&Call) -> CannedResponse + Send + Sync + 'static,
) -> (Arc<Upstream>, Calls) {
    upstream_with(core, public_names(), BuiltinRegistry::default(), answer)
}

pub fn upstream_with(
    core: &TestCore,
    addresses: AddressGuard,
    builtins: BuiltinRegistry,
    answer: impl Fn(&Call) -> CannedResponse + Send + Sync + 'static,
) -> (Arc<Upstream>, Calls) {
    let (fetcher, calls) = mock_fetch(answer);
    let upstream = Upstream::builder(core.core.clone(), builtins)
        .fetcher(fetcher)
        .address_guard(addresses)
        .build();
    (upstream, calls)
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_value(prefix: &str) -> String {
    format!("{prefix}-{}", SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1)
}

pub async fn create_admin(core: &Core) -> i64 {
    let mut admin = User {
        full_name: Some("Test User".to_owned()),
        email: format!("{}@example.com", next_value("user")),
        password: "not a hash: nobody signs in here".to_owned(),
        role: UserRole::Admin,
        session_version: 1,
        ..Default::default()
    };
    admin.insert(&*core.db).await.unwrap();
    admin.id
}

/// A saved MCP: an HTTP one with automatic authentication, ready and
/// enabled, unless `adjust` says otherwise.
pub async fn create_mcp(core: &Core, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    let mut mcp = Mcp {
        name: next_value("MCP"),
        transport: McpTransport::Http,
        auth_type: McpAuthType::Auto,
        status: McpStatus::Ready,
        enabled: true,
        created_by: create_admin(core).await,
        ..Default::default()
    };
    adjust(&mut mcp);
    if mcp.slug.is_empty() {
        mcp.slug = Mcp::slugify(&mcp.name);
    }
    match mcp.transport {
        McpTransport::Http if mcp.http_url.is_none() => {
            mcp.http_url = Some("http://127.0.0.1:9999/mcp".to_owned());
        }
        McpTransport::Npm if mcp.npm_package.is_none() => {
            mcp.npm_package = Some("@example/mcp".to_owned());
        }
        _ => {}
    }
    mcp.insert(&*core.db).await.unwrap();
    mcp
}

/// The row as the database holds it now.
pub async fn find_mcp(core: &Core, id: i64) -> Mcp {
    Mcp::find(&*core.db, id).await.unwrap().unwrap()
}

pub fn decrypt(core: &Core, column: &Option<String>) -> Option<String> {
    core.decrypt_secret(column.as_deref())
}

pub fn encrypt(core: &Core, value: &str) -> Option<String> {
    core.encrypt_secret(Some(value))
}

/// A parameter of the query of a URL.
pub fn query(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

pub fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// One request a local server received.
#[derive(Debug, Clone)]
pub struct Received {
    pub method: String,
    /// Path and query, as on the request line.
    pub target: String,
    pub headers: HeaderMap,
    pub body: String,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    pub fn form(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
}

/// A server on 127.0.0.1, for what a canned answer cannot do: an answer that
/// waits, or a body that arrives in pieces.
pub struct LocalServer {
    address: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Received>>>,
    server: tokio::task::JoinHandle<()>,
}

impl LocalServer {
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin())
    }

    pub fn requests(&self) -> Vec<Received> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Start a server that answers every request with `answer`.
pub async fn local_server<F, Fut>(answer: F) -> LocalServer
where
    F: Fn(Received) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = axum::response::Response> + Send + 'static,
{
    let requests: Arc<Mutex<Vec<Received>>> = Arc::default();
    let handler = {
        let requests = Arc::clone(&requests);
        move |request: axum::extract::Request| {
            let requests = Arc::clone(&requests);
            let answer = answer.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let received = Received {
                    method: parts.method.to_string(),
                    target: parts
                        .uri
                        .path_and_query()
                        .map(|target| target.as_str().to_owned())
                        .unwrap_or_default(),
                    headers: parts.headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                requests.lock().unwrap().push(received.clone());
                answer(received).await
            }
        }
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().fallback(handler))
            .await
            .unwrap();
    });
    LocalServer {
        address,
        requests,
        server,
    }
}

pub fn respond(
    status: u16,
    headers: &[(&str, &str)],
    body: impl Into<axum::body::Body>,
) -> axum::response::Response {
    let mut response = axum::response::Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(*name, *value);
    }
    response.body(body.into()).unwrap()
}

pub fn respond_json(status: u16, body: Value) -> axum::response::Response {
    respond(
        status,
        &[("Content-Type", "application/json")],
        body.to_string(),
    )
}

/// Wait, for a few seconds at most, until `condition` holds.
pub async fn eventually(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition did not hold in time");
}
