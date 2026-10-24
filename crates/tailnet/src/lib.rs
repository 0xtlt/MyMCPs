//! One `tailscaled` per tailnet, so that MCP HTTP traffic can reach the devices
//! of a Tailscale (or Headscale) network.
//!
//! [`Tailnets`] keeps a daemon running for each tailnet id, in userspace
//! networking mode: no TUN device, no root, no change to the routes or the DNS
//! of the machine. Every daemon has a directory of its own, `tailnets/<id>/`
//! under the data directory, with mode 0700: its node key, and `ts.sock`, the
//! unix socket of its LocalAPI. The server talks to the daemon over that socket
//! only; the daemon never opens a port for it.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use mymcps_tailnet::{ConnectOptions, TailnetState, Tailnets, TailnetsConfig};
//! use tokio::io::{AsyncReadExt, AsyncWriteExt};
//!
//! let tailnets = Tailnets::new(TailnetsConfig {
//!     binary: "/usr/local/bin/tailscaled".into(),
//!     data_dir: "/app/tmp".into(),
//! });
//! tailnets.ensure_running(1).await?;
//! tailnets
//!     .connect(1, ConnectOptions {
//!         hostname: "mymcps".into(),
//!         control_url: None,
//!         auth_key: None,
//!     })
//!     .await?;
//! // `NeedsLogin { auth_url: Some(..) }`: show the link, then poll until `Running`.
//! let _state: TailnetState = tailnets.state(1).await;
//!
//! if let Some(tailnet) = tailnets.handle(1) {
//!     if let Some(address) = tailnet.route("nas", 8080).await? {
//!         let mut connection = tailnet.dial(address).await?;
//!         connection.write_all(b"GET / HTTP/1.0\r\n\r\n").await?;
//!         let mut answer = Vec::new();
//!         connection.read_to_end(&mut answer).await?;
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! # The daemon
//!
//! `tailscaled --tun=userspace-networking --statedir=<dir> --socket=<dir>/ts.sock
//! --port=0 --no-logs-no-support`, with an empty environment but `PATH` and
//! `HOME`. Its output goes to `tracing` at debug level, target
//! `mymcps_tailnet::tailscaled`. When it exits it is started again after a
//! wait that doubles from 500 ms to 30 s; it is stopped with SIGTERM (SIGKILL
//! after five seconds), and killed when the last [`Tailnets`] is dropped. A
//! node that joined keeps its state in the directory: after a restart the daemon
//! reconnects by itself, with the preferences `connect` set.
//!
//! # The LocalAPI
//!
//! The protocol is the one of Tailscale **v1.104.2** (the stable release of
//! 2026-10-21), read in `ipn/localapi/localapi.go`, `ipn/ipnserver/server.go`,
//! `ipn/ipnauth/ipnauth.go`, `ipn/backend.go`, `ipn/prefs.go`,
//! `ipn/ipnstate/ipnstate.go` and `client/local/local.go`: see the module
//! `localapi` for the details. A newer daemon is fine as long as those
//! endpoints and field names stay: that is what the `#[ignore]`d smoke test
//! (`tests/smoke.rs`, run by `tests/smoke.sh`) checks against the real binary.
//!
//! # What is routed
//!
//! A host is a peer when the status of the daemon says so, never by a lookup of
//! the machine's DNS: see [`TailnetHandle::route`]. A host that is not a peer
//! is not dialled through the tailnet.
//!
//! The crate is unix only: it is empty elsewhere.
#![cfg(unix)]

mod error;
mod handle;
mod localapi;
mod route;
mod supervisor;
mod tailnets;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use error::Error;
pub use handle::TailnetHandle;
pub use tailnets::{ConnectOptions, TailnetState, Tailnets, TailnetsConfig};
