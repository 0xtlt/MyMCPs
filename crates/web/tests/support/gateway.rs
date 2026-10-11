//! What the tests of `/mcp` and of the approval pages share: the app with
//! MCPs behind it that answer from a closure, as the TypeScript tests
//! replaced `fetch` (`tests/helpers/gateway.ts`), the requests an agent
//! sends to `/mcp`, and the factories those tests need. After
//! `crates/gateway/tests/support`, with every request going through the
//! router.
#![allow(dead_code)]

use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use http::{HeaderMap, Method, StatusCode};
use mymcps_builtin::BuiltinEnv;
use mymcps_core::models::{
    AccessToken, ApprovalRequest, Mcp, McpCallLog, McpStatus, McpTransport, ScopeMode, User,
};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_core::{Config, Core, Db, Timestamp};
use mymcps_gateway::access_token::{self, CreatedAccessToken, NewAccessToken};
use mymcps_gateway::approvals::Decision;
use mymcps_net::{AddressGuard, CannedResponse, Fetcher, Resolver, SentRequest};
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use mymcps_web::testing::factories::{create_mcp, next_value};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::{Map, Value, json};

/// One request an MCP behind the gateway received.
#[derive(Debug, Clone)]
pub struct UpstreamMessage {
    pub host: String,
    pub url: String,
    pub http_method: String,
    /// The JSON-RPC method, or the HTTP method of a request without a message.
    pub method: String,
    pub id: Value,
    pub params: Value,
    pub headers: HeaderMap,
}

impl UpstreamMessage {
    /// The tool a `tools/call` names.
    pub fn tool(&self) -> &str {
        self.params["name"].as_str().unwrap_or_default()
    }
}

/// What an MCP answers a `tools/list` or a `tools/call` with.
pub enum Reply {
    Result(Value),
    /// A JSON-RPC error, which the gateway meets as a call that failed.
    Error(String),
}

/// Everything the MCPs behind the gateway were sent, in order.
#[derive(Debug, Clone, Default)]
pub struct Upstreams(Arc<Mutex<Vec<UpstreamMessage>>>);

impl Upstreams {
    pub fn requests(&self) -> Vec<UpstreamMessage> {
        self.0.lock().unwrap().clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.method)
            .collect()
    }

    pub fn hosts(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.host)
            .collect()
    }

    /// The `params` of every tool call: its name and its arguments.
    pub fn calls(&self) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == "tools/call")
            .map(|request| request.params)
            .collect()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// Every name resolves to a public address: the MCPs of the tests have no
/// DNS records, and no test may depend on the network.
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

/// A fetcher that reaches MCP servers which answer the handshake themselves
/// and leave `tools/list` and `tools/call` to `answer`.
pub fn upstream_fetcher(
    answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static,
) -> (Fetcher, Upstreams) {
    let upstreams = Upstreams::default();
    let fetcher = Fetcher::offline().answering({
        let upstreams = upstreams.clone();
        move |request: &SentRequest| {
            let body: Value = serde_json::from_str(&request.text()).unwrap_or(Value::Null);
            let message = UpstreamMessage {
                host: request.url.host_str().unwrap_or_default().to_owned(),
                url: request.url.to_string(),
                http_method: request.method.to_string(),
                method: body["method"]
                    .as_str()
                    .map_or_else(|| request.method.to_string(), str::to_owned),
                id: body["id"].clone(),
                params: body["params"].clone(),
                headers: request.headers.clone(),
            };
            upstreams.0.lock().unwrap().push(message.clone());

            if request.method != Method::POST {
                // No stream of its own, and no session to end.
                return Some(CannedResponse::new(StatusCode::METHOD_NOT_ALLOWED));
            }
            let reply = |result: Value| {
                CannedResponse::json(
                    StatusCode::OK,
                    &json!({ "jsonrpc": "2.0", "id": message.id, "result": result }),
                )
            };
            Some(match message.method.as_str() {
                "initialize" => reply(json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": message.host, "version": "1.0.0" },
                })),
                "tools/list" | "tools/call" => match answer(&message) {
                    Reply::Result(result) => reply(result),
                    Reply::Error(text) => CannedResponse::json(
                        StatusCode::OK,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": message.id,
                            "error": { "code": -32000, "message": text },
                        }),
                    ),
                },
                method if method.starts_with("notifications/") => {
                    CannedResponse::new(StatusCode::ACCEPTED)
                }
                _ => CannedResponse::new(StatusCode::NOT_FOUND).body("Not found"),
            })
        }
    });
    (fetcher, upstreams)
}

