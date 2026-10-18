//! A tailscaled that lives in the test process: a LocalAPI server on the same
//! Unix socket path, and a launcher that starts and stops it like a process.
//! It speaks raw HTTP so a test controls every byte, chunking included.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::Tailscale;
use crate::launcher::{LaunchSpec, Launcher, Process};
use crate::node::Timing;
use crate::types::{Availability, NodeConfig};

/// One request as the fake server read it.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub method: String,
    pub path: String,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

/// What `POST /localapi/v0/dial` does.
#[derive(Clone)]
pub(crate) enum DialPlan {
    /// 101, then the bytes of `preface` in the same write as the headers, then an echo.
    Echo { preface: Vec<u8> },
    /// 101, then a splice to a local TCP server.
    Tunnel(SocketAddr),
    /// 200 with `Dial-Self: true` and `Dial-Addr`.
    SelfDial(String),
    /// Any other status, with a text body.
    Fail(u16, String),
}

type Hook = Box<dyn FnMut(&mut FakeState, &Value) + Send>;

pub(crate) struct FakeState {
    pub backend_state: String,
    pub have_node_key: bool,
    pub auth_url: String,
    pub health: Vec<String>,
    pub dns_name: String,
    pub tailnet: String,
    pub ips: Vec<String>,
    pub chunked_status: bool,
    pub prefs_error: Option<String>,
    /// Makes `start` fail with this status and text.
    pub start_error: Option<(u16, String)>,
    pub dial: DialPlan,
    pub on_start: Option<Hook>,
    pub on_login_interactive: Option<Hook>,
    pub requests: Vec<Recorded>,
}

/// The state of one fake node. It outlives the processes, like a state directory.
pub(crate) struct FakeNode {
    state: Mutex<FakeState>,
}

impl FakeNode {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeState {
                backend_state: "NoState".into(),
                have_node_key: false,
                auth_url: String::new(),
                health: Vec::new(),
                dns_name: String::new(),
                tailnet: String::new(),
                ips: Vec::new(),
                chunked_status: false,
                prefs_error: None,
                start_error: None,
                dial: DialPlan::Fail(502, "dial failure: no plan".into()),
                on_start: None,
                on_login_interactive: None,
                requests: Vec::new(),
            }),
        }
    }

    pub fn set(&self, change: impl FnOnce(&mut FakeState)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .clone()
    }

    /// How many requests went to `path`, query excluded.
    pub fn count(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| {
                request.method == method && request.path.split('?').next() == Some(path)
            })
            .count()
    }

    pub fn last(&self, method: &str, path: &str) -> Option<Recorded> {
        self.requests().into_iter().rev().find(|request| {
            request.method == method && request.path.split('?').next() == Some(path)
        })
    }

    /// A node that is connected to a tailnet.
    pub fn connect(&self) {
        self.set(|state| {
            state.backend_state = "Running".into();
            state.have_node_key = true;
            state.dns_name = "mymcps.tail1234.ts.net.".into();
            state.tailnet = "me@example.com".into();
            state.ips = vec!["100.64.0.1".into(), "fd7a:115c:a1e0::1".into()];
        });
    }

    fn status_json(&self) -> String {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        json!({
            "Version": "1.104.1",
            "TUN": false,
            "BackendState": state.backend_state,
            "HaveNodeKey": state.have_node_key,
            "AuthURL": state.auth_url,
            "TailscaleIPs": state.ips,
            "Self": if state.dns_name.is_empty() { Value::Null } else {
                json!({"DNSName": state.dns_name, "HostName": "mymcps", "TailscaleIPs": state.ips})
            },
            "Health": if state.health.is_empty() { Value::Null } else { json!(state.health) },
            "CurrentTailnet": if state.tailnet.is_empty() { Value::Null } else {
                json!({"Name": state.tailnet, "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true})
            },
            "SomethingNew": {"nested": [1, 2, 3]},
        })
        .to_string()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads one request: head, then a body of `Content-Length` bytes.
async fn read_request(stream: &mut UnixStream) -> io::Result<Option<Recorded>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(position) = find(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Ok(Some(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }))
}

async fn respond(
    stream: &mut UnixStream,
    status: u16,
    body: &str,
    chunked: bool,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Status",
    };
    let mut out = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    if status == 204 {
        out.push_str("\r\n");
    } else if chunked {
        out.push_str("Content-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n");
        let middle = body.len() / 2;
        for part in [&body[..middle], &body[middle..]] {
            out.push_str(&format!("{:x}\r\n{part}\r\n", part.len()));
        }
        out.push_str("0\r\n\r\n");
    } else {
        out.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
    }
    stream.write_all(out.as_bytes()).await?;
    stream.flush().await
}

async fn handle(mut stream: UnixStream, node: Arc<FakeNode>) -> io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    node.set(|state| state.requests.push(request.clone()));
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    match (request.method.as_str(), path.as_str()) {
        ("GET", "/localapi/v0/status") => {
            let chunked = node
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .chunked_status;
            respond(&mut stream, 200, &node.status_json(), chunked).await
        }
        ("PATCH", "/localapi/v0/prefs") => {
            let error = node
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .prefs_error
                .clone();
            match error {
                Some(error) => {
                    respond(
                        &mut stream,
                        400,
                        &json!({ "Error": error }).to_string(),
                        false,
                    )
                    .await
                }
                None => respond(&mut stream, 200, "{}", false).await,
            }
        }
        ("POST", "/localapi/v0/start") => {
            let options = request.json();
            let failure = node
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .start_error
                .clone();
            if let Some((status, message)) = failure {
                return respond(&mut stream, status, &message, false).await;
            }
            node.set(|state| {
                if let Some(mut hook) = state.on_start.take() {
                    hook(state, &options);
                    state.on_start = Some(hook);
                }
            });
            respond(&mut stream, 204, "", false).await
        }
        ("POST", "/localapi/v0/login-interactive") => {
            node.set(|state| {
                if let Some(mut hook) = state.on_login_interactive.take() {
                    hook(state, &Value::Null);
                    state.on_login_interactive = Some(hook);
                }
            });
            respond(&mut stream, 204, "", false).await
        }
        ("POST", "/localapi/v0/logout") => {
            node.set(|state| state.backend_state = "NeedsLogin".into());
            respond(&mut stream, 204, "", false).await
        }
        ("POST", "/localapi/v0/dial") => {
            let plan = node
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .dial
                .clone();
            dial(&mut stream, plan).await
        }
        _ => respond(&mut stream, 404, "404 page not found", false).await,
    }
}

