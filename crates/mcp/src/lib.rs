//! The Model Context Protocol as the gateway speaks it, in both directions.
//!
//! This crate replaces `@modelcontextprotocol/sdk` 1.32 for the two roles the
//! TypeScript app used it in, with the behaviour of that SDK version:
//!
//! - [`client`]: the gateway as an MCP client of upstream servers, over
//!   Streamable HTTP ([`client::StreamableHttpClientTransport`]) and over the
//!   stdin and stdout of a child process ([`client::StdioClientTransport`]).
//! - [`server`]: the gateway as a stateless MCP server for AI clients, as one
//!   function from an HTTP request to an HTTP response
//!   ([`server::handle_request`]).
//!
//! It is a protocol library. It opens no socket (the HTTP function is handed
//! in by the caller) and knows nothing of the database or of the web framework.

pub mod client;
pub mod json;
mod media_type;
mod schemas;
pub mod server;
mod sse;
mod types;
mod zod;

pub use types::{
    DEFAULT_NEGOTIATED_PROTOCOL_VERSION, DEFAULT_REQUEST_TIMEOUT, Implementation, JSONRPC_VERSION,
    JsonRpcError, JsonRpcMessage, LATEST_PROTOCOL_VERSION, McpError, SUPPORTED_PROTOCOL_VERSIONS,
    Tool, error_code,
};
pub use zod::ValidationError;

#[cfg(test)]
mod zod_parity_tests;