/// What `mockHttpMcp` answers: it lists `tools`, each taking any object, and
/// answers every call with the name of the tool that ran.
pub fn listing(tools: &[(&str, Option<&str>)]) -> impl Fn(&UpstreamMessage) -> Reply + use<> {
    let tools: Vec<Value> = tools
        .iter()
        .map(|(name, description)| {
            let mut tool = Map::new();
            tool.insert("name".into(), json!(name));
            if let Some(description) = description {
                tool.insert("description".into(), json!(description));
            }
            tool.insert("inputSchema".into(), json!({ "type": "object" }));
            Value::Object(tool)
        })
        .collect();
    move |message| {
        Reply::Result(match message.method.as_str() {
            "tools/list" => json!({ "tools": tools }),
            _ => json!({
                "content": [{ "type": "text", "text": format!("ran {}", message.tool()) }],
            }),
        })
    }
}

/// The JSON-RPC message of an answer of `/mcp`, from its event stream or
/// from its JSON body.
pub fn rpc_of(response: &TestResponse) -> Value {
    let text = response.text();
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .unwrap_or(&text);
    serde_json::from_str(data.trim()).unwrap_or_else(|_| panic!("not a JSON-RPC answer: {text}"))
}

/// The `result` of the JSON-RPC answer.
pub fn result_of(response: &TestResponse) -> Value {
    rpc_of(response)["result"].clone()
}

