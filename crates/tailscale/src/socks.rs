//! The SOCKS5 front that sends the requests of an MCP into a tailnet.
//!
//! `reqwest` reaches a node through `socks5h://<id>:<secret>@127.0.0.1:<port>`.
//! The front listens on loopback only and takes nothing but the username and
//! password method of RFC 1929, so a process of the container that does not
//! know a node's secret (an npm MCP, for one) cannot use it. The user name is
//! the node's id, and the password a random value that lives in memory.
//!
//! Only CONNECT is served. The target goes to the node's LocalAPI, which
//! answers in one of three ways:
//!
//! - it dials inside the tailnet and hands the connection over;
//! - the target is not in the tailnet, and it gives back the address it
//!   resolved. That is dialed from here, but only when the address is public:
//!   the container's own loopback and the networks around it stay out of reach;
//! - it fails, and the failure is mapped to a SOCKS reply.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::local_api::{ApiError, DialOutcome};
use crate::node::Node;
use crate::runtime::Registry;

/// The greeting, the login and the request must arrive in this time.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The dial, inside the tailnet or direct, must be open in this time.
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Connections being served at once. A client that opens more waits in the backlog.
const MAX_CONNECTIONS: usize = 1024;

const VERSION: u8 = 5;
const AUTH_VERSION: u8 = 1;
const METHOD_PASSWORD: u8 = 2;
const METHOD_NONE_ACCEPTABLE: u8 = 0xFF;
const COMMAND_CONNECT: u8 = 1;
const ADDRESS_IPV4: u8 = 1;
const ADDRESS_DOMAIN: u8 = 3;
const ADDRESS_IPV6: u8 = 4;

/// The reply codes of RFC 1928 that this front uses.
mod reply {
    pub const SUCCEEDED: u8 = 0x00;
    pub const GENERAL_FAILURE: u8 = 0x01;
    pub const NOT_ALLOWED: u8 = 0x02;
    pub const HOST_UNREACHABLE: u8 = 0x04;
    pub const CONNECTION_REFUSED: u8 = 0x05;
    pub const TTL_EXPIRED: u8 = 0x06;
    pub const COMMAND_NOT_SUPPORTED: u8 = 0x07;
    pub const ADDRESS_NOT_SUPPORTED: u8 = 0x08;
}

/// The running front. Dropping it does not stop it: call [`SocksFront::stop`].
pub(crate) struct SocksFront {
    port: u16,
    cancel: CancellationToken,
}

impl SocksFront {
    /// Listens on a free port of the loopback interface.
    pub async fn bind(registry: Arc<Registry>) -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let cancel = CancellationToken::new();
        tokio::spawn(accept_loop(listener, registry, cancel.clone()));
        Ok(Self { port, cancel })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

async fn accept_loop(listener: TcpListener, registry: Arc<Registry>, cancel: CancellationToken) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok(permit) = slots.clone().acquire_owned().await else {
            return;
        };
        let stream = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    // Out of file descriptors, most likely: do not spin.
                    tracing::warn!("The Tailscale SOCKS front could not accept a connection: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let registry = registry.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = serve(stream, registry) => {}
            }
            drop(permit);
        });
    }
}

/// Where the client wants to go.
#[derive(Debug, PartialEq, Eq)]
enum Host {
    Ip(IpAddr),
    Domain(String),
}

impl Host {
    /// The form the LocalAPI takes in `Dial-Host`: a name, or an address without brackets.
    fn as_dial_host(&self) -> String {
        match self {
            Self::Ip(address) => address.to_string(),
            Self::Domain(name) => name.clone(),
        }
    }
}

struct Request {
    host: Host,
    port: u16,
}

async fn serve(mut client: TcpStream, registry: Arc<Registry>) {
    let handshake = timeout(HANDSHAKE_TIMEOUT, handshake(&mut client, &registry)).await;
    let Ok(Some((node, request))) = handshake else {
        return;
    };

    let dialed = timeout(DIAL_TIMEOUT, open(&node, &request)).await;
    let upstream = match dialed {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(code)) => {
            let _ = send_reply(&mut client, code).await;
            return;
        }
        Err(_) => {
            let _ = send_reply(&mut client, reply::TTL_EXPIRED).await;
            return;
        }
    };
    if send_reply(&mut client, reply::SUCCEEDED).await.is_err() {
        return;
    }
    // No time limit from here: an event stream stays open for as long as it lasts.
    match upstream {
        Upstream::Tunnel(mut stream) => {
            let _ = tokio::io::copy_bidirectional(&mut client, &mut stream).await;
        }
        Upstream::Direct(mut stream) => {
            let _ = tokio::io::copy_bidirectional(&mut client, &mut stream).await;
        }
    }
}

