use std::convert::Infallible;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::aws_lc_rs::default_provider;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::*;
use crate::{CannedResponse, FetchRequest, FetchResponse, Fetcher, UpstreamResponseLimits};

const CA: &[u8] = include_bytes!("../../tests/fixtures/overlay_test_ca.der");
const SERVER: &[u8] = include_bytes!("../../tests/fixtures/overlay_test_server.der");
const SERVER_KEY: &[u8] = include_bytes!("../../tests/fixtures/overlay_test_server_key.der");

/// The address every host of the test overlay is carried to.
const TAILNET_ADDRESS: &str = "100.64.0.7";

/// One request the server behind the overlay received.
#[derive(Debug, Clone)]
struct Received {
    method: Method,
    target: String,
    headers: HeaderMap,
    body: Bytes,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

enum Reply {
    Respond(StatusCode, Vec<(&'static str, String)>, Bytes),
    /// The chunks the test sends, as the body of a 200.
    Stream(mpsc::UnboundedReceiver<Bytes>),
    /// Never answers.
    Silence,
}

impl Reply {
    fn text(text: &str) -> Self {
        Self::Respond(
            StatusCode::OK,
            Vec::new(),
            Bytes::copy_from_slice(text.as_bytes()),
        )
    }

    fn redirect(status: u16, location: &str) -> Self {
        Self::Respond(
            StatusCode::from_u16(status).unwrap(),
            vec![("location", location.to_owned())],
            Bytes::new(),
        )
    }

    /// A stream the test feeds, answered to the first request only.
    fn stream() -> (mpsc::UnboundedSender<Bytes>, impl Fn(&Received) -> Reply) {
        let (send, receive) = mpsc::unbounded_channel();
        let receive = Mutex::new(Some(receive));
        (send, move |_: &Received| {
            Reply::Stream(receive.lock().unwrap().take().unwrap())
        })
    }
}

type Handler = Arc<dyn Fn(&Received) -> Reply + Send + Sync>;

/// How the overlay behaves when it is asked for a connection.
#[derive(Clone, Copy)]
enum Dial {
    Open,
    Refuse,
    Hang,
}

/// The name and ALPN protocol of a handshake the server saw.
type Handshake = (Option<String>, Option<Vec<u8>>);

/// An overlay that lives in memory: it carries the hosts it is given to a
/// server in the same process, over a pipe, and records what it was asked.
struct Memory {
    hosts: Mutex<Vec<&'static str>>,
    handler: Handler,
    received: Arc<Mutex<Vec<Received>>>,
    routed: Mutex<Vec<(String, u16)>>,
    dialed: Mutex<Vec<SocketAddr>>,
    route_delay: Duration,
    dial_delay: Duration,
    dial: Dial,
    tls: Option<TlsAcceptor>,
    handshakes: Arc<Mutex<Vec<Handshake>>>,
}

impl Memory {
    fn new(
        hosts: &[&'static str],
        handler: impl Fn(&Received) -> Reply + Send + Sync + 'static,
    ) -> Self {
        Self {
            hosts: Mutex::new(hosts.to_vec()),
            handler: Arc::new(handler),
            received: Arc::default(),
            routed: Mutex::default(),
            dialed: Mutex::default(),
            route_delay: Duration::ZERO,
            dial_delay: Duration::ZERO,
            dial: Dial::Open,
            tls: None,
            handshakes: Arc::default(),
        }
    }

    fn serving_tls(mut self) -> Self {
        let mut config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(SERVER.to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(SERVER_KEY.to_vec())),
            )
            .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        self.tls = Some(TlsAcceptor::from(Arc::new(config)));
        self
    }

    /// The overlay stops routing `host`, as when a subnet route is withdrawn.
    fn withdraw(&self, host: &str) {
        self.hosts.lock().unwrap().retain(|known| *known != host);
    }

    fn requests(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn routed(&self) -> Vec<(String, u16)> {
        self.routed.lock().unwrap().clone()
    }

    fn dialed(&self) -> Vec<SocketAddr> {
        self.dialed.lock().unwrap().clone()
    }
}

fn network(overlay: &Arc<Memory>) -> OverlayNetwork {
    OverlayNetwork::new(overlay.clone())
}

/// A network that trusts the certificate authority of the test.
fn network_trusting_the_test_authority(overlay: &Arc<Memory>) -> OverlayNetwork {
    OverlayNetwork::trusting(overlay.clone(), vec![CertificateDer::from(CA.to_vec())])
}

#[async_trait]
impl Overlay for Memory {
    fn label(&self) -> String {
        "test tailnet".to_owned()
    }

    async fn route(&self, host: &str, port: u16) -> io::Result<Option<SocketAddr>> {
        self.routed.lock().unwrap().push((host.to_owned(), port));
        tokio::time::sleep(self.route_delay).await;
        Ok(self
            .hosts
            .lock()
            .unwrap()
            .contains(&host)
            .then(|| SocketAddr::new(TAILNET_ADDRESS.parse().unwrap(), port)))
    }

    async fn dial(&self, address: SocketAddr) -> io::Result<OverlayStream> {
        self.dialed.lock().unwrap().push(address);
        tokio::time::sleep(self.dial_delay).await;
        match self.dial {
            Dial::Refuse => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "no node answers",
                ));
            }
            Dial::Hang => std::future::pending::<()>().await,
            Dial::Open => {}
        }
        let (client, server) = tokio::io::duplex(64 * 1024);
        let handler = Arc::clone(&self.handler);
        let received = Arc::clone(&self.received);
        let handshakes = Arc::clone(&self.handshakes);
        let acceptor = self.tls.clone();
        tokio::spawn(async move {
            match acceptor {
                None => serve(server, handler, received).await,
                Some(acceptor) => {
                    let Ok(stream) = acceptor.accept(server).await else {
                        return;
                    };
                    let (_, connection) = stream.get_ref();
                    handshakes.lock().unwrap().push((
                        connection.server_name().map(str::to_owned),
                        connection.alpn_protocol().map(<[u8]>::to_vec),
                    ));
                    serve(stream, handler, received).await;
                }
            }
        });
        Ok(Box::new(client))
    }
}

async fn serve(
    io: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    handler: Handler,
    received: Arc<Mutex<Vec<Received>>>,
) {
    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
        let handler = Arc::clone(&handler);
        let received = Arc::clone(&received);
        async move {
            let (parts, body) = request.into_parts();
            let body = body
                .collect()
                .await
                .map(|body| body.to_bytes())
                .unwrap_or_default();
            let request = Received {
                method: parts.method,
                target: parts
                    .uri
                    .path_and_query()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                headers: parts.headers,
                body,
            };
            received.lock().unwrap().push(request.clone());
            let response = match handler(&request) {
                Reply::Respond(status, headers, body) => {
                    let mut response = http::Response::new(Full::new(body).boxed_unsync());
                    *response.status_mut() = status;
                    for (name, value) in headers {
                        response.headers_mut().insert(name, value.parse().unwrap());
                    }
                    response
                }
                Reply::Stream(chunks) => {
                    let frames = futures::stream::unfold(chunks, |mut chunks| async move {
                        let chunk = chunks.recv().await?;
                        Some((Ok::<_, Infallible>(Frame::data(chunk)), chunks))
                    });
                    let body: UnsyncBoxBody<Bytes, Infallible> =
                        StreamBody::new(frames).boxed_unsync();
                    let mut response = http::Response::new(body);
                    response
                        .headers_mut()
                        .insert("content-type", "text/event-stream".parse().unwrap());
                    response
                }
                Reply::Silence => std::future::pending().await,
            };
            Ok::<_, Infallible>(response)
        }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(io), service)
        .await;
}

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