async fn dial(stream: &mut UnixStream, plan: DialPlan) -> io::Result<()> {
    const SWITCH: &[u8] =
        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: ts-dial\r\nConnection: upgrade\r\n\r\n";
    match plan {
        DialPlan::Fail(status, message) => respond(stream, status, &message, false).await,
        DialPlan::SelfDial(address) => {
            let out = format!(
                "HTTP/1.1 200 OK\r\nDial-Self: true\r\nDial-Addr: {address}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(out.as_bytes()).await
        }
        DialPlan::Echo { preface } => {
            let mut first = SWITCH.to_vec();
            first.extend_from_slice(&preface);
            stream.write_all(&first).await?;
            let mut chunk = [0u8; 1024];
            loop {
                let read = stream.read(&mut chunk).await?;
                if read == 0 {
                    return Ok(());
                }
                stream.write_all(&chunk[..read]).await?;
            }
        }
        DialPlan::Tunnel(target) => {
            let mut upstream = TcpStream::connect(target).await?;
            stream.write_all(SWITCH).await?;
            let _ = tokio::io::copy_bidirectional(stream, &mut upstream).await;
            Ok(())
        }
    }
}

/// Serves a fake LocalAPI on `socket` until the returned task is aborted.
pub(crate) fn serve(socket: &Path, node: Arc<FakeNode>) -> io::Result<JoinHandle<()>> {
    let listener = UnixListener::bind(socket)?;
    Ok(tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let node = node.clone();
            tokio::spawn(async move {
                let _ = handle(stream, node).await;
            });
        }
    }))
}

/// What the next launch of a node does.
pub(crate) enum Launch {
    /// A process that runs until it is stopped or crashed.
    Run,
    /// A process that exits at once, having written `line` to stderr.
    ExitAt { line: Option<String> },
    /// The binary cannot be started.
    SpawnError,
}

#[derive(Default)]
pub(crate) struct FakeLauncher {
    nodes: Mutex<HashMap<i64, Arc<FakeNode>>>,
    script: Mutex<HashMap<i64, VecDeque<Launch>>>,
    crash: Mutex<HashMap<i64, oneshot::Sender<String>>>,
    pub launches: AtomicUsize,
    pub stops: AtomicUsize,
    launch_times: Mutex<Vec<std::time::Instant>>,
}

impl FakeLauncher {
    pub fn node(&self, id: i64) -> Arc<FakeNode> {
        self.nodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(id)
            .or_insert_with(|| Arc::new(FakeNode::new()))
            .clone()
    }

