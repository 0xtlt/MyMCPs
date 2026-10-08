//! npm MCP servers, run as child processes of the gateway inside a Deno
//! sandbox.
//!
//! An npm MCP is a package (`npm:<package>@<version>`) that speaks MCP on its
//! stdin and stdout. [`DenoRunner`] starts it with `deno run` for one request
//! (list its tools, or call one) and ends it:
//!
//! - it may read its sandbox directory, `<data dir>/mcp-sandboxes/<mcp id>`,
//!   and the Deno cache, and write its sandbox only; the database, `.env` and
//!   the sandboxes of other MCPs are out of reach;
//! - its environment is the variables saved for the MCP, minus the names that
//!   address the sandbox rather than the package ([`environment_policy`]),
//!   then `PATH`, `HOME`, `TMPDIR`, `DENO_DIR` and `NO_COLOR` as the gateway
//!   sets them. Nothing else of the server's environment reaches it, apart
//!   from `LOGNAME`, `SHELL`, `TERM` and `USER`;
//! - the Deno binary is always an absolute path, found before anything of the
//!   MCP is looked at ([`locate_deno_binary`]).
//!
//! Network and environment access stay open: upstream packages are trusted
//! software, not tenants.
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_core::Core;
//! use mymcps_core::models::Mcp;
//! use mymcps_deno::{DenoError, DenoRunner};
//!
//! # async fn example(core: Arc<Core>, mcp: Mcp) -> Result<(), DenoError> {
//! let runner = DenoRunner::new(core);
//! for tool in runner.list_tools(&mcp).await? {
//!     println!("{}", tool.name());
//! }
//! match runner.call_tool(&mcp, "search", serde_json::Map::new()).await {
//!     Ok(result) => println!("{result}"),
//!     // The package ran and the request failed.
//!     Err(error) if error.mcp_code().is_some() => println!("{error}"),
//!     // `Failed to start Deno npm MCP "...". ...`, with what Deno wrote.
//!     Err(error) if error.is_startup_failure() => println!("{error}"),
//!     Err(error) => return Err(error),
//! }
//! # Ok(())
//! # }
//! ```

mod cache;
pub mod environment_policy;
mod error;
mod exec;
mod paths;
mod runner;
mod runtime;
mod startup;
mod text;

pub use environment_policy::{is_reserved_environment_name, reserved_environment_name_reason};
pub use error::DenoError;
pub use runner::{ConnectedDenoUpstream, DenoRunner, build_deno_cache_reload_args};
pub use runtime::{DenoRuntime, HostEnvironment, locate_deno_binary, usual_deno_locations};
pub use startup::{STARTUP_STDERR_LIMIT_BYTES, StartupStderr};
