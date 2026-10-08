//! The client over Streamable HTTP, compared with the SDK.
//!
//! `fixtures/client_http.json` holds scenarios: the rules a scripted MCP
//! server answers by, the steps a client runs, and what `Client` +
//! `StreamableHTTPClientTransport` of `@modelcontextprotocol/sdk` 1.32.0 sent
//! and returned. The same rules are played here by a local server.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use mymcps_mcp::client::{
    BoxError, Client, ClientError, ClientOptions, HttpRequest, HttpResponse, HttpSend,
    StreamableHttpClientTransport, StreamableHttpClientTransportOptions, Transport, TransportError,
};
use mymcps_mcp::{Implementation, McpError, json};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use url::Url;

#[derive(Deserialize)]
struct Fixtures {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    client: Option<ClientInfo>,
    #[serde(default)]
    headers: Map<String, Value>,
    #[serde(rename = "timeoutMs")]
    timeout_ms: Option<u64>,
    #[serde(default)]
    unordered: bool,
    rules: Vec<Rule>,
    steps: Vec<String>,
    expect: Expected,
}

#[derive(Deserialize)]
struct ClientInfo {
    name: String,
    version: String,
    title: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Rule {
    when: When,
    times: Option<usize>,
    #[serde(default)]
    never: bool,
    status: Option<u16>,
    #[serde(default)]
    headers: Map<String, Value>,
    body: Option<String>,
    chunks: Option<Vec<String>>,
    #[serde(default, rename = "chunkDelayMs")]
    chunk_delay_ms: u64,
    #[serde(default)]
    hold: bool,
}

#[derive(Deserialize, Clone, Default)]
struct When {
    http: Option<String>,
    header: Option<String>,
    rpc: Option<String>,
    #[serde(default)]
    notification: bool,
    #[serde(default)]
    response: bool,
}

#[derive(Deserialize)]
struct Expected {
    requests: Vec<Seen>,
    outcomes: Vec<Outcome>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
struct Seen {
    method: String,
    headers: Map<String, Value>,
    body: String,
}

#[derive(Deserialize)]
struct Outcome {
    step: String,
    ok: Option<String>,
    error: Option<ExpectedError>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct ExpectedError {
    kind: String,
    message: String,
    code: Option<i64>,
    data: Option<Value>,
}

const KEPT_HEADERS: &[&str] = &[
    "mcp-session-id",
    "mcp-protocol-version",
    "authorization",
    "content-type",
    "accept",
    "last-event-id",
    "x-custom",
];

struct Script {
    rules: Vec<Rule>,
    used: Vec<AtomicUsize>,
    keeps_user_agent: bool,
    seen: Mutex<Vec<Seen>>,
}

impl When {
    fn matches(&self, method: &str, headers: &HeaderMap, message: Option<&Value>) -> bool {
        if self.http.as_deref().is_some_and(|http| http != method) {
            return false;
        }
        if self
            .header
            .as_deref()
            .is_some_and(|name| !headers.contains_key(name))
        {
            return false;
        }
        let rpc_method = message.and_then(|message| message.get("method"));
        if self
            .rpc
            .as_deref()
            .is_some_and(|rpc| rpc_method.and_then(Value::as_str) != Some(rpc))
        {
            return false;
        }
        let has_id = message.is_some_and(|message| message.get("id").is_some());
        if self.notification && !(rpc_method.is_some() && !has_id) {
            return false;
        }
        if self.response && !(message.is_some() && rpc_method.is_none()) {
            return false;
        }
        true
    }
}

async fn scripted(State(script): State<Arc<Script>>, request: Request) -> Response {
    let method = request.method().as_str().to_owned();
    let headers = request.headers().clone();
    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    let message: Option<Value> = (!body.is_empty()).then(|| serde_json::from_str(&body).unwrap());

    let mut kept = Map::new();
    let names = KEPT_HEADERS
        .iter()
        .copied()
        .chain(script.keeps_user_agent.then_some("user-agent"));
    for name in names {
        if let Some(value) = headers.get(name) {
            kept.insert(name.to_owned(), json!(value.to_str().unwrap()));
        }
    }
    script.seen.lock().unwrap().push(Seen {
        method: method.clone(),
        headers: kept,
        body,
    });

    let rule = script
        .rules
        .iter()
        .zip(&script.used)
        .find_map(|(rule, used)| {
            let free = rule
                .times
                .is_none_or(|times| used.load(Ordering::SeqCst) < times);
            (free && rule.when.matches(&method, &headers, message.as_ref())).then(|| {
                used.fetch_add(1, Ordering::SeqCst);
                rule.clone()
            })
        });
    let Some(rule) = rule else {
        return Response::builder()
            .status(599)
            .body(Body::from("no rule"))
            .unwrap();
    };
    if rule.never {
        return std::future::pending().await;
    }

    let id = message
        .as_ref()
        .and_then(|message| message.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let fill = move |text: &str| text.replace("$id", &id.to_string());
    let mut response = Response::builder().status(rule.status.unwrap());
    for (name, value) in &rule.headers {
        response = response.header(name, value.as_str().unwrap());
    }
    let Some(chunks) = rule.chunks else {
        return response
            .body(Body::from(
                rule.body.as_deref().map(fill).unwrap_or_default(),
            ))
            .unwrap();
    };
    let chunks: VecDeque<String> = chunks.iter().map(|chunk| fill(chunk)).collect();
    let delay = Duration::from_millis(rule.chunk_delay_ms);
    let hold = rule.hold;
    let frames = stream::unfold(chunks, move |mut chunks| async move {
        tokio::time::sleep(delay).await;
        match chunks.pop_front() {
            Some(chunk) => Some((Ok::<_, std::io::Error>(Bytes::from(chunk)), chunks)),
            None if hold => std::future::pending().await,
            None => None,
        }
    });
    response.body(Body::from_stream(frames)).unwrap()
}

async fn serve(script: Arc<Script>) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let url = Url::parse(&format!("http://{}/mcp", listener.local_addr().unwrap())).unwrap();
    let app = axum::Router::new().fallback(scripted).with_state(script);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, server)
}

/// The HTTP function a caller injects, here a plain `reqwest` client.
struct ReqwestSend(reqwest::Client);

impl ReqwestSend {
    fn new() -> Arc<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Arc::new(Self(client))
    }
}

#[async_trait::async_trait]
impl HttpSend for ReqwestSend {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut builder = self
            .0
            .request(request.method, request.url)
            .headers(request.headers);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder.send().await?;
        Ok(HttpResponse {
            status: response.status(),
            headers: response.headers().clone(),
            body: Box::pin(
                response
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(BoxError::from)),
            ),
        })
    }
}

