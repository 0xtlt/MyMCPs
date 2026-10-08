use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use http::header::{
    AUTHORIZATION, CONTENT_ENCODING, CONTENT_LANGUAGE, CONTENT_LENGTH, CONTENT_LOCATION,
    CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, LOCATION,
};
use http::{Method, StatusCode};
use serde::de::DeserializeOwned;
use tokio::time::{Instant, Sleep};
use url::Url;

const MAX_REDIRECTS: usize = 5;

/// Most one upstream response may deliver, counted after decompression.
/// Generous for real tool results (base64 screenshots and file contents run to
/// a few MiB) while keeping one upstream from filling the memory of the single
/// gateway process.
pub const MAX_UPSTREAM_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Error responses only become diagnostics, so far less of them is read.
pub const MAX_UPSTREAM_ERROR_RESPONSE_BYTES: usize = 64 * 1024;

/// Sent with a request that names no client of its own: some servers refuse
/// a request without a `User-Agent`. The Node app sent `node` here, which
/// said nothing about who was calling.
pub const DEFAULT_USER_AGENT: &str = concat!("MyMCPs/", env!("CARGO_PKG_VERSION"));

/// Node's HTTP client gave up on a connection that took longer than this to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Node's HTTP client gave up on a response that stayed silent for this long,
/// before its headers or between two chunks of its body. A request without a
/// timeout of its own, and an event stream left open, rely on it.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Ports `fetch` refuses to connect to, whatever the host: those of mail, DNS,
/// file sharing and other services that would take an HTTP request for commands
/// of their own protocol. Node's `fetch` enforced this list of the Fetch standard.
const BAD_PORTS: [u16; 82] = [
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6679, 6697, 10080,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamResponseLimits {
    /// Limit for a 2xx body. Reading past it fails.
    pub max_response_bytes: Option<usize>,
    /// Limit for any other body. It is cut there, since callers only quote it.
    pub max_error_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("{label} redirected to a different origin")]
    CrossOriginRedirect { label: String },
    #[error("{label} exceeded {max} redirects", max = MAX_REDIRECTS)]
    TooManyRedirects { label: String },
    #[error("{label} response exceeded {max_bytes} bytes")]
    ResponseTooLarge { label: String, max_bytes: usize },
    /// The `timeout` of the request ran out, before the response or while its
    /// body was read.
    #[error("The operation was aborted due to timeout")]
    Timeout,
    /// A redirect named a location that is not a URL.
    #[error("Invalid URL")]
    InvalidRedirectUrl,
    #[error("Request with GET/HEAD method cannot have body.")]
    BodyNotAllowed,
    #[error("invalid {name} header")]
    InvalidHeader { name: String },
    /// The URL names a port `fetch` never connects to, such as 25.
    #[error("fetch failed")]
    BadPort,
    /// No response came: the request could not be sent, or the upstream did not
    /// answer. The source says why, without the URL, which may carry a credential.
    #[error("fetch failed")]
    Transport(#[source] Arc<reqwest::Error>),
    /// The response broke off while its body was read.
    #[error("terminated")]
    Terminated(#[source] Arc<reqwest::Error>),
    /// A [`Fetcher::offline`] was left with a request no answer was given for.
    #[error("fetch failed")]
    Offline,
    #[error("{0}")]
    Json(#[source] Arc<serde_json::Error>),
}

impl From<reqwest::Error> for FetchError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(Arc::new(error.without_url()))
    }
}

/// One request, as `fetch` takes it. The body is sent as is: no `Content-Type`
/// is added for it.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub method: Method,
    /// Credentials in the URL are sent as HTTP Basic authentication, unless
    /// `headers` already authorize the request.
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
    /// Bounds the whole exchange: every redirect and the read of the response
    /// body. Without one the request is only bound by the idle limits of the
    /// client. To cancel a request, drop its future or its body.
    pub timeout: Option<Duration>,
}

