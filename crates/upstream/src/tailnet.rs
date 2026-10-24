//! HTTP MCPs that reach their server through a Tailscale tailnet.
//!
//! An MCP whose `tailnet_id` is set sends everything to its own server through
//! the tailnet: the MCP transport and every request of its OAuth flow to that
//! origin (scheme, host and port). Before the first request the gateway checks
//! that the tailnet can carry it, so that what an administrator reads is the
//! reason (Tailscale is off, the tailnet was deleted, it needs a new sign-in,
//! the host is not a device of it) and not "fetch failed".
//!
//! The check is only for that message. What keeps the credentials of the MCP
//! off the public network is that every later request to its origin requires
//! the tailnet too: when the tailnet stops routing the host in the middle of a
//! session (a subnet route is withdrawn, the device left), the request fails
//! and is never sent anywhere else. Only the other origins of the OAuth flow
//! go to the public network, and only the ones that are named by a DNS name
//! of the internet, at public addresses: an IP address, a name of one label
//! and a name of the tailnet are the tailnet's or nobody's (see
//! [`mymcps_tailnet::TailnetHandle::route`]), so when it stops routing one of
//! them the request fails like the ones to the MCP's own origin.
//!
//! There is one [`OverlayNetwork`] per tailnet, kept by the [`Upstream`], so
//! the connections to the devices of a tailnet are pooled across requests.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use mymcps_core::models::{Mcp, McpTransport, Tailnet};
use mymcps_net::{Fetcher, Overlay, OverlayNetwork, OverlayStream};
use mymcps_tailnet::{TailnetState, Tailnets};
use url::{Host, Url};

use crate::Upstream;
use crate::error::UpstreamError;

/// How long `start_tailnets` waits for a daemon to say where it stands.
const START_REPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const START_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// One tailnet as the [`Overlay`] of the gateway's requests.
///
/// The daemon is asked for on every route and every dial, so a tailnet that
/// is stopped or removed stops carrying traffic at once, and one that is
/// restarted carries it again without a new network.
struct TailnetOverlay {
    tailnets: Tailnets,
    id: i64,
    label: String,
}

impl TailnetOverlay {
    fn handle(&self) -> io::Result<mymcps_tailnet::TailnetHandle> {
        self.tailnets.handle(self.id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                format!("{} is not running", self.label),
            )
        })
    }
}

#[async_trait]
impl Overlay for TailnetOverlay {
    fn label(&self) -> String {
        self.label.clone()
    }

    /// The tailnet's route: an error, never `None`, for a host that is the
    /// tailnet's or nobody's (an address, a single-label name, a name of the
    /// tailnet), so that such a request never reaches the fallback network.
    async fn route(&self, host: &str, port: u16) -> io::Result<Option<SocketAddr>> {
        self.handle()?.route(host, port).await
    }

    async fn dial(&self, address: SocketAddr) -> io::Result<OverlayStream> {
        Ok(Box::new(self.handle()?.dial(address).await?))
    }
}

/// An MCP that cannot go through its tailnet, in words an administrator can
/// act on.
fn refusal(message: impl Into<String>) -> UpstreamError {
    UpstreamError::Tailnet(message.into())
}

/// The host of a URL as an overlay is asked for it: lowercase, an IPv6
/// address without brackets.
fn overlay_host(url: &Url) -> Option<(String, u16)> {
    let host = match url.host()? {
        Host::Domain(name) => name.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => address.to_string(),
    };
    Some((host, url.port_or_known_default()?))
}

/// What a tailnet that is not running is doing, for the message of the error.
fn describe(state: &TailnetState) -> String {
    match state {
        TailnetState::Starting => "it is starting".to_owned(),
        TailnetState::NeedsLogin { .. } => "it needs a login".to_owned(),
        TailnetState::NeedsMachineAuth => {
            "an admin of the tailnet has to approve this server".to_owned()
        }
        TailnetState::Stopped => "it is stopped".to_owned(),
        TailnetState::Failed(reason) => format!("it failed: {reason}"),
        TailnetState::Running { .. } => "it is running".to_owned(),
    }
}

