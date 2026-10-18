//! The LocalAPI of `tailscaled`: HTTP/1.1 over the node's Unix socket.
//!
//! This is the same API the `tailscale` command talks to. It is not a stable
//! interface, so the shapes below were checked against the source of the
//! version the image ships (`ipn/localapi/localapi.go`, `client/local/local.go`).
//! Answers are read leniently: unknown and missing fields are ignored.
//!
//! The socket is owned by the node's own directory (0700), and a client with
//! the same user id as `tailscaled` may change it. The server refuses a request
//! that carries an `Origin` or `Referer` header or a `Host` other than
//! `local-tailscaled.sock`, so none is ever sent.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONNECTION, HOST, UPGRADE};
use http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http1;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Deserializer};
use serde_json::json;
use tokio::time::timeout;

/// The only `Host` the LocalAPI accepts over its socket.
const LOCAL_API_HOST: &str = "local-tailscaled.sock";

/// What an answer may weigh. A status without peers is a few kilobytes.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// How much of an error body ends up in a message.
const MAX_MESSAGE_CHARS: usize = 300;

#[cfg(unix)]
type LocalStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type LocalStream = tokio::io::DuplexStream;

async fn connect(socket: &Path) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        tokio::net::UnixStream::connect(socket).await
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tailscale proxies need a Unix system",
        ))
    }
}

/// Why a LocalAPI call failed.
#[derive(Debug)]
pub(crate) enum ApiError {
    /// The socket does not exist yet, or nothing listens on it.
    Connect(io::Error),
    Timeout,
    /// `tailscaled` answered with a status that is not the expected one.
    Http {
        status: u16,
        message: String,
    },
    /// The answer is not HTTP the way the LocalAPI speaks it.
    Protocol(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(formatter, "tailscaled does not answer ({error})"),
            Self::Timeout => formatter.write_str("tailscaled took too long to answer"),
            Self::Http { status, message } if message.is_empty() => {
                write!(formatter, "tailscaled answered HTTP {status}")
            }
            Self::Http { message, .. } => formatter.write_str(message),
            Self::Protocol(message) => write!(
                formatter,
                "tailscaled sent an unexpected answer ({message})"
            ),
        }
    }
}

impl std::error::Error for ApiError {}