impl FetchRequest {
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            body: None,
            timeout: None,
        }
    }

    pub fn get(url: Url) -> Self {
        Self::new(Method::GET, url)
    }

    pub fn post(url: Url) -> Self {
        Self::new(Method::POST, url)
    }

    /// Set a header, replacing its previous values. Whitespace around the value
    /// is dropped, as `fetch` does.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, FetchError> {
        set_header(&mut self.headers, name, value)?;
        Ok(self)
    }

    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// One request as it leaves the gateway: one hop of a fetch, once the
/// credentials of its URL have become its `Authorization` header.
#[derive(Debug, Clone)]
pub struct SentRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
}

impl SentRequest {
    /// The value of a header, its values joined by `, ` when it was set more
    /// than once.
    pub fn header(&self, name: &str) -> Option<String> {
        header_text(&self.headers, name)
    }

    /// The body as text, empty when there is none.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(self.body.as_deref().unwrap_or_default()).into_owned()
    }
}

/// The response a test gives in place of an upstream, see [`Fetcher::answering`].
#[derive(Debug, Clone)]
pub struct CannedResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl CannedResponse {
    /// A response without headers or body.
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
    }

    /// A JSON document, with its content type.
    pub fn json(status: StatusCode, body: &serde_json::Value) -> Self {
        let mut response = Self::new(status).body(body.to_string());
        response
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    }

    /// Set a header, replacing its previous values.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, FetchError> {
        set_header(&mut self.headers, name, value)?;
        Ok(self)
    }

    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }
}

fn set_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), FetchError> {
    let invalid = || FetchError::InvalidHeader {
        name: name.to_owned(),
    };
    let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
    headers.insert(header, header_value(value).ok_or_else(invalid)?);
    Ok(())
}

/// A header value as `fetch` sends it: one byte per character, without the
/// whitespace around it. `None` for what cannot be sent.
fn header_value(value: &str) -> Option<HeaderValue> {
    let trimmed = value.trim_matches(['\t', '\n', '\r', ' ']);
    let bytes = trimmed
        .chars()
        .map(|character| u8::try_from(u32::from(character)).ok())
        .collect::<Option<Vec<u8>>>()?;
    HeaderValue::from_bytes(&bytes).ok()
}

/// How `fetch` reads a header: its values joined, one character per byte.
fn header_text(headers: &HeaderMap, name: impl http::header::AsHeaderName) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(name)
        .iter()
        .map(|value| {
            value
                .as_bytes()
                .iter()
                .map(|byte| char::from(*byte))
                .collect()
        })
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

/// `decodeURIComponent`, which gives up on a malformed escape and on bytes
/// that are not UTF-8.
fn decode_uri_component(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let high = char::from(bytes.next()?).to_digit(16)?;
        let low = char::from(bytes.next()?).to_digit(16)?;
        decoded.push(u8::try_from(high * 16 + low).ok()?);
    }
    String::from_utf8(decoded).ok()
}

fn decoded_url_credential(value: &str) -> String {
    decode_uri_component(value).unwrap_or_else(|| value.to_owned())
}

fn request_with_url_credentials(mut url: Url, mut headers: HeaderMap) -> (Url, HeaderMap) {
    let password = url.password().unwrap_or_default();
    if !url.username().is_empty() || !password.is_empty() {
        if !headers.contains_key(AUTHORIZATION) {
            let username = decoded_url_credential(url.username());
            let password = decoded_url_credential(password);
            let basic = format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            );
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&basic).expect("base64 is a valid header value"),
            );
        }
        // Only a URL without a host refuses this, and such a URL has no credentials.
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }
    (url, headers)
}

fn is_bad_port(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_some_and(|port| BAD_PORTS.contains(&port))
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

fn switches_to_get(status: StatusCode, method: &Method) -> bool {
    (status == StatusCode::SEE_OTHER && method != Method::GET && method != Method::HEAD)
        || ((status == StatusCode::MOVED_PERMANENTLY || status == StatusCode::FOUND)
            && method == Method::POST)
}

fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        // Redirects are followed here, one checked hop at a time.
        .redirect(reqwest::redirect::Policy::none())
        // Node's HTTP client did not read the proxy variables of the environment.
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(IDLE_TIMEOUT)
        .user_agent(DEFAULT_USER_AGENT)
        .build()
}

