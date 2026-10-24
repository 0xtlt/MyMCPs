//! The client of the LocalAPI of one `tailscaled`: HTTP/1 over its unix socket.
//!
//! What the daemon expects of a client, from Tailscale v1.104.2:
//!
//! - `Host: local-tailscaled.sock`. `Handler.ServeHTTP` in
//!   `ipn/localapi/localapi.go` answers 403 to any other Host, and to a request
//!   with a `Referer` or an `Origin`, which this client never sends. The
//!   `Sec-Tailscale: localapi` header of `tsnet` is not part of the protocol of
//!   `tailscaled`: only the in-process server of `tsnet` checks it, so it is not
//!   sent.
//! - Permissions come from the peer credentials of the socket
//!   (`actor.Permissions` in `ipn/ipnserver/server.go`, `IsReadonlyConn` in
//!   `ipn/ipnauth/ipnauth.go`): root and the user the daemon runs as may write.
//!   The supervisor starts tailscaled as the user of this process, so `start`,
//!   `login-interactive` and `logout` are allowed; no header or token is needed.
//! - `GET /localapi/v0/status` is `ipnstate.Status` as JSON, with the field
//!   names of the Go struct (`BackendState`, `AuthURL`, `Self`, `Peer`,
//!   `CurrentTailnet.MagicDNSSuffix`, `PeerStatus.DNSName`, `TailscaleIPs`,
//!   `PrimaryRoutes`).
//! - `POST /localapi/v0/start` takes an `ipn.Options` as JSON: `UpdatePrefs`
//!   (an `ipn.Prefs`, which replaces the stored one except `Persist`) and
//!   `AuthKey`. It answers 204.
//! - `POST /localapi/v0/login-interactive` and `POST /localapi/v0/logout` take
//!   no body and answer 204.
//! - `POST /localapi/v0/dial` with `Connection: upgrade` (lower case, the daemon
//!   looks for that exact word), `Upgrade: ts-dial`, `Dial-Host`, `Dial-Port` and
//!   `Dial-Network` answers 101 and then relays bytes. When the address is not
//!   one of the tailnet it answers 200 with `Dial-Self: true` instead, asking the
//!   client to dial it itself: this client treats that as a refusal.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::error::Error;

/// The Host of every LocalAPI request (`apitype.LocalAPIHost`).
pub(crate) const LOCALAPI_HOST: &str = "local-tailscaled.sock";
/// How long a status answer is reused.
pub(crate) const STATUS_TTL: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A first dial wakes up the tunnel to the peer: give it time.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);
/// A tailnet of thousands of nodes answers a few megabytes.
const MAX_BODY: usize = 32 * 1024 * 1024;

/// `ipnstate.Status`, with the fields this crate reads. Go encodes a nil map or
/// slice as `null`, hence the `Option`s.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Status {
    #[serde(rename = "BackendState")]
    pub backend_state: String,
    #[serde(rename = "AuthURL")]
    pub auth_url: String,
    #[serde(rename = "Self")]
    pub self_node: Option<PeerStatus>,
    #[serde(rename = "CurrentTailnet")]
    pub current_tailnet: Option<TailnetStatus>,
    /// The legacy name of `CurrentTailnet.MagicDNSSuffix`.
    #[serde(rename = "MagicDNSSuffix")]
    pub legacy_magic_dns_suffix: String,
    #[serde(rename = "Peer")]
    pub peer: Option<HashMap<String, PeerStatus>>,
    #[serde(rename = "TailscaleIPs")]
    pub tailscale_ips: Option<Vec<String>>,
}

/// `ipnstate.TailnetStatus`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TailnetStatus {
    #[serde(rename = "MagicDNSSuffix")]
    pub magic_dns_suffix: String,
}

/// `ipnstate.PeerStatus`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PeerStatus {
    /// The FQDN, with the trailing dot.
    #[serde(rename = "DNSName")]
    pub dns_name: String,
    #[serde(rename = "TailscaleIPs")]
    pub tailscale_ips: Option<Vec<String>>,
    #[serde(rename = "PrimaryRoutes")]
    pub primary_routes: Option<Vec<String>>,
}

