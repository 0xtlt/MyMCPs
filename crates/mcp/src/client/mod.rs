//! The gateway as an MCP client of one upstream server.
//!
//! [`Client`] is the SDK's `Client` reduced to what the gateway does with it:
//! the initialization handshake, `tools/list`, `tools/call` and closing. Every
//! request the app made was followed by a close, so nothing here is built for
//! long-lived sessions (no list-changed handlers, no task support).
//!
//! Over Streamable HTTP, with the function that performs the requests handed in:
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_mcp::Implementation;
//! use mymcps_mcp::client::{
//!     BoxError, Client, HttpRequest, HttpResponse, HttpSend, StreamableHttpClientTransport,
//!     StreamableHttpClientTransportOptions, Transport,
//! };
//!
//! /// Wraps the fetch that checks addresses, follows redirects and limits bodies.
//! struct Fetch;
//!
//! #[async_trait::async_trait]
//! impl HttpSend for Fetch {
//!     async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BoxError> {
//!         todo!("perform {} {}", request.method, request.url)
//!     }
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut headers = http::HeaderMap::new();
//! headers.insert("authorization", "Bearer token".parse()?);
//! let transport = StreamableHttpClientTransport::with_options(
//!     url::Url::parse("https://mcp.example/mcp")?,
//!     Arc::new(Fetch),
//!     StreamableHttpClientTransportOptions { headers, ..Default::default() },
//! );
//!
//! let client = Client::new(Implementation::new("mymcps-gateway", "0.4.1"));
//! if let Err(error) = client.connect(transport.clone()).await {
//!     if error.is_unauthorized() {
//!         // HTTP 401: the server wants credentials, or refused the ones it got.
//!     }
//!     return Err(error.into());
//! }
//! for tool in client.list_tools().await?.tools {
//!     println!("{}: {:?} {}", tool.name(), tool.description(), tool.input_schema());
//! }
//! let result = client.call_tool("echo", Some(serde_json::Map::new())).await?;
//! let failed = result.get("isError").and_then(|flag| flag.as_bool()) == Some(true);
//! # let _ = failed;
//!
//! // Teardown, as the app does it. Neither sends a request.
//! client.close().await;
//! transport.close().await;
//! # Ok(())
//! # }
//! ```
//!
//! Over stdio, with a child process as the server:
//!
//! ```no_run
//! use mymcps_mcp::Implementation;
//! use mymcps_mcp::client::{
//!     Client, StdioClientTransport, StdioServerParameters, StdioStderr, Transport,
//! };
//! use tokio::io::AsyncReadExt;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let transport = StdioClientTransport::new(
//!     StdioServerParameters::new("/usr/local/bin/node")
//!         .args(["run", "--quiet", "npm:@example/mcp@latest"])
//!         .env([("PATH", "/usr/bin:/bin"), ("HOME", "/app/tmp/mcp-sandboxes/7")])
//!         .cwd("/app/tmp/mcp-sandboxes/7")
//!         .stderr(StdioStderr::Pipe),
//! );
//! // Read stderr from the start: a child that fills the pipe waits for its reader.
//! let mut stderr = transport.stderr().expect("stderr was asked for");
//! let startup_output = tokio::spawn(async move {
//!     let mut output = Vec::new();
//!     let _ = stderr.read_to_end(&mut output).await;
//!     output
//! });
//!
//! let client = Client::new(Implementation::new("mymcps-gateway", "0.4.1"));
//! if let Err(error) = client.connect(transport.clone()).await {
//!     // `spawn <command> ENOENT` when the command did not start, `MCP error
//!     // -32000: Connection closed` when it exited before answering.
//!     let output = String::from_utf8_lossy(&startup_output.await?).into_owned();
//!     return Err(format!("{error}. Output: {output}").into());
//! }
//! let tools = client.list_tools().await?.tools;
//! # let _ = tools;
//!
//! // Closes the child's stdin, then SIGTERM after 2 s, then SIGKILL after 2 s more.
//! client.close().await;
//! transport.close().await;
//! # Ok(())
//! # }
//! ```

