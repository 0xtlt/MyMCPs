//! The HTTP server of MyMCPs: the pages, the OAuth endpoints, the file links
//! and the `/mcp` gateway endpoint.

pub mod app;
pub mod assets;
pub mod auth;
pub mod client_ip;
pub mod cookies;
pub mod csrf;
pub mod error;
pub mod errors;
pub mod forms;
pub mod guards;
pub mod input;
pub mod install_config;
pub mod mcp_templates;
pub mod redirect;
pub mod respond;
pub mod routes;
pub mod security;
pub mod session;
pub mod state;
#[cfg(feature = "test-util")]
pub mod testing;
pub mod upstream_session;
pub mod validators;
pub mod views;

pub use state::AppState;