/// The HTTP client every outbound request of the gateway shares, so
/// connections are pooled. It never follows a redirect by itself.
pub fn shared_client() -> Result<&'static reqwest::Client, FetchError> {
    static CLIENT: OnceLock<Result<reqwest::Client, FetchError>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_client().map_err(FetchError::from))
        .as_ref()
        .map_err(Clone::clone)
}

async fn before<T>(
    deadline: Option<Instant>,
    future: impl Future<Output = T>,
) -> Result<T, FetchError> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, future)
            .await
            .map_err(|_| FetchError::Timeout),
        None => Ok(future.await),
    }
}

type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, FetchError>> + Send + Sync>>;

/// A body that is already there, as the stream of a body that arrives.
fn whole_body(body: Result<Bytes, FetchError>) -> ChunkStream {
    match body {
        Ok(bytes) if bytes.is_empty() => Box::pin(futures::stream::empty()),
        body => Box::pin(futures::stream::iter([body])),
    }
}

/// A response as it comes back from one hop, before its body is limited.
struct RawResponse {
    status: StatusCode,
    status_text: String,
    headers: HeaderMap,
    chunks: ChunkStream,
}

impl RawResponse {
    fn from_network(response: reqwest::Response) -> Self {
        let status = response.status();
        // hyper only keeps a reason phrase that is not the usual one for the status.
        let status_text = match response.extensions().get::<hyper::ext::ReasonPhrase>() {
            Some(reason) => reason
                .as_bytes()
                .iter()
                .map(|byte| char::from(*byte))
                .collect(),
            None => status.canonical_reason().unwrap_or_default().to_owned(),
        };
        let headers = response.headers().clone();
        let chunks = response.bytes_stream().map(|chunk| {
            chunk.map_err(|error| FetchError::Terminated(Arc::new(error.without_url())))
        });
        Self {
            status,
            status_text,
            headers,
            chunks: Box::pin(chunks),
        }
    }

    fn canned(response: CannedResponse) -> Self {
        Self {
            status: response.status,
            status_text: response
                .status
                .canonical_reason()
                .unwrap_or_default()
                .to_owned(),
            headers: response.headers,
            chunks: whole_body(Ok(response.body)),
        }
    }
}

type Answer = dyn Fn(&SentRequest) -> Option<CannedResponse> + Send + Sync;

enum Transport {
    Network,
    Offline,
    Answering {
        answer: Box<Answer>,
        rest: Arc<Transport>,
    },
}

impl Transport {
    async fn send(&self, request: &SentRequest) -> Result<RawResponse, FetchError> {
        let mut transport = self;
        loop {
            match transport {
                Transport::Network => {
                    let mut hop =
                        reqwest::Request::new(request.method.clone(), request.url.clone());
                    *hop.headers_mut() = request.headers.clone();
                    *hop.body_mut() = request.body.clone().map(reqwest::Body::from);
                    let response = shared_client()?.execute(hop).await?;
                    return Ok(RawResponse::from_network(response));
                }
                Transport::Offline => return Err(FetchError::Offline),
                Transport::Answering { answer, rest } => match answer(request) {
                    Some(response) => return Ok(RawResponse::canned(response)),
                    None => transport = rest,
                },
            }
        }
    }
}

/// The body of a response, which stops at its limit and at the deadline of the
/// request. Dropping it stops the download.
pub struct BodyStream {
    chunks: Option<ChunkStream>,
    deadline: Option<Pin<Box<Sleep>>>,
    endpoint_label: String,
    max_bytes: usize,
    truncates: bool,
    received: usize,
}

impl BodyStream {
    fn limited(
        chunks: ChunkStream,
        endpoint_label: &str,
        max_bytes: usize,
        truncates: bool,
        deadline: Option<Instant>,
    ) -> Self {
        Self {
            chunks: Some(chunks),
            deadline: deadline.map(|deadline| Box::pin(tokio::time::sleep_until(deadline))),
            endpoint_label: endpoint_label.to_owned(),
            max_bytes,
            truncates,
            received: 0,
        }
    }

