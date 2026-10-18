use std::sync::Arc;
use std::time::Duration;

use axum::extract::FromRef;
use mymcps_builtin::BuiltinRegistry;
use mymcps_core::Core;
use mymcps_core::limiter::Limiter;
use mymcps_core::models::Proxy;
use mymcps_gateway::{Gateway, McpGateway};
use mymcps_tailscale::Tailscale;
use mymcps_upstream::Upstream;

use crate::proxy_nodes::{NodeConfig, ProxyDirectoryOf, ProxyNodes};
use crate::routes::backup::BackupWork;
use crate::routes::builtin_files::FileTraffic;

/// Allowances for repeated credential failures and for costly public
/// endpoints (`start/limiter.ts` in the Node app).
#[derive(Debug, Clone)]
pub struct Limiters {
    /// Sign-in attempts on one account from one client address.
    pub login: Limiter,
    /// Failed sign-in attempts from one client address, whichever accounts they target.
    pub login_address: Limiter,
    /// Accounts made from one client address on the sign-up page: each one
    /// costs a password hash, and a team.
    pub registration: Limiter,
    /// Current-password confirmations by one signed-in user.
    pub current_password: Limiter,
    /// The challenges passkeys answered, each counted once: the session is
    /// a cookie, which a browser can send again as it was before an answer.
    pub passkey_challenge: Limiter,
    pub oauth_authorization: Limiter,
    pub oauth_token: Limiter,
    pub oauth_registration: Limiter,
    /// Each download of a built-in MCP file signs in to its provider.
    pub builtin_file: Limiter,
    /// Each upload to a built-in MCP writes a file to the instance's disk.
    pub builtin_upload: Limiter,
    /// Each import of a backup, by anyone who reaches the setup screen,
    /// writes a file to the instance's disk and derives a costly key.
    pub backup_import: Limiter,
}

impl Limiters {
    pub fn new(core: &Core) -> Self {
        const MINUTES: u64 = 60;
        // SQLite is the durable store, and makes increments atomic. Tests
        // count in memory, each with its own counters.
        let limiter = |requests: u32, seconds: u64| {
            if core.config.is_test() {
                Limiter::memory(requests, Duration::from_secs(seconds))
            } else {
                Limiter::database(&core.db, requests, Duration::from_secs(seconds))
            }
        };
        Self {
            login: limiter(5, 15 * MINUTES),
            login_address: limiter(30, 15 * MINUTES),
            registration: limiter(10, 60 * MINUTES),
            current_password: limiter(5, 15 * MINUTES),
            // Longer than a challenge may be answered.
            passkey_challenge: limiter(1, 10 * MINUTES),
            oauth_authorization: limiter(100, 15 * MINUTES),
            oauth_token: limiter(50, 15 * MINUTES),
            oauth_registration: limiter(20, 60 * MINUTES),
            builtin_file: limiter(60, 15 * MINUTES),
            builtin_upload: limiter(60, 15 * MINUTES),
            backup_import: limiter(10, 15 * MINUTES),
        }
    }
}

/// The MCPs MyMCPs implements itself.
pub fn builtins() -> BuiltinRegistry {
    BuiltinRegistry::new(vec![
        mymcps_strava::definition(),
        mymcps_icloud_mail::definition(),
        mymcps_google_ads::definition(),
    ])
}

/// What every handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
    pub limiters: Arc<Limiters>,
    /// Access tokens, the OAuth authorization server, the call log.
    pub gateway: Arc<Gateway>,
    /// The MCPs behind the gateway: their tools, their sign-in, their updates.
    pub upstream: Arc<Upstream>,
    /// What needs both: the `/mcp` endpoint, the approvals of tool calls,
    /// and the schedule npm MCPs are refreshed on.
    pub mcp_gateway: Arc<McpGateway>,
    /// The downloads and uploads of built-in MCP files that are under way.
    pub file_traffic: FileTraffic,
    /// The import of a backup that is under way, and the limits of one.
    pub backups: BackupWork,
    /// The Tailscale nodes behind the proxies of the teams.
    pub proxy_nodes: Arc<dyn ProxyNodes>,
}

impl AppState {
    /// The state around a runtime that runs no proxy: MCPs are reached over
    /// the network, and the Proxies page says proxies are off.
    pub fn new(core: Arc<Core>) -> Self {
        Self::with_proxy_nodes(core, Arc::new(Tailscale::disabled()))
    }

    /// The state of the server: MCPs are reached over the network, those
    /// that name a proxy through the node `proxy_nodes` runs for it.
    pub fn with_proxy_nodes(core: Arc<Core>, proxy_nodes: Arc<dyn ProxyNodes>) -> Self {
        let upstream = Upstream::builder(core.clone(), builtins())
            .proxies(Arc::new(ProxyDirectoryOf(proxy_nodes.clone())))
            .build();
        Self::with_parts(core, upstream, proxy_nodes)
    }

