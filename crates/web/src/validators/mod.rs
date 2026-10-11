//! Validation of what people submit. Every form of the app is checked by a
//! `mymcps-vine` validator here, never by hand in a handler.

pub mod access_token;
pub mod backup;
pub mod builtin_files;
pub mod cron;
pub mod mcp;
pub mod route_params;
pub mod session;
pub mod user;
