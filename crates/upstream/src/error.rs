use std::sync::Arc;

use mymcps_builtin::BuiltinError;
use mymcps_core::public_url::PublicUrlError;
use mymcps_deno::DenoError;
use mymcps_mcp::client::ClientError;
use mymcps_net::{HttpUrlError, RestrictedEndpointError};

use crate::npm_update::McpNpmUpdateError;

/// Why an upstream MCP could not be listed, called, tested or authorized.
///
/// It reads as the message of the error the Node app threw in its place:
/// callers redact it (`sanitize_mcp_diagnostic`), store it in `last_error`
/// and show it to administrators.
///
/// It is cheap to clone: every caller that waits for the same token refresh
/// is handed the failure of that one refresh.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UpstreamError {
    /// The MCP server answered HTTP 401 while the gateway connected to it
    /// (`UpstreamUnauthorizedError`). The message quotes what the server
    /// said, already redacted.
    #[error("{0}")]
    Unauthorized(String),

    /// The MCP client failed: the server could not be reached, answered with
    /// a JSON-RPC error, did not answer in time, or answered something that
    /// is not a result. An HTTP 401 after the handshake is in here, see
    /// [`UpstreamError::is_unauthorized`].
    #[error(transparent)]
    Client(Arc<ClientError>),

    /// An npm MCP could not be started in its Deno sandbox, or failed once started.
    #[error(transparent)]
    Deno(Arc<DenoError>),

    /// A document served by the MCP or by its OAuth provider names an
    /// endpoint inside a network the MCP is not part of.
    #[error(transparent)]
    RestrictedEndpoint(#[from] RestrictedEndpointError),

    /// A step of the OAuth flow failed: discovery, client registration, the
    /// exchange of the code or the refresh of the tokens.
    #[error(transparent)]
    OAuth(Arc<mymcps_mcp_auth::Error>),

    /// A built-in MCP refused or failed. See
    /// [`UpstreamError::is_builtin_tool_error`] for the failures meant for
    /// the agent.
    #[error(transparent)]
    Builtin(Arc<BuiltinError>),

    /// The MCP is not one that can be updated to the latest version of its package.
    #[error(transparent)]
    NpmUpdate(#[from] McpNpmUpdateError),

    #[error(transparent)]
    HttpUrl(#[from] HttpUrlError),

    #[error(transparent)]
    PublicUrl(#[from] PublicUrlError),

    #[error(transparent)]
    Database(Arc<sqlx::Error>),

    /// Anything else, with the message the Node app gave it.
    #[error("{0}")]
    Other(String),
}

impl UpstreamError {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// The upstream wants credentials, or refused the ones it got: what the
    /// Node app recognised as an `UnauthorizedError`, an
    /// `UpstreamUnauthorizedError` or a Streamable HTTP error with code 401.
    pub fn is_unauthorized(&self) -> bool {
        match self {
            Self::Unauthorized(_) => true,
            Self::Client(error) => error.is_unauthorized(),
            _ => false,
        }
    }

    /// A discovered endpoint was refused by the address guard
    /// (`RestrictedEndpointError`).
    pub fn is_restricted_endpoint(&self) -> bool {
        matches!(self, Self::RestrictedEndpoint(_))
    }

    /// The error of a built-in MCP, when that is what failed.
    pub fn builtin(&self) -> Option<&BuiltinError> {
        match self {
            Self::Builtin(error) => Some(error),
            _ => None,
        }
    }

    /// A failure of a built-in MCP whose message is meant for the agent
    /// (`BuiltinToolError`, which a `BuiltinAuthorizationError` is too).
    pub fn is_builtin_tool_error(&self) -> bool {
        self.builtin().is_some_and(BuiltinError::is_tool_error)
    }

    /// The provider of a built-in MCP no longer accepts the saved sign-in
    /// (`BuiltinAuthorizationError`).
    pub fn is_builtin_authorization_error(&self) -> bool {
        self.builtin()
            .is_some_and(BuiltinError::is_authorization_error)
    }

    /// An update was asked for an MCP that cannot be updated
    /// (`McpNpmUpdateError`). Its message is shown as it is.
    pub fn is_npm_update_error(&self) -> bool {
        matches!(self, Self::NpmUpdate(_))
    }

    /// The same failure as the error of a built-in MCP, for the functions
    /// that only fail with one.
    pub(crate) fn into_builtin(self) -> BuiltinError {
        match self {
            Self::Builtin(error) => clone_builtin_error(&error),
            other => BuiltinError::internal(other),
        }
    }
}

fn clone_builtin_error(error: &BuiltinError) -> BuiltinError {
    match error {
        BuiltinError::Tool(message) => BuiltinError::Tool(message.clone()),
        BuiltinError::Authorization(message) => BuiltinError::Authorization(message.clone()),
        BuiltinError::Internal(message) => BuiltinError::Internal(message.clone()),
    }
}

impl From<ClientError> for UpstreamError {
    fn from(error: ClientError) -> Self {
        Self::Client(Arc::new(error))
    }
}

impl From<DenoError> for UpstreamError {
    fn from(error: DenoError) -> Self {
        Self::Deno(Arc::new(error))
    }
}

impl From<BuiltinError> for UpstreamError {
    fn from(error: BuiltinError) -> Self {
        Self::Builtin(Arc::new(error))
    }
}

impl From<sqlx::Error> for UpstreamError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(Arc::new(error))
    }
}

impl From<mymcps_mcp_auth::Error> for UpstreamError {
    /// An address the guard refused comes back from the OAuth helpers inside
    /// the error of the fetch. It is the same refusal wherever it surfaces.
    fn from(error: mymcps_mcp_auth::Error) -> Self {
        match error.downcast_fetch_error::<RestrictedEndpointError>() {
            Some(restricted) => Self::RestrictedEndpoint(restricted.clone()),
            None => Self::OAuth(Arc::new(error)),
        }
    }
}