fn describe(error: &ClientError) -> ExpectedError {
    let kind = match error {
        ClientError::Mcp(_) => "mcp",
        ClientError::Transport(TransportError::StreamableHttp { .. }) => "http",
        ClientError::InvalidResult(_)
        | ClientError::Transport(TransportError::InvalidMessage(_)) => "validation",
        ClientError::Transport(TransportError::InvalidJson(_)) => "json",
        _ => "error",
    };
    let (code, data) = match error {
        ClientError::Mcp(McpError { code, data, .. }) => (Some(*code), data.clone()),
        ClientError::Transport(TransportError::StreamableHttp { code, .. }) => {
            (Some(i64::from(*code)), None)
        }
        _ => (None, None),
    };
    ExpectedError {
        kind: kind.to_owned(),
        message: error.to_string(),
        code,
        data,
    }
}

async fn play(scenario: &Scenario) -> Vec<String> {
    let script = Arc::new(Script {
        used: scenario.rules.iter().map(|_| AtomicUsize::new(0)).collect(),
        rules: scenario.rules.clone(),
        keeps_user_agent: scenario.headers.contains_key("User-Agent"),
        seen: Mutex::new(Vec::new()),
    });
    let (url, server) = serve(script.clone()).await;

    let mut headers = HeaderMap::new();
    for (name, value) in &scenario.headers {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value.as_str().unwrap()).unwrap(),
        );
    }
    let transport = StreamableHttpClientTransport::with_options(
        url,
        ReqwestSend::new(),
        StreamableHttpClientTransportOptions {
            headers,
            ..Default::default()
        },
    );
    let info = match &scenario.client {
        Some(client) => Implementation {
            name: client.name.clone(),
            version: client.version.clone(),
            title: client.title.clone(),
        },
        None => Implementation::new("test-client", "1.0.0"),
    };
    let mut options = ClientOptions::default();
    if let Some(timeout) = scenario.timeout_ms {
        options.request_timeout = Duration::from_millis(timeout);
    }
    let client = Client::with_options(info, options);

    let mut differences = Vec::new();
    for (step, expected) in scenario.steps.iter().zip(&scenario.expect.outcomes) {
        assert_eq!(step, &expected.step);
        let mut parts = step.splitn(3, ':');
        let outcome: Result<Option<String>, ClientError> = match parts.next().unwrap() {
            "connect" => client.connect(transport.clone()).await.map(|()| None),
            "listTools" => client
                .list_tools()
                .await
                .map(|result| Some(json::to_string(&result.value))),
            "ping" => client.ping().await.map(|()| Some("{}".to_owned())),
            "callTool" => {
                let name = parts.next().unwrap();
                let arguments = match parts.next().unwrap() {
                    "-" => None,
                    text => json::parse(text).unwrap().as_object().cloned(),
                };
                client
                    .call_tool(name, arguments)
                    .await
                    .map(|result| Some(json::to_string(&result)))
            }
            "wait" => {
                tokio::time::sleep(Duration::from_millis(
                    parts.next().unwrap().parse().unwrap(),
                ))
                .await;
                Ok(None)
            }
            "close" => {
                stream_requests_arrive(scenario, &script).await;
                client.close().await;
                transport.close().await;
                Ok(None)
            }
            other => panic!("unknown step {other}"),
        };
        match (&outcome, &expected.error) {
            (Ok(got), None) => {
                if got != &expected.ok {
                    differences.push(format!(
                        "{step}: returned {got:?} instead of {:?}",
                        expected.ok
                    ));
                }
            }
            (Err(error), Some(expected)) => {
                let mut got = describe(error);
                // serde_json words a syntax error its own way.
                if expected.kind == "json" && got.kind == "json" {
                    got.message.clone_from(&expected.message);
                }
                if &got != expected {
                    differences.push(format!(
                        "{step}: failed with\n    {got:?}\n  instead of\n    {expected:?}"
                    ));
                }
            }
            (Ok(got), Some(expected)) => {
                differences.push(format!(
                    "{step}: returned {got:?} instead of failing with {expected:?}"
                ));
            }
            (Err(error), None) => differences.push(format!("{step}: failed with {error}")),
        }
    }

    tokio::time::sleep(Duration::from_millis(30)).await;
    server.abort();
    let seen = script.seen.lock().unwrap().clone();
    // A stream is opened while the next request is already on its way, so
    // each method keeps its own order.
    for method in ["POST", "GET", "DELETE"] {
        let of = |requests: &[Seen]| {
            let mut of: Vec<Seen> = requests
                .iter()
                .filter(|request| request.method == method)
                .cloned()
                .collect();
            if scenario.unordered {
                of.sort_by(|left, right| left.body.cmp(&right.body));
            }
            of
        };
        let (got, expected) = (of(&seen), of(&scenario.expect.requests));
        if method == "GET" && closes_before_its_stream_opens(scenario, &got, &expected) {
            continue;
        }
        if got != expected {
            differences.push(format!(
                "{method} requests:\n    {got:#?}\n  instead of\n    {expected:#?}"
            ));
        }
    }
    differences
}

