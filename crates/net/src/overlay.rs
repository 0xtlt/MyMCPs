//! Requests that go through a private network, such as a Tailscale tailnet,
//! instead of the one the instance runs in.
//!
//! The network is an [`Overlay`]: it says whether it carries the traffic to a
//! host ([`route`](Overlay::route)) and opens the streams to it
//! ([`dial`](Overlay::dial)). An [`OverlayNetwork`] speaks HTTP/1.1 over those
//! streams, over TLS for `https`, and pools the connections. A [`Fetcher`] that
//! goes [`through`](Fetcher::through) it sends there what the overlay routes
//! and the rest as it did before.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
//! use std::net::SocketAddr;
//! use std::sync::Arc;
//!
//! use mymcps_net::{Fetcher, Overlay, OverlayNetwork, OverlayStream};
//!
//! /// Carries the hosts of one domain, to the one address that serves them.
//! struct Tailnet(SocketAddr);
//!
//! #[async_trait::async_trait]
//! impl Overlay for Tailnet {
//!     fn label(&self) -> String {
//!         "the tailnet".to_owned()
//!     }
//!
//!     async fn route(&self, host: &str, _port: u16) -> std::io::Result<Option<SocketAddr>> {
//!         Ok(host.ends_with(".ts.net").then_some(self.0))
//!     }
//!
//!     async fn dial(&self, address: SocketAddr) -> std::io::Result<OverlayStream> {
//!         Ok(Box::new(tokio::net::TcpStream::connect(address).await?))
//!     }
//! }
//!
//! // One network per overlay, so its connections are pooled.
//! let network = OverlayNetwork::new(Arc::new(Tailnet("100.64.0.7:443".parse()?)));
//! let fetcher = Fetcher::shared().through(&network);
//! # let _ = fetcher;
//! # Ok(())
//! # })
//! # }
//! ```

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use http::header::{ACCEPT_ENCODING, HOST, HeaderValue, USER_AGENT};
use http::{Request, Uri};
use http_body_util::Full;
use hyper::body::{Body, Incoming};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{Instant, Sleep, timeout, timeout_at};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;
use tower_service::Service;
use url::{Host, Position, Url};

use crate::safe_fetch::{
    CONNECT_TIMEOUT, ChunkStream, DEFAULT_USER_AGENT, FetchError, IDLE_TIMEOUT, RawResponse,
    SentRequest, status_text,
};

/// A connection an [`Overlay`] opened: any duplex byte stream.
pub trait OverlayIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> OverlayIo for T {}

/// A connection an [`Overlay`] opened, ready for the bytes of HTTP or TLS.
pub type OverlayStream = Box<dyn OverlayIo>;

/// A private network that carries the traffic to some hosts.
///
/// The gateway asks it, for every hop of a request, whether it carries the
/// traffic to a host, and dials the address it names. The address is whatever
/// the overlay wants to dial, so a name only the overlay can resolve is fine.
#[async_trait]
pub trait Overlay: Send + Sync + 'static {
    /// Names the overlay in the errors an administrator reads.
    fn label(&self) -> String;

    /// Where the overlay reaches `host`, or `None` when it does not carry the
    /// traffic to it and the request goes where it would have gone without
    /// the overlay. `host` is the host of the URL in lowercase, an IPv6 address
    /// without brackets.
    async fn route(&self, host: &str, port: u16) -> io::Result<Option<SocketAddr>>;

    /// Open a connection to an address `route` returned.
    async fn dial(&self, address: SocketAddr) -> io::Result<OverlayStream>;
}

/// An [`Overlay`] with the HTTP client that sends requests over it. Build one
/// per overlay and share it, see [`Fetcher::through`]: it owns the pool of
/// connections, which are reused as long as they stay idle for less than a
/// minute and a half.
#[derive(Clone)]
pub struct OverlayNetwork {
    inner: Arc<Inner>,
}

struct Inner {
    overlay: Arc<dyn Overlay>,
    client: Client<Connector, Full<Bytes>>,
}

impl OverlayNetwork {
    /// The network of `overlay`. TLS to its hosts is checked by the platform,
    /// as it is over the network.
    pub fn new(overlay: Arc<dyn Overlay>) -> Self {
        Self::with_trust(overlay, Trust::Platform)
    }

    /// A network whose TLS only trusts `roots`, for a test with a certificate
    /// of its own.
    #[cfg(test)]
    pub(crate) fn trusting(
        overlay: Arc<dyn Overlay>,
        roots: Vec<tokio_rustls::rustls::pki_types::CertificateDer<'static>>,
    ) -> Self {
        Self::with_trust(overlay, Trust::Roots(roots))
    }

    fn with_trust(overlay: Arc<dyn Overlay>, trust: Trust) -> Self {
        let connector = Connector {
            overlay: Arc::clone(&overlay),
            tls: Arc::new(Tls {
                trust,
                config: OnceLock::new(),
            }),
        };
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .build(connector);
        Self {
            inner: Arc::new(Inner { overlay, client }),
        }
    }

