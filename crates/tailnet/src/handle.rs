use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::localapi::{LocalClient, Status};
use crate::route;

/// The peers of one tailnet, reached through its daemon. Cheap to clone.
///
/// The daemon belongs to [`Tailnets`](crate::Tailnets): once it is stopped,
/// the calls of a handle fail with `NotConnected`.
#[derive(Clone)]
pub struct TailnetHandle {
    id: i64,
    client: LocalClient,
    /// The MagicDNS suffix of the only tailnet this handle carries traffic
    /// in, see [`pinned_to`](Self::pinned_to).
    pin: Option<String>,
}

impl TailnetHandle {
    pub(crate) fn new(id: i64, client: LocalClient) -> Self {
        TailnetHandle {
            id,
            client,
            pin: None,
        }
    }

    /// This handle, carrying traffic only while the node runs in the tailnet
    /// of this MagicDNS suffix: once it is signed in to another one, every
    /// [`route`](Self::route) and [`dial`](Self::dial) fails, so a
    /// connection set up for the first tailnet never reaches the second.
    pub fn pinned_to(mut self, magic_dns_suffix: &str) -> Self {
        self.pin = Some(magic_dns_suffix.trim_matches('.').to_ascii_lowercase());
        self
    }

    fn check_pin(&self, status: &Status) -> io::Result<()> {
        match &self.pin {
            Some(pin) if !status.magic_dns_suffix().eq_ignore_ascii_case(pin) => {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "the node is now signed in to another tailnet ({}, not {pin})",
                        status.magic_dns_suffix()
                    ),
                ))
            }
            _ => Ok(()),
        }
    }

    /// The tailnet this handle reaches.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// The address to [`dial`](Self::dial) for `host:port`, if `host` is a peer
    /// of the tailnet: its MagicDNS name (in full, or its first label, in any
    /// case), one of its Tailscale addresses, or an address inside a subnet it
    /// routes.
    ///
    /// A host that is the tailnet's or nobody's is never left to another
    /// network: an IP address, a name of one label, a name under `.ts.net` and
    /// a name under the MagicDNS suffix of this tailnet that no peer carries
    /// fail with a [`NotRouted`](crate::NotRouted) error (see
    /// [`is_not_routed`](crate::is_not_routed)). That is the case, always, of
    /// loopback, link-local, unspecified and multicast addresses, and of the
    /// addresses of this node. `None` only for another DNS name, which the
    /// caller may send over a network that resolves it.
    ///
    /// It reads the status of the daemon, at most two seconds old. An error
    /// means the tailnet cannot be asked: the daemon does not answer, or the
    /// node is not connected.
    pub async fn route(&self, host: &str, port: u16) -> io::Result<Option<SocketAddr>> {
        let status = self.client.cached_status().await?;
        if status.backend_state != "Running" {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("the tailnet is not running: {}", status.backend_state),
            ));
        }
        self.check_pin(&status)?;
        match route::resolve(&status, host) {
            Some(address) => Ok(Some(SocketAddr::new(address, port))),
            None if route::is_tailnet_only(&status, host) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                crate::NotRouted {
                    host: host.trim().to_owned(),
                },
            )),
            None => Ok(None),
        }
    }

    /// A TCP connection to `address` through the tailnet. The address is one
    /// [`route`](Self::route) returned: the daemon is asked for no other. It
    /// is checked once more against a status read from the daemon right before
    /// the dial, never the cached one, so that an address which became (or
    /// always was) one of this node's own, or which no peer routes any more,
    /// is refused. Between that read and the dial the daemon may still change
    /// its addresses: the window is the round trip of the dial request.
    pub async fn dial(
        &self,
        address: SocketAddr,
    ) -> io::Result<impl AsyncRead + AsyncWrite + Unpin + Send + use<>> {
        if route::is_forbidden(address.ip()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{address} is not an address a tailnet routes"),
            ));
        }
        let status = self.client.status().await?;
        self.check_pin(&status)?;
        let routed = status.backend_state == "Running"
            && route::resolve(&status, &address.ip().to_string())
                == Some(address.ip().to_canonical());
        if !routed {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{address} is not routed by the tailnet"),
            ));
        }
        Ok(self.client.dial(address).await?)
    }
}