/// The standalone stream is opened in the background, so when its request
/// leaves is a matter of scheduling, not of what the client does. Where the
/// SDK had it out before closing, this client is given the time to send it
/// too: on a busy machine it may still be waiting for its turn.
async fn stream_requests_arrive(scenario: &Scenario, script: &Script) {
    let streams = |requests: &[Seen]| {
        requests
            .iter()
            .filter(|request| request.method == "GET")
            .count()
    };
    let expected = streams(&scenario.expect.requests);
    let started = std::time::Instant::now();
    while streams(&script.seen.lock().unwrap()) < expected
        && started.elapsed() < Duration::from_secs(5)
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A client that connects and closes at once races with itself: its
/// standalone stream is opened in the background once the server took the
/// `initialized` notification, and closing may come before or after that
/// request leaves. The SDK lost that race when the fixtures were recorded
/// (no `GET`); either outcome is the same client.
fn closes_before_its_stream_opens(scenario: &Scenario, got: &[Seen], expected: &[Seen]) -> bool {
    let connects_then_closes = scenario.steps == ["connect", "close"]
        && scenario
            .expect
            .outcomes
            .iter()
            .all(|outcome| outcome.error.is_none());
    let one_stream_request = matches!(got, [request]
        if request.body.is_empty()
            && request.headers.get("accept").and_then(Value::as_str) == Some("text/event-stream"));
    connects_then_closes && expected.is_empty() && one_stream_request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_and_reads_what_the_sdk_client_does() {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("fixtures/client_http.json")).unwrap();
    assert!(fixtures.scenarios.len() > 30);
    let mut failures = Vec::new();
    for scenario in &fixtures.scenarios {
        let differences = play(scenario).await;
        if !differences.is_empty() {
            failures.push(format!(
                "{}:\n  {}",
                scenario.name,
                differences.join("\n  ")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ from the SDK:\n{}",
        failures.len(),
        fixtures.scenarios.len(),
        failures.join("\n")
    );
}

// What follows needs no server: the HTTP function is a closure.

type Requests = Arc<Mutex<Vec<(String, Option<String>)>>>;

fn body_of(chunks: Vec<Result<&'static str, &'static str>>) -> mymcps_mcp::client::HttpBody {
    Box::pin(stream::iter(
        chunks
            .into_iter()
            .map(|chunk| chunk.map(Bytes::from).map_err(BoxError::from)),
    ))
}

fn answer_with(
    status: u16,
    content_type: &'static str,
    body: mymcps_mcp::client::HttpBody,
) -> HttpResponse {
    let mut headers = HeaderMap::new();
    if !content_type.is_empty() {
        headers.insert("content-type", HeaderValue::from_static(content_type));
    }
    HttpResponse {
        status: StatusCode::from_u16(status).unwrap(),
        headers,
        body,
    }
}

const INITIALIZED: &str = r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"srv","version":"1"}}}"#;

/// A server that completes the handshake, refuses the standalone stream and
/// answers every later request with `later`.
fn handshake_then(
    requests: Requests,
    later: impl Fn() -> Result<HttpResponse, BoxError> + Send + Sync + 'static,
) -> Arc<dyn HttpSend> {
    let later = Arc::new(later);
    Arc::new(move |request: HttpRequest| {
        let later = later.clone();
        let requests = requests.clone();
        async move {
            let body = request
                .body
                .map(|body| String::from_utf8(body.to_vec()).unwrap());
            requests
                .lock()
                .unwrap()
                .push((request.method.to_string(), body.clone()));
            if request.method == http::Method::GET {
                return Ok(answer_with(405, "", body_of(vec![])));
            }
            let body = body.unwrap();
            if body.contains("\"initialize\"") {
                return Ok(answer_with(
                    200,
                    "application/json",
                    body_of(vec![Ok(INITIALIZED)]),
                ));
            }
            if body.contains("notifications/") {
                return Ok(answer_with(202, "", body_of(vec![])));
            }
            later()
        }
    })
}

fn url() -> Url {
    Url::parse("https://upstream.example/mcp").unwrap()
}

async fn connected(http: Arc<dyn HttpSend>) -> (Client, StreamableHttpClientTransport) {
    let transport = StreamableHttpClientTransport::new(url(), http);
    let client = Client::new(Implementation::new("test-client", "1.0.0"));
    client.connect(transport.clone()).await.unwrap();
    (client, transport)
}

#[tokio::test]
async fn reports_a_body_that_fails_while_it_is_read_in_the_words_of_the_http_function() {
    let http = handshake_then(Requests::default(), || {
        Ok(answer_with(
            200,
            "application/json",
            body_of(vec![
                Ok("{\"jsonrpc\":"),
                Err("MCP endpoint response exceeded 33554432 bytes"),
            ]),
        ))
    });
    let (client, _transport) = connected(http).await;

    let error = client.list_tools().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP endpoint response exceeded 33554432 bytes"
    );
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::Http(_))
    ));
    assert!(!error.is_unauthorized());
}

