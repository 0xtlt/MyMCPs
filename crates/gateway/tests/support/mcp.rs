//! A gateway with MCPs behind it that answer from a closure, as the
//! TypeScript tests replaced `fetch` (`tests/helpers/gateway.ts`), and the
//! requests an agent sends to `/mcp`.

use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use mymcps_builtin::BuiltinRegistry;
use mymcps_core::models::{ApprovalRequest, Mcp, McpStatus, McpTransport, User, UserRole};
use mymcps_core::{Config, Core, Db, TestCore, Timestamp};
use mymcps_gateway::approvals::Decision;
use mymcps_gateway::{Gateway, McpGateway, McpRequest};
use mymcps_net::{AddressGuard, CannedResponse, Fetcher, Resolver, SentRequest};
use mymcps_upstream::Upstream;
use serde_json::{Map, Value, json};

use super::{create_mcp, next_value};

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

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
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

/// Trim every string of a body and read the empty ones as null, at any
/// depth: what the web server does to a body before the gateway sees it.
pub fn normalize_body(value: &mut Value) {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                *value = Value::Null;
            } else if trimmed.len() != text.len() {
                *text = trimmed.to_owned();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_body),
        Value::Object(entries) => entries.values_mut().for_each(normalize_body),
        _ => {}
    }
}

/// What `/mcp` answered.
#[derive(Debug)]
pub struct McpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl McpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The JSON body of an answer that is not an event stream.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap()
    }

    /// The JSON-RPC message of the answer.
    pub fn rpc(&self) -> Value {
        let data = self
            .text
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .unwrap_or(&self.text);
        serde_json::from_str(data.trim()).unwrap()
    }

    /// The `result` of the JSON-RPC answer.
    pub fn result(&self) -> Value {
        self.rpc()["result"].clone()
    }
}

