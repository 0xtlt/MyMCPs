//! What the protocol layer and a transport ask of each other.

use std::io;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::types::JsonRpcMessage;
use crate::zod::ValidationError;

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The callbacks a transport reports to (`onmessage`, `onerror` and `onclose`
/// in the SDK). They are called from the transport's tasks and must not block.
pub trait TransportHandler: Send + Sync + 'static {
    fn on_message(&self, message: JsonRpcMessage);

    /// A failure that did not, by itself, end the connection.
    fn on_error(&self, error: &TransportError);

    /// The connection is gone. May be called more than once.
    fn on_close(&self);
}

/// A way to exchange JSON-RPC messages with one MCP server.
///
/// A transport is a handle: clones share one connection, and the connection
/// does not outlive the last of them.
pub trait Transport: Send + Sync + 'static {
    fn start(
        &self,
        handler: Arc<dyn TransportHandler>,
    ) -> BoxFuture<'static, Result<(), TransportError>>;

    /// Send one message. What can be done at once (queueing a line for a
    /// child process) is done before this returns, so two sends keep their
    /// order even when their futures are polled later.
    fn send(&self, message: JsonRpcMessage) -> BoxFuture<'static, Result<(), TransportError>>;

    fn close(&self) -> BoxFuture<'static, ()>;

    /// The protocol version the server chose, for transports that have to repeat it.
    fn set_protocol_version(&self, _version: &str) {}
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The SDK's `StreamableHTTPError`. `code` is the HTTP status of the
    /// answer, or -1 when the answer was not an HTTP error (a content type
    /// that is neither JSON nor an event stream).
    #[error("Streamable HTTP error: {message}")]
    StreamableHttp { code: i32, message: String },

    /// The request could not be made or its answer could not be read. The
    /// text is the one of the injected HTTP function, unchanged.
    #[error("{0}")]
    Http(#[source] BoxError),

    /// An answer announced as JSON that is not JSON.
    #[error("{0}")]
    InvalidJson(#[source] serde_json::Error),

    /// JSON that is not a JSON-RPC message (a `ZodError` in the SDK).
    #[error("{0}")]
    InvalidMessage(#[source] ValidationError),

    /// The transport was closed while the request was under way.
    #[error("This operation was aborted")]
    Aborted,

    #[error("Not connected")]
    NotConnected,

    #[error("{0}")]
    AlreadyStarted(&'static str),

    /// The child process could not be started. Reads as Node's
    /// `spawn <command> ENOENT`.
    #[error("spawn {command} {code}")]
    Spawn {
        command: String,
        code: String,
        #[source]
        source: io::Error,
    },

    /// Reading from or writing to the child process failed.
    #[error("{0}")]
    Io(#[source] io::Error),

    /// The child process wrote more than the read buffer holds without ending a line.
    #[error("ReadBuffer exceeded maximum size of {0} bytes")]
    ReadBufferOverflow(usize),

    /// Something the SDK only reports to `onerror`: a stream that broke, a
    /// reconnection that was given up, an answer nobody was waiting for.
    #[error("{0}")]
    Notice(String),
}

impl TransportError {
    /// The HTTP status of the answer that caused this error, when there was one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::StreamableHttp { code, .. } => u16::try_from(*code).ok(),
            _ => None,
        }
    }

    /// The server answered 401: it wants credentials, or refused the ones it got.
    pub fn is_unauthorized(&self) -> bool {
        self.http_status() == Some(401)
    }
}
