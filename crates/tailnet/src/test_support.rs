//! A fake LocalAPI on a unix socket, and a fake `tailscaled` script.
//!
//! The fake server applies the checks of `Handler.ServeHTTP` and `serveDial`
//! in Tailscale v1.104.2 that a client can fail: the Host, no Origin or
//! Referer, and the exact words of the `ts-dial` upgrade.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use tokio::task::JoinHandle;

/// A request the fake server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

#[derive(Default)]
pub struct FakeState {
    requests: Mutex<Vec<Recorded>>,
    status: Mutex<Value>,
    /// Forced answers, by path: `(status, body)`.
    forced: Mutex<HashMap<String, (u16, String)>>,
    /// Where a dial is bridged to.
    echo: Mutex<Option<SocketAddr>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct FakeLocalApi {
    state: Arc<FakeState>,
    task: JoinHandle<()>,
}

impl Drop for FakeLocalApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeLocalApi {
    /// Listens on `socket`; the first status is `status`.
    pub fn bind(socket: &Path, status: Value) -> Self {
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket).expect("bind the fake LocalAPI");
        let state = Arc::new(FakeState::default());
        *locked(&state.status) = status;
        let task = tokio::spawn(accept(listener, Arc::clone(&state)));
        FakeLocalApi { state, task }
    }

    pub fn set_status(&self, status: Value) {
        *locked(&self.state.status) = status;
    }

    pub fn force(&self, path: &str, status: u16, body: &str) {
        locked(&self.state.forced).insert(path.to_owned(), (status, body.to_owned()));
    }

    pub fn bridge_dials_to(&self, address: SocketAddr) {
        *locked(&self.state.echo) = Some(address);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        locked(&self.state.requests).clone()
    }

    pub fn requests_to(&self, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.path == path)
            .collect()
    }
}

async fn accept(listener: UnixListener, state: Arc<FakeState>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, Arc::clone(&state)));
            let connection = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            let _ = connection.await;
        });
    }
}

fn answer(status: u16, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(body.into()))
        .unwrap()
}

async fn handle(
    mut request: Request<Incoming>,
    state: Arc<FakeState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let headers = request.headers().clone();
    let host = headers.get("host").and_then(|value| value.to_str().ok());
    if headers.contains_key("referer")
        || headers.contains_key("origin")
        || !matches!(host, None | Some("local-tailscaled.sock"))
    {
        return Ok(answer(403, "invalid localapi request"));
    }
    let body = request
        .body_mut()
        .collect()
        .await
        .map(|collected| collected.to_bytes().to_vec())
        .unwrap_or_default();
    let path = request.uri().path().to_owned();
    locked(&state.requests).push(Recorded {
        method: request.method().clone(),
        path: path.clone(),
        headers: headers.clone(),
        body: body.clone(),
    });
    let forced = locked(&state.forced).get(&path).cloned();
    if let Some((status, body)) = forced {
        return Ok(answer(status, body));
    }
    let post = request.method() == Method::POST;
    Ok(match path.as_str() {
        "/localapi/v0/status" if request.method() == Method::GET => {
            let status = locked(&state.status).to_string();
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(status)))
                .unwrap()
        }
        "/localapi/v0/start" if post => match serde_json::from_slice::<Value>(&body) {
            Ok(_) => answer(204, ""),
            Err(error) => answer(400, error.to_string()),
        },
        "/localapi/v0/login-interactive" | "/localapi/v0/logout" if post => answer(204, ""),
        "/localapi/v0/dial" => dial(&mut request, &headers, &state),
        _ => answer(404, "404 page not found"),
    })
}