    /// The state around an upstream built by the caller, which a test does
    /// to answer the requests an MCP would receive. It runs no proxy.
    pub fn with_upstream(core: Arc<Core>, upstream: Arc<Upstream>) -> Self {
        Self::with_parts(core, upstream, Arc::new(Tailscale::disabled()))
    }

    /// The state around an upstream and the runtime of proxies the caller
    /// built. The upstream is expected to reach proxies through that runtime.
    pub fn with_parts(
        core: Arc<Core>,
        upstream: Arc<Upstream>,
        proxy_nodes: Arc<dyn ProxyNodes>,
    ) -> Self {
        let limiters = Arc::new(Limiters::new(&core));
        let gateway = Arc::new(Gateway::new(core.clone()));
        // Built from the same two values: a second gateway would count the
        // requests of a token and queue its call logs on its own.
        let mcp_gateway = Arc::new(McpGateway::new(gateway.clone(), upstream.clone()));
        Self {
            core,
            limiters,
            gateway,
            upstream,
            mcp_gateway,
            file_traffic: FileTraffic::new(),
            backups: BackupWork::new(),
            proxy_nodes,
        }
    }
}

impl AppState {
    /// How many tool calls wait for this person's approval in the team
    /// they work in.
    pub async fn pending_approvals(
        &self,
        scope: mymcps_gateway::approvals::ApprovalScope,
    ) -> Result<i64, sqlx::Error> {
        self.mcp_gateway.approvals.pending_count(scope).await
    }

    /// How many tools each MCP exposed the last time its tools were listed
    /// since the server started, by MCP id. Nothing is stored: an MCP whose
    /// tools were not listed yet is absent.
    pub fn known_tool_counts(&self) -> std::collections::HashMap<i64, usize> {
        self.upstream.known_tool_counts()
    }

    /// Tell the background work that the instance settings were saved: the
    /// schedule npm MCPs are refreshed on may have changed.
    pub async fn instance_settings_changed(&self) -> Result<(), sqlx::Error> {
        self.mcp_gateway.auto_update.resync().await
    }

    /// Tell the background work that every row of the database was
    /// replaced: a backup was imported. The schedule npm MCPs are refreshed
    /// on is the one thing the server keeps of the database in memory.
    /// Tool counts, token allowances and queued call logs are keyed by rows
    /// that did not exist before the import, which only an instance without
    /// a user takes.
    pub async fn database_replaced(&self) -> Result<(), sqlx::Error> {
        self.instance_settings_changed().await?;
        // The nodes of the proxies the backup holds start now: they log in
        // again with their saved keys, or ask for a login link.
        self.reconcile_proxy_nodes().await
    }
}

impl AppState {
    /// What the node of a proxy runs with. The saved key is decrypted here,
    /// for the runtime only: it never goes into a page.
    pub fn node_config(&self, proxy: &Proxy) -> NodeConfig {
        NodeConfig {
            id: proxy.id,
            hostname: proxy.tailscale_hostname.clone(),
            auth_key: self
                .core
                .decrypt_secret(proxy.tailscale_auth_key.as_deref())
                .filter(|key| !key.is_empty()),
        }
    }

    /// Bring the nodes in line with the proxies of the database.
    pub async fn reconcile_proxy_nodes(&self) -> Result<(), sqlx::Error> {
        let configs = Proxy::all(&*self.core.db)
            .await?
            .iter()
            .map(|proxy| self.node_config(proxy))
            .collect();
        self.proxy_nodes.reconcile(configs).await;
        Ok(())
    }

    /// Remove the nodes of proxies that were just deleted with their team.
    /// Called with the ids the proxies had before the deletion committed:
    /// those still in the database are kept. A node that cannot be removed
    /// is logged, not an error: the row is gone, and the next start
    /// deletes what is left.
    pub async fn remove_deleted_proxy_nodes(&self, ids: &[i64]) -> Result<(), sqlx::Error> {
        if ids.is_empty() {
            return Ok(());
        }
        let remaining: std::collections::HashSet<i64> = Proxy::all(&*self.core.db)
            .await?
            .into_iter()
            .map(|proxy| proxy.id)
            .collect();
        for id in ids.iter().filter(|id| !remaining.contains(id)) {
            if let Err(error) = self.proxy_nodes.remove(*id).await {
                tracing::error!(proxy = *id, %error, "Could not remove the Tailscale node of a deleted proxy");
            }
        }
        Ok(())
    }
}

impl FromRef<AppState> for Arc<Core> {
    fn from_ref(state: &AppState) -> Self {
        state.core.clone()
    }
}
