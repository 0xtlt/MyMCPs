//! Accepting connections and serving them, with the limits Node's HTTP
//! server applied without being asked: a client that opens a connection and
//! says nothing, or never finishes its request line and headers, is hung up
//! on instead of holding a connection for as long as it likes.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tower::{Service, ServiceExt};

/// How patient the server is with a connection.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The time a client has to send a request line and its headers, counted
    /// from the moment the connection is free to carry a request: when it
    /// opens, and after each answer on a connection kept alive.
    pub header_read: Duration,
    /// The time requests under way have to finish once the server is asked
    /// to stop. Connections still open after it are closed.
    pub shutdown_grace: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Node's `headersTimeout`.
            header_read: Duration::from_secs(60),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

/// Serve `service` on `listener` until `shutdown` resolves, then let the
/// requests under way finish.
///
/// Each request carries the address of its client as [`ConnectInfo`], which
/// is where the rate limits and the logs read it.
pub async fn serve<S>(
    listener: TcpListener,
    service: S,
    limits: Limits,
    shutdown: impl Future<Output = ()>,
) where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send,
{
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(limits.header_read);
    let connections = GracefulShutdown::new();
    tokio::pin!(shutdown);

    loop {
        let (stream, remote) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Out of file descriptors, most often: wait for some to
                    // be returned rather than spin.
                    tracing::warn!(%error, "could not accept a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        // Answers are small and written whole: do not wait to batch them.
        let _ = stream.set_nodelay(true);

        let service = service.clone();
        let per_request = hyper::service::service_fn(move |request: hyper::Request<Incoming>| {
            let mut request = request.map(Body::new);
            request
                .extensions_mut()
                .insert(ConnectInfo::<SocketAddr>(remote));
            service.clone().oneshot(request)
        });
        let connection =
            connections.watch(http.serve_connection(TokioIo::new(stream), per_request));
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                // A client that went away, sent something that is not HTTP,
                // or ran out of time: its problem, worth a line only when
                // someone is looking for it.
                tracing::debug!(%remote, %error, "connection closed");
            }
        });
    }

    // Stop accepting, then let what is under way finish.
    drop(listener);
    tokio::select! {
        () = connections.shutdown() => {}
        () = tokio::time::sleep(limits.shutdown_grace) => {
            tracing::warn!("closing connections that did not finish in time");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::Router;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;

    use super::*;

    async fn who(ConnectInfo(remote): ConnectInfo<SocketAddr>) -> String {
        format!("hello {}", remote.ip())
    }

    /// A server on a free port, and what stops it.
    async fn start(
        limits: Limits,
    ) -> (SocketAddr, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let app = Router::new().route("/", get(who)).into_service::<Body>();
        let server = tokio::spawn(serve(listener, app, limits, async {
            let _ = stopped.await;
        }));
        (address, stop, server)
    }

    async fn read_to_end(stream: &mut TcpStream) -> String {
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer).await;
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test]
    async fn answers_requests_and_tells_handlers_who_asked() {
        let (address, stop, server) = start(Limits::default()).await;

        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let answer = read_to_end(&mut stream).await;
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.ends_with("hello 127.0.0.1"), "{answer}");

        stop.send(()).unwrap();
        server.await.unwrap();
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn hangs_up_on_a_client_that_never_finishes_its_headers() {
        let limits = Limits {
            header_read: Duration::from_millis(200),
            ..Limits::default()
        };
        let (address, stop, server) = start(limits).await;

        // One that says nothing, one that stops in the middle of its headers.
        for opening in [&b""[..], &b"GET / HTTP/1.1\r\nHost: loc"[..]] {
            let started = Instant::now();
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.write_all(opening).await.unwrap();
            let answer = tokio::time::timeout(Duration::from_secs(5), read_to_end(&mut stream))
                .await
                .expect("the server closes the connection");
            assert!(!answer.contains("200 OK"), "{answer}");
            assert!(started.elapsed() >= Duration::from_millis(150));
        }

        // A connection kept alive is given the same time for its next request.
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), read_to_end(&mut stream))
            .await
            .expect("the server closes the idle connection");
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");

        stop.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn lets_a_request_under_way_finish_when_it_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let slow = || async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            "done"
        };
        let app = Router::new().route("/", get(slow)).into_service::<Body>();
        let server = tokio::spawn(serve(listener, app, Limits::default(), async {
            let _ = stopped.await;
        }));

        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.send(()).unwrap();

        let answer = read_to_end(&mut stream).await;
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.ends_with("done"), "{answer}");
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the server stops once the request is answered")
            .unwrap();
    }
}