/// How a tailnet stands when the server starts it.
#[derive(Debug)]
pub struct TailnetStart {
    pub tailnet: Tailnet,
    /// Where the tailnet stands once its daemon has answered, or the last
    /// state it had when it did not within a few seconds. An error means the
    /// daemon could not be started.
    pub state: Result<TailnetState, mymcps_tailnet::Error>,
}

impl Upstream {
    /// The fetcher of the requests of `mcp` to the server at `url`, when the
    /// MCP reaches it through a tailnet: `None` for an MCP that does not.
    ///
    /// It fails when the tailnet cannot carry them, see
    /// [`UpstreamError::Tailnet`]. The fetcher sends through the tailnet what
    /// the tailnet routes, and never anywhere else a request to the origin of
    /// `url`: it fails when the tailnet stops routing that host, even after
    /// this check. What the tailnet does not route for another origin goes to
    /// the public network only: a tailnet MCP cannot be aimed at the network
    /// the instance runs in.
    pub(crate) async fn tailnet_fetcher(
        &self,
        mcp: &Mcp,
        url: &Url,
    ) -> Result<Option<Fetcher>, UpstreamError> {
        let Some(id) = mcp
            .tailnet_id
            .filter(|_| mcp.transport == McpTransport::Http)
        else {
            return Ok(None);
        };
        let Some(tailnets) = &self.tailnets else {
            return Err(refusal(
                "This MCP reaches its server through a tailnet, but Tailscale is turned off on this server (TAILSCALED_BINARY is not set)",
            ));
        };
        let tailnet = Tailnet::find_in_team(&*self.core.db, mcp.team_id, id)
            .await?
            .ok_or_else(|| {
                refusal("The tailnet of this MCP no longer exists. Choose another tailnet or remove it from the MCP")
            })?;

        if let Some(message) = self.tailnet_control_refusal(&tailnet) {
            return Err(refusal(format!("Tailnet \"{}\": {message}", tailnet.name)));
        }
        let state = tailnets.state(id).await;
        if !matches!(state, TailnetState::Running { .. }) {
            return Err(refusal(format!(
                "Tailnet \"{}\" is not running: {}. Sign in again on the Tailnets page",
                tailnet.name,
                describe(&state)
            )));
        }

        let (host, port) = overlay_host(url)
            .ok_or_else(|| refusal("The URL of this MCP has no host a tailnet can reach"))?;
        let routed = match tailnets.handle(id) {
            Some(handle) => handle.route(&host, port).await,
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the tailnet stopped",
            )),
        };
        match routed {
            Ok(Some(_)) => {}
            // `None` is another DNS name, and the error of a host that is the
            // tailnet's or nobody's: for the MCP's own host both say it is
            // not carried.
            Ok(None) => {
                return Err(refusal(format!(
                    "\"{host}\" is not a device or a subnet route of tailnet \"{}\"",
                    tailnet.name
                )));
            }
            Err(error) if mymcps_tailnet::is_not_routed(&error) => {
                return Err(refusal(format!(
                    "\"{host}\" is not a device or a subnet route of tailnet \"{}\"",
                    tailnet.name
                )));
            }
            Err(error) => {
                return Err(refusal(format!(
                    "Tailnet \"{}\" could not be asked for \"{host}\": {error}. Sign in again on the Tailnets page",
                    tailnet.name
                )));
            }
        }

        let network = self.tailnet_network(tailnets, &tailnet);
        Ok(Some(
            self.fetcher.public_only().through_requiring(&network, url),
        ))
    }

    /// The network of a tailnet, the same one for every request so that its
    /// connections are pooled. A tailnet that was renamed gets a new one: the
    /// name is in the errors.
    fn tailnet_network(&self, tailnets: &Tailnets, tailnet: &Tailnet) -> OverlayNetwork {
        let label = format!("tailnet \"{}\"", tailnet.name);
        let mut networks = self
            .tailnet_networks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((known, network)) = networks.get(&tailnet.id)
            && *known == label
        {
            return network.clone();
        }
        let network = OverlayNetwork::new(Arc::new(TailnetOverlay {
            tailnets: tailnets.clone(),
            id: tailnet.id,
            label: label.clone(),
        }));
        networks.insert(tailnet.id, (label, network.clone()));
        network
    }

    /// Why the daemon of this tailnet must not run, when the control server it
    /// was connected to is not one the operator allows (`TAILNET_CONTROL_URLS`):
    /// the daemon makes requests that its control server asks for, outside the
    /// routing of the tailnet. Check it before every start.
    pub fn tailnet_control_refusal(&self, tailnet: &Tailnet) -> Option<String> {
        self.control_url_refusal(tailnet.control_url.as_deref())
    }

    /// [`tailnet_control_refusal`](Self::tailnet_control_refusal) for a saved
    /// control server alone, for a tailnet whose row is gone.
    pub fn control_url_refusal(&self, control_url: Option<&str>) -> Option<String> {
        self.core.config.tailnet_control.refusal(control_url)
    }

    /// Drops the network kept for a tailnet, and with it the connections it
    /// pooled. Call it when a tailnet is deleted or its node signed out.
    pub fn forget_tailnet(&self, id: i64) {
        self.tailnet_networks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }

    /// Starts the daemon of every tailnet of every team, at once, and says
    /// where each one stands: a node that joined before reconnects by itself,
    /// one that needs a login says so. Empty when Tailscale is off on this
    /// server.
    ///
    /// It waits for each daemon to answer, up to twenty seconds: run it in the
    /// background.
    pub async fn start_tailnets(&self) -> Result<Vec<TailnetStart>, UpstreamError> {
        let Some(tailnets) = &self.tailnets else {
            return Ok(Vec::new());
        };
        let starts = self.all_tailnets().await?.into_iter().map(|tailnet| async {
            // A control server the operator no longer allows steers no daemon.
            let state = match self.tailnet_control_refusal(&tailnet) {
                Some(message) => Ok(TailnetState::Failed(message)),
                None => start_tailnet(tailnets, tailnet.id).await,
            };
            TailnetStart { tailnet, state }
        });
        Ok(futures::future::join_all(starts).await)
    }

    /// Stops the daemon of every tailnet, and keeps what they know: the nodes
    /// reconnect without a login the next time the server starts.
    pub async fn stop_tailnets(&self) {
        let Some(tailnets) = &self.tailnets else {
            return;
        };
        match self.all_tailnets().await {
            Ok(rows) => {
                futures::future::join_all(rows.iter().map(|tailnet| tailnets.stop(tailnet.id)))
                    .await;
            }
            Err(error) => tracing::warn!(%error, "could not list the tailnets to stop"),
        }
        self.tailnet_networks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    async fn all_tailnets(&self) -> Result<Vec<Tailnet>, sqlx::Error> {
        sqlx::query_as("select * from `tailnets` order by `id` asc")
            .fetch_all(&*self.core.db)
            .await
    }
}

/// Starts one daemon, and waits for it to leave the state it starts in.
async fn start_tailnet(
    tailnets: &Tailnets,
    id: i64,
) -> Result<TailnetState, mymcps_tailnet::Error> {
    tailnets.ensure_running(id).await?;
    let deadline = tokio::time::Instant::now() + START_REPORT_TIMEOUT;
    loop {
        let state = tailnets.state(id).await;
        if state != TailnetState::Starting || tokio::time::Instant::now() >= deadline {
            return Ok(state);
        }
        tokio::time::sleep(START_REPORT_INTERVAL).await;
    }
}