#[tokio::test]
async fn leaves_a_request_whose_event_stream_breaks_to_its_timeout() {
    let http = handshake_then(Requests::default(), || {
        Ok(answer_with(
            200,
            "text/event-stream",
            body_of(vec![
                Ok(": started\n\n"),
                Err("MCP endpoint response exceeded 33554432 bytes"),
            ]),
        ))
    });
    let notices = Arc::new(Mutex::new(Vec::new()));
    let listener = notices.clone();
    let client = Client::with_options(
        Implementation::new("test-client", "1.0.0"),
        ClientOptions {
            request_timeout: Duration::from_millis(150),
            on_error: Some(Arc::new(move |error: &TransportError| {
                listener.lock().unwrap().push(error.to_string());
            })),
            ..ClientOptions::default()
        },
    );
    client
        .connect(StreamableHttpClientTransport::new(url(), http))
        .await
        .unwrap();

    // The SDK only tells `onerror` about the broken stream; the request waits on.
    let started = std::time::Instant::now();
    let error = client.list_tools().await.unwrap_err();
    assert_eq!(error.to_string(), "MCP error -32001: Request timed out");
    assert!(started.elapsed() >= Duration::from_millis(140));
    assert!(
        notices.lock().unwrap().contains(
            &"SSE stream disconnected: Error: MCP endpoint response exceeded 33554432 bytes"
                .to_owned()
        ),
        "{notices:?}"
    );
}