async fn fetch(fetcher: &Fetcher, request: FetchRequest) -> Result<FetchResponse, FetchError> {
    fetcher
        .fetch_with_same_origin_redirects(
            request,
            "MCP endpoint",
            UpstreamResponseLimits::default(),
        )
        .await
}

async fn get(fetcher: &Fetcher, address: &str) -> Result<FetchResponse, FetchError> {
    fetch(fetcher, FetchRequest::get(url(address))).await
}

#[tokio::test]
async fn sends_a_request_to_the_address_the_overlay_routed() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::text("through")
    }));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let request = FetchRequest::get(url("http://user:secret@MCP.tailnet.test:8080/mcp?x=1#top"))
        .header("Accept-Encoding", "gzip")
        .unwrap();
    let mut response = fetch(&fetcher, request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.status_text(), "OK");
    assert_eq!(
        response.url().as_str(),
        "http://mcp.tailnet.test:8080/mcp?x=1"
    );
    assert_eq!(response.text().await.unwrap(), "through");
    assert_eq!(overlay.routed(), [("mcp.tailnet.test".to_owned(), 8080)]);
    assert_eq!(overlay.dialed(), ["100.64.0.7:8080".parse().unwrap()]);
    let [received] = overlay.requests().try_into().unwrap();
    assert_eq!(received.method, Method::GET);
    assert_eq!(received.target, "/mcp?x=1");
    assert_eq!(received.header("host"), Some("mcp.tailnet.test:8080"));
    assert_eq!(
        received.header("user-agent"),
        Some(crate::DEFAULT_USER_AGENT)
    );
    assert_eq!(
        received.header("authorization"),
        Some("Basic dXNlcjpzZWNyZXQ=")
    );
    assert_eq!(received.header("accept-encoding"), None);
}

