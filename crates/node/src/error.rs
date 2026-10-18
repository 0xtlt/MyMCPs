use std::io;
use std::path::PathBuf;

use mymcps_core::secrets::EnvironmentError;
use mymcps_mcp::McpError;
use mymcps_mcp::client::{ClientError, RequestTimedOut};

use crate::paths::file_system_message;

/// Why an npm MCP could not be reached. Callers redact the message
/// (`sanitize_mcp_diagnostic`) and show it to administrators.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("npm MCP is missing a package name")]
    MissingPackage,

    #[error("\"{0}\" is not the name of a package of the npm registry")]
    InvalidPackage(String),

    #[error("\"{0}\" is not a version, a range of versions or a tag of an npm package")]
    InvalidVersion(String),

    #[error(
        "Node.js was not found. Install Node.js, or set NODE_BINARY to the absolute path of the node binary."
    )]
    BinaryNotFound,

    #[error(
        "npm was not found beside the Node.js binary \"{}\". Set NODE_BINARY to the node binary of an installation that has npm.",
        .0.display()
    )]
    NpmNotFound(PathBuf),

    #[error(
        "\"{}\" is inside \"{system_path}\", which every npm MCP can read in its sandbox. Keep the application and its data directory (DATA_DIR) out of the directories of the system.",
        .directory.display()
    )]
    ExposedDirectory {
        directory: PathBuf,
        system_path: &'static str,
    },

    /// The saved environment of the MCP cannot be read.
    #[error(transparent)]
    Environment(#[from] EnvironmentError),

    /// Node or the package did not get through the MCP handshake. The message
    /// is redacted already, and says what the process wrote to stderr.
    #[error("{0}")]
    Startup(String),

    /// The package started and the request then failed: the server answered
    /// with a JSON-RPC error, did not answer in time, closed the connection,
    /// or answered something that is not a result.
    #[error(transparent)]
    Client(#[from] ClientError),

    /// `detail` is the start of what `npm install` wrote to stderr, as
    /// written, or what is wrong with the package it installed.
    #[error("Failed to install npm package \"{package}\". {detail}")]
    Install { package: String, detail: String },

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

impl NodeError {
    /// The error of the protocol, when the started package answered with one
    /// or the request timed out or lost its connection.
    pub fn mcp_error(&self) -> Option<&McpError> {
        match self {
            Self::Client(ClientError::Mcp(error)) => Some(error),
            _ => None,
        }
    }

    /// The JSON-RPC error code of [`NodeError::mcp_error`].
    pub fn mcp_code(&self) -> Option<i64> {
        self.mcp_error().map(|error| error.code)
    }

    /// The request got no answer in time and was cancelled: what it was and
    /// how long it waited.
    pub fn timed_out(&self) -> Option<&RequestTimedOut> {
        match self {
            Self::Client(error) => error.timed_out(),
            _ => None,
        }
    }

    /// Node was started and the MCP handshake with the package failed.
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

    pub(crate) fn install(package: &str, detail: impl Into<String>) -> Self {
        Self::Install {
            package: package.to_owned(),
            detail: detail.into(),
        }
    }
}
