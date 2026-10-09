use std::io;
use std::path::PathBuf;

use mymcps_core::secrets::EnvironmentError;
use mymcps_mcp::McpError;
use mymcps_mcp::client::ClientError;

use crate::paths::file_system_message;

/// Why an npm MCP could not be reached. Each variant reads as the message of
/// the error the Node app threw in its place: callers redact it
/// (`sanitize_mcp_diagnostic`) and show it to administrators.
#[derive(Debug, thiserror::Error)]
pub enum DenoError {
    #[error("npm MCP is missing a package name")]
    MissingPackage,

    #[error(
        "Deno was not found. Install Deno, or set DENO_PATH to the absolute path of the deno binary."
    )]
    BinaryNotFound,

    #[error(
        "The Deno cache directory \"{}\" must not contain the application or lie inside an MCP sandbox. Set DENO_DIR to a dedicated directory.",
        .0.display()
    )]
    UnsafeCacheDirectory(PathBuf),

    /// The saved environment of the MCP cannot be read.
    #[error(transparent)]
    Environment(#[from] EnvironmentError),

    /// Deno or the package did not get through the MCP handshake. The message
    /// is redacted already, and says what the process wrote to stderr.
    #[error("{0}")]
    Startup(String),

    /// The package started and the request then failed: the server answered
    /// with a JSON-RPC error, did not answer in time, closed the connection,
    /// or answered something that is not a result.
    #[error(transparent)]
    Client(#[from] ClientError),

    /// `detail` is the start of what `deno cache` wrote to stderr, as written.
    #[error("Failed to reload Deno cache for \"{package}\". {detail}")]
    CacheReload { package: String, detail: String },

    #[error("{}", file_system_message(.source, .operation, .path))]
    FileSystem {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// What a binary lookup replaced in a test fails with.
    #[error("{0}")]
    Other(String),
}

impl DenoError {
    /// The error of the protocol, when the started package answered with one
    /// or the request timed out or lost its connection.
    pub fn mcp_error(&self) -> Option<&McpError> {
        match self {
            Self::Client(ClientError::Mcp(error)) => Some(error),
            _ => None,
        }
    }

    /// The JSON-RPC error code of [`DenoError::mcp_error`].
    pub fn mcp_code(&self) -> Option<i64> {
        self.mcp_error().map(|error| error.code)
    }

    /// Deno was started and the MCP handshake with the package failed.
    pub fn is_startup_failure(&self) -> bool {
        matches!(self, Self::Startup(_))
    }

    pub(crate) fn file_system(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: io::Error,
    ) -> Self {
        Self::FileSystem {
            operation,
            path: path.into(),
            source,
        }
    }
}
