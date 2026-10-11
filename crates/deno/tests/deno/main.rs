//! The Node app's unit tests of the Deno runner, and what they left to its
//! functional suites. The stand-in for Deno is a shell script, so none of
//! this runs on Windows.
//!
//! `real_deno` starts the real Deno against a local npm registry; its tests
//! skip, with a message, on a machine without Deno.
#![cfg(unix)]

mod deno_cache_version;
mod hardening_upstream_deno;
mod mcp_environment;
mod real_deno;
mod security;
mod support;