#[tokio::test]
async fn keeps_the_user_agent_of_the_request_and_sends_its_body() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::text("ok")));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let request = FetchRequest::post(url("http://mcp.tailnet.test/rpc"))
        .header("User-Agent", "Inspector/1")
        .unwrap()
        .header("Content-Type", "application/json")
        .unwrap()
        .body(r#"{"jsonrpc":"2.0"}"#);
    fetch(&fetcher, request).await.unwrap();

    // The port is the default one of the scheme.
    assert_eq!(overlay.routed(), [("mcp.tailnet.test".to_owned(), 80)]);
    let [received] = overlay.requests().try_into().unwrap();
    assert_eq!(received.method, Method::POST);
    assert_eq!(received.header("host"), Some("mcp.tailnet.test"));
    assert_eq!(received.header("user-agent"), Some("Inspector/1"));
    assert_eq!(received.header("content-type"), Some("application/json"));
    assert_eq!(received.header("content-length"), Some("17"));
    assert_eq!(received.body, r#"{"jsonrpc":"2.0"}"#);
}

#[tokio::test]
async fn reuses_the_connection_of_the_pool() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::text("again")));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    for _ in 0..3 {
        let mut response = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap();
        assert_eq!(response.text().await.unwrap(), "again");
    }

    // The overlay is asked about every request, but dialled once.
    assert_eq!(overlay.routed().len(), 3);
    assert_eq!(overlay.dialed().len(), 1);
}

#[tokio::test]
async fn follows_a_redirect_of_the_same_origin_over_the_overlay() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |request| match request
        .target
        .as_str()
    {
        "/old" => Reply::redirect(307, "/new"),
        "/seen" => Reply::redirect(302, "http://mcp.tailnet.test/final"),
        _ => Reply::text(&format!("{} {}", request.method, request.target)),
    }));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    // 307 keeps the method and the body.
    let post = FetchRequest::post(url("http://mcp.tailnet.test/old")).body("payload");
    let mut response = fetch(&fetcher, post).await.unwrap();
    assert_eq!(response.url().path(), "/new");
    assert_eq!(response.text().await.unwrap(), "POST /new");
    assert_eq!(overlay.requests()[1].body, "payload");
    // The overlay is asked on every hop.
    assert_eq!(overlay.routed().len(), 2);

    // 302 turns a POST into a GET.
    let post = FetchRequest::post(url("http://mcp.tailnet.test/seen")).body("payload");
    let mut response = fetch(&fetcher, post).await.unwrap();
    assert_eq!(response.text().await.unwrap(), "GET /final");
    assert!(overlay.requests().last().unwrap().body.is_empty());
}

#[tokio::test]
async fn refuses_a_redirect_to_another_origin() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::redirect(302, "http://other.tailnet.test/")
    }));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let error = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap_err();

    assert!(
        matches!(error, FetchError::CrossOriginRedirect { .. }),
        "{error}"
    );
    assert_eq!(overlay.routed().len(), 1);
}

#[tokio::test]
async fn streams_a_body_as_it_arrives() {
    let (send, handler) = Reply::stream();
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], handler));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let response = get(&fetcher, "http://mcp.tailnet.test/events")
        .await
        .unwrap();
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("text/event-stream")
    );
    let mut events = response.into_stream();

    // Each event is read before the next one exists: nothing waits for the end.
    send.send(Bytes::from_static(b"data: one\n\n")).unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), "data: one\n\n");
    send.send(Bytes::from_static(b"data: two\n\n")).unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), "data: two\n\n");
    drop(send);
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn keeps_the_limits_of_the_response() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |request| {
        let status = if request.target == "/failure" {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::OK
        };
        Reply::Respond(status, Vec::new(), Bytes::from(vec![b'x'; 5000]))
    }));
    let fetcher = Fetcher::offline().through(&network(&overlay));
    let limits = UpstreamResponseLimits {
        max_response_bytes: Some(1000),
        max_error_response_bytes: Some(100),
    };
    let send = |path: &str| {
        fetcher.fetch_with_same_origin_redirects(
            FetchRequest::get(url(&format!("http://mcp.tailnet.test{path}"))),
            "MCP endpoint",
            limits,
        )
    };

    let error = send("/").await.unwrap().bytes().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP endpoint response exceeded 1000 bytes"
    );
    let mut failure = send("/failure").await.unwrap();
    assert_eq!(failure.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(failure.bytes().await.unwrap().len(), 100);
}