impl Status {
    /// The MagicDNS suffix of the tailnet, without dots around it.
    pub(crate) fn magic_dns_suffix(&self) -> &str {
        let suffix = self
            .current_tailnet
            .as_ref()
            .map(|tailnet| tailnet.magic_dns_suffix.as_str())
            .filter(|suffix| !suffix.is_empty())
            .unwrap_or(&self.legacy_magic_dns_suffix);
        suffix.trim_matches('.')
    }

    pub(crate) fn peers(&self) -> impl Iterator<Item = &PeerStatus> {
        self.peer.iter().flat_map(HashMap::values)
    }

    /// The addresses of this node.
    pub(crate) fn own_addresses(&self) -> Vec<IpAddr> {
        let own = self
            .self_node
            .as_ref()
            .and_then(|node| node.tailscale_ips.as_ref())
            .or(self.tailscale_ips.as_ref());
        parse_ips(own)
    }
}

impl PeerStatus {
    pub(crate) fn addresses(&self) -> Vec<IpAddr> {
        parse_ips(self.tailscale_ips.as_ref())
    }

    pub(crate) fn routes(&self) -> Vec<Prefix> {
        self.primary_routes
            .iter()
            .flatten()
            .filter_map(|route| route.parse().ok())
            .collect()
    }

    /// The FQDN as the user types it: lower case, no trailing dot.
    pub(crate) fn fqdn(&self) -> String {
        self.dns_name.trim_end_matches('.').to_ascii_lowercase()
    }
}

fn parse_ips(ips: Option<&Vec<String>>) -> Vec<IpAddr> {
    ips.into_iter()
        .flatten()
        .filter_map(|ip| ip.parse().ok())
        .collect()
}

/// A CIDR prefix, `netip.Prefix` in Go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prefix {
    address: IpAddr,
    len: u8,
}

impl FromStr for Prefix {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (address, len) = text.split_once('/').ok_or(())?;
        let address: IpAddr = address.parse().map_err(|_| ())?;
        let len: u8 = len.parse().map_err(|_| ())?;
        let max = if address.is_ipv4() { 32 } else { 128 };
        (len <= max).then_some(Prefix { address, len }).ok_or(())
    }
}

impl Prefix {
    pub(crate) fn bits(&self) -> u8 {
        self.len
    }

    pub(crate) fn contains(&self, ip: IpAddr) -> bool {
        // An IPv4-mapped address is an IPv4 one; a prefix is never mapped.
        match (self.address, ip.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(network) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.len))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// The LocalAPI of one daemon. Cheap to clone; clones share the status cache.
#[derive(Clone)]
pub(crate) struct LocalClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    socket: PathBuf,
    ttl: Duration,
    cache: Mutex<Option<(Instant, Arc<Status>)>>,
}

type Sender = http1::SendRequest<Full<Bytes>>;

impl LocalClient {
    pub(crate) fn with_ttl(socket: PathBuf, ttl: Duration) -> Self {
        LocalClient {
            inner: Arc::new(ClientInner {
                socket,
                ttl,
                cache: Mutex::new(None),
            }),
        }
    }

    /// The status, at most `ttl` old. Calls that arrive while one is being read
    /// wait for it instead of asking again.
    pub(crate) async fn cached_status(&self) -> Result<Arc<Status>, Error> {
        let mut cache = self.inner.cache.lock().await;
        if let Some((read_at, status)) = cache.as_ref()
            && read_at.elapsed() < self.inner.ttl
        {
            return Ok(Arc::clone(status));
        }
        let status = Arc::new(self.read_status().await?);
        *cache = Some((Instant::now(), Arc::clone(&status)));
        Ok(status)
    }

    /// The status as it is now. It also refreshes the cache.
    pub(crate) async fn status(&self) -> Result<Arc<Status>, Error> {
        let status = Arc::new(self.read_status().await?);
        *self.inner.cache.lock().await = Some((Instant::now(), Arc::clone(&status)));
        Ok(status)
    }

    async fn read_status(&self) -> Result<Status, Error> {
        let body = self
            .send(Method::GET, "/localapi/v0/status", None, REQUEST_TIMEOUT)
            .await?;
        serde_json::from_slice(&body).map_err(|error| Error::Protocol(format!("status: {error}")))
    }

