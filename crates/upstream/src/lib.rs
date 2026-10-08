//! How the gateway talks to the MCP servers behind it: HTTP MCPs (with
//! bearer, header or OAuth authentication), npm MCPs run in a Deno sandbox,
//! and the MCPs MyMCPs implements itself.
//!
//! One [`Upstream`] is built at startup and shared. Nothing is global: the
//! token renewals under way, the HTTP client, the name resolution behind the
//! address check and the Deno runner all belong to that value, so each test
//! builds its own.
//!
//! Functions take the row of the MCP as `&mut Mcp`: renewing an OAuth access
//! token reads the row again, and the caller goes on with the row that was
//! read. Every caller holds its own copy of a row. Whatever copies are in
//! flight, the refresh token of an MCP is used once: callers that need a
//! renewal at the same time share it, each then reads the saved pair, and
//! none goes on with the old token when the renewal fails.
//!
//! # Listing, calling, testing
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_builtin::BuiltinRegistry;
//! use mymcps_core::Core;
//! use mymcps_core::models::Mcp;
//! use mymcps_upstream::{Upstream, UpstreamError, approvals};
//!
//! # async fn example(core: Arc<Core>, mut mcp: Mcp) -> Result<(), UpstreamError> {
//! // The server registers the providers it ships with.
//! let upstream = Upstream::new(core, BuiltinRegistry::new(vec![]));
//!
//! // The tools of one MCP, with a note on those that wait for a person.
//! let tools = upstream.probe(&mut mcp).await?;
//! for tool in approvals::with_approval_notes(upstream.builtins(), &mcp, tools) {
//!     println!("{}: {:?}", tool.name, tool.description);
//! }
//!
//! match upstream.call_tool(&mut mcp, "search", None).await {
//!     // What the tool answered. A result with `isError` is a result.
//!     Ok(result) => println!("{result}"),
//!     // The MCP wants an OAuth authorization, or refused the saved one.
//!     Err(error) if error.is_unauthorized() => println!("{error}"),
//!     // The message is the one administrators read, once redacted with
//!     // `mymcps_core::redaction::sanitize_mcp_diagnostic`.
//!     Err(error) => return Err(error),
//! }
//!
//! // Sets `status`, `last_error` and `oauth_required`, and saves them.
//! upstream.test_and_update_status(&mut mcp).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Authorizing an MCP with OAuth
//!
//! The pending authorization lives in the browser session between the
//! redirect to the provider and its callback. The session belongs to the web
//! layer, which implements [`OauthSessionStore`] for its session type.
//!
//! ```no_run
//! use mymcps_core::models::Mcp;
//! use mymcps_upstream::{OauthSessionStore, Upstream, UpstreamError};
//!
//! /// `GET /mcps/:id/oauth/start`: where to send the browser.
//! async fn start(
//!     upstream: &Upstream,
//!     session: &dyn OauthSessionStore,
//!     mcp: &mut Mcp,
//! ) -> Result<String, UpstreamError> {
//!     // Discovers the provider, registers a client when it has to, saves
//!     // both on the row and keeps the pending authorization in the session.
//!     upstream.start_oauth_flow(session, mcp).await
//! }
//!
//! /// `GET /mcps/oauth/callback?code=...&state=...`
//! async fn callback(
//!     upstream: &Upstream,
//!     session: &dyn OauthSessionStore,
//!     mcp: &mut Mcp,
//!     code: &str,
//!     state: &str,
//! ) -> Result<(), UpstreamError> {
//!     let pending = upstream.read_oauth_session(session, Some(state));
//!     upstream.clear_oauth_session(session, Some(state));
//!     let Some(pending) = pending.filter(|pending| pending.mcp_id == mcp.id) else {
//!         return Ok(()); // "Invalid OAuth callback"
//!     };
//!     // Saves the tokens, and marks the MCP ready.
//!     upstream.exchange_authorization_code(mcp, &pending, code, None).await?;
//!     upstream.test_and_update_status(mcp).await
//! }
//! ```
//!
//! Every endpoint a document of the MCP or of its provider names is checked
//! before it is requested, and again on every renewal: a remote MCP cannot
//! aim the gateway at the network the instance runs in. A refusal is an
//! [`UpstreamError::RestrictedEndpoint`], and nothing falls back after one.
//!
//! # In a test
//!
//! ```
//! # async fn example() {
//! use std::sync::Arc;
//!
//! use mymcps_builtin::BuiltinRegistry;
//! use mymcps_core::TestCore;
//! use mymcps_net::{AddressGuard, CannedResponse, Fetcher, StaticResolver};
//! use mymcps_upstream::Upstream;
//!
//! let core = TestCore::new().await;
//! // Only the names declared here resolve, and nothing is asked of the system.
//! let resolver = StaticResolver::new().with("auth.example", &["203.0.113.10".parse().unwrap()]);
//! let upstream = Upstream::builder(core.core.clone(), BuiltinRegistry::new(vec![]))
//!     // Nothing leaves the machine: every request is answered here, by the
//!     // MCP server and by its OAuth provider alike.
//!     .fetcher(Fetcher::offline().answering(|_request| {
//!         Some(CannedResponse::new(http::StatusCode::NOT_FOUND))
//!     }))
//!     .address_guard(AddressGuard::new(Arc::new(resolver)))
//!     .build();
//! # let _ = upstream;
//! # }
//! ```
//!
//! The builder also takes a [`DenoRunner`] whose Deno binary is a stand-in,
//! a `BuiltinEnv` with the seams of the providers, and an
//! [`NpmUpdateRuntime`] in place of the two steps of an npm update.