#[tokio::test]
async fn sends_what_the_overlay_does_not_route_where_it_went_before() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::text("overlay")
    }));
    let rest = Fetcher::offline().answering(|request| {
        (request.url.host_str() == Some("api.example"))
            .then(|| CannedResponse::new(StatusCode::OK).body("fallback"))
    });
    let fetcher = rest.through(&network(&overlay));

    let mut routed = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap();
    let mut other = get(&fetcher, "http://api.example/").await.unwrap();
    assert_eq!(routed.text().await.unwrap(), "overlay");
    assert_eq!(other.text().await.unwrap(), "fallback");
    assert_eq!(
        overlay.routed(),
        [
            ("mcp.tailnet.test".to_owned(), 80),
            ("api.example".to_owned(), 80)
        ]
    );
    assert_eq!(overlay.requests().len(), 1);

    // What nothing answers fails as it did without the overlay.
    let error = get(&fetcher, "http://unknown.example/").await.unwrap_err();
    assert!(matches!(error, FetchError::Offline), "{error}");

    // Answers added afterwards come first, even for a host of the overlay.
    let fetcher = fetcher.answering(|_| Some(CannedResponse::new(StatusCode::IM_A_TEAPOT)));
    let response = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap();
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(overlay.requests().len(), 1);
}

#[tokio::test]
async fn leaves_a_url_that_is_not_http_to_the_rest() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::text("overlay")
    }));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let error = get(&fetcher, "ftp://mcp.tailnet.test/").await.unwrap_err();

    assert!(matches!(error, FetchError::Offline), "{error}");
    assert!(overlay.routed().is_empty());
}

#[tokio::test]
async fn keeps_the_overlay_when_the_rest_must_be_public() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await;
        }
    });
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::text("overlay")
    }));
    let network = network(&overlay);
    let local = format!("http://localhost:{port}/");

    // The overlay carries what it routes, even though the address is a private one.
    let fetcher = Fetcher::shared().through(&network).public_only();
    let mut response = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap();
    assert_eq!(response.text().await.unwrap(), "overlay");

    // Whatever it does not carry is held to public addresses.
    assert!(get(&fetcher, &local).await.is_err());
    assert_eq!(accepted.load(Ordering::SeqCst), 0);

    // Without the restriction the same request is answered.
    let fetcher = Fetcher::shared().through(&network);
    assert_eq!(
        get(&fetcher, &local).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1);

    // A fetcher that does not use the network is left as it is.
    let fetcher = Fetcher::offline().through(&network).public_only();
    assert_eq!(
        get(&fetcher, "http://mcp.tailnet.test/")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let error = get(&fetcher, "http://elsewhere.example/")
        .await
        .unwrap_err();
    assert!(matches!(error, FetchError::Offline), "{error}");
}

#[tokio::test]
async fn asks_the_overlay_for_an_ipv6_host_without_brackets() {
    let overlay = Arc::new(Memory::new(&["fd7a:115c:a1e0::1"], |_| Reply::text("six")));
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let mut response = get(&fetcher, "http://[FD7A:115C:A1E0::1]:3000/")
        .await
        .unwrap();

    assert_eq!(response.text().await.unwrap(), "six");
    assert_eq!(overlay.routed(), [("fd7a:115c:a1e0::1".to_owned(), 3000)]);
    assert_eq!(
        overlay.requests()[0].header("host"),
        Some("[fd7a:115c:a1e0::1]:3000")
    );
}

