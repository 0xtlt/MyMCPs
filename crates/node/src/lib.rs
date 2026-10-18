//! npm MCP servers, run as child processes of the gateway by Node.js, inside
//! the sandbox of [`mymcps_sandbox`].
//!
//! An npm MCP is a package of the npm registry that speaks MCP on its stdin
//! and stdout. [`NodeRunner`] installs it once with `npm`, then starts it
//! with `node` for one request (list its tools, or call one) and ends it:
//!
//! - the package reads the files it was installed with,
//!   `<data dir>/mcp-packages/<mcp id>`, and reads and writes its sandbox
//!   directory, `<data dir>/mcp-sandboxes/<mcp id>`. The kernel refuses it
//!   every other file that is not part of the system: the database, `.env`,
//!   the key, the directories of other MCPs. It starts no other process,
//!   signals none, and holds a limited amount of memory;
//! - `npm` installs it under the same sandbox, in a directory that is the
//!   only place it can write, and runs none of the package's scripts;
//! - its environment is the variables saved for the MCP, minus the names
//!   that address Node rather than the package ([`environment_policy`]),
//!   then `PATH`, `HOME`, `TMPDIR` and `NO_COLOR` as the gateway sets them.
//!   Nothing else of the server's environment reaches it, apart from
//!   `LOGNAME`, `SHELL`, `TERM` and `USER`;
//! - the Node.js binary is always an absolute path, found before anything
//!   of the MCP is looked at ([`locate_node_binary`]).
//!
//! The network stays open to a package. The sandbox needs Linux, and the
//! runner starts nothing where it cannot be enforced, unless the server is
//! told to do without it ([`Sandbox::Off`]).
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use mymcps_core::Core;
//! use mymcps_core::models::Mcp;
//! use mymcps_node::{NodeError, NodeRunner};
//!
//! # async fn example(core: Arc<Core>, mcp: Mcp) -> Result<(), NodeError> {
//! let runner = NodeRunner::new(core);
//! for tool in runner.list_tools(&mcp).await? {
//!     println!("{}", tool.name());
//! }
//! match runner.call_tool(&mcp, "search", serde_json::Map::new()).await {
//!     Ok(result) => println!("{result}"),
//!     // The package ran and the request failed.
//!     Err(error) if error.mcp_code().is_some() => println!("{error}"),
//!     // `Failed to start npm MCP "...". ...`, with what the process wrote.
//!     Err(error) if error.is_startup_failure() => println!("{error}"),
//!     Err(error) => return Err(error),
//! }
//! # Ok(())
//! # }
//! ```

pub mod environment_policy;
mod error;
mod exec;
pub mod package;
mod paths;
mod runner;
mod runtime;
mod startup;
mod text;

pub use environment_policy::{is_reserved_environment_name, reserved_environment_name_reason};
pub use error::NodeError;
pub use package::{InstalledPackage, PackageSpec, is_registry_package_name, is_version_request};
pub use runner::{ConnectedNodeUpstream, NodeRunner, SandboxStatus, client_options};
pub use runtime::{
    HostEnvironment, NodeRuntime, Sandbox, locate_node_binary, locate_npm_cli, usual_node_locations,
};
pub use startup::{STARTUP_STDERR_LIMIT_BYTES, StartupStderr};