mod http;
mod stdio;
mod transport;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use futures::FutureExt;
use futures::future::FusedFuture;
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use crate::json::id_as_request_number;
use crate::schemas::{CALL_TOOL_RESULT, INITIALIZE_RESULT, LIST_TOOLS_RESULT};
use crate::types::{
    DEFAULT_REQUEST_TIMEOUT, Implementation, JsonRpcError, JsonRpcMessage, LATEST_PROTOCOL_VERSION,
    McpError, SUPPORTED_PROTOCOL_VERSIONS, Tool, error_code,
};
use crate::zod::ValidationError;

pub use http::{
    HttpBody, HttpRequest, HttpResponse, HttpSend, ReconnectionOptions,
    StreamableHttpClientTransport, StreamableHttpClientTransportOptions,
};
pub use stdio::{
    ChildStderr, DEFAULT_INHERITED_ENV_VARS, STDIO_DEFAULT_MAX_BUFFER_SIZE, StdioClientTransport,
    StdioServerParameters, StdioStderr, get_default_environment,
};
pub use transport::{BoxError, Transport, TransportError, TransportHandler};

/// How long the client waits for a transport to take the cancellation of a
/// request it gave up on. Callers close the transport right after a failed
/// call, and over HTTP closing abandons what is still in flight: without this
/// wait the cancellation of a call that ran for an hour would almost never
/// reach the server.
const CANCELLATION_GRACE: Duration = Duration::from_secs(5);

/// How far ahead a deadline is put when the wait is too long to add to an instant.
const FAR_FUTURE: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Receives what the SDK hands to `onerror`: failures that did not fail a request.
pub type ErrorListener = Arc<dyn Fn(&TransportError) + Send + Sync>;

/// How long one request waits for its answer: the SDK's `timeout`,
/// `resetTimeoutOnProgress` and `maxTotalTimeout` request options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTimeout {
    /// How long the request waits for its answer. With `reset_on_progress`,
    /// the wait starts again at each progress notification of the request.
    pub timeout: Duration,
    /// Ask the server for progress (`_meta.progressToken`) and let each
    /// `notifications/progress` for this request restart `timeout`.
    pub reset_on_progress: bool,
    /// The longest the request waits in all, progress or not.
    pub max_total: Option<Duration>,
}

impl RequestTimeout {
    /// A wait that nothing restarts or extends: the SDK's default.
    pub const fn fixed(timeout: Duration) -> Self {
        Self {
            timeout,
            reset_on_progress: false,
            max_total: None,
        }
    }
}

#[derive(Clone)]
pub struct ClientOptions {
    /// The `capabilities` of the `initialize` request. The gateway announces none.
    pub capabilities: Value,
    /// How long every request but `tools/call` waits for its answer, and how
    /// long a notification waits to be taken by the transport.
    pub request_timeout: Duration,
    /// How long `tools/call` waits. A tool may run much longer than a
    /// listing, so it has its own wait.
    pub tool_call_timeout: RequestTimeout,
    /// The app sets no `onerror`, so by default these are dropped, unlogged:
    /// they can quote upstream answers.
    pub on_error: Option<ErrorListener>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            capabilities: json!({}),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            tool_call_timeout: RequestTimeout::fixed(DEFAULT_REQUEST_TIMEOUT),
            on_error: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with a JSON-RPC error, or the connection closed
    /// under the request (`-32000`).
    #[error(transparent)]
    Mcp(#[from] McpError),

    /// The request got no answer in time and was cancelled, or the server did
    /// not take a notification in time. Reads as the `McpError` (`-32001`).
    /// Boxed: it would make every `Result` of the client large.
    #[error(transparent)]
    TimedOut(Box<RequestTimedOut>),

    #[error(transparent)]
    Transport(#[from] TransportError),

    /// The answer does not have the shape of the result (a `ZodError` in the
    /// SDK, whose message this is).
    #[error(transparent)]
    InvalidResult(#[from] ValidationError),

    #[error("Server's protocol version is not supported: {0}")]
    UnsupportedProtocolVersion(String),

    #[error("Not connected")]
    NotConnected,

    #[error(
        "Already connected to a transport. Call close() before connecting to a new transport, or use a separate Protocol instance per connection."
    )]
    AlreadyConnected,
}

impl ClientError {
    /// The HTTP status of the answer that caused this error, when there was one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Transport(error) => error.http_status(),
            _ => None,
        }
    }

    /// The server answered 401. The SDK reports this as a
    /// `StreamableHTTPError` with `code` 401 when, as here, it is given no
    /// OAuth provider.
    pub fn is_unauthorized(&self) -> bool {
        self.http_status() == Some(401)
    }

    /// The JSON-RPC error code, for an error of the protocol.
    pub fn mcp_code(&self) -> Option<i64> {
        match self {
            Self::Mcp(error) => Some(error.code),
            Self::TimedOut(timed_out) => Some(timed_out.error.code),
            _ => None,
        }
    }

    /// The request or notification this client gave up waiting for. A server
    /// that answered with an error whose code is `-32001` is not this.
    pub fn timed_out(&self) -> Option<&RequestTimedOut> {
        match self {
            Self::TimedOut(timed_out) => Some(timed_out),
            _ => None,
        }
    }
}