/// The text of a tool result, which is all that built-in tools and the
/// gateway return.
pub fn result_text(result: &Value) -> &str {
    result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The app on a fresh database, with MCPs behind its gateway that a closure
/// answers for.
pub struct TestGateway {
    pub app: TestApp,
    pub upstreams: Upstreams,
}

impl std::ops::Deref for TestGateway {
    type Target = TestApp;

    fn deref(&self) -> &TestApp {
        &self.app
    }
}

impl TestGateway {
    /// An app whose MCPs answer `tools/list` and `tools/call` with `answer`.
    pub async fn new(answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static) -> Self {
        Self::with(|_| {}, |fetcher| fetcher, answer).await
    }

    /// An app that reaches no MCP at all.
    pub async fn offline() -> Self {
        Self::new(|_| Reply::Error("no MCP is expected to be reached".into())).await
    }

    /// An app with a configuration of the test's choosing. `layer` adds the
    /// answers of what is not an MCP server, such as the API a built-in MCP
    /// calls, to the fetcher everything shares.
    pub async fn with(
        adjust: impl FnOnce(&mut Config),
        layer: impl FnOnce(Fetcher) -> Fetcher,
        answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static,
    ) -> Self {
        Self::build(adjust, layer, None::<fn(BuiltinEnv) -> BuiltinEnv>, answer).await
    }

    /// An app whose built-in MCPs find what `extend` adds to their
    /// environment: the mail servers of a test, for one.
    pub async fn with_builtin_env(
        adjust: impl FnOnce(&mut Config),
        extend: impl FnOnce(BuiltinEnv) -> BuiltinEnv,
    ) -> Self {
        Self::build(
            adjust,
            |fetcher| fetcher,
            Some(extend),
            |_| Reply::Error("no MCP is expected to be reached".into()),
        )
        .await
    }

    async fn build(
        adjust: impl FnOnce(&mut Config),
        layer: impl FnOnce(Fetcher) -> Fetcher,
        extend: Option<impl FnOnce(BuiltinEnv) -> BuiltinEnv>,
        answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let (fetcher, upstreams) = upstream_fetcher(answer);
        let fetcher = layer(fetcher);
        let app = TestApp::with_state(adjust, |core| {
            let mut upstream = Upstream::builder(core.clone(), mymcps_web::state::builtins())
                .fetcher(fetcher.clone())
                .address_guard(public_names());
            if let Some(extend) = extend {
                let env = BuiltinEnv::new(core.clone()).with_fetcher(fetcher);
                upstream = upstream.builtin_env(extend(env));
            }
            AppState::with_upstream(core, upstream.build())
        })
        .await;
        Self { app, upstreams }
    }

    pub fn db(&self) -> &Db {
        &self.app.core.db
    }

    pub fn core(&self) -> &Arc<Core> {
        &self.app.core
    }

    /// A request to `/mcp` as a program sends it: these headers and no other.
    pub fn mcp(&self, method: Method, headers: &[(&str, &str)]) -> TestRequest<'_> {
        headers.iter().fold(
            self.app.request(method, "/mcp").api(),
            |request, (name, value)| request.header(name, value),
        )
    }

    /// Send one request to `/mcp` and read its whole answer.
    pub async fn mcp_request(
        &self,
        method: Method,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> TestResponse {
        let request = self.mcp(method, headers);
        match body {
            Some(body) => request.json(body).send().await,
            None => request.send().await,
        }
    }

    /// POST one JSON-RPC message as an agent would, with these headers on
    /// top of the ones every agent sends.
    pub async fn post_message(
        &self,
        plaintext: &str,
        message: Value,
        headers: &[(&str, &str)],
    ) -> TestResponse {
        let authorization = format!("Bearer {plaintext}");
        let mut all = vec![
            ("authorization", authorization.as_str()),
            ("accept", "application/json, text/event-stream"),
        ];
        all.extend_from_slice(headers);
        self.mcp_request(Method::POST, &all, Some(message)).await
    }

    /// POST one JSON-RPC message as an agent would, to a path of its own:
    /// `/mcp` with a query string.
    pub async fn post_to(&self, path: &str, plaintext: &str, message: Value) -> TestResponse {
        self.app
            .post(path)
            .api()
            .header("authorization", &format!("Bearer {plaintext}"))
            .header("accept", "application/json, text/event-stream")
            .json(message)
            .send()
            .await
    }

    /// Wait for the call log to hold every call made so far.
    pub async fn flush(&self) {
        self.app.state.gateway.call_log.flush().await;
    }

    /// Send one JSON-RPC request to the gateway as an agent would, and wait
    /// for its call log. Returns its `result`.
    pub async fn rpc(&self, plaintext: &str, method: &str, params: Value, mode: &str) -> Value {
        let response = self
            .post_message(
                plaintext,
                json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
                &[("x-mymcps-tool-mode", mode)],
            )
            .await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        self.flush().await;
        result_of(&response)
    }

    /// Call a tool of the eager gateway, and return the result of the call.
    pub async fn call(&self, plaintext: &str, name: &str, arguments: Value) -> Value {
        self.rpc(
            plaintext,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            "eager",
        )
        .await
    }

    /// Call a tool of an MCP through `call_tool` of the lazy gateway.
    pub async fn call_lazily(
        &self,
        plaintext: &str,
        mcp: &str,
        tool: &str,
        arguments: Value,
    ) -> Value {
        self.rpc(
            plaintext,
            "tools/call",
            json!({
                "name": "call_tool",
                "arguments": { "mcp": mcp, "tool": tool, "arguments": arguments },
            }),
            "lazy",
        )
        .await
    }

    /// The call log, the oldest record first.
    pub async fn call_logs(&self) -> Vec<McpCallLog> {
        sqlx::query_as("select * from `mcp_call_logs` order by `id` asc")
            .fetch_all(&**self.db())
            .await
            .unwrap()
    }

    /// The approval requests there are, the oldest first.
    pub async fn approval_requests(&self) -> Vec<ApprovalRequest> {
        sqlx::query_as("select * from `approval_requests` order by `id` asc")
            .fetch_all(&**self.db())
            .await
            .unwrap()
    }

    /// The last approval request made for an MCP.
    pub async fn last_request(&self, mcp: &Mcp) -> ApprovalRequest {
        sqlx::query_as(
            "select * from `approval_requests` where `mcp_id` = ? order by `id` desc limit 1",
        )
        .bind(mcp.id)
        .fetch_one(&**self.db())
        .await
        .unwrap()
    }

    /// Decide the last request made for an MCP, without going through its page.
    pub async fn decide(&self, mcp: &Mcp, decision: Decision, user: &User) -> bool {
        let request = self.last_request(mcp).await;
        self.app
            .state
            .mcp_gateway
            .approvals
            .decide(&request, decision, user)
            .await
            .unwrap()
    }
}

pub async fn create_access_token(
    app: &TestApp,
    created_by: i64,
    scope_mode: ScopeMode,
    mcp_ids: &[i64],
) -> CreatedAccessToken {
    create_named_access_token(app, created_by, &next_value("token"), scope_mode, mcp_ids).await
}

pub async fn create_named_access_token(
    app: &TestApp,
    created_by: i64,
    name: &str,
    scope_mode: ScopeMode,
    mcp_ids: &[i64],
) -> CreatedAccessToken {
    access_token::create(
        &app.core.db,
        NewAccessToken {
            name,
            scope_mode,
            mcp_ids,
            expires_at: None,
            created_by,
        },
    )
    .await
    .unwrap()
}

/// A token row whose secret is `plaintext`, to make expired and revoked
/// ones. Change it with `adjust`.
pub async fn create_token_with_secret(
    app: &TestApp,
    created_by: i64,
    plaintext: &str,
    adjust: impl FnOnce(&mut AccessToken),
) -> AccessToken {
    let mut token = AccessToken {
        name: next_value("stored-token"),
        token_hash: access_token::hash(plaintext),
        token_prefix: "mcp_stored".into(),
        scope_mode: ScopeMode::All,
        created_by,
        ..Default::default()
    };
    adjust(&mut token);
    token.insert(&*app.core.db).await.unwrap();
    token
}

/// An HTTP MCP at `https://{slug}.example/mcp`.
pub async fn create_http_mcp(app: &TestApp, created_by: i64, name: &str, slug: &str) -> Mcp {
    create_mcp(app, created_by, |mcp| {
        mcp.name = name.into();
        mcp.slug = slug.into();
        mcp.http_url = Some(format!("https://{slug}.example/mcp"));
    })
    .await
}

/// A built-in Strava MCP as the setup form leaves it. `connected` is false
/// before its account was authorized.
pub async fn create_strava_mcp(
    app: &TestApp,
    created_by: i64,
    write_enabled: bool,
    connected: bool,
) -> Mcp {
    let core = &app.core;
    let scopes = format!(
        "read read_all profile:read_all activity:read_all{}",
        if write_enabled {
            " activity:write profile:write"
        } else {
            ""
        }
    );
    create_mcp(app, created_by, |mcp| {
        mcp.name = "Strava".into();
        mcp.slug = "strava".into();
        mcp.transport = McpTransport::Builtin;
        mcp.http_url = None;
        mcp.builtin_key = Some("strava".into());
        mcp.builtin_write_enabled = write_enabled;
        mcp.oauth_client_id = Some("123456".into());
        mcp.oauth_client_secret = core.encrypt_secret(Some("strava-client-secret"));
        if connected {
            mcp.status = McpStatus::Ready;
            mcp.oauth_required = false;
            mcp.oauth_access_token = core.encrypt_secret(Some("strava-access-token"));
            mcp.oauth_refresh_token = core.encrypt_secret(Some("strava-refresh-token"));
            mcp.oauth_token_type = Some("Bearer".into());
            mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::hours(5));
            mcp.oauth_scopes = Some(scopes);
        } else {
            mcp.status = McpStatus::Draft;
            mcp.oauth_required = true;
        }
    })
    .await
}