    /// `POST /localapi/v0/start` with an `ipn.Options`.
    pub(crate) async fn start(&self, options: &Value) -> Result<(), Error> {
        let body = serde_json::to_vec(options)
            .map_err(|error| Error::Protocol(format!("start options: {error}")))?;
        self.send(
            Method::POST,
            "/localapi/v0/start",
            Some(body),
            REQUEST_TIMEOUT,
        )
        .await
        .map(drop)
    }

    pub(crate) async fn login_interactive(&self) -> Result<(), Error> {
        self.send(
            Method::POST,
            "/localapi/v0/login-interactive",
            None,
            REQUEST_TIMEOUT,
        )
        .await
        .map(drop)
    }

    pub(crate) async fn logout(&self) -> Result<(), Error> {
        self.send(Method::POST, "/localapi/v0/logout", None, REQUEST_TIMEOUT)
            .await
            .map(drop)
    }

    async fn connect(&self) -> Result<Sender, Error> {
        let stream = UnixStream::connect(&self.inner.socket)
            .await
            .map_err(|error| Error::Unreachable(error.to_string()))?;
        let (sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|error| Error::Protocol(error.to_string()))?;
        tokio::spawn(async move {
            // The error of a connection is the error of the request on it.
            let _ = connection.with_upgrades().await;
        });
        Ok(sender)
    }

    fn request(method: Method, path: &str, body: Option<Vec<u8>>) -> http::request::Builder {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, LOCALAPI_HOST);
        if body.is_some() {
            builder.header(header::CONTENT_TYPE, "application/json")
        } else {
            builder
        }
    }

    /// One request on a connection of its own; a 2xx answer is its body.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<Bytes, Error> {
        let exchange = async {
            let mut sender = self.connect().await?;
            let request = Self::request(method, path, body.clone())
                .body(Full::new(Bytes::from(body.unwrap_or_default())))
                .map_err(|error| Error::Protocol(error.to_string()))?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|error| Error::Protocol(error.to_string()))?;
            let status = response.status();
            let body = read_body(response).await?;
            if status.is_success() {
                Ok(body)
            } else {
                Err(local_api_error(status, &body))
            }
        };
        tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| Error::Unreachable(format!("{path} timed out")))?
    }

    /// Opens a connection to `address` through the tailnet: the socket of the
    /// daemon, upgraded to a raw relay.
    pub(crate) async fn dial(&self, address: SocketAddr) -> Result<TokioIo<Upgraded>, Error> {
        let exchange = async {
            let mut sender = self.connect().await?;
            let request = Self::request(Method::POST, "/localapi/v0/dial", None)
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, "ts-dial")
                .header("Dial-Host", address.ip().to_string())
                .header("Dial-Port", address.port().to_string())
                .header("Dial-Network", "tcp")
                .body(Full::new(Bytes::new()))
                .map_err(|error| Error::Protocol(error.to_string()))?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|error| Error::Protocol(error.to_string()))?;
            let status = response.status();
            if status == StatusCode::SWITCHING_PROTOCOLS {
                let upgraded = hyper::upgrade::on(response)
                    .await
                    .map_err(|error| Error::Protocol(error.to_string()))?;
                return Ok(TokioIo::new(upgraded));
            }
            let dial_self = response
                .headers()
                .get("Dial-Self")
                .is_some_and(|value| value == "true");
            let body = read_body(response).await?;
            if status == StatusCode::OK && dial_self {
                return Err(Error::Protocol(format!(
                    "tailscaled does not route {address} through the tailnet"
                )));
            }
            Err(local_api_error(status, &body))
        };
        tokio::time::timeout(DIAL_TIMEOUT, exchange)
            .await
            .map_err(|_| Error::Unreachable(format!("dial {address} timed out")))?
    }
}

async fn read_body(response: http::Response<Incoming>) -> Result<Bytes, Error> {
    Limited::new(response.into_body(), MAX_BODY)
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .map_err(|error| Error::Protocol(format!("reading the answer: {error}")))
}

fn local_api_error(status: StatusCode, body: &[u8]) -> Error {
    let text = String::from_utf8_lossy(body);
    let message: String = text.trim().chars().take(300).collect();
    Error::LocalApi {
        status: status.as_u16(),
        message,
    }
}