/// What `GET /localapi/v0/status?peers=false` says about the node.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LocalStatus {
    /// `NoState`, `NeedsLogin`, `NeedsMachineAuth`, `Stopped`, `Starting` or `Running`.
    #[serde(rename = "BackendState", deserialize_with = "nullable")]
    pub backend_state: String,
    /// Whether the node holds a node key, that is whether it has logged in before.
    #[serde(rename = "HaveNodeKey", deserialize_with = "nullable")]
    pub have_node_key: bool,
    /// The link to open to log in, once control has given one.
    #[serde(rename = "AuthURL", deserialize_with = "nullable")]
    pub auth_url: String,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
    #[serde(rename = "Self")]
    self_node: Option<SelfNode>,
    #[serde(rename = "Health")]
    health: Option<Vec<String>>,
    #[serde(rename = "CurrentTailnet")]
    current_tailnet: Option<TailnetInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SelfNode {
    #[serde(rename = "DNSName", deserialize_with = "nullable")]
    dns_name: String,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TailnetInfo {
    #[serde(rename = "Name", deserialize_with = "nullable")]
    name: String,
}

fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

impl LocalStatus {
    /// `Self.DNSName` without the trailing dot of a fully qualified name.
    pub fn dns_name(&self) -> Option<String> {
        let name = self.self_node.as_ref()?.dns_name.trim_end_matches('.');
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// The addresses of the node in the tailnet.
    pub fn addresses(&self) -> Vec<IpAddr> {
        let own = self
            .self_node
            .as_ref()
            .and_then(|node| node.tailscale_ips.as_ref());
        own.or(self.tailscale_ips.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| address.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn tailnet(&self) -> Option<String> {
        let name = &self.current_tailnet.as_ref()?.name;
        (!name.is_empty()).then(|| name.clone())
    }

    pub fn health(&self) -> Vec<String> {
        self.health.clone().unwrap_or_default()
    }
}

/// What a dial through the LocalAPI came to.
pub(crate) enum DialOutcome {
    /// The connection to the target, already open inside the tailnet.
    Tunnel(TokioIo<Upgraded>),
    /// The target is not in the tailnet: `tailscaled` gives its address back and
    /// leaves the connection to the caller.
    Direct(String),
}

/// A client of one node's LocalAPI. Cheap to clone: it only holds the socket path.
#[derive(Debug, Clone)]
pub(crate) struct LocalApi {
    socket: PathBuf,
    timeout: Duration,
}

struct Answer {
    status: StatusCode,
    body: Bytes,
}

impl LocalApi {
    pub fn new(socket: PathBuf, timeout: Duration) -> Self {
        Self { socket, timeout }
    }

    /// The same client with another time limit per call.
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            socket: self.socket.clone(),
            timeout,
        }
    }

    pub async fn status(&self) -> Result<LocalStatus, ApiError> {
        let answer = self
            .call(Method::GET, "/localapi/v0/status?peers=false", None)
            .await?;
        expect(&answer, StatusCode::OK)?;
        serde_json::from_slice(&answer.body).map_err(|error| ApiError::Protocol(error.to_string()))
    }

    /// Sets what a proxy node needs and nothing else, so the defaults of a new
    /// node (control server, DNS) stay. A masked patch changes only the fields it names.
    ///
    /// - `WantRunning`: the node is meant to be connected, also after a restart.
    /// - `ShieldsUp`: no connection from the tailnet reaches the container.
    /// - `RouteAll`: subnet routes advertised in the tailnet are used.
    pub async fn apply_prefs(&self, hostname: &str) -> Result<(), ApiError> {
        let patch = json!({
            "WantRunning": true,
            "WantRunningSet": true,
            "Hostname": hostname,
            "HostnameSet": true,
            "ShieldsUp": true,
            "ShieldsUpSet": true,
            "RouteAll": true,
            "RouteAllSet": true,
        });
        let answer = self
            .call(Method::PATCH, "/localapi/v0/prefs", Some(patch.to_string()))
            .await?;
        expect(&answer, StatusCode::OK)
    }

    /// Logs in with an auth key, the way `tailscale up --auth-key` does: `start`
    /// hands the key to a fresh control client, and `login-interactive` makes
    /// that client register (it uses the key instead of asking for a link).
    /// `tailscale up` skips the second call when the node already has a node
    /// key, because `start` then logs in on its own.
    pub async fn login_with_key(&self, key: &str, have_node_key: bool) -> Result<(), ApiError> {
        let options = json!({ "AuthKey": key });
        let answer = self
            .call(
                Method::POST,
                "/localapi/v0/start",
                Some(options.to_string()),
            )
            .await?;
        expect(&answer, StatusCode::NO_CONTENT)?;
        if !have_node_key {
            self.login_interactive().await?;
        }
        Ok(())
    }

    /// Asks control for a login link. It shows up as `AuthURL` in the status.
    pub async fn login_interactive(&self) -> Result<(), ApiError> {
        let answer = self
            .call(Method::POST, "/localapi/v0/login-interactive", None)
            .await?;
        expect(&answer, StatusCode::NO_CONTENT)
    }

    pub async fn logout(&self) -> Result<(), ApiError> {
        let answer = self.call(Method::POST, "/localapi/v0/logout", None).await?;
        expect(&answer, StatusCode::NO_CONTENT)
    }

    /// Opens a TCP connection to `host:port` from inside the tailnet.
    ///
    /// `host` is a name `tailscaled` resolves (a tailnet name or any DNS name) or an IP
    /// address without brackets. The timeout covers the answer, not the connection afterwards.
    pub async fn dial(&self, host: &str, port: u16) -> Result<DialOutcome, ApiError> {
        let work = async {
            let stream = connect(&self.socket).await.map_err(ApiError::Connect)?;
            let (mut sender, connection) = http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
                .await
                .map_err(protocol)?;
            // `with_upgrades` hands the socket over once the server says 101.
            tokio::spawn(async move {
                let _ = connection.with_upgrades().await;
            });
            let request = Request::post("/localapi/v0/dial")
                .header(HOST, LOCAL_API_HOST)
                .header(UPGRADE, "ts-dial")
                .header(CONNECTION, "upgrade")
                .header(HeaderName::from_static("dial-host"), header_value(host)?)
                .header(HeaderName::from_static("dial-port"), port.to_string())
                .header(HeaderName::from_static("dial-network"), "tcp")
                .body(Full::new(Bytes::new()))
                .map_err(protocol)?;
            let response = sender.send_request(request).await.map_err(protocol)?;

            if response.status() == StatusCode::SWITCHING_PROTOCOLS {
                // The bytes the server sent right after its headers are kept by `Upgraded`.
                let upgraded = hyper::upgrade::on(response).await.map_err(protocol)?;
                return Ok(DialOutcome::Tunnel(TokioIo::new(upgraded)));
            }
            let (parts, body) = response.into_parts();
            let body = read_body(body).await?;
            let dial_self = parts
                .headers
                .get("dial-self")
                .is_some_and(|value| value.as_bytes() == b"true");
            if parts.status == StatusCode::OK && dial_self {
                let address = parts
                    .headers
                    .get("dial-addr")
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| ApiError::Protocol("Dial-Self without Dial-Addr".into()))?;
                return Ok(DialOutcome::Direct(address.to_owned()));
            }
            Err(http_error(parts.status, &body))
        };
        timeout(self.timeout, work)
            .await
            .map_err(|_| ApiError::Timeout)?
    }

    /// One request on a fresh connection. A Unix socket connects in microseconds,
    /// and it keeps a stuck call from blocking the next one.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> Result<Answer, ApiError> {
        let work = async {
            let stream = connect(&self.socket).await.map_err(ApiError::Connect)?;
            let (mut sender, connection) = http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
                .await
                .map_err(protocol)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header(HOST, LOCAL_API_HOST);
            if body.is_some() {
                request = request.header(http::header::CONTENT_TYPE, "application/json");
            }
            let request = request
                .body(Full::new(Bytes::from(body.unwrap_or_default())))
                .map_err(protocol)?;
            let response = sender.send_request(request).await.map_err(protocol)?;
            let (parts, body) = response.into_parts();
            Ok(Answer {
                status: parts.status,
                body: read_body(body).await?,
            })
        };
        timeout(self.timeout, work)
            .await
            .map_err(|_| ApiError::Timeout)?
    }
}

