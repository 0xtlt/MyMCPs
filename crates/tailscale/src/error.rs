use std::path::PathBuf;

/// Why a Tailscale node could not be started, changed or removed. Each message
/// is a sentence an admin can read as it is.
#[derive(Debug, thiserror::Error)]
pub enum TailscaleError {
    /// `TAILSCALE_MAX_PROXIES` is 0.
    #[error("Tailscale proxies are off: TAILSCALE_MAX_PROXIES is 0")]
    Disabled,
    /// The binary is missing or does not run, or the state directory cannot be used.
    #[error("Tailscale proxies are not available: {0}")]
    Unavailable(String),
    /// The instance already runs `TAILSCALE_MAX_PROXIES` nodes.
    #[error("This server runs at most {max} Tailscale proxies: raise TAILSCALE_MAX_PROXIES")]
    TooManyNodes { max: usize },
    /// A Unix socket path holds about a hundred bytes, and `tailscaled` cannot
    /// listen on a longer one.
    #[error(
        "The Tailscale socket path is too long ({length} bytes, at most {max}): {}. Use a shorter data directory",
        path.display()
    )]
    SocketPathTooLong {
        path: PathBuf,
        length: usize,
        max: usize,
    },
    /// The runtime was shut down, so no node starts any more.
    #[error("Tailscale proxies are shutting down")]
    ShuttingDown,
    /// `tailscaled` could not be started.
    #[error("tailscaled could not be started: {0}")]
    Process(String),
    /// `tailscaled` answered with an error, or did not answer.
    #[error("Tailscale did not accept the request: {0}")]
    LocalApi(String),
    /// A node's directory could not be created or deleted.
    #[error("Cannot use the Tailscale state in {}: {message}", path.display())]
    Io { path: PathBuf, message: String },
}
