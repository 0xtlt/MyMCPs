//! The gateway as a stateless MCP server: what the SDK's `Server` answers
//! through a `StreamableHTTPServerTransport` created without a session id
//! generator, a fresh pair for every HTTP request.
//!
//! [`handle_request`] takes one HTTP request and returns its response. It
//! knows four methods, like the gateway's server: `initialize` and `ping`,
//! which it answers itself, and `tools/list` and `tools/call`, which it hands
//! to a [`ToolHandler`]. Everything else is `Method not found`.
//!
//! Nothing is remembered between requests. A request does not have to be
//! preceded by `initialize`, and no `Mcp-Session-Id` is issued or checked.
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_mcp::Implementation;
//! use mymcps_mcp::server::{HandlerError, RequestBody, ServerOptions, ToolHandler, handle_request};
//! use serde_json::{Map, Value, json};
//!
//! struct Gateway;
//!
//! #[async_trait::async_trait]
//! impl ToolHandler for Gateway {
//!     async fn list_tools(&self) -> Result<Value, HandlerError> {
//!         Ok(json!({ "tools": [{ "name": "issues__create", "inputSchema": { "type": "object" } }] }))
//!     }
//!
//!     async fn call_tool(
//!         &self,
//!         name: String,
//!         arguments: Option<Map<String, Value>>,
//!     ) -> Result<Value, HandlerError> {
//!         let _ = arguments;
//!         // A refusal the agent should read is a result, not an error.
//!         Ok(json!({ "content": [{ "type": "text", "text": format!("ran {name}") }] }))
//!     }
//! }
//!
//! # async fn example(method: http::Method, headers: http::HeaderMap, body: bytes::Bytes) {
//! let mut options = ServerOptions::new(Implementation::new("mymcps", "0.4.1"));
//! options.instructions = Some("Available MCPs:\n- issues: Project issues".to_owned());
//!
//! let response = handle_request(&method, &headers, RequestBody::Raw(body), &options, Arc::new(Gateway)).await;
//! let (parts, body) = response.into_parts();
//! // `body` is a `Stream<Item = Result<Bytes, Infallible>>`: with axum,
//! // `Response::from_parts(parts, Body::from_stream(body))`.
//! # let _ = (parts, body);
//! # }
//! ```

use std::convert::Infallible;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{FutureExt, Stream};
use http::header::{ALLOW, CACHE_CONTROL, CONNECTION, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use crate::json;
use crate::media_type::{header_value, is_json_content_type};
use crate::schemas::{
    CALL_TOOL_REQUEST, CALL_TOOL_RESULT, CREATE_TASK_RESULT, INITIALIZE_REQUEST,
    LIST_TOOLS_REQUEST, PING_REQUEST, TASK_AUGMENTED_REQUEST_PARAMS,
};
use crate::types::{
    Implementation, JsonRpcError, JsonRpcMessage, LATEST_PROTOCOL_VERSION, McpError,
    SUPPORTED_PROTOCOL_VERSIONS, error_code,
};

/// The most a request body may hold when the transport parses it itself.
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: usize = 4 * 1024 * 1024;

/// The most messages one JSON-RPC batch may hold.
pub const MAX_BATCH_SIZE: usize = 100;

/// How often an open event stream is sent a comment, so that proxies and
/// idle timeouts leave it alone.
pub const DEFAULT_SSE_KEEP_ALIVE: Duration = Duration::from_millis(15_000);

const KEEP_ALIVE_FRAME: &[u8] = b": keepalive\n\n";

/// The code of every refusal that is the transport's own, not a JSON-RPC one.
const TRANSPORT_ERROR: i64 = -32000;

/// What `GET` on the endpoint does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StandaloneStream {
    /// What the SDK does: answer 200 with an event stream that stays open
    /// until the client leaves. A stateless server has nothing to say on it,
    /// so it only ever carries keep-alive comments.
    #[default]
    Open,
    /// Answer 405, which the protocol defines as "this server offers no such
    /// stream" and which SDK clients take without an error. Not what the
    /// TypeScript gateway does.
    MethodNotAllowed,
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub server_info: Implementation,
    /// The `capabilities` of the `initialize` result. The gateway's are `{"tools": {}}`.
    pub capabilities: Value,
    /// Left out of the `initialize` result when empty.
    pub instructions: Option<String>,
    pub standalone_stream: StandaloneStream,
    /// `None` sends no keep-alive comments.
    pub keep_alive: Option<Duration>,
    /// Only applies to [`RequestBody::Raw`].
    pub max_request_body_size: usize,
}