#[tokio::test]
async fn reports_a_request_that_cannot_be_made_in_the_words_of_the_http_function() {
    let http: Arc<dyn HttpSend> = Arc::new(|_request: HttpRequest| async {
        Err::<HttpResponse, _>(BoxError::from(
            "MCP endpoint redirected to a different origin",
        ))
    });
    let client = Client::new(Implementation::new("test-client", "1.0.0"));
    let error = client
        .connect(StreamableHttpClientTransport::new(url(), http))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP endpoint redirected to a different origin"
    );
    assert_eq!(error.http_status(), None);
}

#[tokio::test]
async fn quotes_null_for_an_error_page_that_cannot_be_read() {
    let http: Arc<dyn HttpSend> = Arc::new(|_request: HttpRequest| async {
        Ok::<_, BoxError>(answer_with(
            502,
            "text/html",
            body_of(vec![Ok("<h1>"), Err("connection reset")]),
        ))
    });
    let client = Client::new(Implementation::new("test-client", "1.0.0"));
    let error = client
        .connect(StreamableHttpClientTransport::new(url(), http))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Streamable HTTP error: Error POSTing to endpoint: null"
    );
    assert_eq!(error.http_status(), Some(502));
}

#[tokio::test]
async fn tells_unauthorized_from_other_http_failures() {
    for (status, unauthorized) in [(401, true), (403, false), (500, false)] {
        let http: Arc<dyn HttpSend> = Arc::new(move |_request: HttpRequest| async move {
            Ok::<_, BoxError>(answer_with(
                status,
                "text/plain",
                body_of(vec![Ok("denied")]),
            ))
        });
        let client = Client::new(Implementation::new("test-client", "1.0.0"));
        let error = client
            .connect(StreamableHttpClientTransport::new(url(), http))
            .await
            .unwrap_err();
        assert_eq!(error.is_unauthorized(), unauthorized, "{status}");
        assert_eq!(error.http_status(), Some(status));
        assert_eq!(
            error.to_string(),
            "Streamable HTTP error: Error POSTing to endpoint: denied"
        );
    }
}