    pub(crate) fn label(&self) -> String {
        self.inner.overlay.label()
    }

    fn failure(&self, reason: impl Into<String>) -> FetchError {
        FetchError::Overlay {
            label: self.label(),
            reason: reason.into(),
        }
    }

    /// Send one hop over the overlay, or `None` when the overlay does not
    /// carry the traffic to its host, or the request is not an HTTP one.
    pub(crate) async fn send(
        &self,
        request: &SentRequest,
    ) -> Result<Option<RawResponse>, FetchError> {
        let Some((host, port)) = target(&request.url) else {
            return Ok(None);
        };

        // The route, the connection and the TLS handshake share one limit.
        let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
        let routed = timeout_at(connect_deadline, self.inner.overlay.route(&host, port))
            .await
            .map_err(|_| self.failure(connect_timed_out()))?
            .map_err(|error| self.failure(reason(&error)))?;
        let Some(address) = routed else {
            return Ok(None);
        };

        let hop = self.request(request, address)?;
        let response = timeout(
            IDLE_TIMEOUT,
            CONNECT_DEADLINE.scope(connect_deadline, self.inner.client.request(hop)),
        )
        .await
        .map_err(|_| self.failure(silent_for()))?
        .map_err(|error| self.failure(reason(&error)))?;

        let (parts, body) = response.into_parts();
        let chunks: ChunkStream = Box::pin(Chunks {
            body,
            idle: Box::pin(tokio::time::sleep(IDLE_TIMEOUT)),
            label: self.label(),
            ended: false,
        });
        Ok(Some(RawResponse::new(
            parts.status,
            status_text(parts.status, parts.extensions.get()),
            parts.headers,
            chunks,
        )))
    }

    fn request(
        &self,
        request: &SentRequest,
        address: SocketAddr,
    ) -> Result<Request<Full<Bytes>>, FetchError> {
        let url = &request.url;
        let uri = destination(url, address).ok_or_else(|| self.failure("invalid address"))?;
        let mut hop = Request::new(Full::new(request.body.clone().unwrap_or_default()));
        *hop.method_mut() = request.method.clone();
        *hop.uri_mut() = uri;
        *hop.headers_mut() = request.headers.clone();

        let headers = hop.headers_mut();
        // The body is not decoded here: ask for the bytes as they are.
        headers.remove(ACCEPT_ENCODING);
        if !headers.contains_key(USER_AGENT) {
            headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
        }
        // The connection goes to the address of the overlay, the server is
        // still the host of the URL.
        if !headers.contains_key(HOST) {
            let host = url.host_str().unwrap_or_default();
            let value = match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            };
            headers.insert(
                HOST,
                HeaderValue::from_str(&value).map_err(|_| self.failure("invalid host"))?,
            );
        }
        Ok(hop)
    }
}

impl fmt::Debug for OverlayNetwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OverlayNetwork")
            .field("label", &self.label())
            .finish_non_exhaustive()
    }
}

/// The host and port of a URL as the overlay is asked for them, or `None` for
/// a URL that is not an HTTP one.
fn target(url: &Url) -> Option<(String, u16)> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = match url.host()? {
        Host::Domain(name) => name.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => address.to_string(),
    };
    Some((host, url.port_or_known_default()?))
}

/// The URI the pooled client is given: the connector only receives the scheme
/// and authority, so they carry what it needs to open a connection, the address
/// to dial and, as the user, the host to verify TLS against. The pool is keyed
/// by them, so a connection is only reused for the same host at the same address.
fn destination(url: &Url, address: SocketAddr) -> Option<Uri> {
    let host: String = url
        .host_str()?
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}://{host}@{address}{}",
        url.scheme(),
        &url[Position::BeforePath..Position::AfterQuery]
    )
    .parse()
    .ok()
}

