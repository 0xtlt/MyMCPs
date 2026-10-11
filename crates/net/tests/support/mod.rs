//! A local HTTP server standing in for an upstream, as the TypeScript tests
//! replaced `fetch` or started a `node:http` server.
#![allow(dead_code)]

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use bytes::Bytes;
use futures::Stream;
use http::{HeaderMap, Method};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use url::Url;

/// One request the upstream received.
#[derive(Debug, Clone)]
pub struct Received {
    pub method: Method,
    /// Path and query, as on the request line.
    pub target: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

pub struct Upstream {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Received>>>,
    server: JoinHandle<()>,
}

impl Upstream {
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!("{}{path}", self.origin())).unwrap()
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<Received> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Start an upstream on 127.0.0.1 that answers every request with `answer`,
/// which is given the request and its rank, starting at 1.
pub async fn upstream<F, Fut>(answer: F) -> Upstream
where
    F: Fn(Received, usize) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    let requests: Arc<Mutex<Vec<Received>>> = Arc::default();
    let handler = {
        let requests = Arc::clone(&requests);
        move |request: Request| {
            let requests = Arc::clone(&requests);
            let answer = answer.clone();
            async move {
                let (parts, body) = request.into_parts();
                let received = Received {
                    method: parts.method,
                    target: parts
                        .uri
                        .path_and_query()
                        .map(|target| target.as_str().to_owned())
                        .unwrap_or_default(),
                    headers: parts.headers,
                    body: axum::body::to_bytes(body, usize::MAX).await.unwrap(),
                };
                let call = {
                    let mut requests = requests.lock().unwrap();
                    requests.push(received.clone());
                    requests.len()
                };
                answer(received, call).await
            }
        }
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(handler))
            .await
            .unwrap();
    });
    Upstream {
        address,
        requests,
        server,
    }
}

/// A port of 127.0.0.1 nothing listens on.
pub async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

pub fn respond(status: u16, headers: &[(&str, &str)], body: impl Into<Body>) -> Response {
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(*name, *value);
    }
    response.body(body.into()).unwrap()
}

pub fn redirect(status: u16, location: &str) -> Response {
    respond(status, &[("Location", location)], Body::empty())
}

/// What became of a [`streamed_body`].
#[derive(Debug, Default)]
pub struct StreamedState {
    sent: AtomicUsize,
    cancelled: AtomicBool,
}

impl StreamedState {
    /// Chunks handed to the HTTP server so far.
    pub fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }

    /// Whether the body was dropped before its last chunk.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Wait for the server to notice that the client stopped reading.
    pub async fn wait_until_cancelled(&self) {
        eventually(|| self.cancelled()).await;
    }
}

struct StreamedBody {
    chunk_bytes: usize,
    chunks: usize,
    fill: u8,
    state: Arc<StreamedState>,
}

impl Stream for StreamedBody {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.state.sent() >= self.chunks {
            return Poll::Ready(None);
        }
        self.state.sent.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(Some(Ok(Bytes::from(vec![self.fill; self.chunk_bytes]))))
    }
}

impl Drop for StreamedBody {
    fn drop(&mut self) {
        if self.state.sent() < self.chunks {
            self.state.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

/// A body of `chunks` chunks of `chunk_bytes`, endless with `usize::MAX`, that
/// records being cancelled. It is sent only as fast as the client reads it.
pub fn streamed_body(chunk_bytes: usize, chunks: usize) -> (Body, Arc<StreamedState>) {
    let state = Arc::new(StreamedState::default());
    let body = StreamedBody {
        chunk_bytes,
        chunks,
        fill: b'a',
        state: Arc::clone(&state),
    };
    (Body::from_stream(body), state)
}

/// Sets its flag when dropped, to tell that a pending answer was abandoned.
pub struct OnDrop(pub Arc<AtomicBool>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Wait, for a few seconds at most, until `condition` holds.
pub async fn eventually(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition did not hold in time");
}