#[tokio::test]
async fn fails_the_requests_still_waiting_when_it_is_closed() {
    let http = handshake_then(Requests::default(), || {
        Ok(answer_with(
            200,
            "text/event-stream",
            Box::pin(stream::pending()),
        ))
    });
    let (client, _transport) = connected(http).await;

    let waiting = tokio::spawn({
        let client = client.clone();
        async move { client.list_tools().await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    client.close().await;

    let error = waiting.await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "MCP error -32000: Connection closed");
    assert_eq!(error.mcp_code(), Some(-32000));
    assert!(matches!(
        client.list_tools().await,
        Err(ClientError::NotConnected)
    ));
}

#[tokio::test]
async fn closes_without_a_request_and_ends_the_session_only_when_asked() {
    let requests = Requests::default();
    let recorded = requests.clone();
    let http: Arc<dyn HttpSend> = Arc::new(move |request: HttpRequest| {
        let requests = recorded.clone();
        async move {
            let body = request
                .body
                .as_ref()
                .map(|body| String::from_utf8(body.to_vec()).unwrap());
            requests
                .lock()
                .unwrap()
                .push((request.method.to_string(), body.clone()));
            let mut response = match (request.method.as_str(), body) {
                ("POST", Some(body)) if body.contains("\"initialize\"") => {
                    answer_with(200, "application/json", body_of(vec![Ok(INITIALIZED)]))
                }
                ("POST", _) => answer_with(202, "", body_of(vec![])),
                ("DELETE", _) => {
                    assert_eq!(request.headers.get("mcp-session-id").unwrap(), "session-9");
                    answer_with(405, "", body_of(vec![]))
                }
                _ => answer_with(405, "", body_of(vec![])),
            };
            response
                .headers
                .insert("mcp-session-id", HeaderValue::from_static("session-9"));
            Ok::<_, BoxError>(response)
        }
    });

    let transport = StreamableHttpClientTransport::new(url(), http.clone());
    let client = Client::new(Implementation::new("test-client", "1.0.0"));
    client.connect(transport.clone()).await.unwrap();
    assert_eq!(transport.session_id().as_deref(), Some("session-9"));
    assert_eq!(transport.protocol_version().as_deref(), Some("2025-06-18"));

    // A server may refuse to end sessions; that is not an error.
    transport.terminate_session().await.unwrap();
    assert_eq!(transport.session_id(), None);
    let deletes = |requests: &Requests| {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "DELETE")
            .count()
    };
    assert_eq!(deletes(&requests), 1);

    client.close().await;
    transport.close().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(deletes(&requests), 1);
}

#[tokio::test]
async fn leaves_no_stream_open_once_the_last_handle_is_dropped() {
    struct Open(Arc<AtomicBool>);
    impl Drop for Open {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    let open = Arc::new(AtomicBool::new(false));
    let flag = open.clone();
    let http: Arc<dyn HttpSend> = Arc::new(move |request: HttpRequest| {
        let flag = flag.clone();
        async move {
            if request.method == http::Method::GET {
                flag.store(true, Ordering::SeqCst);
                let guard = Open(flag);
                // A standalone stream the server keeps open and never writes to.
                let body = stream::pending::<Result<Bytes, BoxError>>().map(move |chunk| {
                    let _held = &guard;
                    chunk
                });
                return Ok::<_, BoxError>(answer_with(200, "text/event-stream", Box::pin(body)));
            }
            let body = String::from_utf8(request.body.unwrap().to_vec()).unwrap();
            Ok(if body.contains("\"initialize\"") {
                answer_with(200, "application/json", body_of(vec![Ok(INITIALIZED)]))
            } else {
                answer_with(202, "", body_of(vec![]))
            })
        }
    });

    let (client, transport) = connected(http).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(open.load(Ordering::SeqCst));

    // Dropped, never closed.
    drop(client);
    drop(transport);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!open.load(Ordering::SeqCst));
}

#[tokio::test]
async fn lets_the_callers_headers_win_except_for_the_content_negotiation() {
    let seen = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let recorded = seen.clone();
    let http: Arc<dyn HttpSend> = Arc::new(move |request: HttpRequest| {
        recorded.lock().unwrap().push(request.headers.clone());
        async move {
            let body = request
                .body
                .map(|body| String::from_utf8(body.to_vec()).unwrap())
                .unwrap_or_default();
            let mut response = if body.contains("\"initialize\"") {
                answer_with(200, "application/json", body_of(vec![Ok(INITIALIZED)]))
            } else {
                answer_with(405, "", body_of(vec![]))
            };
            response
                .headers
                .insert("mcp-session-id", HeaderValue::from_static("from-server"));
            Ok::<_, BoxError>(response)
        }
    });
    let mut headers = HeaderMap::new();
    headers.insert("accept", HeaderValue::from_static("text/plain"));
    headers.insert("content-type", HeaderValue::from_static("text/plain"));
    headers.insert("mcp-session-id", HeaderValue::from_static("from-caller"));
    headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
    let transport = StreamableHttpClientTransport::with_options(
        url(),
        http,
        StreamableHttpClientTransportOptions {
            headers,
            ..Default::default()
        },
    );
    let client = Client::new(Implementation::new("test-client", "1.0.0"));
    // The notification is answered 405, which fails the handshake after two requests.
    assert_eq!(
        client.connect(transport).await.unwrap_err().http_status(),
        Some(405)
    );

    let seen = seen.lock().unwrap();
    for request in seen.iter() {
        assert_eq!(
            request.get("accept").unwrap(),
            "application/json, text/event-stream"
        );
        assert_eq!(request.get("content-type").unwrap(), "application/json");
        assert_eq!(request.get("authorization").unwrap(), "Bearer secret");
        assert_eq!(request.get("mcp-session-id").unwrap(), "from-caller");
    }
    assert_eq!(seen.len(), 2);
}

#[tokio::test]
async fn refuses_a_second_connection_and_a_second_start() {
    let http = handshake_then(Requests::default(), || {
        Ok(answer_with(202, "", body_of(vec![])))
    });
    let (client, transport) = connected(http.clone()).await;

    let again = client
        .connect(StreamableHttpClientTransport::new(url(), http))
        .await
        .unwrap_err();
    assert!(
        again
            .to_string()
            .starts_with("Already connected to a transport.")
    );

    let other = Client::new(Implementation::new("other", "1"));
    let error = other.connect(transport).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "StreamableHTTPClientTransport already started! If using Client class, note that connect() calls start() automatically."
    );
}

