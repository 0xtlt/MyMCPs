//! The stateless server endpoint, compared with the SDK.
//!
//! `fixtures/server.json` holds what `Server` + `StreamableHTTPServerTransport`
//! of `@modelcontextprotocol/sdk` 1.32.0 answered to each request, wired as
//! the TypeScript gateway wired them and with the handlers of [`Harness`].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode};
use mymcps_mcp::server::{
    HandlerError, RequestBody, ResponseBody, ServerOptions, StandaloneStream, ToolHandler,
    handle_request,
};
use mymcps_mcp::{Implementation, json};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    mode: String,
    request: FixtureRequest,
    expect: Expected,
}

#[derive(Deserialize)]
struct FixtureRequest {
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    #[serde(rename = "bodyPadding")]
    body_padding: Option<usize>,
    config: Config,
}

#[derive(Deserialize, Default)]
struct Config {
    capabilities: Option<Value>,
    instructions: Option<String>,
    #[serde(default, rename = "listFails")]
    list_fails: bool,
    #[serde(default, rename = "listSlow")]
    list_slow: bool,
}

#[derive(Deserialize)]
struct Expected {
    status: u16,
    headers: Map<String, Value>,
    body: String,
    /// The server left the stream open.
    open: bool,
}

/// The handlers of `gen_server_fixtures.mjs`, one tool name per behaviour.
#[derive(Default)]
struct Harness {
    list_fails: bool,
    list_slow: bool,
    calls: AtomicUsize,
}

fn text(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

#[async_trait]
impl ToolHandler for Harness {
    async fn list_tools(&self) -> Result<Value, HandlerError> {
        if self.list_fails {
            return Err(HandlerError::new("list failed"));
        }
        if self.list_slow {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok(json!({
            "tools": [
                {
                    "name": "a__echo",
                    "description": "Echo",
                    "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } },
                },
                { "inputSchema": { "type": "object" }, "name": "a__second", "vendor": { "kept": true } },
            ],
        }))
    }

    async fn call_tool(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, HandlerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match name.as_str() {
            "echo" => {
                let arguments = arguments.map_or(Value::Null, Value::Object);
                Ok(text(&json::to_string(
                    &json!({ "name": name, "arguments": arguments }),
                )))
            }
            "slow" => {
                tokio::time::sleep(Duration::from_millis(40)).await;
                Ok(text("slow"))
            }
            "slower" => {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok(text("slower"))
            }
            "minute" => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(text("minute"))
            }
            "throw" => Err(HandlerError::new("boom")),
            "throw-coded" => Err(HandlerError {
                code: Some(-32050),
                message: "refused".to_owned(),
                data: Some(json!({ "why": "policy" })),
            }),
            "throw-text-code" => Err(HandlerError::new("disk full")),
            "panic" => panic!("handler bug"),
            "invalid" => Ok(json!({ "content": [{ "type": "video" }] })),
            "not-an-object" => Ok(json!("text")),
            "no-content" => Ok(json!({ "structuredContent": { "b": 1, "a": 2 } })),
            "reshaped" => Ok(json!({
                "isError": true,
                "other": 1,
                "content": [
                    { "text": "x", "type": "text", "extra": 1 },
                    { "type": "image", "mimeType": "image/png", "data": "aGk=", "k": 1 },
                ],
                "_meta": { "z": 1 },
            })),
            _ => Ok(
                json!({ "content": [{ "type": "text", "text": "Invalid tool name" }], "isError": true }),
            ),
        }
    }
}

fn options() -> ServerOptions {
    ServerOptions::new(Implementation::new("mymcps", "0.4.1"))
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

const POST_HEADERS: &[(&str, &str)] = &[
    ("accept", "application/json, text/event-stream"),
    ("content-type", "application/json"),
];

async fn post(
    body: Value,
    options: &ServerOptions,
    handler: Arc<Harness>,
) -> Response<ResponseBody> {
    handle_request(
        &Method::POST,
        &headers(POST_HEADERS),
        RequestBody::Raw(Bytes::from(body.to_string())),
        options,
        handler,
    )
    .await
}

async fn read(body: ResponseBody) -> String {
    let chunks: Vec<_> = body.collect().await;
    let bytes: Vec<u8> = chunks
        .into_iter()
        .flat_map(|chunk| chunk.unwrap().to_vec())
        .collect();
    String::from_utf8(bytes).unwrap()
}

fn call(name: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name } })
}

