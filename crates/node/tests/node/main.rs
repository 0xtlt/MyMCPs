//! The runner of npm MCPs. The stand-in for Node.js is a shell script, so
//! none of this runs on Windows.
//!
//! `real_node` installs a package from a local npm registry with the real
//! npm and starts it with the real Node.js; its tests skip, with a message,
//! on a machine without them. Neither file starts a sandbox: what the
//! kernel enforces is tested where the launcher is, in `crates/sandbox` and
//! `crates/cli`.
#![cfg(unix)]

mod real_node;
mod registry;
mod runner;
mod support;
