//! Streamable HTTP, client side: the SDK's `StreamableHTTPClientTransport`
//! as the gateway uses it, without an OAuth provider.
//!
//! Every message is a POST. The answer to a request is either JSON or an
//! event stream that carries it. After the handshake the transport also tries
//! a GET for a stream of server-initiated messages, which a server may refuse
//! with 405.
//!
//! The transport opens no socket: the caller hands in the function that
//! performs a request, and that function decides about addresses, redirects,
//! timeouts and how much of a body it lets through.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{Stream, StreamExt};
use http::header::{ACCEPT, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde_json::Value;
use tokio_util::sync::{CancellationToken, DropGuard};
use url::Url;

use super::transport::{BoxError, Transport, TransportError, TransportHandler};
use crate::json;
use crate::media_type::{header_value, media_type_essence};
use crate::sse::{SseItem, SseParser};
use crate::types::JsonRpcMessage;

const SESSION_ID: HeaderName = HeaderName::from_static("mcp-session-id");
const PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");

pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
}

/// The body of an answer, as it arrives. Dropping it abandons the request.
pub type HttpBody = Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send + 'static>>;

pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: HttpBody,
}

/// The function that performs one HTTP request: the `fetch` option of the
/// SDK transport.
///
/// It returns once the headers of the answer are known. Redirects are its
/// business: an answer with a 3xx status is reported as a failed request. The
/// future and the body it returns are dropped when the transport is closed,
/// and dropping them has to abandon the request.
#[async_trait]
pub trait HttpSend: Send + Sync + 'static {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BoxError>;
}

#[async_trait]
impl<F, Fut> HttpSend for F
where
    F: Fn(HttpRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<HttpResponse, BoxError>> + Send + 'static,
{
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BoxError> {
        self(request).await
    }
}

/// How the stream of server-initiated messages is reopened when it ends.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconnectionOptions {
    pub initial_reconnection_delay: Duration,
    pub max_reconnection_delay: Duration,
    pub reconnection_delay_grow_factor: f64,
    pub max_retries: u32,
}