#[tokio::test]
async fn answers_every_request_as_the_sdk_does() {
    let fixtures: Fixtures = serde_json::from_str(include_str!("fixtures/server.json")).unwrap();
    assert!(fixtures.cases.len() > 150);
    let mut failures = Vec::new();

    for case in &fixtures.cases {
        let request = &case.request;
        let mut options = options();
        if let Some(capabilities) = &request.config.capabilities {
            options.capabilities = capabilities.clone();
        }
        options.instructions = request.config.instructions.clone();
        let handler = Arc::new(Harness {
            list_fails: request.config.list_fails,
            list_slow: request.config.list_slow,
            ..Harness::default()
        });

        let pairs: Vec<(&str, &str)> = request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let mut request_headers = headers(&pairs);
        let text = match request.body_padding {
            // The one body too large to store: a ping padded past the limit.
            Some(length) => {
                let frame = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"pad":""}}"#;
                let (start, end) = frame.split_at(frame.len() - 3);
                format!("{start}{}{end}", "x".repeat(length - frame.len()))
            }
            None => request.body.clone().unwrap_or_default(),
        };
        if request.body.is_some() || request.body_padding.is_some() {
            request_headers.insert("content-length", HeaderValue::from(text.len()));
        }
        let body = match case.mode.as_str() {
            // The gateway handed over `{}` for an empty body.
            "parsed" if text.is_empty() => RequestBody::Parsed(json!({})),
            "parsed" => RequestBody::Parsed(serde_json::from_str(&text).unwrap()),
            _ => RequestBody::Raw(Bytes::from(text)),
        };

        let method = Method::from_bytes(request.method.as_bytes()).unwrap();
        let response = handle_request(&method, &request_headers, body, &options, handler).await;

        let mut differences = Vec::new();
        if response.status().as_u16() != case.expect.status {
            differences.push(format!(
                "status {} instead of {}",
                response.status(),
                case.expect.status
            ));
        }
        let event_stream = case
            .expect
            .headers
            .get("content-type")
            .and_then(Value::as_str)
            == Some("text/event-stream");
        for name in [
            "content-type",
            "cache-control",
            "x-accel-buffering",
            "allow",
            "mcp-session-id",
            "connection",
        ] {
            // On other answers `connection` is Node's own, not the SDK's.
            if name == "connection" && !event_stream {
                continue;
            }
            let got = response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok());
            let expected = case.expect.headers.get(name).and_then(Value::as_str);
            if got != expected {
                differences.push(format!("{name}: {got:?} instead of {expected:?}"));
            }
        }

        let body = response.into_body();
        if case.expect.open {
            // Open until the client leaves, with nothing to read yet.
            let mut body = body;
            let first = tokio::time::timeout(Duration::from_millis(50), body.next()).await;
            if first.is_ok() {
                differences.push("the stream ended or carried data".to_owned());
            }
        } else if method != Method::HEAD {
            let got = read(body).await;
            if got != case.expect.body {
                differences.push(format!(
                    "body\n    got:      {got:?}\n    expected: {:?}",
                    case.expect.body
                ));
            }
        }
        if !differences.is_empty() {
            failures.push(format!("{}:\n  {}", case.name, differences.join("\n  ")));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} cases differ from the SDK:\n{}",
        failures.len(),
        fixtures.cases.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn answers_without_a_handler_call_as_one_complete_body() {
    let handler = Arc::new(Harness::default());
    let response = post(
        json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
        &options(),
        handler.clone(),
    )
    .await;
    assert!(matches!(response.body(), ResponseBody::Full(_)));
    assert_eq!(
        read(response.into_body()).await,
        "event: message\ndata: {\"result\":{},\"jsonrpc\":\"2.0\",\"id\":2}\n\n"
    );

    let response = post(call("echo"), &options(), handler).await;
    assert!(matches!(response.body(), ResponseBody::Stream(_)));
}

#[tokio::test]
async fn initialize_carries_the_server_info_capabilities_and_instructions() {
    let mut options =
        ServerOptions::new(Implementation::new("mymcps", "9.9.9").with_title("MyMCPs"));
    options.instructions = Some("Use list_mcps first.".to_owned());
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "gateway-lazy-test", "version": "1.0.0" },
        },
    });
    let response = post(initialize, &options, Arc::new(Harness::default())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read(response.into_body()).await,
        concat!(
            "event: message\ndata: {\"result\":{\"protocolVersion\":\"2025-06-18\",",
            "\"capabilities\":{\"tools\":{}},",
            "\"serverInfo\":{\"name\":\"mymcps\",\"version\":\"9.9.9\",\"title\":\"MyMCPs\"},",
            "\"instructions\":\"Use list_mcps first.\"},\"jsonrpc\":\"2.0\",\"id\":1}\n\n"
        )
    );
}

