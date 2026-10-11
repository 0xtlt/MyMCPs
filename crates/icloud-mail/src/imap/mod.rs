//! The IMAP client of the iCloud Mail MCP.
//!
//! The Node app used imapflow. No Rust crate reads what servers send the way
//! it does: the ones there are follow the grammar of RFC 3501, fail a whole
//! response over a header that is not UTF-8, and do not hand out the response
//! codes that say where an appended or moved message went. So this is a port
//! of the parts of imapflow the tools use, over a `tokio` stream: [`wire`]
//! writes commands and reads responses, [`parse`] and [`decode`] make
//! messages of them, and [`client`] is the session.

pub(crate) mod client;
pub(crate) mod decode;
pub(crate) mod parse;
mod special_use;
mod special_use_names;
pub(crate) mod wire;

pub(crate) use client::{Criterion, FetchQuery, ImapClient, ImapServer, Timeouts};

/// How the server refused a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    No,
    Bad,
}

/// Why something asked of the IMAP server did not happen. The variants are
/// the cases the Node app told apart on an imapflow error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ImapError {
    /// The sign-in did not go through (`authenticationFailed`).
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    /// The server limits the requests of the account (`ETHROTTLE`).
    #[error("Command failed: {0}")]
    Throttled(String),
    /// The server was not reached, or the connection was lost: the codes of
    /// `UNREACHABLE_CODES`.
    #[error("{0}")]
    Unreachable(String),
    /// The server answered NO or BAD. `text` is what it said (`responseText`).
    #[error("Command failed: {text}")]
    Refused {
        status: Status,
        text: String,
        /// The command was about a mailbox the account does not have.
        mailbox_missing: bool,
    },
    /// Anything else, which nobody planned for.
    #[error("{0}")]
    Other(String),
}