#[tokio::test]
async fn names_the_overlay_when_it_cannot_connect() {
    let mut overlay = Memory::new(&["mcp.tailnet.test"], |_| Reply::text("never"));
    overlay.dial = Dial::Refuse;
    let overlay = Arc::new(overlay);
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let error = get(&fetcher, "http://mcp.tailnet.test/secret-path?token=abc")
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::Overlay { .. }), "{error}");
    assert_eq!(
        error.to_string(),
        "fetch failed through test tailnet: no node answers"
    );
    assert!(overlay.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn gives_up_on_a_request_at_the_timeout_of_the_caller() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::Silence));
    let fetcher = Fetcher::offline().through(&network(&overlay));
    let started = Instant::now();

    let request =
        FetchRequest::get(url("http://mcp.tailnet.test/")).timeout(Duration::from_secs(5));
    let error = fetch(&fetcher, request).await.unwrap_err();

    assert!(matches!(error, FetchError::Timeout), "{error}");
    assert_eq!(started.elapsed(), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn limits_the_route_the_connection_and_the_handshake_together() {
    let cases = [
        // Each step is within the limit, their sum is not.
        (Duration::from_secs(6), Duration::from_secs(6), Dial::Open),
        (Duration::from_secs(60), Duration::ZERO, Dial::Open),
        (Duration::ZERO, Duration::ZERO, Dial::Hang),
    ];
    for (route_delay, dial_delay, dial) in cases {
        let mut overlay = Memory::new(&["mcp.tailnet.test"], |_| Reply::text("late"));
        overlay.route_delay = route_delay;
        overlay.dial_delay = dial_delay;
        overlay.dial = dial;
        let overlay = Arc::new(overlay);
        let fetcher = Fetcher::offline().through(&network(&overlay));
        let started = Instant::now();

        let error = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "fetch failed through test tailnet: no connection within 10 s"
        );
        assert_eq!(started.elapsed(), CONNECT_TIMEOUT);
    }
}

#[tokio::test(start_paused = true)]
async fn gives_up_on_a_server_that_stays_silent() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::Silence));
    let fetcher = Fetcher::offline().through(&network(&overlay));
    let started = Instant::now();

    let error = get(&fetcher, "http://mcp.tailnet.test/").await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "fetch failed through test tailnet: no response within 300 s"
    );
    assert_eq!(started.elapsed(), IDLE_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn gives_up_on_a_body_that_stays_silent_between_two_chunks() {
    let (send, handler) = Reply::stream();
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], handler));
    let fetcher = Fetcher::offline().through(&network(&overlay));
    let mut events = get(&fetcher, "http://mcp.tailnet.test/")
        .await
        .unwrap()
        .into_stream();
    send.send(Bytes::from_static(b"data: one\n\n")).unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), "data: one\n\n");
    let started = Instant::now();

    let error = events.next().await.unwrap().unwrap_err();

    assert_eq!(
        error.to_string(),
        "terminated through test tailnet: no data within 300 s"
    );
    assert_eq!(started.elapsed(), IDLE_TIMEOUT);
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn speaks_tls_to_the_host_of_the_url_over_the_overlay() {
    let overlay =
        Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::text("secure")).serving_tls());
    let fetcher = Fetcher::offline().through(&network_trusting_the_test_authority(&overlay));

    let mut response = get(&fetcher, "https://mcp.tailnet.test/mcp").await.unwrap();
    assert_eq!(response.text().await.unwrap(), "secure");
    let mut again = get(&fetcher, "https://mcp.tailnet.test/mcp").await.unwrap();
    assert_eq!(again.text().await.unwrap(), "secure");

    assert_eq!(overlay.routed()[0], ("mcp.tailnet.test".to_owned(), 443));
    assert_eq!(overlay.dialed(), ["100.64.0.7:443".parse().unwrap()]);
    // The name is the host of the URL, not the address that was dialled, and
    // the connection of the first request was kept for the second.
    let handshakes = overlay.handshakes.lock().unwrap().clone();
    assert_eq!(
        handshakes,
        [(
            Some("mcp.tailnet.test".to_owned()),
            Some(b"http/1.1".to_vec())
        )]
    );
    assert_eq!(
        overlay.requests()[0].header("host"),
        Some("mcp.tailnet.test")
    );
}

#[tokio::test]
async fn refuses_a_certificate_that_is_not_for_the_host_of_the_url() {
    let overlay =
        Arc::new(Memory::new(&["other.tailnet.test"], |_| Reply::text("never")).serving_tls());
    let fetcher = Fetcher::offline().through(&network_trusting_the_test_authority(&overlay));

    let error = get(&fetcher, "https://other.tailnet.test/")
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::Overlay { .. }), "{error}");
    assert!(
        error
            .to_string()
            .starts_with("fetch failed through test tailnet: "),
        "{error}"
    );
    assert!(overlay.requests().is_empty());
}

