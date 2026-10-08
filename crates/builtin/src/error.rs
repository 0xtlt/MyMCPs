use mymcps_net::FetchError;

pub type BuiltinResult<T> = Result<T, BuiltinError>;

/// Why a built-in MCP could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    /// A failure the calling agent can act on. Its message becomes the tool
    /// result, so it must never contain credentials. (`BuiltinToolError`)
    #[error("{0}")]
    Tool(String),

    /// The provider no longer accepts the saved authorization. Also a
    /// failure the agent is told about. (`BuiltinAuthorizationError`)
    #[error("{0}")]
    Authorization(String),

    /// Anything else: a bug, or the provider answering in a way nobody
    /// planned for. Logged after redaction; the agent only learns that the
    /// call failed.
    #[error("{0}")]
    Internal(String),
}

impl BuiltinError {
    pub fn tool(message: impl Into<String>) -> Self {
        Self::Tool(message.into())
    }

    pub fn authorization(message: impl Into<String>) -> Self {
        Self::Authorization(message.into())
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }

    /// Whether the message is meant for the agent: what the Node app tested
    /// with `error instanceof BuiltinToolError`.
    pub fn is_tool_error(&self) -> bool {
        matches!(self, Self::Tool(_) | Self::Authorization(_))
    }

    pub fn is_authorization_error(&self) -> bool {
        matches!(self, Self::Authorization(_))
    }
}

impl From<sqlx::Error> for BuiltinError {
    fn from(error: sqlx::Error) -> Self {
        Self::internal(error)
    }
}

impl From<std::io::Error> for BuiltinError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}

impl From<serde_json::Error> for BuiltinError {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(error)
    }
}

impl From<FetchError> for BuiltinError {
    fn from(error: FetchError) -> Self {
        Self::internal(error)
    }
}

impl From<crate::upload_store::UploadError> for BuiltinError {
    fn from(error: crate::upload_store::UploadError) -> Self {
        Self::internal(error)
    }
}