#[tokio::test]
async fn hands_background_failures_to_the_error_listener() {
    let http: Arc<dyn HttpSend> = Arc::new(|request: HttpRequest| async move {
        if request.method == http::Method::GET {
            return Ok::<_, BoxError>(answer_with(503, "", body_of(vec![])));
        }
        let body = String::from_utf8(request.body.unwrap().to_vec()).unwrap();
        Ok(if body.contains("\"initialize\"") {
            answer_with(200, "application/json", body_of(vec![Ok(INITIALIZED)]))
        } else {
            answer_with(202, "", body_of(vec![]))
        })
    });
    let notices = Arc::new(Mutex::new(Vec::new()));
    let listener = notices.clone();
    let options = ClientOptions {
        on_error: Some(Arc::new(move |error: &TransportError| {
            listener.lock().unwrap().push(error.to_string());
        })),
        ..ClientOptions::default()
    };
    let client = Client::with_options(Implementation::new("test-client", "1.0.0"), options);
    client
        .connect(StreamableHttpClientTransport::new(url(), http))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert_eq!(
        *notices.lock().unwrap(),
        vec!["Streamable HTTP error: Failed to open SSE stream: Service Unavailable".to_owned()]
    );
}

/// How long [`counting_server`] takes to end what a client does not wait for.
const SERVER_LAG: Duration = Duration::from_millis(15);