pub mod allowlisted_client;
pub mod approvals;
mod builtin;
mod error;
mod http_client;
mod manager;
mod npm_update;
mod oauth;
mod session;
mod shared;
pub mod validators;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mymcps_builtin::{BuiltinEnv, BuiltinRegistry};
use mymcps_core::Core;
use mymcps_deno::DenoRunner;
use mymcps_net::{AddressGuard, Fetcher};

pub use error::UpstreamError;
pub use http_client::{ConnectedHttpUpstream, UpstreamTool};
pub use manager::{NamespacedTool, NamespacedToolName, namespace_tool, parse_namespaced_tool};
pub use npm_update::{
    McpNpmUpdateError, McpNpmUpdateFailure, McpNpmUpdateRunResult, NpmUpdateRuntime,
    is_latest_npm_version, is_tracking_latest,
};
pub use oauth::uses_pasted_oauth_callback;
pub use session::{
    MAX_PENDING_OAUTH_START_BYTES, OauthSession, OauthSessionStore, OauthStart,
    clear_oauth_session, read_oauth_session, start_oauth_session,
};

/// The gateway's side of every MCP it exposes. See the crate documentation.
///
/// Built once and shared as an `Arc`. Cloning it is cheap, and clones share
/// everything, the token renewals under way included.
#[derive(Clone)]
pub struct Upstream {
    core: Arc<Core>,
    /// Sends the requests to HTTP MCPs and to their OAuth providers.
    fetcher: Fetcher,
    /// Checks the endpoints that documents of an MCP or its provider name.
    addresses: AddressGuard,
    builtin_env: BuiltinEnv,
    builtins: BuiltinRegistry,
    deno: DenoRunner,
    /// The renewal of the OAuth tokens of an MCP that is under way, by MCP id.
    pending_refreshes: Arc<Mutex<HashMap<i64, oauth::PendingRefresh>>>,
    /// How many tools each MCP listed the last time, by MCP id.
    known_tool_counts: Arc<Mutex<HashMap<i64, usize>>>,
    /// The update of the latest-tracking npm MCPs that is under way.
    pending_npm_updates: Arc<Mutex<Option<npm_update::PendingNpmUpdates>>>,
    npm_update_runtime: Option<Arc<dyn NpmUpdateRuntime>>,
}

impl Upstream {
    /// The upstream of the server: requests go over the network, names are
    /// resolved by the system, and npm MCPs run with the Deno of the machine.
    pub fn new(core: Arc<Core>, builtins: BuiltinRegistry) -> Arc<Self> {
        Self::builder(core, builtins).build()
    }