/// A request or notification that the client stopped waiting for.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{error}")]
pub struct RequestTimedOut {
    /// The method of the request or notification, such as `tools/call`.
    pub method: String,
    /// How long it waited, from when it was sent to the deadline that ran out.
    pub waited: Duration,
    pub error: McpError,
}

/// One page of `tools/list`, as the SDK's `client.listTools()` returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    pub next_cursor: Option<String>,
    /// The whole result, with what else the server put in it.
    pub value: Value,
}

/// What settles a pending request.
enum Answer {
    /// The server's result or error.
    Response(Result<Value, McpError>),
    /// The transport knows the answer will not come.
    Lost,
}

struct State {
    transport: Option<Arc<dyn Transport>>,
    next_id: u64,
    pending: HashMap<u64, oneshot::Sender<Answer>>,
    /// Woken by each progress notification of a request that asked for them.
    progress: HashMap<u64, Arc<Notify>>,
    server_capabilities: Option<Value>,
    server_version: Option<Value>,
    instructions: Option<String>,
}

struct Inner {
    client_info: Implementation,
    options: ClientOptions,
    state: Mutex<State>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn report(&self, error: &TransportError) {
        if let Some(listener) = &self.options.on_error {
            listener(error);
        }
    }
}

/// An MCP client. Clones share one connection.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    pub fn new(client_info: Implementation) -> Self {
        Self::with_options(client_info, ClientOptions::default())
    }

    pub fn with_options(client_info: Implementation, options: ClientOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                client_info,
                options,
                state: Mutex::new(State {
                    transport: None,
                    next_id: 0,
                    pending: HashMap::new(),
                    progress: HashMap::new(),
                    server_capabilities: None,
                    server_version: None,
                    instructions: None,
                }),
            }),
        }
    }

    /// Start the transport and run the initialization handshake: `initialize`,
    /// then `notifications/initialized`.
    pub async fn connect<T: Transport>(&self, transport: T) -> Result<(), ClientError> {
        let transport: Arc<dyn Transport> = Arc::new(transport);
        {
            let mut state = self.inner.state();
            if state.transport.is_some() {
                return Err(ClientError::AlreadyConnected);
            }
            state.transport = Some(transport.clone());
        }

        // The transport's tasks must not keep the client, and through it the transport, alive.
        let handler = Arc::new(Handler(Arc::downgrade(&self.inner)));
        if let Err(error) = transport.start(handler).await {
            self.inner.state().transport = None;
            return Err(error.into());
        }

        if let Err(error) = self.initialize(&transport).await {
            // Disconnect if initialization fails, without making the caller wait for it.
            tokio::spawn(transport.close());
            return Err(error);
        }
        Ok(())
    }

    async fn initialize(&self, transport: &Arc<dyn Transport>) -> Result<(), ClientError> {
        let mut params = Map::new();
        params.insert("protocolVersion".to_owned(), json!(LATEST_PROTOCOL_VERSION));
        params.insert(
            "capabilities".to_owned(),
            self.inner.options.capabilities.clone(),
        );
        params.insert("clientInfo".to_owned(), self.inner.client_info.to_value());

        let result = self
            .request("initialize", Some(Value::Object(params)))
            .await?;
        let result = INITIALIZE_RESULT.parse_value(&result)?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
            return Err(ClientError::UnsupportedProtocolVersion(version.to_owned()));
        }
        {
            let mut state = self.inner.state();
            state.server_capabilities = result.get("capabilities").cloned();
            state.server_version = result.get("serverInfo").cloned();
            state.instructions = result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        // HTTP transports repeat the protocol version in a header from here on.
        transport.set_protocol_version(version);

        self.notification("notifications/initialized", None).await
    }

    /// One page of the server's tools. Like the app, this does not follow `nextCursor`.
    pub async fn list_tools(&self) -> Result<ListToolsResult, ClientError> {
        let result = self.request("tools/list", None).await?;
        let value = LIST_TOOLS_RESULT.parse_value(&result)?;
        let tools = value
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool.as_object().cloned().map(Tool::from))
                    .collect()
            })
            .unwrap_or_default();
        let next_cursor = value
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(ListToolsResult {
            tools,
            next_cursor,
            value,
        })
    }

    /// Call a tool and return its result as the SDK's `client.callTool()`
    /// does: `content` is always there, and each content block holds the keys
    /// the protocol defines for its type. A result with `isError: true` is a
    /// result, not an error.
    ///
    /// `arguments: None` leaves the key out of the request.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, ClientError> {
        let mut params = Map::new();
        params.insert("name".to_owned(), json!(name));
        if let Some(arguments) = arguments {
            params.insert("arguments".to_owned(), Value::Object(arguments));
        }
        let result = self
            .request_with(
                "tools/call",
                Some(Value::Object(params)),
                self.inner.options.tool_call_timeout,
            )
            .await?;
        Ok(CALL_TOOL_RESULT.parse_value(&result)?)
    }

    pub async fn ping(&self) -> Result<(), ClientError> {
        self.request("ping", None).await.map(|_| ())
    }

    /// Send a request and wait for its result, which is returned as the
    /// server sent it. The wait is `request_timeout`.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let timeout = RequestTimeout::fixed(self.inner.options.request_timeout);
        self.request_with(method, params, timeout).await
    }

    /// [`Client::request`] with its own wait.
    pub async fn request_with(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: RequestTimeout,
    ) -> Result<Value, ClientError> {
        let (id, mut answer, transport, progress) = {
            let mut state = self.inner.state();
            let transport = state.transport.clone().ok_or(ClientError::NotConnected)?;
            let id = state.next_id;
            state.next_id += 1;
            let (sender, receiver) = oneshot::channel();
            state.pending.insert(id, sender);
            let progress = timeout.reset_on_progress.then(|| {
                let progress = Arc::new(Notify::new());
                state.progress.insert(id, progress.clone());
                progress
            });
            (id, receiver, transport, progress)
        };
        // A request abandoned by its caller must not stay registered.
        let _registration = PendingRequest {
            inner: &self.inner,
            id,
        };

        let params = if timeout.reset_on_progress {
            with_progress_token(params, id)
        } else {
            params
        };
        let request = JsonRpcMessage::Request {
            id: json!(id),
            method: method.to_owned(),
            params,
        };
        let started = Instant::now();
        let mut restarted = started;
        let mut sent = transport.send(request).fuse();
        let step = tokio::time::sleep_until(deadline_after(started, timeout.timeout));
        tokio::pin!(step);
        // Fixed when the request is sent: progress does not move it.
        let last_moment = timeout.max_total.and_then(|max| started.checked_add(max));
        let last = tokio::time::sleep_until(last_moment.unwrap_or(started + FAR_FUTURE));
        tokio::pin!(last);

        loop {
            tokio::select! {
                // The answer first: a connection that closes fails the request
                // as closed, whatever the send in flight then reports.
                biased;
                answer = &mut answer => {
                    return match answer {
                        Ok(Answer::Response(Ok(result))) => Ok(result),
                        Ok(Answer::Response(Err(error))) => Err(error.into()),
                        Ok(Answer::Lost) => {
                            let error = McpError::connection_closed();
                            self.cancel(&transport, id, &error).await;
                            Err(error.into())
                        }
                        Err(_) => Err(McpError::connection_closed().into()),
                    };
                }
                sent = &mut sent, if !sent.is_terminated() => sent?,
                () = progressed(progress.as_deref()) => {
                    restarted = Instant::now();
                    step.as_mut().reset(deadline_after(restarted, timeout.timeout));
                }
                () = &mut step => {
                    // Counted up to the deadline that ran out, not to when this woke.
                    let waited = restarted.duration_since(started).saturating_add(timeout.timeout);
                    let error = McpError::request_timeout(timeout.timeout);
                    return Err(self.time_out(&transport, id, method, waited, error).await);
                }
                () = &mut last, if last_moment.is_some() => {
                    let max_total = timeout.max_total.unwrap_or_default();
                    let error = McpError::max_total_timeout_exceeded(max_total, max_total);
                    return Err(self.time_out(&transport, id, method, max_total, error).await);
                }
            }
        }
    }

    /// Cancel a request that ran out of time, then report it as timed out.
    async fn time_out(
        &self,
        transport: &Arc<dyn Transport>,
        id: u64,
        method: &str,
        waited: Duration,
        error: McpError,
    ) -> ClientError {
        self.cancel(transport, id, &error).await;
        ClientError::TimedOut(Box::new(RequestTimedOut {
            method: method.to_owned(),
            waited,
            error,
        }))
    }

    /// Tell the server that the request is given up, and wait a moment for the
    /// transport to take the message: see [`CANCELLATION_GRACE`].
    async fn cancel(&self, transport: &Arc<dyn Transport>, id: u64, error: &McpError) {
        let cancelled = transport.send(JsonRpcMessage::Notification {
            method: "notifications/cancelled".to_owned(),
            params: Some(json!({ "requestId": id, "reason": format!("McpError: {error}") })),
        });
        let failure = match tokio::time::timeout(CANCELLATION_GRACE, cancelled).await {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!("not taken within {} seconds", CANCELLATION_GRACE.as_secs()),
        };
        self.inner.report(&TransportError::Notice(format!(
            "Failed to send cancellation: {failure}"
        )));
    }

    /// Send a notification. Over HTTP this waits for the server to accept it.
    pub async fn notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), ClientError> {
        let transport = self
            .inner
            .state()
            .transport
            .clone()
            .ok_or(ClientError::NotConnected)?;
        let timeout = self.inner.options.request_timeout;
        let sent = transport.send(JsonRpcMessage::Notification {
            method: method.to_owned(),
            params,
        });
        // The HTTP function may be left waiting a long time for a server that
        // takes the notification and says nothing.
        match tokio::time::timeout(timeout, sent).await {
            Ok(sent) => sent.map_err(ClientError::from),
            Err(_) => Err(ClientError::TimedOut(Box::new(RequestTimedOut {
                method: method.to_owned(),
                waited: timeout,
                error: McpError::request_timeout(timeout),
            }))),
        }
    }

    /// Close the connection. Requests still waiting fail with
    /// `MCP error -32000: Connection closed`.
    pub async fn close(&self) {
        let transport = self.inner.state().transport.clone();
        if let Some(transport) = transport {
            transport.close().await;
        }
    }

    /// The `capabilities` of the server's `initialize` result.
    pub fn server_capabilities(&self) -> Option<Value> {
        self.inner.state().server_capabilities.clone()
    }

    /// The `serverInfo` of the server's `initialize` result.
    pub fn server_version(&self) -> Option<Value> {
        self.inner.state().server_version.clone()
    }

    pub fn instructions(&self) -> Option<String> {
        self.inner.state().instructions.clone()
    }
}