/// A built-in Google Ads MCP as the setup form leaves it, already authorized
/// unless `connected` is false. `customer_ids` are the accounts agents may
/// use, as digits.
pub async fn create_google_ads_mcp(
    app: &TestApp,
    created_by: i64,
    connected: bool,
    write_enabled: bool,
    customer_ids: &[&str],
) -> Mcp {
    let core = &app.core;
    let settings: Vec<EnvironmentInput> = (!customer_ids.is_empty())
        .then(|| EnvironmentInput {
            name: "customerIds".into(),
            value: Some(customer_ids.join(" ")),
        })
        .into_iter()
        .collect();
    create_mcp(app, created_by, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.slug = "google-ads".into();
        mcp.transport = McpTransport::Builtin;
        mcp.http_url = None;
        mcp.builtin_key = Some("google-ads".into());
        mcp.status = if connected {
            McpStatus::Ready
        } else {
            McpStatus::Draft
        };
        mcp.oauth_required = !connected;
        mcp.builtin_write_enabled = write_enabled;
        mcp.oauth_client_id = Some("1234567890-abc.apps.googleusercontent.com".into());
        mcp.oauth_client_secret = core.encrypt_secret(Some("google-client-secret"));
        mcp.builtin_settings = merge_environment(&core.encryption, None, &settings);
        if connected {
            mcp.oauth_access_token = core.encrypt_secret(Some("google-access-token"));
            mcp.oauth_refresh_token = core.encrypt_secret(Some("google-refresh-token"));
            mcp.oauth_token_type = Some("Bearer".into());
            mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::minutes(50));
            mcp.oauth_scopes = Some("https://www.googleapis.com/auth/adwords".into());
        }
    })
    .await
}