    /// An upstream with some of its parts replaced: by the server, which may
    /// hand in the environment of its built-in MCPs, and by tests.
    pub fn builder(core: Arc<Core>, builtins: BuiltinRegistry) -> UpstreamBuilder {
        UpstreamBuilder {
            core,
            builtins,
            fetcher: None,
            addresses: None,
            builtin_env: None,
            deno: None,
            npm_update_runtime: None,
        }
    }

    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// What the tools of built-in MCPs run with.
    pub fn builtin_env(&self) -> &BuiltinEnv {
        &self.builtin_env
    }

    /// The runner of npm MCPs.
    pub fn deno(&self) -> &DenoRunner {
        &self.deno
    }

    /// Semver currently present in the Deno npm cache for this package: see
    /// [`DenoRunner::cached_npm_package_version`].
    pub fn cached_npm_package_version(
        &self,
        npm_package: &str,
        npm_version: Option<&str>,
    ) -> Option<String> {
        self.deno
            .cached_npm_package_version(npm_package, npm_version)
    }

    /// Delete what an npm MCP wrote to its sandbox, when the MCP is deleted
    /// or runs another package.
    pub async fn remove_mcp_sandbox(&self, mcp_id: i64) -> Result<(), UpstreamError> {
        Ok(self.deno.remove_sandbox(mcp_id).await?)
    }

    /// [`uses_pasted_oauth_callback`].
    pub fn uses_pasted_oauth_callback(&self, mcp: &mymcps_core::models::Mcp) -> bool {
        uses_pasted_oauth_callback(mcp)
    }

    /// [`read_oauth_session`].
    pub fn read_oauth_session(
        &self,
        session: &dyn OauthSessionStore,
        state: Option<&str>,
    ) -> Option<OauthSession> {
        read_oauth_session(session, state)
    }

    /// [`clear_oauth_session`].
    pub fn clear_oauth_session(&self, session: &dyn OauthSessionStore, state: Option<&str>) {
        clear_oauth_session(session, state);
    }
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Upstream")
            .field("builtins", &self.builtins)
            .finish_non_exhaustive()
    }
}

/// Builds an [`Upstream`]. What is not set is what the server uses.
pub struct UpstreamBuilder {
    core: Arc<Core>,
    builtins: BuiltinRegistry,
    fetcher: Option<Fetcher>,
    addresses: Option<AddressGuard>,
    builtin_env: Option<BuiltinEnv>,
    deno: Option<DenoRunner>,
    npm_update_runtime: Option<Arc<dyn NpmUpdateRuntime>>,
}

impl UpstreamBuilder {
    /// Where the requests to HTTP MCPs and OAuth providers go. The tools of
    /// built-in MCPs use it too, unless [`builtin_env`](Self::builtin_env)
    /// gives them an environment of their own.
    pub fn fetcher(mut self, fetcher: Fetcher) -> Self {
        self.fetcher = Some(fetcher);
        self
    }

    /// The check of discovered OAuth endpoints, with the name resolution it
    /// relies on.
    pub fn address_guard(mut self, addresses: AddressGuard) -> Self {
        self.addresses = Some(addresses);
        self
    }

    /// What the tools of built-in MCPs run with, used as it is given.
    pub fn builtin_env(mut self, builtin_env: BuiltinEnv) -> Self {
        self.builtin_env = Some(builtin_env);
        self
    }

    pub fn deno(mut self, deno: DenoRunner) -> Self {
        self.deno = Some(deno);
        self
    }

    /// Replace the two steps of an npm update.
    pub fn npm_update_runtime(mut self, runtime: impl NpmUpdateRuntime) -> Self {
        self.npm_update_runtime = Some(Arc::new(runtime));
        self
    }

    pub fn build(self) -> Arc<Upstream> {
        let fetcher = self.fetcher.unwrap_or_default();
        let builtin_env = self.builtin_env.unwrap_or_else(|| {
            BuiltinEnv::new(Arc::clone(&self.core)).with_fetcher(fetcher.clone())
        });
        let deno = self
            .deno
            .unwrap_or_else(|| DenoRunner::new(Arc::clone(&self.core)));
        Arc::new(Upstream {
            core: self.core,
            fetcher,
            addresses: self.addresses.unwrap_or_default(),
            builtin_env,
            builtins: self.builtins,
            deno,
            pending_refreshes: Arc::default(),
            known_tool_counts: Arc::default(),
            pending_npm_updates: Arc::default(),
            npm_update_runtime: self.npm_update_runtime,
        })
    }
}