/// The text of a tool result, which is all that built-in tools and the
/// gateway return.
pub fn result_text(result: &Value) -> &str {
    result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A gateway on a fresh database, with MCPs behind it that a closure answers for.
pub struct TestMcpGateway {
    pub mcp_gateway: Arc<McpGateway>,
    pub gateway: Arc<Gateway>,
    pub upstream: Arc<Upstream>,
    pub core: Arc<Core>,
    pub upstreams: Upstreams,
    _test_core: TestCore,
}

impl std::ops::Deref for TestMcpGateway {
    type Target = McpGateway;

    fn deref(&self) -> &McpGateway {
        &self.mcp_gateway
    }
}

impl TestMcpGateway {
    /// A gateway whose MCPs answer `tools/list` and `tools/call` with `answer`.
    pub async fn new(answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static) -> Self {
        Self::with(
            |_| {},
            BuiltinRegistry::default(),
            |fetcher| fetcher,
            answer,
        )
        .await
    }

    /// A gateway that reaches no MCP at all.
    pub async fn offline() -> Self {
        Self::new(|_| Reply::Error("no MCP is expected to be reached".into())).await
    }

    /// A gateway with a configuration and built-in MCPs of the test's
    /// choosing. `layer` adds the answers of what is not an MCP server, such
    /// as the API of a built-in MCP, to the fetcher everything shares.
    pub async fn with(
        adjust: impl FnOnce(&mut Config),
        builtins: BuiltinRegistry,
        layer: impl FnOnce(Fetcher) -> Fetcher,
        answer: impl Fn(&UpstreamMessage) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let test_core = TestCore::with_config(adjust).await;
        let core = test_core.core.clone();
        let (fetcher, upstreams) = upstream_fetcher(answer);
        let upstream = Upstream::builder(core.clone(), builtins)
            .fetcher(layer(fetcher))
            .address_guard(AddressGuard::new(Arc::new(EveryNameIsPublic)))
            .build();
        let gateway = Arc::new(Gateway::new(core.clone()));
        Self {
            mcp_gateway: Arc::new(McpGateway::new(gateway.clone(), upstream.clone())),
            gateway,
            upstream,
            core,
            upstreams,
            _test_core: test_core,
        }
    }

    pub fn db(&self) -> &Db {
        &self.core.db
    }

    /// Send one request to `/mcp` and read its whole answer.
    pub async fn request(
        &self,
        method: Method,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> McpResponse {
        let mut header_map = HeaderMap::new();
        for (name, value) in headers {
            header_map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let mut body = body.unwrap_or_else(|| json!({}));
        normalize_body(&mut body);
        let caller_ip = header_map
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response = self
            .mcp_gateway
            .handle(McpRequest {
                method: &method,
                headers: &header_map,
                body,
                caller_ip: caller_ip.or_else(|| Some("127.0.0.1".to_owned())),
            })
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let chunks: Vec<_> = body.collect().await;
        let bytes: Vec<u8> = chunks
            .into_iter()
            .flat_map(|chunk| chunk.unwrap().to_vec())
            .collect();
        McpResponse {
            status: parts.status,
            headers: parts.headers,
            text: String::from_utf8(bytes).unwrap(),
        }
    }

    /// POST one JSON-RPC message as an agent would, with these headers on
    /// top of the ones every agent sends.
    pub async fn post(
        &self,
        plaintext: &str,
        message: Value,
        headers: &[(&str, &str)],
    ) -> McpResponse {
        let authorization = format!("Bearer {plaintext}");
        let mut all = vec![
            ("authorization", authorization.as_str()),
            ("accept", "application/json, text/event-stream"),
            ("content-type", "application/json"),
        ];
        all.extend_from_slice(headers);
        self.request(Method::POST, &all, Some(message)).await
    }

    /// Send one JSON-RPC request to the gateway as an agent would, and wait
    /// for its call log. Returns its `result`.
    pub async fn rpc(&self, plaintext: &str, method: &str, params: Value, mode: &str) -> Value {
        let response = self
            .post(
                plaintext,
                json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
                &[("x-mymcps-tool-mode", mode)],
            )
            .await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text);
        self.gateway.call_log.flush().await;
        response.result()
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

    /// The approval requests there are, the oldest first.
    pub async fn approval_requests(&self) -> Vec<ApprovalRequest> {
        sqlx::query_as("select * from `approval_requests` order by `id` asc")
            .fetch_all(&**self.db())
            .await
            .unwrap()
    }

    /// Decide the last request made for an MCP, as a person does on its page.
    pub async fn decide(&self, mcp: &Mcp, decision: Decision, user: &User) -> bool {
        let request: ApprovalRequest = sqlx::query_as(
            "select * from `approval_requests` where `mcp_id` = ? order by `id` desc limit 1",
        )
        .bind(mcp.id)
        .fetch_one(&**self.db())
        .await
        .unwrap();
        self.approvals
            .decide(&request, decision, user)
            .await
            .unwrap()
    }
}

pub async fn create_member(db: &Db) -> User {
    let mut user = User {
        full_name: Some("Test Member".into()),
        email: format!("{}@example.com", next_value("member")),
        password: "not-a-password-hash".into(),
        role: UserRole::Member,
        session_version: 1,
        ..Default::default()
    };
    user.insert(&**db).await.unwrap();
    user
}

/// An HTTP MCP at `https://{slug}.example/mcp`.
pub async fn create_http_mcp(db: &Db, created_by: i64, name: &str, slug: &str) -> Mcp {
    create_mcp(db, created_by, |mcp| {
        mcp.name = name.into();
        mcp.slug = slug.into();
        mcp.http_url = Some(format!("https://{slug}.example/mcp"));
    })
    .await
}

/// A built-in Strava MCP as the setup form leaves it, already authorized.
pub async fn create_strava_mcp(core: &Core, created_by: i64, write_enabled: bool) -> Mcp {
    let scopes = format!(
        "read read_all profile:read_all activity:read_all{}",
        if write_enabled {
            " activity:write profile:write"
        } else {
            ""
        }
    );
    create_mcp(&core.db, created_by, |mcp| {
        mcp.name = "Strava".into();
        mcp.transport = McpTransport::Builtin;
        mcp.http_url = None;
        mcp.builtin_key = Some("strava".into());
        mcp.status = McpStatus::Ready;
        mcp.oauth_required = false;
        mcp.builtin_write_enabled = write_enabled;
        mcp.oauth_client_id = Some("123456".into());
        mcp.oauth_client_secret = core.encrypt_secret(Some("strava-client-secret"));
        mcp.oauth_access_token = core.encrypt_secret(Some("strava-access-token"));
        mcp.oauth_refresh_token = core.encrypt_secret(Some("strava-refresh-token"));
        mcp.oauth_token_type = Some("Bearer".into());
        mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::hours(5));
        mcp.oauth_scopes = Some(scopes);
    })
    .await
}
