//! The Tailscale nodes the Proxies page manages, as the web layer sees them.
//!
//! The pages and the handlers talk to this trait, not to the
//! [`Tailscale`] runtime itself, so that a test can stand in a runtime that
//! reports any status and records what it was asked to do. The server
//! passes the real runtime, which implements it.

use std::sync::Arc;

use async_trait::async_trait;
pub use mymcps_tailscale::{
    Availability, NodeConfig, NodeState, NodeStatus, Tailscale, TailscaleError,
};
use mymcps_upstream::ProxyDirectory;
use url::Url;

/// What the web layer asks of the runtime that runs the nodes of proxies.
#[async_trait]
pub trait ProxyNodes: Send + Sync {
    /// Whether nodes can run on this server, and if not, why.
    fn availability(&self) -> Availability;
    /// Start the node, or apply a changed name or key to a running one.
    async fn ensure(&self, config: NodeConfig) -> Result<(), TailscaleError>;
    /// Restart the node and log it in again.
    async fn reconnect(&self, config: NodeConfig) -> Result<(), TailscaleError>;
    /// What the node was last seen doing.
    fn status(&self, id: i64) -> NodeStatus;
    /// Log the node out, stop it and delete its state.
    async fn remove(&self, id: i64) -> Result<(), TailscaleError>;
    /// Bring the nodes in line with these proxies: others are removed.
    async fn reconcile(&self, configs: Vec<NodeConfig>);
    /// The address an MCP reaches the node through, or why it cannot now.
    fn proxy_url(&self, id: i64) -> Result<Url, String>;
    /// Stop every node. Their state stays on disk.
    async fn shutdown(&self);
}

#[async_trait]
impl ProxyNodes for Tailscale {
    fn availability(&self) -> Availability {
        Tailscale::availability(self).clone()
    }

    async fn ensure(&self, config: NodeConfig) -> Result<(), TailscaleError> {
        Tailscale::ensure(self, config).await
    }

    async fn reconnect(&self, config: NodeConfig) -> Result<(), TailscaleError> {
        Tailscale::reconnect(self, config).await
    }

    fn status(&self, id: i64) -> NodeStatus {
        Tailscale::status(self, id)
    }

    async fn remove(&self, id: i64) -> Result<(), TailscaleError> {
        Tailscale::remove(self, id).await
    }

    async fn reconcile(&self, configs: Vec<NodeConfig>) {
        Tailscale::reconcile(self, configs).await;
    }

    fn proxy_url(&self, id: i64) -> Result<Url, String> {
        Tailscale::proxy_url(self, id)
    }

    async fn shutdown(&self) {
        Tailscale::shutdown(self).await;
    }
}

/// The proxies of the gateway, as the upstream looks them up: the runtime
/// behind the seam. A newtype, since the trait and the runtime are both
/// from other crates.
pub struct ProxyDirectoryOf(pub Arc<dyn ProxyNodes>);

impl ProxyDirectory for ProxyDirectoryOf {
    fn proxy_url(&self, proxy_id: i64) -> Result<Url, String> {
        self.0.proxy_url(proxy_id)
    }
}
