//! What the runner takes from the machine it runs on: the Node.js binary with
//! its npm, the server's own environment, and the launcher of the sandbox.
//! They are a value handed to [`crate::NodeRunner::with_runtime`], so that
//! each test builds its own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mymcps_core::Config;

use crate::error::NodeError;
use crate::paths::{is_executable_file, normalize, resolve};

const NODE_BINARY_NAME: &str = if cfg!(windows) { "node.exe" } else { "node" };
const PATH_DELIMITER: char = if cfg!(windows) { ';' } else { ':' };

/// The part of the server's own process the runner depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnvironment {
    /// `PATH`: searched for `node`, and handed to every npm MCP.
    pub path: Option<String>,
    /// Where Node.js is looked for when it is neither configured nor on the
    /// `PATH`: [`usual_node_locations`], unless a test wants none.
    pub known_node_locations: Vec<PathBuf>,
    /// The working directory of the server. Relative paths are resolved
    /// against it, and it is the application directory: `.env` and, unless
    /// `DATA_DIR` says otherwise, the database are in it.
    pub current_dir: PathBuf,
}

impl HostEnvironment {
    /// Read from the environment and the working directory of this process.
    pub fn from_process() -> Self {
        let current_dir = std::env::current_dir().unwrap_or_default();
        Self {
            path: std::env::var_os("PATH").map(|value| value.to_string_lossy().into_owned()),
            known_node_locations: usual_node_locations(),
            current_dir,
        }
    }

    /// [`locate_node_binary`] on this server: its `PATH`, then the usual
    /// install locations.
    pub fn locate_node_binary(&self, configured: Option<&str>) -> Result<PathBuf, NodeError> {
        locate_from(
            &self.current_dir,
            configured,
            self.path.as_deref(),
            &self.known_node_locations,
        )
    }
}

/// Usual install locations, tried after the server's `PATH`.
pub fn usual_node_locations() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin/node"),
        PathBuf::from("/usr/bin/node"),
        PathBuf::from("/opt/homebrew/bin/node"),
    ]
}

/// Absolute path of the Node.js binary: `configured` (NODE_BINARY) when it
/// points at one, else the first `node` on `search_path`, else one of
/// `known_locations`.
///
/// A bare `node` must never reach spawn. A bare command is looked up in the
/// `PATH` of the environment the child is given, and that environment carries
/// the variables operators set on the MCP.
pub fn locate_node_binary(
    configured: Option<&str>,
    search_path: Option<&str>,
    known_locations: &[PathBuf],
) -> Result<PathBuf, NodeError> {
    let current_dir = std::env::current_dir().unwrap_or_default();
    locate_from(&current_dir, configured, search_path, known_locations)
}

fn locate_from(
    current_dir: &Path,
    configured: Option<&str>,
    search_path: Option<&str>,
    known_locations: &[PathBuf],
) -> Result<PathBuf, NodeError> {
    let configured = configured
        .filter(|configured| !configured.is_empty())
        .map(|configured| resolve(current_dir, configured));
    let on_path = search_path
        .unwrap_or_default()
        .split(PATH_DELIMITER)
        .map(Path::new)
        // A relative PATH entry would be resolved against the working directory.
        .filter(|directory| directory.is_absolute())
        .map(|directory| normalize(&directory.join(NODE_BINARY_NAME)));

    configured
        .into_iter()
        .chain(on_path)
        .chain(known_locations.iter().cloned())
        // A relative path, were one handed in, would be resolved by the child.
        .find(|candidate| candidate.is_absolute() && is_executable_file(candidate))
        .ok_or(NodeError::BinaryNotFound)
}

/// `npm-cli.js` of the npm that came with the Node.js binary `node`, which
/// `node` runs to install a package.
///
/// An installation of Node.js keeps npm in `lib/node_modules/npm` beside its
/// `bin`, and links `bin/npm` to it. Both are looked at, from the binary as
/// it was found and from the file it links to, which is how the package
/// managers that keep several versions lay them out.
pub fn locate_npm_cli(node: &Path) -> Result<PathBuf, NodeError> {
    const NPM_CLI: &str = "npm-cli.js";
    let real_node = node.canonicalize().ok();
    let bin_directories: Vec<&Path> = [Some(node), real_node.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(Path::parent)
        .collect();

    let linked = bin_directories
        .iter()
        .filter_map(|bin| bin.join("npm").canonicalize().ok())
        .filter(|linked| linked.file_name().is_some_and(|name| name == NPM_CLI));
    let beside = bin_directories.iter().filter_map(|bin| {
        bin.parent()?
            .join("lib")
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join(NPM_CLI)
            .canonicalize()
            .ok()
    });
    linked
        .chain(beside)
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| NodeError::NpmNotFound(node.to_owned()))
}

