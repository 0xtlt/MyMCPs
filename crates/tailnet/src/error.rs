use std::io;

/// Why an operation on a tailnet failed.
///
/// No variant carries an auth key: a message tailscaled sent back is scrubbed
/// of the key of the request before it is kept.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The options of `connect` were refused by their validator.
    #[error("invalid tailnet options: {0}")]
    InvalidOptions(String),
    /// The tailnet has no supervised `tailscaled`: call `ensure_running` first.
    #[error("tailnet {0} is not running")]
    NotRunning(i64),
    /// The path of the LocalAPI socket does not fit in `sockaddr_un`.
    #[error("the socket path {0} is too long for a unix socket; use a shorter data directory")]
    SocketPathTooLong(String),
    /// tailscaled did not answer on its socket in time.
    #[error("tailscaled did not answer on its socket: {0}")]
    Unreachable(String),
    /// tailscaled refused a LocalAPI request.
    #[error("tailscaled answered {status}: {message}")]
    LocalApi { status: u16, message: String },
    /// A LocalAPI answer that is not what tailscaled sends.
    #[error("unexpected LocalAPI answer: {0}")]
    Protocol(String),
    /// A file or the socket.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        let kind = match &error {
            Error::Io(inner) => inner.kind(),
            Error::NotRunning(_) | Error::Unreachable(_) => io::ErrorKind::NotConnected,
            Error::InvalidOptions(_) => io::ErrorKind::InvalidInput,
            Error::LocalApi { status: 403, .. } => io::ErrorKind::PermissionDenied,
            Error::LocalApi { .. } | Error::Protocol(_) | Error::SocketPathTooLong(_) => {
                io::ErrorKind::Other
            }
        };
        io::Error::new(kind, error)
    }
}

impl Error {
    /// The same error with every occurrence of `secret` removed from its text.
    pub(crate) fn scrubbed(self, secret: &str) -> Self {
        if secret.is_empty() {
            return self;
        }
        match self {
            Error::LocalApi { status, message } => Error::LocalApi {
                status,
                message: message.replace(secret, "***"),
            },
            Error::Protocol(message) => Error::Protocol(message.replace(secret, "***")),
            Error::Unreachable(message) => Error::Unreachable(message.replace(secret, "***")),
            other => other,
        }
    }
}