enum Upstream {
    Tunnel(hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>),
    Direct(TcpStream),
}

/// Greeting, login and request. Answers every refusal itself and returns `None`
/// for it; a client that goes quiet or sends nonsense is dropped without a word.
async fn handshake(client: &mut TcpStream, registry: &Registry) -> Option<(Arc<Node>, Request)> {
    // Greeting: the client lists the methods it knows.
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await.ok()?;
    if head[0] != VERSION {
        return None;
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods).await.ok()?;
    if !methods.contains(&METHOD_PASSWORD) {
        client
            .write_all(&[VERSION, METHOD_NONE_ACCEPTABLE])
            .await
            .ok()?;
        return None;
    }
    client.write_all(&[VERSION, METHOD_PASSWORD]).await.ok()?;

    // Login (RFC 1929): user name, then password, each with a length byte.
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await.ok()?;
    if head[0] != AUTH_VERSION {
        return None;
    }
    let mut user = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut user).await.ok()?;
    let mut length = [0u8; 1];
    client.read_exact(&mut length).await.ok()?;
    let mut password = vec![0u8; usize::from(length[0])];
    client.read_exact(&mut password).await.ok()?;
    let Some(node) = authenticate(registry, &user, &password) else {
        client.write_all(&[AUTH_VERSION, 1]).await.ok()?;
        return None;
    };
    client.write_all(&[AUTH_VERSION, 0]).await.ok()?;

    // Request: only CONNECT.
    let mut head = [0u8; 4];
    client.read_exact(&mut head).await.ok()?;
    let [version, command, _, address_type] = head;
    if version != VERSION {
        return None;
    }
    if command != COMMAND_CONNECT {
        let _ = send_reply(client, reply::COMMAND_NOT_SUPPORTED).await;
        return None;
    }
    let host = match address_type {
        ADDRESS_IPV4 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets).await.ok()?;
            Host::Ip(IpAddr::from(octets))
        }
        ADDRESS_IPV6 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets).await.ok()?;
            Host::Ip(IpAddr::from(octets))
        }
        ADDRESS_DOMAIN => {
            let mut length = [0u8; 1];
            client.read_exact(&mut length).await.ok()?;
            let mut name = vec![0u8; usize::from(length[0])];
            client.read_exact(&mut name).await.ok()?;
            match domain(name) {
                Some(name) => Host::Domain(name),
                None => {
                    let _ = send_reply(client, reply::GENERAL_FAILURE).await;
                    return None;
                }
            }
        }
        _ => {
            let _ = send_reply(client, reply::ADDRESS_NOT_SUPPORTED).await;
            return None;
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await.ok()?;
    Some((
        node,
        Request {
            host,
            port: u16::from_be_bytes(port),
        },
    ))
}

/// A name that is safe to send in a header: printable ASCII without spaces.
/// `socks5h` clients send the name of the URL, which is already ASCII (punycode).
fn domain(bytes: Vec<u8>) -> Option<String> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_graphic) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// The user is a node id and the password its secret. A wrong id is compared
/// against a throwaway secret, so both failures take the same time.
fn authenticate(registry: &Registry, user: &[u8], password: &[u8]) -> Option<Arc<Node>> {
    let node = std::str::from_utf8(user)
        .ok()
        .filter(|user| {
            !user.is_empty()
                && user
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
        })
        .and_then(|user| user.parse::<i64>().ok())
        .and_then(|id| registry.get(id));
    let expected = node.as_ref().map_or("-", |node| node.secret());
    let matches = bool::from(expected.as_bytes().ct_eq(password));
    if matches { node } else { None }
}