/// How the processes of npm MCPs are held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sandbox {
    /// Each one is started through `launcher`, the `mymcps` binary, which
    /// applies the sandbox of [`mymcps_sandbox`] and starts nothing when
    /// the kernel cannot enforce it.
    Enforced { launcher: PathBuf },
    /// Each one is started as it is, with the rights of the server: for
    /// development on a system that is not Linux, and for the tests.
    Off,
}

type BinaryLookup = dyn Fn() -> Result<PathBuf, NodeError> + Send + Sync;

/// Where a [`crate::NodeRunner`] finds Node.js, npm, the server's
/// environment and the sandbox.
///
/// [`NodeRuntime::new`] is the server's own. Tests, which cannot rely on
/// Node.js being installed, replace the lookup of the binary (a stand-in
/// executable, or a lookup that fails) and the environment:
///
/// ```
/// use mymcps_node::{HostEnvironment, NodeError, NodeRuntime};
///
/// let runtime = NodeRuntime::with_host(None, HostEnvironment::from_process())
///     .binary(|| Err(NodeError::Other("Node.js is not started in this test".into())));
/// assert!(runtime.resolve_binary().is_err());
/// ```
#[derive(Clone)]
pub struct NodeRuntime {
    node_binary: Option<String>,
    host: HostEnvironment,
    lookup: Option<Arc<BinaryLookup>>,
    located: OnceLock<PathBuf>,
    npm_cli: Option<PathBuf>,
    sandbox: Sandbox,
}

impl NodeRuntime {
    /// The runtime of the server: `NODE_BINARY` and `NPM_SANDBOX` from its
    /// configuration, the rest from its process.
    pub fn new(config: &Config) -> Self {
        let runtime = Self::with_host(config.node_binary.clone(), HostEnvironment::from_process());
        if config.npm_sandbox {
            runtime
        } else {
            runtime.unsandboxed()
        }
    }

    /// `node_binary` is `NODE_BINARY`. The sandbox is enforced, with this
    /// program as its launcher: a program that is not `mymcps` chooses
    /// another with [`NodeRuntime::sandbox`].
    pub fn with_host(node_binary: Option<String>, host: HostEnvironment) -> Self {
        Self {
            node_binary,
            host,
            lookup: None,
            located: OnceLock::new(),
            npm_cli: None,
            sandbox: Sandbox::Enforced {
                // A program that cannot tell where it is starts nothing.
                launcher: std::env::current_exe().unwrap_or_default(),
            },
        }
    }

    /// Replace the lookup of the Node.js binary. It is called for every
    /// start and every install, before anything is spawned.
    pub fn binary<F>(mut self, lookup: F) -> Self
    where
        F: Fn() -> Result<PathBuf, NodeError> + Send + Sync + 'static,
    {
        self.lookup = Some(Arc::new(lookup));
        self
    }

    /// Use this executable as the Node.js binary.
    pub fn binary_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.binary(move || Ok(path.clone()))
    }

    /// Use this file as `npm-cli.js`, wherever the Node.js binary is.
    pub fn npm_cli(mut self, path: impl Into<PathBuf>) -> Self {
        self.npm_cli = Some(path.into());
        self
    }

    pub fn sandbox(mut self, sandbox: Sandbox) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Start npm MCPs without a sandbox: see [`Sandbox::Off`].
    pub fn unsandboxed(self) -> Self {
        self.sandbox(Sandbox::Off)
    }

    pub fn host(&self) -> &HostEnvironment {
        &self.host
    }

    pub fn sandbox_mode(&self) -> &Sandbox {
        &self.sandbox
    }

    /// Looked up once on the server's own PATH; a failed lookup is retried.
    pub fn resolve_binary(&self) -> Result<PathBuf, NodeError> {
        if let Some(lookup) = &self.lookup {
            return lookup();
        }
        if let Some(located) = self.located.get() {
            return Ok(located.clone());
        }
        let located = self.host.locate_node_binary(self.node_binary.as_deref())?;
        Ok(self.located.get_or_init(|| located).clone())
    }

    /// The `npm-cli.js` that installs packages for `node`.
    pub fn resolve_npm_cli(&self, node: &Path) -> Result<PathBuf, NodeError> {
        match &self.npm_cli {
            Some(npm_cli) => Ok(npm_cli.clone()),
            None => locate_npm_cli(node),
        }
    }
}

