use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

/// How the runtime is set up. The server reads it from its configuration.
#[derive(Debug, Clone)]
pub struct TailscaleOptions {
    /// `TAILSCALED_BINARY`; `None` looks up `tailscaled` on `PATH`.
    pub binary: Option<PathBuf>,
    /// `<DATA_DIR>/tailscale`, created 0700 if missing. Each node lives in a
    /// directory named after its id.
    pub root: PathBuf,
    /// `TAILSCALE_MAX_PROXIES`; 0 = proxies are off.
    pub max_nodes: usize,
}

/// Whether nodes can run on this server, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// `version` from `tailscaled --version` (first line).
    Available { version: String },
    /// `TAILSCALE_MAX_PROXIES=0`.
    Disabled,
    /// One sentence, e.g. "tailscaled was not found: install Tailscale or set TAILSCALED_BINARY".
    Unavailable { reason: String },
}

/// What a node needs to run. It comes from a proxy row of the database.
#[derive(Clone)]
pub struct NodeConfig {
    pub id: i64,
    /// Name in the tailnet, e.g. "mymcps".
    pub hostname: String,
    /// Decrypted auth key, used only when the node needs to log in.
    pub auth_key: Option<String>,
}

impl fmt::Debug for NodeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeConfig")
            .field("id", &self.id)
            .field("hostname", &self.hostname)
            .field("auth_key", &self.auth_key.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeState {
    /// Process starting or backend not settled yet.
    Starting,
    /// Needs a login: `auth_url` once Tailscale gave one (interactive login).
    NeedsLogin {
        auth_url: Option<String>,
    },
    /// Logged in, waiting for an admin of the tailnet to approve the device.
    NeedsApproval,
    Connected,
    /// The process keeps failing, or the auth key was refused. One sentence.
    Failed {
        error: String,
    },
    /// Not running (proxies off, binary missing, or removed).
    Stopped,
}

/// What the last poll of the node said. Nothing here is a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStatus {
    pub state: NodeState,
    /// `CurrentTailnet.Name`
    pub tailnet: Option<String>,
    /// `Self.DNSName` without the trailing dot
    pub dns_name: Option<String>,
    /// `Self.TailscaleIPs`
    pub addresses: Vec<IpAddr>,
    /// `Health`
    pub health: Vec<String>,
}

impl NodeStatus {
    pub(crate) fn new(state: NodeState) -> Self {
        Self {
            state,
            tailnet: None,
            dns_name: None,
            addresses: Vec::new(),
            health: Vec::new(),
        }
    }
}