impl Default for ReconnectionOptions {
    fn default() -> Self {
        Self {
            initial_reconnection_delay: Duration::from_millis(1000),
            max_reconnection_delay: Duration::from_millis(30_000),
            reconnection_delay_grow_factor: 1.5,
            max_retries: 2,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamableHttpClientTransportOptions {
    /// Sent with every request (`requestInit.headers` in the SDK): credentials
    /// and client identity. They win over the session and protocol version
    /// headers, and lose to `Content-Type` and `Accept`.
    pub headers: HeaderMap,
    pub reconnection: ReconnectionOptions,
}

#[derive(Clone, Default)]
struct StreamOptions {
    /// `Last-Event-ID` of a stream that is being resumed.
    resumption_token: Option<String>,
}

struct Inner {
    url: Url,
    http: Arc<dyn HttpSend>,
    options: StreamableHttpClientTransportOptions,
    handler: Mutex<Option<Arc<dyn TransportHandler>>>,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    /// The `retry:` a server last sent, which replaces the backoff.
    server_retry: Mutex<Option<Duration>>,
    started: AtomicBool,
    aborted: CancellationToken,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// See the module documentation. Clones share one connection.
#[derive(Clone)]
pub struct StreamableHttpClientTransport {
    inner: Arc<Inner>,
    /// Held by the handles only: when the last one goes, the streams still open are abandoned.
    _abort_on_drop: Arc<DropGuard>,
}

impl StreamableHttpClientTransport {
    pub fn new(url: Url, http: Arc<dyn HttpSend>) -> Self {
        Self::with_options(url, http, StreamableHttpClientTransportOptions::default())
    }

    pub fn with_options(
        url: Url,
        http: Arc<dyn HttpSend>,
        options: StreamableHttpClientTransportOptions,
    ) -> Self {
        let aborted = CancellationToken::new();
        Self {
            _abort_on_drop: Arc::new(aborted.clone().drop_guard()),
            inner: Arc::new(Inner {
                url,
                http,
                options,
                handler: Mutex::new(None),
                session_id: Mutex::new(None),
                protocol_version: Mutex::new(None),
                server_retry: Mutex::new(None),
                started: AtomicBool::new(false),
                aborted,
            }),
        }
    }

    /// The `Mcp-Session-Id` the server assigned, once it has.
    pub fn session_id(&self) -> Option<String> {
        lock(&self.inner.session_id).clone()
    }

    pub fn protocol_version(&self) -> Option<String> {
        lock(&self.inner.protocol_version).clone()
    }

    /// End the session with a DELETE, which a server may refuse with 405.
    /// [`Transport::close`] does not do this, and neither does the app.
    pub async fn terminate_session(&self) -> Result<(), TransportError> {
        let inner = &self.inner;
        if lock(&inner.session_id).is_none() {
            return Ok(());
        }
        let result = async {
            let response = inner
                .fetch(Method::DELETE, inner.common_headers(), None)
                .await?;
            if !response.status.is_success() && response.status != StatusCode::METHOD_NOT_ALLOWED {
                return Err(TransportError::StreamableHttp {
                    code: i32::from(response.status.as_u16()),
                    message: format!("Failed to terminate session: {}", reason(response.status)),
                });
            }
            *lock(&inner.session_id) = None;
            Ok(())
        }
        .await;
        inner.report_failure(&result);
        result
    }
}

impl Transport for StreamableHttpClientTransport {
    fn start(
        &self,
        handler: Arc<dyn TransportHandler>,
    ) -> BoxFuture<'static, Result<(), TransportError>> {
        let result = if self.inner.started.swap(true, Ordering::SeqCst) {
            Err(TransportError::AlreadyStarted(
                "StreamableHTTPClientTransport already started! If using Client class, note that connect() calls start() automatically.",
            ))
        } else {
            *lock(&self.inner.handler) = Some(handler);
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn send(&self, message: JsonRpcMessage) -> BoxFuture<'static, Result<(), TransportError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let result = inner.clone().post(message).await;
            inner.report_failure(&result);
            result
        })
    }

    /// Abandon what is in flight. No request is made: the session is not terminated.
    fn close(&self) -> BoxFuture<'static, ()> {
        self.inner.aborted.cancel();
        let handler = lock(&self.inner.handler).clone();
        if let Some(handler) = handler {
            handler.on_close();
        }
        Box::pin(std::future::ready(()))
    }

    fn set_protocol_version(&self, version: &str) {
        *lock(&self.inner.protocol_version) = Some(version.to_owned());
    }
}

impl Inner {
    fn handler(&self) -> Option<Arc<dyn TransportHandler>> {
        lock(&self.handler).clone()
    }

    fn report(&self, error: &TransportError) {
        if let Some(handler) = self.handler() {
            handler.on_error(error);
        }
    }

    fn report_failure<T>(&self, result: &Result<T, TransportError>) {
        if let Err(error) = result {
            self.report(error);
        }
    }

    fn common_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let mut set = |name: HeaderName, value: Option<String>| {
            if let Some(value) = value.and_then(|value| HeaderValue::from_str(&value).ok()) {
                headers.insert(name, value);
            }
        };
        set(SESSION_ID, lock(&self.session_id).clone());
        set(PROTOCOL_VERSION, lock(&self.protocol_version).clone());
        for (name, value) in &self.options.headers {
            headers.insert(name.clone(), value.clone());
        }
        headers
    }

    /// One request, abandoned if the transport closes before its headers arrive.
    async fn fetch(
        &self,
        method: Method,
        headers: HeaderMap,
        body: Option<Bytes>,
    ) -> Result<HttpResponse, TransportError> {
        let request = HttpRequest {
            method,
            url: self.url.clone(),
            headers,
            body,
        };
        tokio::select! {
            biased;
            () = self.aborted.cancelled() => Err(TransportError::Aborted),
            response = self.http.send(request) => response.map_err(TransportError::Http),
        }
    }