/// A server on real sockets that counts the connections it accepts. It
/// answers the handshake and `tools/list`, as JSON or as an event stream
/// that ends a moment after its answer, and takes a moment to refuse the
/// stream of server-initiated messages, with a body to read: a client that
/// closes as soon as it has its answer closes before either has ended.
async fn counting_server(event_streams: bool) -> (Url, Arc<AtomicUsize>) {
    use axum::serve::ListenerExt;

    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let url = Url::parse(&format!("http://{}/mcp", listener.local_addr().unwrap())).unwrap();
    let app = axum::Router::new().fallback(move |request: Request| async move {
        let refused = || {
            Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"error":"This server offers no stream"}"#))
                .unwrap()
        };
        if request.method() != http::Method::POST {
            tokio::time::sleep(SERVER_LAG).await;
            return refused();
        }
        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
            .await
            .unwrap();
        let message: Value = serde_json::from_slice(&body).unwrap();
        let result = match message["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "srv", "version": "1" },
            }),
            Some("tools/list") => json!({ "tools": [] }),
            // A notification.
            _ => {
                return Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Body::empty())
                    .unwrap();
            }
        };
        let answer = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }).to_string();
        if event_streams {
            let event = format!("event: message\ndata: {answer}\n\n");
            let end = stream::once(async {
                tokio::time::sleep(SERVER_LAG).await;
                Ok::<_, std::convert::Infallible>(String::new())
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream::iter([Ok(event)]).chain(end)))
                .unwrap()
        } else {
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(answer))
                .unwrap()
        }
    });
    tokio::spawn(async move {
        let listener = listener.tap_io(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        axum::serve(listener, app).await.unwrap();
    });
    (url, accepted)
}

/// The gateway opens a session for each call it relays and closes it as soon
/// as the call is answered. Closing must not cost a connection: what it
/// leaves unread is what the next session would have reused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_opened_and_closed_in_a_row_share_their_connections() {
    const SESSIONS: usize = 30;

    for event_streams in [false, true] {
        let (url, accepted) = counting_server(event_streams).await;
        let http = ReqwestSend::new();
        for _ in 0..SESSIONS {
            let transport = StreamableHttpClientTransport::new(url.clone(), http.clone());
            let client = Client::new(Implementation::new("test-client", "1.0.0"));
            client.connect(transport).await.unwrap();
            client.list_tools().await.unwrap();
            client.close().await;
            // The next session starts once what this one left has ended, so
            // that it finds every connection free.
            tokio::time::sleep(SERVER_LAG * 3).await;
        }

        // One connection for the requests and one for the stream that is
        // asked for meanwhile, with room for a session that found one busy.
        let connections = accepted.load(Ordering::SeqCst);
        assert!(
            connections <= 5,
            "{connections} connections for {SESSIONS} sessions (event streams: {event_streams})"
        );
    }
}
