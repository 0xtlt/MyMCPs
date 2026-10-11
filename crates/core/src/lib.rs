//! Shared kernel of MyMCPs: configuration, the SQLite database and its
//! models, the encryption and hashing the AdonisJS app used (an existing
//! database must keep working), and the small services every other crate
//! needs.
//!
//! Nothing here is global. A [`Core`] is built once at startup and handed to
//! whoever needs the database, the encryption key or the configuration, so
//! that tests can each build their own.

pub mod backup;
pub mod client_ip;
pub mod config;
pub mod crypto;
#[macro_use]
pub mod db;
pub mod error;
pub mod limiter;
pub mod models;
pub mod public_url;
pub mod redaction;
pub mod secrets;
pub mod time;

mod context;

pub use config::{Config, Environment};
pub use context::Core;
#[cfg(feature = "test-util")]
pub use context::TestCore;
pub use db::Db;
pub use error::{Error, Result};
pub use time::Timestamp;

/// The version of the application, as the gateway reports it to MCP clients
/// and upstream servers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
