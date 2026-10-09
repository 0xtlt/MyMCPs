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
//!     StdioServerParameters::new("/usr/local/bin/deno")
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
use tokio::sync::oneshot;

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

/// Receives what the SDK hands to `onerror`: failures that did not fail a request.
pub type ErrorListener = Arc<dyn Fn(&TransportError) + Send + Sync>;

#[derive(Clone)]
pub struct ClientOptions {
    /// The `capabilities` of the `initialize` request. The gateway announces none.
    pub capabilities: Value,
    /// How long each request waits for its answer.
    pub request_timeout: Duration,
    /// The app sets no `onerror`, so by default these are dropped, unlogged:
    /// they can quote upstream answers.
    pub on_error: Option<ErrorListener>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            capabilities: json!({}),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            on_error: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with a JSON-RPC error, the request timed out
    /// (`-32001`), or the connection closed under it (`-32000`).
    #[error(transparent)]
    Mcp(#[from] McpError),

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
            _ => None,
        }
    }
}

/// One page of `tools/list`, as the SDK's `client.listTools()` returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    pub next_cursor: Option<String>,
    /// The whole result, with what else the server put in it.
    pub value: Value,
}

struct State {
    transport: Option<Arc<dyn Transport>>,
    next_id: u64,
    pending: HashMap<u64, oneshot::Sender<Result<Value, McpError>>>,
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
            .request("tools/call", Some(Value::Object(params)))
            .await?;
        Ok(CALL_TOOL_RESULT.parse_value(&result)?)
    }

    pub async fn ping(&self) -> Result<(), ClientError> {
        self.request("ping", None).await.map(|_| ())
    }

    /// Send a request and wait for its result, which is returned as the
    /// server sent it.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let timeout = self.inner.options.request_timeout;
        let (id, mut answer, transport) = {
            let mut state = self.inner.state();
            let transport = state.transport.clone().ok_or(ClientError::NotConnected)?;
            let id = state.next_id;
            state.next_id += 1;
            let (sender, receiver) = oneshot::channel();
            state.pending.insert(id, sender);
            (id, receiver, transport)
        };
        // A request abandoned by its caller must not stay registered.
        let _registration = PendingRequest {
            inner: &self.inner,
            id,
        };

        let request = JsonRpcMessage::Request {
            id: json!(id),
            method: method.to_owned(),
            params,
        };
        let mut sent = transport.send(request).fuse();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                // The answer first: a connection that closes fails the request
                // as closed, whatever the send in flight then reports.
                biased;
                answer = &mut answer => {
                    return match answer {
                        Ok(Ok(result)) => Ok(result),
                        Ok(Err(error)) => Err(error.into()),
                        Err(_) => Err(McpError::connection_closed().into()),
                    };
                }
                sent = &mut sent, if !sent.is_terminated() => sent?,
                () = &mut deadline => {
                    let error = McpError::request_timeout(timeout);
                    let cancelled = transport.send(JsonRpcMessage::Notification {
                        method: "notifications/cancelled".to_owned(),
                        params: Some(json!({ "requestId": id, "reason": format!("McpError: {error}") })),
                    });
                    let inner = Arc::downgrade(&self.inner);
                    tokio::spawn(async move {
                        if let Err(error) = cancelled.await
                            && let Some(inner) = inner.upgrade()
                        {
                            inner.report(&TransportError::Notice(format!(
                                "Failed to send cancellation: {error}"
                            )));
                        }
                    });
                    return Err(error.into());
                }
            }
        }
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
        transport
            .send(JsonRpcMessage::Notification {
                method: method.to_owned(),
                params,
            })
            .await
            .map_err(ClientError::from)
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
        self.inner.state().pending.remove(&self.id);
    }
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
            Some(waiting) => drop(waiting.send(outcome)),
            None => inner.report(&TransportError::Notice(format!(
                "Received a response for an unknown message ID: {}",
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
            // Nothing subscribes to notifications: no progress is asked for,
            // and no request of the server runs long enough to be cancelled.
            JsonRpcMessage::Notification { .. } => {}
        }
    }

    fn on_error(&self, error: &TransportError) {
        if let Some(inner) = self.0.upgrade() {
            inner.report(error);
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
            drop(waiting.send(Err(McpError::connection_closed())));
        }
    }
}