impl ServerOptions {
    pub fn new(server_info: Implementation) -> Self {
        Self {
            server_info,
            capabilities: json!({ "tools": {} }),
            instructions: None,
            standalone_stream: StandaloneStream::default(),
            keep_alive: Some(DEFAULT_SSE_KEEP_ALIVE),
            max_request_body_size: DEFAULT_MAX_REQUEST_BODY_SIZE,
        }
    }
}

/// The body of a POST.
#[derive(Debug, Clone)]
pub enum RequestBody {
    /// The bytes as they arrived. They are refused over the size limit (413)
    /// and when they are not JSON (400).
    Raw(Bytes),
    /// A body that was parsed before it got here, which is how the TypeScript
    /// gateway called the SDK. Neither check of `Raw` applies.
    Parsed(Value),
}

/// A failure of a [`ToolHandler`], answered as a JSON-RPC error: what a
/// thrown `Error` is to the SDK.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerError {
    /// `-32603` (internal error) when `None`.
    pub code: Option<i64>,
    pub message: String,
    pub data: Option<Value>,
}

impl HandlerError {
    /// An error without a code of its own.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
            data: None,
        }
    }
}

impl From<McpError> for HandlerError {
    /// A thrown `McpError`: its message keeps the `MCP error <code>: ` prefix.
    fn from(error: McpError) -> Self {
        Self {
            code: Some(error.code),
            message: error.to_string(),
            data: error.data,
        }
    }
}

/// The two requests the gateway answers itself.
#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    /// The result of `tools/list`, usually `{"tools": [...]}`. It is sent as it is.
    async fn list_tools(&self) -> Result<Value, HandlerError>;

    /// The result of `tools/call`. `arguments` is `None` when the request
    /// has none.
    ///
    /// The result is checked like the SDK checks it before it is sent: it has
    /// to be an object, `content` is added when missing, and each content
    /// block keeps the keys the protocol defines for its type. A result the
    /// protocol does not allow is answered as an invalid params error.
    async fn call_tool(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, HandlerError>;
}

/// The body of a response. As a stream, it yields its chunks and never fails.
pub enum ResponseBody {
    Empty,
    Full(Bytes),
    /// An event stream whose frames arrive as their requests are answered.
    /// The handler keeps running if this is dropped, as it does in the
    /// TypeScript gateway when a client leaves.
    Stream(Pin<Box<dyn Stream<Item = Bytes> + Send + 'static>>),
}

impl Stream for ResponseBody {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this {
            Self::Empty => Poll::Ready(None),
            Self::Full(bytes) => {
                let bytes = std::mem::take(bytes);
                *this = Self::Empty;
                Poll::Ready(Some(Ok(bytes)))
            }
            Self::Stream(stream) => stream
                .as_mut()
                .poll_next(context)
                .map(|chunk| chunk.map(Ok)),
        }
    }
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Full(bytes) => formatter.debug_tuple("Full").field(bytes).finish(),
            Self::Stream(_) => formatter.write_str("Stream"),
        }
    }
}

/// Answer one HTTP request to the MCP endpoint.
///
/// The response is returned as soon as its status is known. For a request
/// that reaches the handler that is before the handler is done: the answer
/// then arrives on the body.
pub async fn handle_request(
    method: &Method,
    headers: &HeaderMap,
    body: RequestBody,
    options: &ServerOptions,
    handler: Arc<dyn ToolHandler>,
) -> Response<ResponseBody> {
    match method.as_str() {
        "POST" => handle_post(headers, body, options, handler),
        "GET" => handle_get(headers, options),
        "DELETE" => match validate_protocol_version(headers) {
            Some(refusal) => refusal,
            // A stateless server has no session to end.
            None => empty(StatusCode::OK),
        },
        _ => method_not_allowed("GET, POST, DELETE"),
    }
}

fn handle_get(headers: &HeaderMap, options: &ServerOptions) -> Response<ResponseBody> {
    // The client must list text/event-stream among the types it accepts.
    let accept = header_value(headers, "accept").unwrap_or_default();
    if !accept.contains("text/event-stream") {
        return json_error(
            StatusCode::NOT_ACCEPTABLE,
            TRANSPORT_ERROR,
            "Not Acceptable: Client must accept text/event-stream",
        );
    }
    if let Some(refusal) = validate_protocol_version(headers) {
        return refusal;
    }
    match options.standalone_stream {
        StandaloneStream::MethodNotAllowed => method_not_allowed("POST"),
        StandaloneStream::Open => {
            let (never_written, frames) = mpsc::unbounded_channel();
            event_stream(ResponseBody::Stream(Box::pin(EventStream {
                frames,
                keep_alive: keep_alive_timer(options),
                // Only the client ends this stream.
                _keep_open: Some(never_written),
            })))
        }
    }
}