    /// A body that was already read, or that failed to be.
    fn replayed(body: Result<Bytes, FetchError>) -> Self {
        Self {
            chunks: Some(whole_body(body)),
            deadline: None,
            endpoint_label: String::new(),
            max_bytes: usize::MAX,
            truncates: true,
            received: 0,
        }
    }
}

impl Stream for BodyStream {
    type Item = Result<Bytes, FetchError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let Some(chunks) = this.chunks.as_mut() else {
            return Poll::Ready(None);
        };
        if let Some(deadline) = this.deadline.as_mut()
            && deadline.as_mut().poll(context).is_ready()
        {
            this.chunks = None;
            return Poll::Ready(Some(Err(FetchError::Timeout)));
        }

        let chunk = match ready!(chunks.as_mut().poll_next(context)) {
            Some(Ok(chunk)) => chunk,
            end => {
                this.chunks = None;
                return Poll::Ready(end);
            }
        };

        this.received = this.received.saturating_add(chunk.len());
        if this.received <= this.max_bytes {
            return Poll::Ready(Some(Ok(chunk)));
        }

        // Dropping the upstream body stops the download.
        this.chunks = None;
        if !this.truncates {
            return Poll::Ready(Some(Err(FetchError::ResponseTooLarge {
                label: this.endpoint_label.clone(),
                max_bytes: this.max_bytes,
            })));
        }
        let kept = chunk.len().saturating_sub(this.received - this.max_bytes);
        Poll::Ready((kept > 0).then(|| Ok(chunk.slice(..kept))))
    }
}

impl fmt::Debug for BodyStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BodyStream")
            .field("max_bytes", &self.max_bytes)
            .field("truncates", &self.truncates)
            .field("received", &self.received)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum Body {
    Unread(BodyStream),
    Read(Result<Bytes, FetchError>),
}

/// The response to a [`FetchRequest`], with a body that stops at its limit.
///
/// The body can be read whole with [`bytes`](Self::bytes), [`text`](Self::text)
/// and [`json`](Self::json), which keep it so it can be read again, or chunk by
/// chunk with [`into_stream`](Self::into_stream).
#[derive(Debug)]
pub struct FetchResponse {
    status: StatusCode,
    status_text: String,
    headers: HeaderMap,
    url: Url,
    body: Body,
}

impl FetchResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The reason phrase the server sent with the status, such as `Not Found`.
    pub fn status_text(&self) -> &str {
        &self.status_text
    }

    /// Whether the status is a 2xx one.
    pub fn ok(&self) -> bool {
        self.status.is_success()
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The value of a header, its values joined by `, ` when it was sent more
    /// than once.
    pub fn header(&self, name: &str) -> Option<String> {
        header_text(&self.headers, name)
    }

    /// The URL that answered, after redirects and without credentials.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Read the whole body. A 2xx body larger than its limit fails with
    /// [`FetchError::ResponseTooLarge`]; any other body is cut at its limit.
    ///
    /// The outcome is kept: reading again returns it again. If the future is
    /// dropped before it completes, what it had read is lost.
    pub async fn bytes(&mut self) -> Result<Bytes, FetchError> {
        let stream = match &mut self.body {
            Body::Read(outcome) => return outcome.clone(),
            Body::Unread(stream) => stream,
        };
        let mut read = BytesMut::new();
        let outcome = loop {
            match stream.next().await {
                Some(Ok(chunk)) => read.extend_from_slice(&chunk),
                Some(Err(error)) => break Err(error),
                None => break Ok(read.freeze()),
            }
        };
        self.body = Body::Read(outcome.clone());
        outcome
    }

    /// Read the whole body as UTF-8, replacing what is not, like `Response.text()`.
    pub async fn text(&mut self) -> Result<String, FetchError> {
        let bytes = self.bytes().await?;
        let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        Ok(String::from_utf8_lossy(text).into_owned())
    }

    /// Read the whole body as JSON, like `Response.json()`.
    pub async fn json<T: DeserializeOwned>(&mut self) -> Result<T, FetchError> {
        let text = self.text().await?;
        serde_json::from_str(&text).map_err(|error| FetchError::Json(Arc::new(error)))
    }

    /// The body as it arrives, for an event stream or a large download. The
    /// limit and the timeout of the request apply to it in the same way.
    pub fn into_stream(self) -> BodyStream {
        match self.body {
            Body::Unread(stream) => stream,
            Body::Read(outcome) => BodyStream::replayed(outcome),
        }
    }
}