/// `serveDial`.
fn dial(
    request: &mut Request<Incoming>,
    headers: &HeaderMap,
    state: &FakeState,
) -> Response<Full<Bytes>> {
    let value = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if request.method() != Method::POST {
        return answer(405, "POST required");
    }
    if !value("connection").is_some_and(|connection| connection.contains("upgrade"))
        || value("upgrade") != Some("ts-dial")
    {
        return answer(400, "bad ts-dial upgrade");
    }
    if value("dial-host").is_none_or(str::is_empty) || value("dial-port").is_none_or(str::is_empty)
    {
        return answer(400, "missing Dial-Host or Dial-Port header");
    }
    if value("dial-network").unwrap_or("tcp") != "tcp" {
        return answer(400, "unsupported network");
    }
    let Some(target) = *locked(&state.echo) else {
        // The daemon asks the client to dial an address outside the tailnet
        // itself.
        return Response::builder()
            .header("Dial-Self", "true")
            .header("Dial-Addr", "93.184.216.34:80")
            .body(Full::new(Bytes::new()))
            .unwrap();
    };
    let upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else {
            return;
        };
        let Ok(mut outbound) = TcpStream::connect(target).await else {
            return;
        };
        let mut inbound = TokioIo::new(upgraded);
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    });
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "ts-dial")
        .header("Connection", "upgrade")
        .body(Full::new(Bytes::new()))
        .unwrap()
}

/// A TCP server that sends back what it receives.
pub async fn spawn_echo_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    (address, task)
}

/// The status of a node of a tailnet that is up.
pub fn running_status() -> Value {
    json!({
        "Version": "1.104.2",
        "BackendState": "Running",
        "AuthURL": "",
        "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"],
        "Self": {
            "DNSName": "mymcps.tail1234.ts.net.",
            "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"]
        },
        "MagicDNSSuffix": "tail1234.ts.net",
        "CurrentTailnet": {"Name": "me@example.com", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true},
        "Peer": {
            "nodekey:aaaa": {
                "DNSName": "nas.tail1234.ts.net.",
                "TailscaleIPs": ["100.64.0.2", "fd7a:115c:a1e0::2"]
            },
            "nodekey:bbbb": {
                "DNSName": "router.tail1234.ts.net.",
                "TailscaleIPs": ["100.64.0.3"],
                "PrimaryRoutes": ["192.168.1.0/24"]
            }
        }
    })
}

pub fn status_in(state: &str) -> Value {
    json!({"Version": "1.104.2", "BackendState": state})
}

/// A `tailscaled` that records how it was started and then sleeps: the first
/// `fail_runs` runs exit with status 3 at once. `--probe` exits at once and
/// records nothing.
///
/// Next to the script: `pid` (of the last run), `runs` (a count),
/// `args.<run>`, `env.<run>`, `cwd.<run>`.
pub fn fake_tailscaled(dir: &Path, fail_runs: u32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--probe" ] && exit 0
here="$(dirname "$0")"
n=$(cat "$here/runs" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$here/runs"
printf '%s\n' "$@" > "$here/args.$n"
env > "$here/env.$n"
pwd > "$here/cwd.$n"
echo "a line of the log"
echo "a line of the error log" >&2
if [ "$n" -le {fail_runs} ]; then
  exit 3
fi
echo "$$" > "$here/pid"
exec sleep 600
"#
    );
    let path = dir.join("tailscaled");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(&path);
    path
}

/// Runs the script once, until the system lets it. A process forked by a test
/// that runs at the same time can still hold the file open for writing, and
/// `exec` then fails with ETXTBSY ("Text file busy"): the daemon of a test
/// would not start, or would start late. Once the script has run, no process
/// holds it for writing any more, and none gets to.
fn wait_until_executable(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match std::process::Command::new(path).arg("--probe").status() {
            Ok(_) => return,
            Err(error) if std::time::Instant::now() < deadline => {
                // Only ETXTBSY is worth waiting for: anything else is final.
                assert!(
                    error.kind() == std::io::ErrorKind::ExecutableFileBusy,
                    "cannot run the fake tailscaled: {error}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("cannot run the fake tailscaled: {error}"),
        }
    }
}

/// Polls `condition` until it holds, for at most `timeout`.
pub async fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