fn handle_post(
    headers: &HeaderMap,
    body: RequestBody,
    options: &ServerOptions,
    handler: Arc<dyn ToolHandler>,
) -> Response<ResponseBody> {
    // Accept is a comma-separated list, so a substring check is what is meant here.
    let accept = header_value(headers, "accept").unwrap_or_default();
    if !accept.contains("application/json") || !accept.contains("text/event-stream") {
        return json_error(
            StatusCode::NOT_ACCEPTABLE,
            TRANSPORT_ERROR,
            "Not Acceptable: Client must accept both application/json and text/event-stream",
        );
    }
    // The parsed media type, never a substring.
    if !is_json_content_type(header_value(headers, "content-type").as_deref()) {
        return json_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            TRANSPORT_ERROR,
            "Unsupported Media Type: Content-Type must be application/json",
        );
    }

    let raw_message = match body {
        RequestBody::Parsed(mut value) => {
            json::js_key_order(&mut value);
            value
        }
        RequestBody::Raw(bytes) => {
            let limit = options.max_request_body_size;
            let declared = header_value(headers, "content-length")
                .and_then(|length| length.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            if declared > limit as f64 || bytes.len() > limit {
                return json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    TRANSPORT_ERROR,
                    &format!("Payload Too Large: Request body must not exceed {limit} bytes"),
                );
            }
            match json::parse_bytes(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        error_code::PARSE_ERROR,
                        "Parse error: Invalid JSON",
                    );
                }
            }
        }
    };

    let raw_messages = match raw_message {
        Value::Array(items) if items.len() > MAX_BATCH_SIZE => {
            return json_error(
                StatusCode::BAD_REQUEST,
                error_code::INVALID_REQUEST,
                &format!("Invalid Request: Batch must not exceed {MAX_BATCH_SIZE} messages"),
            );
        }
        Value::Array(items) => items,
        single => vec![single],
    };
    let Ok(messages) = raw_messages
        .iter()
        .map(JsonRpcMessage::parse)
        .collect::<Result<Vec<_>, _>>()
    else {
        return json_error(
            StatusCode::BAD_REQUEST,
            error_code::PARSE_ERROR,
            "Parse error: Invalid JSON-RPC message",
        );
    };

    let initialization = messages
        .iter()
        .any(|message| INITIALIZE_REQUEST.accepts(Some(&message.to_value())));
    if initialization && messages.len() > 1 {
        return json_error(
            StatusCode::BAD_REQUEST,
            error_code::INVALID_REQUEST,
            "Invalid Request: Only one initialization request is allowed",
        );
    }
    // Version negotiation is the business of initialize itself.
    if !initialization && let Some(refusal) = validate_protocol_version(headers) {
        return refusal;
    }

    let requests: Vec<Request> = messages
        .into_iter()
        .filter_map(|message| match message {
            JsonRpcMessage::Request { id, method, params } => Some(Request { id, method, params }),
            // Notifications have no answer, and a response answers nothing this server asked.
            _ => None,
        })
        .collect();
    if requests.is_empty() {
        return empty(StatusCode::ACCEPTED);
    }

    // Answers leave in the order the SDK produces them: a method nobody
    // handles is refused on the spot, a request that fails its checks is
    // answered next, then come the answers that need no handler, and the
    // handler's as they complete.
    let mut refused = Vec::new();
    let mut failed = Vec::new();
    let mut answered = Vec::new();
    let mut pending = Vec::new();
    for request in requests {
        match plan(request, options) {
            Planned::Refused(frame) => refused.push(frame),
            Planned::Failed(frame) => failed.push(frame),
            Planned::Answered(frame) => answered.push(frame),
            Planned::Handler(call) => pending.push(call),
        }
    }
    let mut ready = Vec::new();
    for frame in refused.into_iter().chain(failed).chain(answered) {
        ready.extend_from_slice(&frame);
    }
    if pending.is_empty() {
        return event_stream(ResponseBody::Full(Bytes::from(ready)));
    }

    let (written, frames) = mpsc::unbounded_channel();
    if !ready.is_empty() {
        let _ = written.send(Bytes::from(ready));
    }
    for call in pending {
        let handler = handler.clone();
        let written = written.clone();
        // Not tied to the response: a call goes through, and is logged by the
        // handler, when the client is gone before its answer.
        tokio::spawn(async move {
            let frame = AssertUnwindSafe(call.run(handler.as_ref()))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| call.error(None, "Internal error".to_owned(), None));
            let _ = written.send(frame);
        });
    }
    // The stream ends when the last answer is written.
    drop(written);
    event_stream(ResponseBody::Stream(Box::pin(EventStream {
        frames,
        keep_alive: keep_alive_timer(options),
        _keep_open: None,
    })))
}