struct PendingRequest<'a> {
    inner: &'a Inner,
    id: u64,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        let mut state = self.inner.state();
        state.pending.remove(&self.id);
        state.progress.remove(&self.id);
    }
}

/// The instant `wait` after `from`, which a wait too long to add stops at.
fn deadline_after(from: Instant, wait: Duration) -> Instant {
    from.checked_add(wait).unwrap_or(from + FAR_FUTURE)
}

/// Resolves at each progress notification of a request that asked for them.
/// A notification that came while the request was busy is not lost.
async fn progressed(progress: Option<&Notify>) {
    match progress {
        Some(progress) => progress.notified().await,
        None => std::future::pending().await,
    }
}

/// Ask the server for progress on this request. The token is the request id,
/// which is what the SDK uses, and goes into the `_meta` the params may have.
fn with_progress_token(params: Option<Value>, id: u64) -> Option<Value> {
    let mut params = match params {
        None => Map::new(),
        Some(Value::Object(params)) => params,
        other => return other,
    };
    if let Value::Object(meta) = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()))
    {
        meta.insert("progressToken".to_owned(), json!(id));
    }
    Some(Value::Object(params))
}

struct Handler(Weak<Inner>);

impl Handler {
    fn settle(
        inner: &Inner,
        id: Option<&Value>,
        outcome: Result<Value, McpError>,
        message: &JsonRpcMessage,
    ) {
        let waiting = id
            .and_then(id_as_request_number)
            .and_then(|id| inner.state().pending.remove(&id));
        match waiting {
            // The caller may have stopped waiting.
            Some(waiting) => drop(waiting.send(Answer::Response(outcome))),
            None => inner.report(&TransportError::Notice(format!(
                "Received a response for an unknown message ID: {}",
                message.to_json()
            ))),
        }
    }