impl std::fmt::Debug for NodeRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeRuntime")
            .field("node_binary", &self.node_binary)
            .field("host", &self.host)
            .field("replaced_lookup", &self.lookup.is_some())
            .field("npm_cli", &self.npm_cli)
            .field("sandbox", &self.sandbox)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn host() -> HostEnvironment {
        HostEnvironment {
            path: None,
            known_node_locations: Vec::new(),
            current_dir: PathBuf::from("/srv/app"),
        }
    }

    #[test]
    fn reads_the_environment_of_the_process() {
        let host = HostEnvironment::from_process();
        assert_eq!(host.path, std::env::var("PATH").ok());
        assert!(host.current_dir.is_absolute());
        assert_eq!(host.known_node_locations, usual_node_locations());
    }

    #[test]
    fn tries_the_usual_install_locations_last() {
        assert_eq!(
            usual_node_locations(),
            [
                "/usr/local/bin/node",
                "/usr/bin/node",
                "/opt/homebrew/bin/node"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn asks_a_replaced_lookup_every_time_and_keeps_its_own_answer() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let starts = Arc::new(AtomicUsize::new(0));
        let counted = starts.clone();
        let runtime = NodeRuntime::with_host(None, host()).binary(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(NodeError::Other(
                "Node.js is not started in this test".into(),
            ))
        });
        for _ in 0..2 {
            assert_eq!(
                runtime.resolve_binary().unwrap_err().to_string(),
                "Node.js is not started in this test"
            );
        }
        assert_eq!(starts.load(Ordering::SeqCst), 2);

        let fixed = NodeRuntime::with_host(None, host()).binary_path("/opt/fake/node");
        assert_eq!(fixed.resolve_binary().unwrap(), Path::new("/opt/fake/node"));
    }

    #[test]
    fn enforces_the_sandbox_with_this_program_unless_told_otherwise() {
        let runtime = NodeRuntime::with_host(None, host());
        assert_eq!(
            runtime.sandbox_mode(),
            &Sandbox::Enforced {
                launcher: std::env::current_exe().unwrap()
            }
        );
        assert_eq!(runtime.unsandboxed().sandbox_mode(), &Sandbox::Off);

        let mut config = Config::for_tests("/srv/app/tmp");
        assert!(matches!(
            NodeRuntime::new(&config).sandbox_mode(),
            Sandbox::Enforced { .. }
        ));
        config.npm_sandbox = false;
        assert_eq!(NodeRuntime::new(&config).sandbox_mode(), &Sandbox::Off);
    }

    #[test]
    fn finds_npm_where_an_installation_of_node_keeps_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let npm_cli = root.join("opt/node/lib/node_modules/npm/bin/npm-cli.js");
        std::fs::create_dir_all(npm_cli.parent().unwrap()).unwrap();
        std::fs::write(&npm_cli, "").unwrap();
        let node = root.join("opt/node/bin/node");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "").unwrap();

        // Beside the binary.
        assert_eq!(locate_npm_cli(&node).unwrap(), npm_cli);

        // Beside what a link to the binary points at.
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        let linked_node = root.join("usr/bin/node");
        std::os::unix::fs::symlink(&node, &linked_node).unwrap();
        assert_eq!(locate_npm_cli(&linked_node).unwrap(), npm_cli);

        // Where `bin/npm` links to, when npm is kept elsewhere.
        let elsewhere = root.join("usr/share/nodejs/npm/bin/npm-cli.js");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "").unwrap();
        let packaged_node = root.join("packaged/bin/node");
        std::fs::create_dir_all(packaged_node.parent().unwrap()).unwrap();
        std::fs::write(&packaged_node, "").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("packaged/bin/npm")).unwrap();
        assert_eq!(locate_npm_cli(&packaged_node).unwrap(), elsewhere);

        // A `bin/npm` that is a program of its own says nothing.
        let alone = root.join("alone/bin/node");
        std::fs::create_dir_all(alone.parent().unwrap()).unwrap();
        std::fs::write(&alone, "").unwrap();
        std::fs::write(root.join("alone/bin/npm"), "#!/bin/sh\n").unwrap();
        let error = locate_npm_cli(&alone).unwrap_err();
        assert!(matches!(error, NodeError::NpmNotFound(_)));
        assert_eq!(
            error.to_string(),
            format!(
                "npm was not found beside the Node.js binary \"{}\". Set NODE_BINARY to the node binary of an installation that has npm.",
                alone.display()
            )
        );

        // The runtime of a test names it itself.
        let runtime = NodeRuntime::with_host(None, host()).npm_cli("/fake/npm-cli.js");
        assert_eq!(
            runtime.resolve_npm_cli(&alone).unwrap(),
            Path::new("/fake/npm-cli.js")
        );
    }
}