/// The row as the database holds it now.
pub async fn find_mcp(app: &TestApp, id: i64) -> Mcp {
    Mcp::find(&*app.core.db, id).await.unwrap().unwrap()
}

pub async fn find_request(app: &TestApp, id: i64) -> ApprovalRequest {
    ApprovalRequest::find(&*app.core.db, id)
        .await
        .unwrap()
        .unwrap()
}

/// The answer to a link a tool handed out, followed as anyone holding it
/// would: no session, no access token.
pub async fn follow(app: &TestApp, link: &str) -> TestResponse {
    app.get(path_of(link)).api().send().await
}

/// The path and query of a link to this instance.
pub fn path_of(link: &str) -> &str {
    link.strip_prefix("http://localhost:3333")
        .unwrap_or_else(|| panic!("not a link to this instance: {link}"))
}

/// The JSON a built-in tool answered with, which is the text of its result.
pub fn result_json(result: &Value) -> Value {
    serde_json::from_str(result_text(result))
        .unwrap_or_else(|_| panic!("not JSON: {}", result_text(result)))
}

/// The text of a piece of markup: its tags removed, its entities read, and
/// each run of white space (the inlined icons hold some) made one space.
pub fn text_of(markup: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in markup.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The text between the first `start` and the next `end` after it.
pub fn between<'a>(page: &'a str, start: &str, end: &str) -> &'a str {
    let from = page
        .find(start)
        .unwrap_or_else(|| panic!("{start:?} is not in the page"))
        + start.len();
    let length = page[from..]
        .find(end)
        .unwrap_or_else(|| panic!("{end:?} does not follow {start:?}"));
    &page[from..from + length]
}

/// Send a request and hand back its answer with the body unread: for an
/// event stream that only the client ends.
pub async fn send_unread(
    app: &TestApp,
    mut request: http::Request<axum::body::Body>,
) -> http::Response<axum::body::Body> {
    use tower::ServiceExt;

    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));
    mymcps_web::app::service(app.state.clone())
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {})
}