/// The host and address a [`destination`] carries.
fn dialed(uri: &Uri) -> Option<(String, SocketAddr)> {
    let authority = uri.authority()?.as_str();
    let (host, address) = authority.split_once('@')?;
    let host = (0..host.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(host.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    let host = String::from_utf8(host).ok()?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(&host);
    Some((host.to_owned(), address.parse().ok()?))
}

fn connect_timed_out() -> String {
    format!("no connection within {} s", CONNECT_TIMEOUT.as_secs())
}

fn silent_for() -> String {
    format!("no response within {} s", IDLE_TIMEOUT.as_secs())
}

/// Why an error happened, without the generic layers the HTTP client wraps
/// around it: its causes, from the outermost, or the error when it has none.
fn reason(error: &(dyn Error + 'static)) -> String {
    let mut causes = Vec::new();
    let mut source = error.source();
    while let Some(cause) = source {
        causes.push(cause.to_string());
        source = cause.source();
    }
    if causes.is_empty() {
        error.to_string()
    } else {
        causes.join(": ")
    }
}

tokio::task_local! {
    /// When the connection of the request being sent has to be open: the limit
    /// started before the overlay was asked for the route, which the connector
    /// does not see otherwise.
    static CONNECT_DEADLINE: Instant;
}

/// Which certificates the TLS of an overlay trusts.
enum Trust {
    Platform,
    #[cfg(test)]
    Roots(Vec<tokio_rustls::rustls::pki_types::CertificateDer<'static>>),
}

struct Tls {
    trust: Trust,
    /// Built by the first `https` connection, so a network that only carries
    /// `http` never loads the certificates of the platform.
    config: OnceLock<Result<Arc<ClientConfig>, String>>,
}

impl Tls {
    fn connector(&self) -> io::Result<TlsConnector> {
        self.config
            .get_or_init(|| self.build())
            .as_ref()
            .map(|config| TlsConnector::from(Arc::clone(config)))
            .map_err(|message| io::Error::other(message.clone()))
    }

    fn build(&self) -> Result<Arc<ClientConfig>, String> {
        use tokio_rustls::rustls::client::danger::ServerCertVerifier;
        use tokio_rustls::rustls::crypto::aws_lc_rs::default_provider;

        let provider = Arc::new(default_provider());
        let verifier: Arc<dyn ServerCertVerifier> = match &self.trust {
            Trust::Platform => Arc::new(
                rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
                    .map_err(|error| format!("TLS verification is unavailable: {error}"))?,
            ),
            #[cfg(test)]
            Trust::Roots(roots) => {
                use tokio_rustls::rustls::RootCertStore;
                use tokio_rustls::rustls::client::WebPkiServerVerifier;

                let mut store = RootCertStore::empty();
                for root in roots {
                    store.add(root.clone()).map_err(|error| error.to_string())?;
                }
                WebPkiServerVerifier::builder_with_provider(Arc::new(store), Arc::clone(&provider))
                    .build()
                    .map_err(|error| error.to_string())?
            }
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

/// Opens the connections of the pooled client: dials through the overlay, and
/// speaks TLS over the stream for `https`.
#[derive(Clone)]
struct Connector {
    overlay: Arc<dyn Overlay>,
    tls: Arc<Tls>,
}

impl Service<Uri> for Connector {
    type Response = OverlayConnection;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<OverlayConnection>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, destination: Uri) -> Self::Future {
        let deadline = CONNECT_DEADLINE
            .try_with(|deadline| *deadline)
            .unwrap_or_else(|_| Instant::now() + CONNECT_TIMEOUT);
        let overlay = Arc::clone(&self.overlay);
        let tls = Arc::clone(&self.tls);
        Box::pin(async move {
            let (host, address) = dialed(&destination)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid address"))?;
            let open = async {
                let stream = overlay.dial(address).await?;
                if destination.scheme_str() != Some("https") {
                    return Ok(stream);
                }
                let name = ServerName::try_from(host)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
                let stream = tls.connector()?.connect(name, stream).await?;
                Ok::<OverlayStream, io::Error>(Box::new(stream))
            };
            let stream = timeout_at(deadline, open)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, connect_timed_out()))??;
            Ok(OverlayConnection(TokioIo::new(stream)))
        })
    }
}

/// A stream of the overlay, as the connection of the HTTP client.
struct OverlayConnection(TokioIo<OverlayStream>);

impl hyper::rt::Read for OverlayConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl hyper::rt::Write for OverlayConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(context, buffers)
    }
}

impl Connection for OverlayConnection {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

/// The body of a response, as the chunks it arrives in. Gives up on a body that
/// stays silent for [`IDLE_TIMEOUT`].
struct Chunks {
    body: Incoming,
    idle: Pin<Box<Sleep>>,
    label: String,
    ended: bool,
}

impl Chunks {
    fn terminated(&mut self, reason: String) -> Poll<Option<Result<Bytes, FetchError>>> {
        self.ended = true;
        Poll::Ready(Some(Err(FetchError::OverlayTerminated {
            label: self.label.clone(),
            reason,
        })))
    }
}

impl Stream for Chunks {
    type Item = Result<Bytes, FetchError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        while !this.ended {
            match Pin::new(&mut this.body).poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => {
                    this.idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                    // Trailers are not kept, and an empty chunk says nothing.
                    if let Ok(data) = frame.into_data()
                        && !data.is_empty()
                    {
                        return Poll::Ready(Some(Ok(data)));
                    }
                }
                Poll::Ready(Some(Err(error))) => return this.terminated(reason(&error)),
                Poll::Ready(None) => {
                    this.ended = true;
                }
                Poll::Pending => {
                    if this.idle.as_mut().poll(context).is_ready() {
                        return this
                            .terminated(format!("no data within {} s", IDLE_TIMEOUT.as_secs()));
                    }
                    return Poll::Pending;
                }
            }
        }
        Poll::Ready(None)
    }
}

#[cfg(test)]
mod tests;