#[tokio::test(start_paused = true)]
async fn keeps_a_slow_answer_alive_with_comments() {
    let response = post(call("minute"), &options(), Arc::new(Harness::default())).await;
    let mut body = response.into_body();
    let started = tokio::time::Instant::now();

    let mut frames = Vec::new();
    while let Some(frame) = body.next().await {
        frames.push((
            started.elapsed().as_secs(),
            String::from_utf8(frame.unwrap().to_vec()).unwrap(),
        ));
    }
    let (answer_at, answer) = frames.pop().unwrap();
    assert_eq!(answer_at, 60);
    assert!(answer.starts_with(
        "event: message\ndata: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"minute\"}]}"
    ));
    // Every 15 seconds; the one due with the answer may come just before it.
    let comments: Vec<_> = frames.iter().filter(|(at, _)| *at < 60).collect();
    assert_eq!(
        comments.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        vec![15, 30, 45]
    );
    assert!(frames.iter().all(|(_, frame)| frame == ": keepalive\n\n"));
}

#[tokio::test(start_paused = true)]
async fn sends_no_comments_when_keep_alive_is_off() {
    let mut options = options();
    options.keep_alive = None;
    let response = post(call("minute"), &options, Arc::new(Harness::default())).await;
    let frames: Vec<_> = response.into_body().collect().await;
    assert_eq!(frames.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn get_opens_a_stream_that_only_carries_comments() {
    let request_headers = headers(&[("accept", "text/event-stream")]);
    let response = handle_request(
        &Method::GET,
        &request_headers,
        RequestBody::Raw(Bytes::new()),
        &options(),
        Arc::new(Harness::default()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "no-cache, no-transform"
    );
    assert!(response.headers().get("mcp-session-id").is_none());

    let mut body = response.into_body();
    let started = tokio::time::Instant::now();
    for expected in [15, 30, 45] {
        assert_eq!(
            body.next().await.unwrap().unwrap(),
            Bytes::from_static(b": keepalive\n\n")
        );
        assert_eq!(started.elapsed().as_secs(), expected);
    }
}

#[tokio::test]
async fn get_can_be_switched_to_method_not_allowed() {
    let mut options = options();
    options.standalone_stream = StandaloneStream::MethodNotAllowed;
    let handler = Arc::new(Harness::default());
    let get = |pairs: &'static [(&'static str, &'static str)]| {
        let options = options.clone();
        let handler = handler.clone();
        async move {
            handle_request(
                &Method::GET,
                &headers(pairs),
                RequestBody::Raw(Bytes::new()),
                &options,
                handler,
            )
            .await
        }
    };

    let response = get(&[("accept", "text/event-stream")]).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers().get("allow").unwrap(), "POST");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(
        read(response.into_body()).await,
        r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"Method not allowed."},"id":null}"#
    );

    // What the SDK refuses before it would open the stream is still refused the same way.
    assert_eq!(get(&[]).await.status(), StatusCode::NOT_ACCEPTABLE);
    let response = get(&[
        ("accept", "text/event-stream"),
        ("mcp-protocol-version", "1999-01-01"),
    ])
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn finishes_a_call_whose_client_left() {
    let handler = Arc::new(Harness::default());
    let response = post(call("slow"), &options(), handler.clone()).await;
    drop(response);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn answers_a_handler_that_panics_with_an_internal_error() {
    let response = post(call("panic"), &options(), Arc::new(Harness::default())).await;
    assert_eq!(
        read(response.into_body()).await,
        "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}\n\n"
    );
}

#[tokio::test]
async fn accepts_repeated_accept_headers_as_one_list() {
    let request_headers = headers(&[
        ("accept", "application/json"),
        ("accept", "text/event-stream"),
        ("content-type", "application/json"),
    ]);
    let body = RequestBody::Raw(Bytes::from_static(
        br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    ));
    let response = handle_request(
        &Method::POST,
        &request_headers,
        body,
        &options(),
        Arc::new(Harness::default()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn applies_the_body_limit_to_raw_bodies_only() {
    let mut options = options();
    options.max_request_body_size = 64;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping", "params": { "pad": "x".repeat(100) } });

    let response = post(ping.clone(), &options, Arc::new(Harness::default())).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        read(response.into_body()).await,
        r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"Payload Too Large: Request body must not exceed 64 bytes"},"id":null}"#
    );

    let response = handle_request(
        &Method::POST,
        &headers(POST_HEADERS),
        RequestBody::Parsed(ping),
        &options,
        Arc::new(Harness::default()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}