    /// Restart the wait of the request the notification is about. The SDK
    /// reports a token that no request is waiting for.
    fn progress(inner: &Inner, params: Option<&Value>, message: &JsonRpcMessage) {
        let waiting = params
            .and_then(|params| params.get("progressToken"))
            .and_then(id_as_request_number)
            .and_then(|id| inner.state().progress.get(&id).cloned());
        match waiting {
            Some(waiting) => waiting.notify_one(),
            None => inner.report(&TransportError::Notice(format!(
                "Received a progress notification for an unknown token: {}",
                message.to_json()
            ))),
        }
    }

    /// A request from the server. Without capabilities, the SDK client
    /// answers `ping` and refuses everything else.
    fn answer(inner: &Arc<Inner>, id: Value, method: &str) {
        let Some(transport) = inner.state().transport.clone() else {
            return;
        };
        let (response, context) = if method == "ping" {
            (
                JsonRpcMessage::Result {
                    id,
                    result: json!({}),
                },
                "Failed to send response",
            )
        } else {
            let error = JsonRpcError {
                code: error_code::METHOD_NOT_FOUND,
                message: "Method not found".to_owned(),
                data: None,
            };
            (
                JsonRpcMessage::Error {
                    id: Some(id),
                    error,
                },
                "Failed to send an error response",
            )
        };
        let sent = transport.send(response);
        let inner = Arc::downgrade(inner);
        tokio::spawn(async move {
            if let Err(error) = sent.await
                && let Some(inner) = inner.upgrade()
            {
                inner.report(&TransportError::Notice(format!("{context}: {error}")));
            }
        });
    }
}

