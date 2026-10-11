//! What AI clients meet: the access tokens they authenticate with and the
//! OAuth 2.1 authorization server that issues them, the tools of the lazy
//! gateway, and the log of the calls they make.
//!
//! Nothing here is global. A [`Gateway`] is built once per server, next to
//! the [`Core`] it works on, and holds the state the services keep between
//! requests: the request allowance of each token, the queue of call log
//! writes, and when unused OAuth clients were last pruned.
//!
//! - [`access_token`]: making, finding, refreshing and revoking tokens.
//! - [`oauth`]: the OAuth server, as [`Gateway::oauth`].
//! - [`bearer`]: [`Gateway::authenticate_bearer`], which lets a request in.
//! - [`lazy_tools`]: the three tools of the lazy tool mode.
//! - [`call_log`]: the call log, as [`Gateway::call_log`].
//! - [`validators`]: the Vine schemas the above read their input with.
//!
//! What needs the MCPs behind the gateway is in an [`McpGateway`], built
//! once per server from the [`Gateway`] and the `Upstream` that reaches them:
//!
//! - [`endpoint`]: [`McpGateway::handle`], which answers `/mcp`.
//! - [`approvals`]: the tool calls that wait for a person, as
//!   [`McpGateway::approvals`].
//! - [`auto_update`]: the job that keeps npm MCPs on their latest version,
//!   as [`McpGateway::auto_update`].
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_core::Core;
//! use mymcps_gateway::{Gateway, McpGateway, McpRequest};
//! use mymcps_upstream::Upstream;
//!
//! # async fn example(core: Arc<Core>, upstream: Arc<Upstream>) -> Result<(), sqlx::Error> {
//! // Once, when the server starts.
//! let gateway = Arc::new(Gateway::new(core));
//! let mcp_gateway = Arc::new(McpGateway::new(gateway, upstream));
//! mcp_gateway.auto_update.start().await?;
//!
//! // For each GET or POST on /mcp.
//! # let (method, headers) = (http::Method::POST, http::HeaderMap::new());
//! let response = mcp_gateway
//!     .handle(McpRequest {
//!         method: &method,
//!         headers: &headers,
//!         // The parsed body, under the parameters of the query string.
//!         body: serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
//!         caller_ip: Some("203.0.113.7".to_owned()),
//!     })
//!     .await?;
//! // The body is a stream of `Result<Bytes, Infallible>`: with axum,
//! // `Response::from_parts(parts, Body::from_stream(body))`.
//! let (parts, body) = response.into_parts();
//! # let _ = (parts, body);
//!
//! // After the instance settings were saved, and when the server stops.
//! mcp_gateway.auto_update.resync().await?;
//! mcp_gateway.auto_update.stop();
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use mymcps_core::Core;
use mymcps_core::limiter::Limiter;

pub mod access_token;
pub mod approvals;
pub mod auto_update;
pub mod bearer;
pub mod call_log;
pub mod endpoint;
pub mod lazy_tools;
pub mod oauth;
pub mod rate_limiter;
pub mod validators;

mod db;
mod error;
mod js;

// The tests in `src/` share the helpers of those in `tests/`, which name
// this crate from the outside.
#[cfg(test)]
extern crate self as mymcps_gateway;

#[cfg(test)]
mod concurrency_tests;

pub use endpoint::{McpGateway, McpRequest};
pub use error::{Error, GatewayOauthError, Result};

use call_log::McpCallLogService;
use oauth::OauthServer;

/// The gateway of one instance.
#[derive(Debug)]
pub struct Gateway {
    pub core: Arc<Core>,
    /// The request allowance of each access token on `/mcp`.
    pub rate_limiter: Limiter,
    pub call_log: McpCallLogService,
    pub oauth: OauthServer,
}

impl Gateway {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            rate_limiter: rate_limiter::gateway_rate_limiter(),
            call_log: McpCallLogService::new(core.clone()),
            oauth: OauthServer::new(core.clone()),
            core,
        }
    }
}