/// Hand the response back with a body that stops at its limit. Callers read
/// bodies whole (`json()`, `text()`) or as an event stream, so the limit has to
/// sit in the stream they read from.
fn with_limited_body(
    response: RawResponse,
    url: &Url,
    endpoint_label: &str,
    limits: UpstreamResponseLimits,
    deadline: Option<Instant>,
) -> FetchResponse {
    let truncates = !response.status.is_success();
    let max_bytes = if truncates {
        limits
            .max_error_response_bytes
            .unwrap_or(MAX_UPSTREAM_ERROR_RESPONSE_BYTES)
    } else {
        limits
            .max_response_bytes
            .unwrap_or(MAX_UPSTREAM_RESPONSE_BYTES)
    };
    let mut url = url.clone();
    url.set_fragment(None);

    FetchResponse {
        status: response.status,
        status_text: response.status_text,
        headers: response.headers,
        url,
        body: Body::Unread(BodyStream::limited(
            response.chunks,
            endpoint_label,
            max_bytes,
            truncates,
            deadline,
        )),
    }
}

/// Where the requests of a fetch go. The application uses [`Fetcher::shared`].
/// A test builds one that answers by itself, as the TypeScript tests replaced
/// `fetch`: whatever answers, redirects, credentials, limits and timeouts are
/// handled in the same way.
#[derive(Clone)]
pub struct Fetcher {
    transport: Arc<Transport>,
}

impl Fetcher {
    /// Sends requests over the network, through [`shared_client`].
    pub fn shared() -> Self {
        Self {
            transport: Arc::new(Transport::Network),
        }
    }

    /// Sends nothing: a request fails like one the network could not carry.
    /// With [`answering`](Self::answering), for a test that must stay off the
    /// network.
    pub fn offline() -> Self {
        Self {
            transport: Arc::new(Transport::Offline),
        }
    }

    /// Ask `answer` before sending a request. What it returns is the response
    /// to that request, and a request it leaves (`None`) goes on to this
    /// fetcher. The answer added last is asked first.
    pub fn answering(
        self,
        answer: impl Fn(&SentRequest) -> Option<CannedResponse> + Send + Sync + 'static,
    ) -> Self {
        Self {
            transport: Arc::new(Transport::Answering {
                answer: Box::new(answer),
                rest: self.transport,
            }),
        }
    }

    /// Follow ordinary endpoint redirects without forwarding credentials to
    /// another origin. A small redirect cap avoids loops while supporting
    /// canonical paths. Response bodies are size-limited, see
    /// [`UpstreamResponseLimits`].
    ///
    /// `endpoint_label` names the endpoint in the errors an administrator reads.
    pub async fn fetch_with_same_origin_redirects(
        &self,
        request: FetchRequest,
        endpoint_label: &str,
        limits: UpstreamResponseLimits,
    ) -> Result<FetchResponse, FetchError> {
        // Every hop, and the body of the final response, stays within the caller's timeout.
        let deadline = match request.timeout {
            Some(timeout) => Instant::now().checked_add(timeout),
            None => None,
        };

        let FetchRequest {
            mut method,
            url,
            headers,
            mut body,
            ..
        } = request;
        if body.is_some() && (method == Method::GET || method == Method::HEAD) {
            return Err(FetchError::BodyNotAllowed);
        }
        let (mut url, mut headers) = request_with_url_credentials(url, headers);

        let mut redirects = 0;
        loop {
            if is_bad_port(&url) {
                return Err(FetchError::BadPort);
            }
            let hop = SentRequest {
                method: method.clone(),
                url: url.clone(),
                headers: headers.clone(),
                body: body.clone(),
            };
            let response = before(deadline, self.transport.send(&hop)).await??;

            let location = if is_redirect(response.status) {
                header_text(&response.headers, LOCATION).filter(|location| !location.is_empty())
            } else {
                None
            };
            let Some(location) = location else {
                return Ok(with_limited_body(
                    response,
                    &url,
                    endpoint_label,
                    limits,
                    deadline,
                ));
            };

            let next_url = url
                .join(&location)
                .map_err(|_| FetchError::InvalidRedirectUrl)?;
            if next_url.origin() != url.origin() {
                return Err(FetchError::CrossOriginRedirect {
                    label: endpoint_label.to_owned(),
                });
            }
            if redirects >= MAX_REDIRECTS {
                return Err(FetchError::TooManyRedirects {
                    label: endpoint_label.to_owned(),
                });
            }

            if switches_to_get(response.status, &method) {
                method = Method::GET;
                body = None;
                for header in [
                    CONTENT_ENCODING,
                    CONTENT_LANGUAGE,
                    CONTENT_LENGTH,
                    CONTENT_LOCATION,
                    CONTENT_TYPE,
                ] {
                    headers.remove(header);
                }
            }
            drop(response);

            (url, headers) = request_with_url_credentials(next_url, headers);
            redirects += 1;
        }
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::shared()
    }
}

