use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::localapi::LocalClient;
use crate::route;

/// The peers of one tailnet, reached through its daemon. Cheap to clone.
///
/// The daemon belongs to [`Tailnets`](crate::Tailnets): once it is stopped,
/// the calls of a handle fail with `NotConnected`.
#[derive(Clone)]
pub struct TailnetHandle {
    id: i64,
    client: LocalClient,
}

impl TailnetHandle {
    pub(crate) fn new(id: i64, client: LocalClient) -> Self {
        TailnetHandle { id, client }
    }

    /// The tailnet this handle reaches.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// The address to [`dial`](Self::dial) for `host:port`, if `host` is a peer
    /// of the tailnet: its MagicDNS name (in full, or its first label, in any
    /// case), one of its Tailscale addresses, or an address inside a subnet it
    /// routes. `None` for any other host, and always for loopback, link-local,
    /// unspecified and multicast addresses.
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
        Ok(route::resolve(&status, host).map(|address| SocketAddr::new(address, port)))
    }

    /// A TCP connection to `address` through the tailnet. The address is one
    /// [`route`](Self::route) returned: the daemon is asked for no other. It
    /// is checked once more against the status of the daemon right before the
    /// dial, so that an address which became (or always was) one of this
    /// node's own, or which no peer routes any more, is refused.
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
        let status = self.client.cached_status().await?;
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