fn protocol(error: impl fmt::Display) -> ApiError {
    ApiError::Protocol(error.to_string())
}

fn header_value(value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|_| ApiError::Protocol("invalid host name".into()))
}

async fn read_body<B>(body: B) -> Result<Bytes, ApiError>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Limited::new(body, MAX_BODY_BYTES)
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .map_err(|error| ApiError::Protocol(error.to_string()))
}

fn expect(answer: &Answer, wanted: StatusCode) -> Result<(), ApiError> {
    if answer.status == wanted {
        Ok(())
    } else {
        Err(http_error(answer.status, &answer.body))
    }
}

fn http_error(status: StatusCode, body: &[u8]) -> ApiError {
    ApiError::Http {
        status: status.as_u16(),
        message: message_from_body(body),
    }
}

/// `http.Error` writes plain text; the prefs handler writes `{"Error": "…"}`.
fn message_from_body(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            ["Error", "error"]
                .iter()
                .find_map(|key| value.get(key)?.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| text.trim().to_owned());
    message.chars().take(MAX_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::testing::{DialPlan, FakeNode, serve};

    /// A LocalAPI on a temporary socket, and a client of it.
    struct Setup {
        _dir: tempfile::TempDir,
        node: Arc<FakeNode>,
        api: LocalApi,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tailscaled.sock");
        let node = Arc::new(FakeNode::new());
        let server = serve(&socket, node.clone()).unwrap();
        Setup {
            api: LocalApi::new(socket, Duration::from_secs(3)),
            _dir: dir,
            node,
            server,
        }
    }

    const STATUS: &str = "/localapi/v0/status";

    #[test]
    fn status_tolerates_nulls_and_unknown_fields() {
        let status: LocalStatus = serde_json::from_str(
            r#"{"Version":"1.104.1","TUN":false,"BackendState":"NeedsLogin","HaveNodeKey":null,
                "AuthURL":"","TailscaleIPs":null,"Self":null,"Health":null,"CurrentTailnet":null,
                "SomethingNew":{"a":1}}"#,
        )
        .unwrap();
        assert_eq!(status.backend_state, "NeedsLogin");
        assert!(!status.have_node_key);
        assert_eq!(status.dns_name(), None);
        assert!(status.addresses().is_empty());
        assert!(status.health().is_empty());
        assert_eq!(status.tailnet(), None);
    }

    #[test]
    fn status_reads_the_node() {
        let status: LocalStatus = serde_json::from_str(
            r#"{"BackendState":"Running","HaveNodeKey":true,
                "TailscaleIPs":["100.64.0.9"],
                "Self":{"DNSName":"mymcps.tail1234.ts.net.","HostName":"mymcps",
                        "TailscaleIPs":["100.64.0.1","fd7a:115c:a1e0::1","not an address"]},
                "Health":["a warning"],
                "CurrentTailnet":{"Name":"me@example.com","MagicDNSSuffix":"tail1234.ts.net"}}"#,
        )
        .unwrap();
        assert_eq!(status.dns_name().as_deref(), Some("mymcps.tail1234.ts.net"));
        assert_eq!(
            status.addresses(),
            vec![
                "100.64.0.1".parse::<IpAddr>().unwrap(),
                "fd7a:115c:a1e0::1".parse().unwrap()
            ]
        );
        assert_eq!(status.tailnet().as_deref(), Some("me@example.com"));
        assert_eq!(status.health(), vec!["a warning".to_owned()]);
    }

    #[test]
    fn status_falls_back_to_the_top_level_addresses() {
        let status: LocalStatus =
            serde_json::from_str(r#"{"TailscaleIPs":["100.64.0.9"],"Self":{"DNSName":""}}"#)
                .unwrap();
        assert_eq!(
            status.addresses(),
            vec!["100.64.0.9".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(status.dns_name(), None);
    }

    #[test]
    fn messages_come_from_text_or_json() {
        assert_eq!(message_from_body(b"access denied\n"), "access denied");
        assert_eq!(message_from_body(br#"{"Error":"bad prefs"}"#), "bad prefs");
        assert_eq!(message_from_body(b""), "");
        assert_eq!(
            message_from_body("é".repeat(1000).as_bytes())
                .chars()
                .count(),
            300
        );
    }

    #[tokio::test]
    async fn status_is_read_from_the_socket_with_a_localapi_host() {
        let setup = setup();
        setup.node.connect();
        let status = setup.api.status().await.unwrap();
        assert_eq!(status.backend_state, "Running");
        assert_eq!(status.dns_name().as_deref(), Some("mymcps.tail1234.ts.net"));
        assert_eq!(status.tailnet().as_deref(), Some("me@example.com"));
        assert_eq!(status.addresses().len(), 2);

        let request = setup.node.last("GET", STATUS).unwrap();
        assert_eq!(request.path, "/localapi/v0/status?peers=false");
        assert_eq!(request.header("host"), Some("local-tailscaled.sock"));
        // The server turns away a request that carries either.
        assert_eq!(request.header("origin"), None);
        assert_eq!(request.header("referer"), None);
    }

    #[tokio::test]
    async fn status_can_arrive_chunked() {
        let setup = setup();
        setup.node.connect();
        setup.node.set(|state| state.chunked_status = true);
        let status = setup.api.status().await.unwrap();
        assert_eq!(status.backend_state, "Running");
        assert_eq!(status.dns_name().as_deref(), Some("mymcps.tail1234.ts.net"));
    }

    #[tokio::test]
    async fn prefs_patch_names_only_the_fields_it_sets() {
        let setup = setup();
        setup.api.apply_prefs("office").await.unwrap();
        let request = setup.node.last("PATCH", "/localapi/v0/prefs").unwrap();
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(
            request.json(),
            json!({
                "WantRunning": true, "WantRunningSet": true,
                "Hostname": "office", "HostnameSet": true,
                "ShieldsUp": true, "ShieldsUpSet": true,
                "RouteAll": true, "RouteAllSet": true,
            })
        );
    }

    #[tokio::test]
    async fn login_with_a_key_starts_then_asks_to_log_in_like_tailscale_up() {
        let setup = setup();
        setup
            .api
            .login_with_key("tskey-auth-abc", false)
            .await
            .unwrap();
        let requests = setup.node.requests();
        let paths: Vec<_> = requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect();
        assert_eq!(
            paths,
            [
                ("POST", "/localapi/v0/start"),
                ("POST", "/localapi/v0/login-interactive")
            ]
        );
        // Only the key: the prefs of the node stay as they are.
        assert_eq!(requests[0].json(), json!({ "AuthKey": "tskey-auth-abc" }));
    }

    #[tokio::test]
    async fn a_node_that_has_a_node_key_logs_in_on_start_alone() {
        let setup = setup();
        setup
            .api
            .login_with_key("tskey-auth-abc", true)
            .await
            .unwrap();
        assert_eq!(setup.node.count("POST", "/localapi/v0/start"), 1);
        assert_eq!(
            setup.node.count("POST", "/localapi/v0/login-interactive"),
            0
        );
    }

    #[tokio::test]
    async fn logout_and_interactive_login_are_plain_posts() {
        let setup = setup();
        setup.api.login_interactive().await.unwrap();
        setup.api.logout().await.unwrap();
        for path in ["/localapi/v0/login-interactive", "/localapi/v0/logout"] {
            let request = setup.node.last("POST", path).unwrap();
            assert_eq!(request.body, "");
            assert_eq!(request.header("content-type"), None);
        }
    }

    #[tokio::test]
    async fn errors_carry_what_tailscaled_said() {
        let setup = setup();
        setup
            .node
            .set(|state| state.prefs_error = Some("bad hostname".into()));
        let error = setup.api.apply_prefs("x").await.unwrap_err();
        assert!(
            matches!(&error, ApiError::Http { status: 400, message } if message == "bad hostname"),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "bad hostname");

        let unknown = LocalApi::new(setup._dir.path().join("none.sock"), Duration::from_secs(1));
        assert!(matches!(unknown.status().await, Err(ApiError::Connect(_))));
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mute.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let _held = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let api = LocalApi::new(socket, Duration::from_millis(150));
        assert!(matches!(api.status().await, Err(ApiError::Timeout)));
    }

    #[tokio::test]
    async fn dial_upgrades_and_keeps_the_bytes_sent_with_the_headers() {
        let setup = setup();
        setup.node.set(|state| {
            state.dial = DialPlan::Echo {
                preface: b"hello".to_vec(),
            };
        });
        let outcome = setup.api.dial("box.tail1234.ts.net", 8443).await.unwrap();
        let DialOutcome::Tunnel(mut stream) = outcome else {
            panic!("expected a tunnel");
        };

        let request = setup.node.last("POST", "/localapi/v0/dial").unwrap();
        assert_eq!(request.header("upgrade"), Some("ts-dial"));
        assert!(
            request
                .header("connection")
                .unwrap()
                .to_ascii_lowercase()
                .contains("upgrade")
        );
        assert_eq!(request.header("dial-host"), Some("box.tail1234.ts.net"));
        assert_eq!(request.header("dial-port"), Some("8443"));
        assert_eq!(request.header("dial-network"), Some("tcp"));
        assert_eq!(request.header("host"), Some("local-tailscaled.sock"));
        assert_eq!(request.header("origin"), None);

        // Sent in the same packet as the 101: nothing of it may be lost.
        let mut preface = [0u8; 5];
        stream.read_exact(&mut preface).await.unwrap();
        assert_eq!(&preface, b"hello");
        stream.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        stream.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
    }

    #[tokio::test]
    async fn dial_sends_an_ipv6_address_without_brackets() {
        let setup = setup();
        setup.node.set(|state| {
            state.dial = DialPlan::Echo {
                preface: Vec::new(),
            }
        });
        setup.api.dial("fd7a:115c:a1e0::1", 80).await.unwrap();
        let request = setup.node.last("POST", "/localapi/v0/dial").unwrap();
        assert_eq!(request.header("dial-host"), Some("fd7a:115c:a1e0::1"));
    }

    #[tokio::test]
    async fn dial_self_gives_the_address_back() {
        let setup = setup();
        setup
            .node
            .set(|state| state.dial = DialPlan::SelfDial("93.184.216.34:443".into()));
        match setup.api.dial("example.com", 443).await.unwrap() {
            DialOutcome::Direct(address) => assert_eq!(address, "93.184.216.34:443"),
            DialOutcome::Tunnel(_) => panic!("expected a direct dial"),
        }
    }

    #[tokio::test]
    async fn a_failed_dial_reports_the_status_and_the_text() {
        let setup = setup();
        setup.node.set(|state| {
            state.dial = DialPlan::Fail(
                502,
                "dial failure: dial tcp 100.64.0.2:80: i/o timeout\n".into(),
            );
        });
        match setup.api.dial("down.tail1234.ts.net", 80).await {
            Err(ApiError::Http {
                status: 502,
                message,
            }) => {
                assert_eq!(message, "dial failure: dial tcp 100.64.0.2:80: i/o timeout");
            }
            _ => panic!("expected a 502"),
        }
        setup
            .node
            .set(|state| state.dial = DialPlan::Fail(403, "denied".into()));
        assert!(matches!(
            setup.api.dial("x", 1).await,
            Err(ApiError::Http { status: 403, .. })
        ));
        // A host that cannot be a header is refused before it is sent.
        assert!(matches!(
            setup.api.dial("a\r\nb", 1).await,
            Err(ApiError::Protocol(_))
        ));
        let _: Value = Value::Null;
    }
}