    /// Queues what the next launches of this node do; after them, it runs.
    pub fn script(&self, id: i64, launches: impl IntoIterator<Item = Launch>) {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(id)
            .or_default()
            .extend(launches);
    }

    /// Makes the running process of a node exit, as if it crashed.
    pub fn crash(&self, id: i64) {
        let sender = self
            .crash
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        if let Some(sender) = sender {
            let _ = sender.send("exit status: 2".into());
        }
    }

    pub fn launch_times(&self) -> Vec<std::time::Instant> {
        self.launch_times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }

    pub fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }
}

struct FakeProcess {
    exit: oneshot::Receiver<String>,
    server: Option<JoinHandle<()>>,
    socket: std::path::PathBuf,
    line: Option<String>,
    launcher: Arc<FakeLauncher>,
}

#[async_trait]
impl Process for FakeProcess {
    async fn wait(&mut self) -> String {
        (&mut self.exit)
            .await
            .unwrap_or_else(|_| "the fake was dropped".into())
    }

    async fn stop(&mut self) {
        self.launcher.stops.fetch_add(1, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            server.abort();
        }
        let _ = std::fs::remove_file(&self.socket);
    }

    async fn last_error(&mut self) -> Option<String> {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        let _ = std::fs::remove_file(&self.socket);
        self.line.clone()
    }
}

#[async_trait]
impl Launcher for Arc<FakeLauncher> {
    async fn launch(&self, spec: &LaunchSpec) -> io::Result<Box<dyn Process>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        self.launch_times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(std::time::Instant::now());
        let next = self
            .script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&spec.id)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Launch::Run);
        let (exit_tx, exit_rx) = oneshot::channel();
        match next {
            Launch::SpawnError => Err(io::Error::new(io::ErrorKind::NotFound, "no such file")),
            Launch::ExitAt { line } => {
                let _ = exit_tx.send("exit status: 1".into());
                Ok(Box::new(FakeProcess {
                    exit: exit_rx,
                    server: None,
                    socket: spec.socket.clone(),
                    line,
                    launcher: self.clone(),
                }))
            }
            Launch::Run => {
                let _ = std::fs::remove_file(&spec.socket);
                let server = serve(&spec.socket, self.node(spec.id))?;
                self.crash
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(spec.id, exit_tx);
                Ok(Box::new(FakeProcess {
                    exit: exit_rx,
                    server: Some(server),
                    socket: spec.socket.clone(),
                    line: Some("fake tailscaled crashed".into()),
                    launcher: self.clone(),
                }))
            }
        }
    }
}

/// Short delays, so that a test sees restarts and polls within milliseconds.
pub(crate) fn fast() -> Timing {
    Timing {
        backoff_start: Duration::from_millis(20),
        backoff_max: Duration::from_millis(160),
        healthy_run: Duration::from_secs(60),
        quick_failures: 3,
        poll_startup: Duration::from_millis(10),
        poll_pending: Duration::from_millis(20),
        poll_connected: Duration::from_millis(40),
        startup_limit: Duration::from_secs(5),
        key_grace: Duration::from_millis(400),
        interactive_retry: Duration::from_secs(60),
        request: Duration::from_secs(3),
        logout: Duration::from_secs(1),
    }
}

pub(crate) struct Harness {
    pub dir: tempfile::TempDir,
    pub launcher: Arc<FakeLauncher>,
    pub tailscale: Tailscale,
}

impl Harness {
    pub async fn new(max_nodes: usize) -> Self {
        Self::with_timing(max_nodes, fast()).await
    }

    pub async fn with_timing(max_nodes: usize, timing: Timing) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(FakeLauncher::default());
        let boxed: Arc<dyn Launcher> = Arc::new(launcher.clone());
        let tailscale = Tailscale::build(
            dir.path().join("tailscale"),
            max_nodes,
            Availability::Available {
                version: "1.104.1".into(),
            },
            Some(boxed),
            timing,
        )
        .await
        .unwrap();
        std::fs::create_dir_all(dir.path().join("tailscale")).unwrap();
        Self {
            dir,
            launcher,
            tailscale,
        }
    }

    pub fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("tailscale")
    }
}

pub(crate) fn config(id: i64, key: Option<&str>) -> NodeConfig {
    NodeConfig {
        id,
        hostname: "mymcps".into(),
        auth_key: key.map(str::to_owned),
    }
}

/// Polls until `check` holds, for five seconds at most.
pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}