    async fn read_body(&self, mut body: HttpBody) -> Result<Vec<u8>, TransportError> {
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                () = self.aborted.cancelled() => return Err(TransportError::Aborted),
                chunk = body.next() => chunk,
            };
            match chunk {
                Some(Ok(chunk)) => bytes.extend_from_slice(&chunk),
                Some(Err(error)) => return Err(TransportError::Http(error)),
                None => return Ok(bytes),
            }
        }
    }

    async fn post(self: Arc<Self>, message: JsonRpcMessage) -> Result<(), TransportError> {
        let mut headers = self.common_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        let body = Bytes::from(message.to_json());
        let response = self.fetch(Method::POST, headers, Some(body)).await?;

        // A session starts with whichever answer first names one.
        if let Some(session_id) = header_value(&response.headers, SESSION_ID.as_str())
            && !session_id.is_empty()
        {
            *lock(&self.session_id) = Some(session_id);
        }

        if !response.status.is_success() {
            let text = match self.read_body(response.body).await {
                Ok(bytes) => decode_text(&bytes),
                Err(_) => "null".to_owned(),
            };
            return Err(TransportError::StreamableHttp {
                code: i32::from(response.status.as_u16()),
                message: format!("Error POSTing to endpoint: {text}"),
            });
        }

        if response.status == StatusCode::ACCEPTED {
            drop(response.body);
            // The handshake is over: listen for what the server sends on its
            // own, if it offers that.
            if message.is_initialized_notification() {
                let transport = self.clone();
                tokio::spawn(async move {
                    // The failure was reported when it happened.
                    let _ = transport.open_stream(StreamOptions::default()).await;
                });
            }
            return Ok(());
        }

        if !message.is_request() {
            return Ok(());
        }
        let content_type = header_value(&response.headers, CONTENT_TYPE.as_str());
        match media_type_essence(content_type.as_deref()).as_deref() {
            Some("text/event-stream") => {
                self.read_stream(response.body, false);
                Ok(())
            }
            Some("application/json") => {
                let bytes = self.read_body(response.body).await?;
                let data = json::parse_bytes(&bytes).map_err(TransportError::InvalidJson)?;
                let messages = match &data {
                    Value::Array(items) => items
                        .iter()
                        .map(JsonRpcMessage::parse)
                        .collect::<Result<Vec<_>, _>>(),
                    single => JsonRpcMessage::parse(single).map(|message| vec![message]),
                }
                .map_err(TransportError::InvalidMessage)?;
                if let Some(handler) = self.handler() {
                    for message in messages {
                        handler.on_message(message);
                    }
                }
                Ok(())
            }
            _ => Err(TransportError::StreamableHttp {
                code: -1,
                message: format!(
                    "Unexpected content type: {}",
                    content_type.as_deref().unwrap_or("null")
                ),
            }),
        }
    }

    /// Open the stream of server-initiated messages with a GET. A server that
    /// does not offer one answers 405, which is not an error.
    fn open_stream(
        self: Arc<Self>,
        options: StreamOptions,
    ) -> BoxFuture<'static, Result<(), TransportError>> {
        Box::pin(async move {
            let result = async {
                let mut headers = self.common_headers();
                headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
                if let Some(token) = options.resumption_token.as_deref()
                    && let Ok(token) = HeaderValue::from_str(token)
                {
                    headers.insert(LAST_EVENT_ID, token);
                }
                let response = self.fetch(Method::GET, headers, None).await?;
                if !response.status.is_success() {
                    if response.status == StatusCode::METHOD_NOT_ALLOWED {
                        return Ok(());
                    }
                    return Err(TransportError::StreamableHttp {
                        code: i32::from(response.status.as_u16()),
                        message: format!("Failed to open SSE stream: {}", reason(response.status)),
                    });
                }
                self.clone().read_stream(response.body, true);
                Ok(())
            }
            .await;
            self.report_failure(&result);
            result
        })
    }

    /// Read an event stream in the background, handing over each message as it completes.
    ///
    /// A request whose stream ends without its answer is not failed here: like
    /// the SDK, the client learns of it when the request times out.
    fn read_stream(self: Arc<Self>, mut body: HttpBody, reconnectable: bool) {
        tokio::spawn(async move {
            let mut parser = SseParser::new();
            let mut last_event_id = None;
            // An event with an id makes the stream resumable.
            let mut resumable = reconnectable;
            let mut received_response = false;

            loop {
                let chunk = tokio::select! {
                    biased;
                    () = self.aborted.cancelled() => return,
                    chunk = body.next() => chunk,
                };
                let chunk = match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(error)) => {
                        self.report(&TransportError::Notice(format!(
                            "SSE stream disconnected: Error: {error}"
                        )));
                        break;
                    }
                    None => break,
                };
                for item in parser.feed(&chunk) {
                    let event = match item {
                        SseItem::Retry(milliseconds) => {
                            *lock(&self.server_retry) = Some(Duration::from_millis(milliseconds));
                            continue;
                        }
                        SseItem::Event(event) => event,
                    };
                    if let Some(id) = event.id.filter(|id| !id.is_empty()) {
                        last_event_id = Some(id);
                        resumable = true;
                    }
                    // Priming events and keep-alives carry no data.
                    if event.data.is_empty() {
                        continue;
                    }
                    if event.event.as_deref().is_some_and(|name| name != "message") {
                        continue;
                    }
                    let message = json::parse(&event.data)
                        .map_err(TransportError::InvalidJson)
                        .and_then(|data| {
                            JsonRpcMessage::parse(&data).map_err(TransportError::InvalidMessage)
                        });
                    match message {
                        Ok(message) => {
                            if matches!(message, JsonRpcMessage::Result { .. }) {
                                received_response = true;
                            }
                            if let Some(handler) = self.handler() {
                                handler.on_message(message);
                            }
                        }
                        Err(error) => self.report(&error),
                    }
                }
            }

            // The server may end a stream it expects the client to resume.
            // A stream that delivered its answer is complete.
            if resumable && !received_response && !self.aborted.is_cancelled() {
                self.schedule_reconnection(
                    StreamOptions {
                        resumption_token: last_event_id,
                    },
                    0,
                );
            }
        });
    }

    fn schedule_reconnection(self: Arc<Self>, options: StreamOptions, attempt: u32) {
        let reconnection = &self.options.reconnection;
        if attempt >= reconnection.max_retries {
            self.report(&TransportError::Notice(format!(
                "Maximum reconnection attempts ({}) exceeded.",
                reconnection.max_retries
            )));
            return;
        }
        let server_retry = *lock(&self.server_retry);
        let delay = server_retry.unwrap_or_else(|| {
            let grown = reconnection.initial_reconnection_delay.as_secs_f64()
                * reconnection
                    .reconnection_delay_grow_factor
                    .powf(f64::from(attempt));
            Duration::from_secs_f64(
                grown
                    .min(reconnection.max_reconnection_delay.as_secs_f64())
                    .max(0.0),
            )
        });

        tokio::spawn(async move {
            tokio::select! {
                biased;
                () = self.aborted.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
            if let Err(error) = self.clone().open_stream(options.clone()).await {
                self.report(&TransportError::Notice(format!(
                    "Failed to reconnect SSE stream: {error}"
                )));
                self.schedule_reconnection(options, attempt + 1);
            }
        });
    }
}

/// `Response.text()`: UTF-8 with invalid sequences replaced, without a byte order mark.
fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

/// The reason phrase is not part of what the HTTP function reports; the
/// standard one for the status stands in for `response.statusText`.
fn reason(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or_default()
}