struct Request {
    id: Value,
    method: String,
    params: Option<Value>,
}

impl Request {
    /// The request as the SDK's handlers see it.
    fn to_value(&self) -> Value {
        JsonRpcMessage::Request {
            id: self.id.clone(),
            method: self.method.clone(),
            params: self.params.clone(),
        }
        .to_value()
    }
}

enum Planned {
    Refused(Bytes),
    Failed(Bytes),
    Answered(Bytes),
    Handler(HandlerCall),
}

struct HandlerCall {
    id: Value,
    kind: HandlerCallKind,
}

enum HandlerCallKind {
    ListTools,
    CallTool {
        name: String,
        arguments: Option<Map<String, Value>>,
        task: bool,
    },
}

fn plan(request: Request, options: &ServerOptions) -> Planned {
    let id = request.id.clone();
    if !matches!(
        request.method.as_str(),
        "initialize" | "ping" | "tools/list" | "tools/call"
    ) {
        return Planned::Refused(error_frame(
            &id,
            error_code::METHOD_NOT_FOUND,
            "Method not found".to_owned(),
            None,
        ));
    }
    let internal = |message: String| {
        Planned::Failed(error_frame(&id, error_code::INTERNAL_ERROR, message, None))
    };

    // A request that asks to be run as a task needs a server that offers tasks.
    let task = request
        .params
        .as_ref()
        .filter(|params| TASK_AUGMENTED_REQUEST_PARAMS.accepts(Some(params)))
        .and_then(|params| params.get("task"))
        .is_some_and(Value::is_object);
    if task && let Err(message) = assert_task_capability(&options.capabilities, &request.method) {
        return internal(message);
    }

    let message = request.to_value();
    match request.method.as_str() {
        "ping" => match PING_REQUEST.parse_value(&message) {
            Ok(_) => Planned::Answered(result_frame(&id, json!({}))),
            Err(error) => internal(error.to_string()),
        },
        "initialize" => match INITIALIZE_REQUEST.parse_value(&message) {
            Ok(parsed) => {
                let requested = parsed
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str);
                let version = requested
                    .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
                    .unwrap_or(LATEST_PROTOCOL_VERSION);
                let mut result = Map::new();
                result.insert("protocolVersion".to_owned(), json!(version));
                result.insert("capabilities".to_owned(), options.capabilities.clone());
                result.insert("serverInfo".to_owned(), options.server_info.to_value());
                if let Some(instructions) = options
                    .instructions
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
                    result.insert("instructions".to_owned(), json!(instructions));
                }
                Planned::Answered(result_frame(&id, Value::Object(result)))
            }
            Err(error) => internal(error.to_string()),
        },
        "tools/list" => match LIST_TOOLS_REQUEST.parse_value(&message) {
            Ok(_) => Planned::Handler(HandlerCall {
                id,
                kind: HandlerCallKind::ListTools,
            }),
            Err(error) => internal(error.to_string()),
        },
        _ => match CALL_TOOL_REQUEST.parse_value(&message) {
            Ok(parsed) => {
                let name = parsed
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let arguments = parsed
                    .pointer("/params/arguments")
                    .and_then(Value::as_object)
                    .cloned();
                Planned::Handler(HandlerCall {
                    id,
                    kind: HandlerCallKind::CallTool {
                        name,
                        arguments,
                        task,
                    },
                })
            }
            Err(error) => internal(error.to_string()),
        },
    }
}

impl HandlerCall {
    async fn run(&self, handler: &dyn ToolHandler) -> Bytes {
        let outcome = match &self.kind {
            HandlerCallKind::ListTools => handler.list_tools().await,
            HandlerCallKind::CallTool {
                name,
                arguments,
                task,
            } => match handler.call_tool(name.clone(), arguments.clone()).await {
                Ok(result) => {
                    let (schema, subject) = if *task {
                        (&CREATE_TASK_RESULT, "task creation")
                    } else {
                        (&CALL_TOOL_RESULT, "tools/call")
                    };
                    schema.parse_value(&result).map_err(|error| {
                        McpError::new(
                            error_code::INVALID_PARAMS,
                            format!("Invalid {subject} result: {error}"),
                        )
                        .into()
                    })
                }
                Err(error) => Err(error),
            },
        };
        match outcome {
            Ok(mut result) => {
                json::js_key_order(&mut result);
                result_frame(&self.id, result)
            }
            Err(error) => self.error(error.code, error.message, error.data),
        }
    }