impl TransportHandler for Handler {
    fn on_message(&self, message: JsonRpcMessage) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        match &message {
            JsonRpcMessage::Result { id, result } => {
                Self::settle(&inner, Some(id), Ok(result.clone()), &message);
            }
            JsonRpcMessage::Error { id, error } => {
                Self::settle(&inner, id.as_ref(), Err(error.clone().into()), &message);
            }
            JsonRpcMessage::Request { id, method, .. } => Self::answer(&inner, id.clone(), method),
            JsonRpcMessage::Notification { method, params }
                if method == "notifications/progress" =>
            {
                Self::progress(&inner, params.as_ref(), &message);
            }
            // Nothing else is subscribed to: no request of the server runs
            // long enough to be cancelled.
            JsonRpcMessage::Notification { .. } => {}
        }
    }

    fn on_error(&self, error: &TransportError) {
        if let Some(inner) = self.0.upgrade() {
            inner.report(error);
        }
    }

    fn on_answer_lost(&self, id: &Value) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        let waiting = id_as_request_number(id).and_then(|id| inner.state().pending.remove(&id));
        if let Some(waiting) = waiting {
            drop(waiting.send(Answer::Lost));
        }
    }

    fn on_close(&self) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        let pending = {
            let mut state = inner.state();
            state.transport = None;
            std::mem::take(&mut state.pending)
        };
        for (_, waiting) in pending {
            drop(waiting.send(Answer::Response(Err(McpError::connection_closed()))));
        }
    }
}