#[tokio::test]
async fn refuses_a_certificate_the_platform_does_not_trust() {
    // The platform knows nothing of the certificate authority of the test.
    let overlay =
        Arc::new(Memory::new(&["mcp.tailnet.test"], |_| Reply::text("never")).serving_tls());
    let fetcher = Fetcher::offline().through(&network(&overlay));

    let error = get(&fetcher, "https://mcp.tailnet.test/")
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::Overlay { .. }), "{error}");
    assert!(overlay.requests().is_empty());
}

#[test]
fn carries_the_address_and_the_host_in_the_uri_of_the_pool() {
    let cases = [
        (
            "http://mcp.example:8080/a/b?c=d#e",
            "100.64.0.1:8080",
            "mcp.example",
            "/a/b?c=d",
        ),
        ("https://[fd7a::1]/", "[fd7a::1]:443", "fd7a::1", "/"),
    ];
    for (text, address, host, path) in cases {
        let address: SocketAddr = address.parse().unwrap();
        let uri = destination(&url(text), address).unwrap();
        assert_eq!(uri.path_and_query().unwrap().as_str(), path);
        assert_eq!(dialed(&uri), Some((host.to_owned(), address)));
    }
}

/// A fetcher whose fallback counts the requests it receives.
fn counting_fallback(counter: &Arc<AtomicUsize>) -> Fetcher {
    let counter = Arc::clone(counter);
    Fetcher::offline().answering(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Some(CannedResponse::new(StatusCode::OK).body("fallback"))
    })
}

#[tokio::test]
async fn never_falls_back_for_the_required_origin() {
    let overlay = Arc::new(Memory::new(&["mcp.tailnet.test"], |_| {
        Reply::text("overlay")
    }));
    let fallback = Arc::new(AtomicUsize::new(0));
    let fetcher = counting_fallback(&fallback)
        .through_requiring(&network(&overlay), &url("http://mcp.tailnet.test:8080/mcp"));

    // While the overlay routes the host, the request goes through it.
    let mut response = get(&fetcher, "http://mcp.tailnet.test:8080/mcp")
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "overlay");

    // The route disappears: the request fails, with its credentials unsent.
    overlay.withdraw("mcp.tailnet.test");
    let request = FetchRequest::post(url("http://MCP.tailnet.test:8080/other"))
        .header("Authorization", "Bearer secret")
        .unwrap();
    let error = fetch(&fetcher, request).await.unwrap_err();
    assert!(
        matches!(&error, FetchError::Overlay { label, reason }
            if label == "test tailnet" && reason.contains("mcp.tailnet.test")),
        "{error}"
    );
    assert_eq!(fallback.load(Ordering::SeqCst), 0);
    assert_eq!(overlay.requests().len(), 1);

    // The same holds when the restriction to the public network is added.
    let error = get(&fetcher.public_only(), "http://mcp.tailnet.test:8080/mcp")
        .await
        .unwrap_err();
    assert!(matches!(error, FetchError::Overlay { .. }), "{error}");
    assert_eq!(fallback.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn requires_the_overlay_for_the_whole_origin_and_only_for_it() {
    let overlay = Arc::new(Memory::new(&[], |_| Reply::text("overlay")));
    let fallback = Arc::new(AtomicUsize::new(0));
    let fetcher = counting_fallback(&fallback)
        .through_requiring(&network(&overlay), &url("https://mcp.tailnet.test/mcp"));

    // Another scheme, another port or another host is another origin.
    for other in [
        "http://mcp.tailnet.test/mcp",
        "https://mcp.tailnet.test:8443/mcp",
        "https://auth.example/token",
    ] {
        let mut response = get(&fetcher, other).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "fallback", "{other}");
    }
    assert_eq!(fallback.load(Ordering::SeqCst), 3);

    // The default port is the origin's port, written or not.
    for same in [
        "https://mcp.tailnet.test/mcp",
        "https://mcp.tailnet.test:443/mcp",
        "https://MCP.tailnet.test/.well-known/oauth-protected-resource",
    ] {
        let error = get(&fetcher, same).await.unwrap_err();
        assert!(
            matches!(error, FetchError::Overlay { .. }),
            "{same}: {error}"
        );
    }
    assert_eq!(fallback.load(Ordering::SeqCst), 3);
    assert!(overlay.requests().is_empty());
}