/// Opens the connection to the target, or says why not as a reply code.
async fn open(node: &Node, request: &Request) -> Result<Upstream, u8> {
    let dialed = node
        .api()
        .dial(&request.host.as_dial_host(), request.port)
        .await;
    match dialed {
        Ok(DialOutcome::Tunnel(stream)) => Ok(Upstream::Tunnel(stream)),
        Ok(DialOutcome::Direct(address)) => {
            let address = direct_target(&address)?;
            match TcpStream::connect(address).await {
                Ok(stream) => Ok(Upstream::Direct(stream)),
                Err(error) => {
                    tracing::debug!(proxy = node.id, "Direct dial failed: {error}");
                    Err(code_for_io(&error))
                }
            }
        }
        Err(error) => {
            // The message can name the target, so it stays at debug.
            tracing::debug!(proxy = node.id, "Tailscale dial failed: {error}");
            Err(code_for_dial(&error))
        }
    }
}

/// `tailscaled` answers 502 when it cannot resolve the name or reach the host.
fn code_for_dial(error: &ApiError) -> u8 {
    match error {
        ApiError::Http { status: 502, .. } => reply::HOST_UNREACHABLE,
        ApiError::Timeout => reply::TTL_EXPIRED,
        _ => reply::GENERAL_FAILURE,
    }
}

fn code_for_io(error: &io::Error) -> u8 {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => reply::CONNECTION_REFUSED,
        io::ErrorKind::TimedOut => reply::TTL_EXPIRED,
        io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => {
            reply::HOST_UNREACHABLE
        }
        _ => reply::GENERAL_FAILURE,
    }
}

/// Decides whether a target outside the tailnet may be dialed from here: only a
/// public address may. Anything else is the container's own network.
fn direct_target(address: &str) -> Result<SocketAddr, u8> {
    let address: SocketAddr = address.parse().map_err(|_| reply::GENERAL_FAILURE)?;
    if mymcps_net::is_restricted_ip(address.ip()) {
        return Err(reply::NOT_ALLOWED);
    }
    Ok(address)
}

/// The reply has no meaningful bound address: it carries zeros.
async fn send_reply(stream: &mut (impl AsyncWrite + Unpin), code: u8) -> io::Result<()> {
    stream
        .write_all(&[VERSION, code, 0, ADDRESS_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_are_dialed_directly() {
        assert_eq!(
            direct_target("93.184.216.34:80"),
            Ok("93.184.216.34:80".parse().unwrap())
        );
        assert_eq!(
            direct_target("[2606:2800:220:1:248:1893:25c8:1946]:443"),
            Ok("[2606:2800:220:1:248:1893:25c8:1946]:443".parse().unwrap())
        );
        for restricted in [
            "127.0.0.1:80",
            "10.0.0.5:80",
            "192.168.1.10:8080",
            "172.16.0.1:80",
            "169.254.169.254:80",
            "[::1]:80",
            "[fe80::1]:80",
            "[::ffff:10.0.0.1]:80",
            "0.0.0.0:80",
        ] {
            assert_eq!(
                direct_target(restricted),
                Err(reply::NOT_ALLOWED),
                "{restricted}"
            );
        }
        assert_eq!(direct_target("not an address"), Err(reply::GENERAL_FAILURE));
        assert_eq!(direct_target("93.184.216.34"), Err(reply::GENERAL_FAILURE));
    }

    #[test]
    fn dial_failures_map_to_reply_codes() {
        let http = |status| ApiError::Http {
            status,
            message: String::new(),
        };
        assert_eq!(code_for_dial(&http(502)), reply::HOST_UNREACHABLE);
        assert_eq!(code_for_dial(&http(500)), reply::GENERAL_FAILURE);
        assert_eq!(code_for_dial(&http(403)), reply::GENERAL_FAILURE);
        assert_eq!(code_for_dial(&ApiError::Timeout), reply::TTL_EXPIRED);
        assert_eq!(
            code_for_dial(&ApiError::Connect(io::ErrorKind::NotFound.into())),
            reply::GENERAL_FAILURE
        );
    }

    #[test]
    fn domains_are_checked_before_they_reach_a_header() {
        assert_eq!(
            domain(b"box.tail1234.ts.net".to_vec()).as_deref(),
            Some("box.tail1234.ts.net")
        );
        assert_eq!(domain(Vec::new()), None);
        assert_eq!(domain(b"a b".to_vec()), None);
        assert_eq!(domain(b"a\r\nb".to_vec()), None);
        assert_eq!(domain("é".as_bytes().to_vec()), None);
    }
}