impl fmt::Debug for Fetcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let transport = match *self.transport {
            Transport::Network => "network",
            Transport::Offline => "offline",
            Transport::Answering { .. } => "answering",
        };
        formatter
            .debug_struct("Fetcher")
            .field("transport", &transport)
            .finish()
    }
}

/// [`Fetcher::fetch_with_same_origin_redirects`] over the network.
pub async fn fetch_with_same_origin_redirects(
    request: FetchRequest,
    endpoint_label: &str,
    limits: UpstreamResponseLimits,
) -> Result<FetchResponse, FetchError> {
    Fetcher::shared()
        .fetch_with_same_origin_redirects(request, endpoint_label, limits)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    /// A body of `chunks` chunks of `chunk_bytes`, or endless, that records being dropped.
    struct StreamedBody {
        chunk_bytes: usize,
        chunks: usize,
        state: Arc<StreamedState>,
    }

    #[derive(Default)]
    struct StreamedState {
        sent: AtomicUsize,
        cancelled: AtomicBool,
    }

    impl Stream for StreamedBody {
        type Item = Result<Bytes, FetchError>;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if self.state.sent.load(Ordering::SeqCst) >= self.chunks {
                return Poll::Ready(None);
            }
            self.state.sent.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok(Bytes::from(vec![b'a'; self.chunk_bytes]))))
        }
    }

    impl Drop for StreamedBody {
        fn drop(&mut self) {
            if self.state.sent.load(Ordering::SeqCst) < self.chunks {
                self.state.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    fn streamed_body(
        chunk_bytes: usize,
        chunks: usize,
        max_bytes: usize,
        truncates: bool,
    ) -> (BodyStream, Arc<StreamedState>) {
        let state = Arc::new(StreamedState::default());
        let body = StreamedBody {
            chunk_bytes,
            chunks,
            state: Arc::clone(&state),
        };
        (
            BodyStream::limited(Box::pin(body), "MCP endpoint", max_bytes, truncates, None),
            state,
        )
    }

    async fn read(mut body: BodyStream) -> Result<Vec<u8>, FetchError> {
        let mut read = Vec::new();
        while let Some(chunk) = body.next().await {
            read.extend_from_slice(&chunk?);
        }
        Ok(read)
    }

    #[tokio::test]
    async fn fails_the_read_of_a_successful_body_past_its_limit_and_stops_the_download() {
        let (mut body, state) = streamed_body(400, usize::MAX, 1000, false);

        assert_eq!(body.next().await.unwrap().unwrap().len(), 400);
        assert_eq!(body.next().await.unwrap().unwrap().len(), 400);
        let error = body.next().await.unwrap().unwrap_err();
        assert_eq!(
            error.to_string(),
            "MCP endpoint response exceeded 1000 bytes"
        );
        assert!(state.cancelled.load(Ordering::SeqCst));
        assert_eq!(state.sent.load(Ordering::SeqCst), 3);
        assert!(body.next().await.is_none());
    }

    #[tokio::test]
    async fn cuts_an_error_body_short_instead_of_reading_it_whole() {
        let (body, state) = streamed_body(400, usize::MAX, 1000, true);

        assert_eq!(read(body).await.unwrap(), vec![b'a'; 1000]);
        assert!(state.cancelled.load(Ordering::SeqCst));
        assert_eq!(state.sent.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn ends_an_error_body_that_stops_exactly_at_its_limit() {
        let (body, state) = streamed_body(500, usize::MAX, 1000, true);

        assert_eq!(read(body).await.unwrap(), vec![b'a'; 1000]);
        assert!(state.cancelled.load(Ordering::SeqCst));
        assert_eq!(state.sent.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn passes_a_body_within_the_limit_through_unchanged() {
        for truncates in [false, true] {
            let (body, state) = streamed_body(250, 4, 1000, truncates);

            assert_eq!(read(body).await.unwrap(), vec![b'a'; 1000]);
            assert!(!state.cancelled.load(Ordering::SeqCst));
            assert_eq!(state.sent.load(Ordering::SeqCst), 4);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fails_the_read_of_a_body_at_the_deadline_of_its_request() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stalled: ChunkStream = Box::pin(
            futures::stream::iter([Ok(Bytes::from_static(b"partial"))])
                .chain(futures::stream::pending()),
        );
        let mut body = BodyStream::limited(stalled, "MCP endpoint", 1000, false, Some(deadline));

        assert_eq!(body.next().await.unwrap().unwrap(), "partial");
        let error = body.next().await.unwrap().unwrap_err();
        assert!(matches!(error, FetchError::Timeout));
        assert_eq!(Instant::now(), deadline);
        assert!(body.next().await.is_none());
    }

    #[test]
    fn decodes_url_credentials_like_decode_uri_component() {
        let cases = [
            ("user", "user"),
            ("p%40ss", "p@ss"),
            ("b%3A%2F", "b:/"),
            ("a%25", "a%"),
            ("us%C3%A9r", "us\u{e9}r"),
            ("a%c3%a9", "a\u{e9}"),
            ("p%E2%82%AC", "p\u{20ac}"),
            // Left as written: malformed escapes, and bytes that are not UTF-8.
            ("u%zz", "u%zz"),
            ("a%2", "a%2"),
            ("a%", "a%"),
            ("a%+1", "a%+1"),
            ("p%ff", "p%ff"),
            ("a%ED%A0%80", "a%ED%A0%80"),
            ("a%C0%80", "a%C0%80"),
            ("a%F4%90%80%80", "a%F4%90%80%80"),
        ];
        for (value, expected) in cases {
            assert_eq!(decoded_url_credential(value), expected, "{value}");
        }
    }

    #[test]
    fn turns_url_credentials_into_basic_authentication() {
        let cases = [
            (
                "https://user:p%40ss@trusted.example/mcp",
                "Basic dXNlcjpwQHNz",
            ),
            ("https://u%zz:p%41@host/", "Basic dSV6ejpwQQ=="),
            ("https://user@host/", "Basic dXNlcjo="),
            ("https://user:@host/", "Basic dXNlcjo="),
            ("https://:pw@host/", "Basic OnB3"),
            (
                "https://us%C3%A9r:p%E2%82%AC@host/",
                "Basic dXPDqXI6cOKCrA==",
            ),
            ("https://a%25:b%3A%2F@host/", "Basic YSU6Yjov"),
            (
                "https://\u{fc}ser:p\u{e4} ss@host/",
                "Basic w7xzZXI6cMOkIHNz",
            ),
            ("https://a:b:c@host/", "Basic YTpiOmM="),
            ("https://a%ED%A0%80:x@host/", "Basic YSVFRCVBMCU4MDp4"),
        ];
        for (value, expected) in cases {
            let (url, headers) =
                request_with_url_credentials(Url::parse(value).unwrap(), HeaderMap::new());
            assert_eq!(headers.get(AUTHORIZATION).unwrap(), expected, "{value}");
            assert_eq!(url.username(), "");
            assert_eq!(url.password(), None);
            assert!(!url.as_str().contains('@'), "{url}");
        }
    }

    #[test]
    fn leaves_a_url_without_credentials_and_an_authorized_request_alone() {
        let plain = Url::parse("https://trusted.example/mcp?key=value").unwrap();
        let (url, headers) = request_with_url_credentials(plain.clone(), HeaderMap::new());
        assert_eq!(url, plain);
        assert!(headers.is_empty());

        let mut authorized = HeaderMap::new();
        authorized.insert(AUTHORIZATION, HeaderValue::from_static("Bearer token"));
        let (url, headers) = request_with_url_credentials(
            Url::parse("https://user:secret@trusted.example/mcp").unwrap(),
            authorized,
        );
        assert_eq!(url.as_str(), "https://trusted.example/mcp");
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer token");
    }

    #[test]
    fn switches_to_get_like_a_browser() {
        let cases = [
            (303, Method::POST, true),
            (303, Method::PUT, true),
            (303, Method::DELETE, true),
            (303, Method::GET, false),
            (303, Method::HEAD, false),
            (301, Method::POST, true),
            (302, Method::POST, true),
            (301, Method::PUT, false),
            (302, Method::DELETE, false),
            (301, Method::GET, false),
            (307, Method::POST, false),
            (308, Method::POST, false),
        ];
        for (status, method, expected) in cases {
            let status = StatusCode::from_u16(status).unwrap();
            assert_eq!(
                switches_to_get(status, &method),
                expected,
                "{status} {method}"
            );
        }
    }

    #[test]
    fn knows_the_ports_fetch_never_connects_to() {
        for port in [
            1, 22, 25, 53, 110, 143, 465, 587, 993, 2049, 6000, 6667, 10080,
        ] {
            let url = Url::parse(&format!("http://upstream.example:{port}/mcp")).unwrap();
            assert!(is_bad_port(&url), "{port}");
            let url = Url::parse(&format!("https://upstream.example:{port}/mcp")).unwrap();
            assert!(is_bad_port(&url), "{port}");
        }
        for url in [
            "http://upstream.example/mcp",
            "https://upstream.example/mcp",
            "http://upstream.example:80/mcp",
            "https://upstream.example:443/mcp",
            "http://upstream.example:8080/mcp",
            "http://upstream.example:3333/mcp",
            "http://upstream.example:6379/mcp",
            "http://upstream.example:24/mcp",
            "http://upstream.example:26/mcp",
            "ftp://upstream.example:25/mcp",
        ] {
            assert!(!is_bad_port(&Url::parse(url).unwrap()), "{url}");
        }
        assert_eq!(BAD_PORTS.len(), 82);
        assert!(BAD_PORTS.is_sorted());
    }

    #[test]
    fn normalizes_header_values_like_fetch() {
        let request = || FetchRequest::get(Url::parse("https://trusted.example/").unwrap());

        let sent = request().header("X-Token", "\ttoken\r\n").unwrap();
        assert_eq!(sent.headers.get("x-token").unwrap(), "token");
        let sent = request().header("X-Name", "ok\u{ff}").unwrap();
        assert_eq!(sent.headers.get("x-name").unwrap().as_bytes(), b"ok\xff");
        let replaced = request()
            .header("Accept", "a")
            .unwrap()
            .header("accept", "b")
            .unwrap();
        assert_eq!(replaced.headers.get_all("accept").iter().count(), 1);
        assert_eq!(replaced.headers.get("accept").unwrap(), "b");

        for (name, value) in [
            ("X-Token", "a\nb"),
            ("X-Token", "a\0b"),
            ("X-Token", "\u{20ac}"),
        ] {
            let error = request().header(name, value).unwrap_err();
            assert_eq!(error.to_string(), "invalid X-Token header");
        }
        let error = request().header("bad name", "value").unwrap_err();
        assert_eq!(error.to_string(), "invalid bad name header");
    }
}