    fn error(&self, code: Option<i64>, message: String, data: Option<Value>) -> Bytes {
        error_frame(
            &self.id,
            code.unwrap_or(error_code::INTERNAL_ERROR),
            message,
            data,
        )
    }
}

/// `assertToolsCallTaskCapability` of the SDK, for the server.
fn assert_task_capability(capabilities: &Value, method: &str) -> Result<(), String> {
    let truthy = |value: Option<&Value>| {
        value.is_some_and(|value| match value {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
            Value::String(text) => !text.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        })
    };
    let requests = capabilities.pointer("/tasks/requests");
    if !truthy(requests) {
        return Err(format!(
            "Server does not support task creation (required for {method})"
        ));
    }
    if method == "tools/call"
        && !truthy(requests.and_then(|requests| requests.pointer("/tools/call")))
    {
        return Err(format!(
            "Server does not support task creation for tools/call (required for {method})"
        ));
    }
    Ok(())
}

fn frame(message: &JsonRpcMessage) -> Bytes {
    Bytes::from(format!("event: message\ndata: {}\n\n", message.to_json()))
}

fn result_frame(id: &Value, result: Value) -> Bytes {
    frame(&JsonRpcMessage::Result {
        id: id.clone(),
        result,
    })
}

fn error_frame(id: &Value, code: i64, message: String, data: Option<Value>) -> Bytes {
    frame(&JsonRpcMessage::Error {
        id: Some(id.clone()),
        error: JsonRpcError {
            code,
            message,
            data,
        },
    })
}

/// A request that names a protocol version has to name one this server speaks.
fn validate_protocol_version(headers: &HeaderMap) -> Option<Response<ResponseBody>> {
    let version = header_value(headers, "mcp-protocol-version")?;
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&version.as_str()) {
        return None;
    }
    Some(json_error(
        StatusCode::BAD_REQUEST,
        TRANSPORT_ERROR,
        &format!(
            "Bad Request: Unsupported protocol version: {version} (supported versions: {})",
            SUPPORTED_PROTOCOL_VERSIONS.join(", ")
        ),
    ))
}

fn response(
    status: StatusCode,
    headers: &[(HeaderName, &'static str)],
    body: ResponseBody,
) -> Response<ResponseBody> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    for (name, value) in headers {
        response
            .headers_mut()
            .insert(name.clone(), HeaderValue::from_static(value));
    }
    response
}

fn empty(status: StatusCode) -> Response<ResponseBody> {
    response(status, &[], ResponseBody::Empty)
}

/// An error of the transport itself: no request can be named, so `id` is null.
fn json_error(status: StatusCode, code: i64, message: &str) -> Response<ResponseBody> {
    let body = format!(
        "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":{code},\"message\":{}}},\"id\":null}}",
        json::to_string(&json!(message))
    );
    response(
        status,
        &[(CONTENT_TYPE, "application/json")],
        ResponseBody::Full(Bytes::from(body)),
    )
}

fn method_not_allowed(allow: &'static str) -> Response<ResponseBody> {
    let mut refusal = json_error(
        StatusCode::METHOD_NOT_ALLOWED,
        TRANSPORT_ERROR,
        "Method not allowed.",
    );
    refusal
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    refusal
}

fn event_stream(body: ResponseBody) -> Response<ResponseBody> {
    response(
        StatusCode::OK,
        &[
            (CACHE_CONTROL, "no-cache, no-transform"),
            (CONNECTION, "keep-alive"),
            (CONTENT_TYPE, "text/event-stream"),
            // Tells nginx not to hold the stream back.
            (HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
}

fn keep_alive_timer(options: &ServerOptions) -> Option<Interval> {
    let period = options.keep_alive.filter(|period| !period.is_zero())?;
    let mut timer = tokio::time::interval_at(Instant::now() + period, period);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    Some(timer)
}

struct EventStream {
    frames: mpsc::UnboundedReceiver<Bytes>,
    keep_alive: Option<Interval>,
    /// A writer nobody uses, for the stream that only the client ends.
    _keep_open: Option<mpsc::UnboundedSender<Bytes>>,
}

impl Stream for EventStream {
    type Item = Bytes;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Bytes>> {
        let this = self.get_mut();
        match this.frames.poll_recv(context) {
            Poll::Ready(Some(frame)) => return Poll::Ready(Some(frame)),
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        if let Some(timer) = this.keep_alive.as_mut()
            && timer.poll_tick(context).is_ready()
        {
            return Poll::Ready(Some(Bytes::from_static(KEEP_ALIVE_FRAME)));
        }
        Poll::Pending
    }
}
