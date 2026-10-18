//! Tailscale proxies: each proxy of a team is a Tailscale node that MyMCPs
//! runs itself, and an HTTP MCP can send its requests through one of them to
//! reach a machine of that tailnet.
//!
//! A node is one `tailscaled` child process in userspace networking mode, so it
//! needs no privilege and no TUN device. It has its own state directory and its
//! own LocalAPI socket under `<DATA_DIR>/tailscale/<proxy id>/`. The process
//! opens no listening port of its own: the traffic of an MCP goes
//!
//! ```text
//! reqwest ── socks5h://<id>:<secret>@127.0.0.1:<port> ──> SOCKS front (this process)
//!         ── POST /localapi/v0/dial over the node's Unix socket ──> tailscaled ──> tailnet
//! ```
//!
//! The front is the one place that decides what may be reached.
//! Its password is random per node and never leaves memory.
//!
//! [`Tailscale::start`] checks the binary and starts the front;
//! [`Tailscale::reconcile`] brings the nodes in line with the proxies of the
//! database; [`Tailscale::proxy_url`] gives the address a fetcher uses.
//!
//! The crate talks to `tailscaled` over a Unix socket, so nodes run on Unix systems only.

mod error;
mod launcher;
mod local_api;
mod node;
mod runtime;
mod socks;
mod types;

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod socks_tests;
#[cfg(test)]
mod testing;

pub use error::TailscaleError;
pub use runtime::Tailscale;
pub use types::{Availability, NodeConfig, NodeState, NodeStatus, TailscaleOptions};
