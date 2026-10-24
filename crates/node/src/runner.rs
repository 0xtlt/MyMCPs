use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mymcps_core::Core;
use mymcps_core::models::Mcp;
use mymcps_mcp::client::{
    Client, StdioClientTransport, StdioServerParameters, StdioStderr, Transport,
    get_default_environment,
};
use mymcps_mcp::{Implementation, Tool};
use mymcps_sandbox::{LAUNCH_COMMAND, Policy};
use serde_json::{Map, Value};

use crate::environment_policy::is_reserved_environment_name;
use crate::error::NodeError;
use crate::exec::{ExecOptions, exec_file};
use crate::package::{InstalledPackage, PackageSpec, inspect, make_current, read_current};
use crate::paths::{remove_recursively, resolve};
use crate::runtime::{NodeRuntime, Sandbox};
use crate::startup::{StartupStderr, StartupStderrCapture, create_startup_error};

const NPM_INSTALL_TIMEOUT: Duration = Duration::from_millis(120_000);
const NPM_INSTALL_MAX_BUFFER: usize = 1024 * 1024;
const SANDBOX_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Where `npm` keeps its download cache, its logs and its temporary files
/// during an install, inside the install. Deleted when `npm` is done.
const NPM_SCRATCH: &str = ".npm";

/// The largest id JavaScript holds exactly, `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// An npm MCP that started and answered the MCP handshake.
pub struct ConnectedNodeUpstream {
    pub client: Client,
    pub transport: StdioClientTransport,
}

impl ConnectedNodeUpstream {
    /// End the process: its stdin is closed, and it is signalled when it
    /// does not exit by itself.
    pub async fn close(&self) {
        self.client.close().await;
        self.transport.close().await;
    }
}

/// What the kernel does for the processes of npm MCPs on this server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxStatus {
    /// What is enforced, as the launcher reports it.
    Enforced(String),
    /// Why the launcher starts nothing: no npm MCP runs.
    Unavailable(String),
    /// The server was told to run npm MCPs without a sandbox.
    Off,
}

type InstallLocks = Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>;

/// Runs the npm MCPs of one server. Clones share its configuration.
#[derive(Clone, Debug)]
pub struct NodeRunner {
    core: Arc<Core>,
    runtime: Arc<NodeRuntime>,
    /// One install at a time for an MCP: requests that arrive together for a
    /// package that is not installed yet wait for the first to install it.
    installing: Arc<InstallLocks>,
}

impl NodeRunner {
    /// The runner of the server: Node.js is looked up from `NODE_BINARY`,
    /// the server's `PATH` and the usual install locations, and the sandbox
    /// is enforced unless `NPM_SANDBOX` turns it off.
    pub fn new(core: Arc<Core>) -> Self {
        let runtime = NodeRuntime::new(&core.config);
        Self::with_runtime(core, runtime)
    }

    /// A runner with another Node.js binary, another environment or another
    /// sandbox than the server's own: see [`NodeRuntime`].
    pub fn with_runtime(core: Arc<Core>, runtime: NodeRuntime) -> Self {
        Self {
            core,
            runtime: Arc::new(runtime),
            installing: Arc::default(),
        }
    }

    pub fn runtime(&self) -> &NodeRuntime {
        &self.runtime
    }

    /// The data directory, which the configuration may name relative to the
    /// working directory.
    fn data_dir(&self) -> PathBuf {
        resolve(&self.runtime.host().current_dir, &self.core.config.data_dir)
    }

    /// The directory an npm MCP runs in and may write: its working
    /// directory, its `HOME` and its `TMPDIR`.
    pub fn sandbox_root_for(&self, mcp_id: i64) -> PathBuf {
        self.data_dir()
            .join("mcp-sandboxes")
            .join(mcp_id.to_string())
    }

    /// The directory the installs of an npm MCP's package are kept in: see
    /// [`crate::package`]. The package reads the install in use and writes
    /// none.
    pub fn packages_root_for(&self, mcp_id: i64) -> PathBuf {
        self.data_dir()
            .join("mcp-packages")
            .join(mcp_id.to_string())
    }

    /// A sandboxed process reads the directories of the system (`/usr`...)
    /// whatever its policy, and Landlock cannot take a directory back out of
    /// one it gave. An application installed there, say in `/usr/local`,
    /// would hand its database, its key and its `.env` to every package:
    /// nothing is started then.
    fn assert_nothing_exposed(&self) -> Result<(), NodeError> {
        for directory in [self.data_dir(), self.runtime.host().current_dir.clone()] {
            if let Some(system_path) = mymcps_sandbox::exposed_by_the_system(&directory) {
                return Err(NodeError::ExposedDirectory {
                    directory,
                    system_path,
                });
            }
        }
        Ok(())
    }

    /// The command line that runs `program` held by `policy`: through the
    /// launcher of the sandbox, unless the sandbox is off.
    fn command(
        &self,
        policy: Policy,
        program: &Path,
        arguments: Vec<OsString>,
    ) -> Result<(PathBuf, Vec<OsString>), NodeError> {
        match self.runtime.sandbox_mode() {
            Sandbox::Off => Ok((program.to_owned(), arguments)),
            Sandbox::Enforced { launcher } => {
                self.assert_nothing_exposed()?;
                let mut launch = vec![OsString::from(LAUNCH_COMMAND)];
                launch.extend(
                    policy
                        .launch_arguments(program, arguments)
                        .map_err(|error| NodeError::Other(error.to_string()))?,
                );
                Ok((launcher.clone(), launch))
            }
        }
    }

    /// What the kernel enforces for npm MCPs here, asked of the launcher
    /// itself: it applies an empty policy and starts nothing.
    pub async fn sandbox_status(&self) -> SandboxStatus {
        let Sandbox::Enforced { launcher } = self.runtime.sandbox_mode() else {
            return SandboxStatus::Off;
        };
        let arguments = match Policy::default().probe_arguments() {
            Ok(arguments) => arguments,
            Err(error) => return SandboxStatus::Unavailable(error.to_string()),
        };
        let probe = tokio::process::Command::new(launcher)
            .arg(LAUNCH_COMMAND)
            .args(arguments)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let line = |output: &[u8]| {
            let text = String::from_utf8_lossy(output);
            let text = text.trim();
            text.strip_prefix("sandbox: ").unwrap_or(text).to_owned()
        };
        match tokio::time::timeout(SANDBOX_PROBE_TIMEOUT, probe).await {
            Ok(Ok(output)) if output.status.success() => {
                SandboxStatus::Enforced(line(&output.stdout))
            }
            Ok(Ok(output)) => SandboxStatus::Unavailable(line(&output.stderr)),
            Ok(Err(error)) => SandboxStatus::Unavailable(format!(
                "the launcher \"{}\" cannot be started: {error}",
                launcher.display()
            )),
            Err(_) => SandboxStatus::Unavailable("the launcher did not answer".to_owned()),
        }
    }

    /// Arguments of `node` for an npm MCP: the executable of its package,
    /// then the arguments saved for the MCP.
    pub fn build_args(&self, mcp: &Mcp, installed: &InstalledPackage) -> Vec<OsString> {
        let mut args = vec![installed.entry.clone().into_os_string()];
        args.extend(mcp.npm_args_list().into_iter().map(OsString::from));
        args
    }

    /// Arguments of `node` to install `spec` in `install_dir` with the npm
    /// at `npm_cli`. Each option is on the command line, where it wins over
    /// the variables of the MCP, which `npm` reads its configuration from
    /// (`NPM_CONFIG_REGISTRY`...):
    ///
    /// - nothing of the package or of what it depends on is run
    ///   (`--ignore-scripts`), and no link to an executable is made
    ///   (`--no-bin-links`): Node is given the file itself;
    /// - `npm` reads no configuration file and keeps its cache inside the
    ///   install, the one place it can write.
    pub fn build_install_args(
        npm_cli: &Path,
        install_dir: &Path,
        spec: &PackageSpec,
    ) -> Vec<OsString> {
        let scratch = install_dir.join(NPM_SCRATCH);
        let mut args: Vec<OsString> = vec![npm_cli.into(), "install".into()];
        for (option, path) in [
            ("--prefix", install_dir.to_owned()),
            ("--cache", scratch.join("cache")),
            ("--userconfig", scratch.join("npmrc")),
            ("--globalconfig", scratch.join("npmrc-global")),
        ] {
            args.extend([option.into(), path.into_os_string()]);
        }
        args.extend(
            [
                "--global=false",
                "--ignore-scripts",
                "--no-bin-links",
                "--no-package-lock",
                "--omit=dev",
                "--no-audit",
                "--no-fund",
                "--no-update-notifier",
                "--no-progress",
                "--loglevel=error",
                // What follows is a package, whatever it looks like.
                "--",
            ]
            .map(OsString::from),
        );
        args.push(spec.argument().into());
        args
    }

    /// Environment of a process of the MCP, in the order it is put together:
    /// the variables saved for the MCP, then what the gateway sets. `home`
    /// is the directory the process may write: its `HOME` and its `TMPDIR`.
    /// Reserved names are dropped here again: rows saved before the
    /// validator refused them may still hold some.
    ///
    /// The process is started with these on top of what an MCP client hands
    /// every stdio server from its own environment (`LOGNAME`, `SHELL`, `TERM`
    /// and `USER`; its `HOME` and `PATH` are replaced here), and nothing else.
    pub fn build_environment(
        &self,
        mcp: &Mcp,
        home: &Path,
    ) -> Result<Vec<(String, String)>, NodeError> {
        let mut environment: Vec<(String, String)> = mcp
            .npm_environment(&self.core.encryption)?
            .into_iter()
            .filter(|(name, _)| !is_reserved_environment_name(name))
            .collect();

        let home = home.display().to_string();
        // Last, so nothing stored can replace what the gateway sets.
        environment.extend([
            (
                "PATH".to_owned(),
                self.runtime.host().path.clone().unwrap_or_default(),
            ),
            ("HOME".to_owned(), home.clone()),
            ("TMPDIR".to_owned(), home),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        Ok(environment)
    }

    /// The error of an npm MCP that did not start, as administrators read it.
    /// `error` is the message of the failure; `stderr` is what the process
    /// wrote, when it ran. The secrets of the MCP are redacted from both.
    pub fn create_startup_error(
        &self,
        mcp: &Mcp,
        error: &str,
        stderr: Option<&StartupStderr>,
    ) -> NodeError {
        create_startup_error(&self.core.encryption, mcp, error, stderr)
    }

    /// The version of the package that is installed for this MCP, and so
    /// runs: none before its first install, and none once the MCP names
    /// another package or another version than the one installed.
    pub fn installed_npm_package_version(&self, mcp: &Mcp) -> Option<String> {
        let spec = PackageSpec::of(mcp).ok()?;
        read_current(&self.packages_root_for(mcp.id), &spec).map(|installed| installed.version)
    }

    /// Delete what an npm MCP wrote to its sandbox, and the installs of its
    /// package. Called when the MCP is deleted or runs another package, so
    /// nothing is inherited by different code or by a later MCP that reuses
    /// the id.
    pub async fn remove_sandbox(&self, mcp_id: i64) -> Result<(), NodeError> {
        if !(1..=MAX_SAFE_INTEGER).contains(&mcp_id) {
            return Ok(());
        }
        for directory in [
            self.sandbox_root_for(mcp_id),
            self.packages_root_for(mcp_id),
        ] {
            remove_recursively(&directory)
                .await
                .map_err(|error| NodeError::file_system("rm", directory, error))?;
        }
        Ok(())
    }

    async fn create_sandbox(&self, mcp: &Mcp) -> Result<PathBuf, NodeError> {
        let sandbox_dir = self.sandbox_root_for(mcp.id);
        tokio::fs::create_dir_all(&sandbox_dir)
            .await
            .map_err(|error| NodeError::file_system("mkdir", &sandbox_dir, error))?;
        Ok(sandbox_dir)
    }

    fn install_lock(&self, mcp_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.installing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(mcp_id)
            .or_default()
            .clone()
    }

    /// Install the package of an npm MCP again, at the version the MCP asks
    /// for, without changing the MCP row: for an MCP that tracks `latest`,
    /// this is its update. Requests under way finish on the install they
    /// started with; the next ones run the new one.
    pub async fn install_npm_package(&self, mcp: &Mcp) -> Result<InstalledPackage, NodeError> {
        let spec = PackageSpec::of(mcp)?;
        let lock = self.install_lock(mcp.id);
        let _installing = lock.lock().await;
        self.install(mcp, &spec).await
    }

    /// The install in use for the MCP, made first when there is none.
    async fn installed(&self, mcp: &Mcp) -> Result<InstalledPackage, NodeError> {
        let spec = PackageSpec::of(mcp)?;
        let lock = self.install_lock(mcp.id);
        let _installing = lock.lock().await;
        match read_current(&self.packages_root_for(mcp.id), &spec) {
            Some(installed) => Ok(installed),
            None => self.install(mcp, &spec).await,
        }
    }

    async fn install(&self, mcp: &Mcp, spec: &PackageSpec) -> Result<InstalledPackage, NodeError> {
        let node = self.runtime.resolve_binary()?;
        let npm_cli = self.runtime.resolve_npm_cli(&node)?;
        // The whole of npm, which is what its `bin/npm-cli.js` loads.
        let npm_root = npm_cli
            .parent()
            .and_then(Path::parent)
            .unwrap_or(&npm_cli)
            .to_owned();

        let packages_dir = self.packages_root_for(mcp.id);
        let install = uuid::Uuid::new_v4().simple().to_string();
        let install_dir = packages_dir.join(&install);
        let scratch = install_dir.join(NPM_SCRATCH);
        tokio::fs::create_dir_all(&scratch)
            .await
            .map_err(|error| NodeError::file_system("mkdir", &scratch, error))?;

        let installed = async {
            let mut environment: HashMap<String, String> = get_default_environment();
            environment.extend(self.build_environment(mcp, &scratch)?);
            let policy = Policy::default()
                .read(&node)
                .read(&npm_root)
                .write(&install_dir);
            let (program, arguments) = self.command(
                policy,
                &node,
                Self::build_install_args(&npm_cli, &install_dir, spec),
            )?;
            exec_file(
                &program,
                &arguments,
                ExecOptions {
                    name: &format!("npm install {}", spec.argument()),
                    cwd: &install_dir,
                    env: &environment,
                    timeout: NPM_INSTALL_TIMEOUT,
                    max_buffer: NPM_INSTALL_MAX_BUFFER,
                },
            )
            .await
            .map_err(|error| NodeError::install(&spec.name, error.detail()))?;
            let _ = remove_recursively(&scratch).await;

            let installed = inspect(&install_dir, spec)?;
            make_current(&packages_dir, &install, spec, &installed).await?;
            // As every reader of `current.json` names it.
            Ok(InstalledPackage {
                entry: install_dir.join(
                    installed
                        .entry
                        .strip_prefix(&installed.directory)
                        .unwrap_or(&installed.entry),
                ),
                directory: install_dir.clone(),
                version: installed.version,
            })
        }
        .await;
        if installed.is_err() {
            let _ = remove_recursively(&install_dir).await;
        }
        installed
    }

    /// Start the package of an npm MCP in its sandbox and connect to it. The
    /// package is installed first when it is not yet.
    pub async fn connect(&self, mcp: &Mcp) -> Result<ConnectedNodeUpstream, NodeError> {
        let node = self.runtime.resolve_binary()?;
        let installed = self.installed(mcp).await?;
        let sandbox_dir = self.create_sandbox(mcp).await?;

        let policy = Policy::default()
            .read(&node)
            .read(&installed.directory)
            .write(&sandbox_dir);
        let (program, arguments) = self.command(policy, &node, self.build_args(mcp, &installed))?;
        let transport = StdioClientTransport::new(
            StdioServerParameters::new(program)
                .args(arguments)
                .cwd(&sandbox_dir)
                .stderr(StdioStderr::Pipe)
                .env(self.build_environment(mcp, &sandbox_dir)?),
        );
        let stderr = StartupStderrCapture::start(transport.stderr());

        let client = Client::new(Implementation::new("mymcps-gateway", mymcps_core::VERSION));

        if let Err(error) = client.connect(transport.clone()).await {
            let stderr = stderr.read().await;
            // The client is closing the transport: stdin first, then signals.
            // The process is killed with the last handle, so one is kept until then.
            tokio::spawn(async move { transport.close().await });
            return Err(self.create_startup_error(mcp, &error.to_string(), Some(&stderr)));
        }
        stderr.discard();

        Ok(ConnectedNodeUpstream { client, transport })
    }

    /// The tools of an npm MCP: start it, ask, end it.
    pub async fn list_tools(&self, mcp: &Mcp) -> Result<Vec<Tool>, NodeError> {
        let connected = self.connect(mcp).await?;
        let result = connected.client.list_tools().await;
        connected.close().await;

        Ok(result?
            .tools
            .iter()
            .map(|tool| {
                let mut listed = Map::new();
                listed.insert("name".to_owned(), Value::from(tool.name()));
                if let Some(description) = tool.description() {
                    listed.insert("description".to_owned(), Value::from(description));
                }
                listed.insert("inputSchema".to_owned(), tool.input_schema().clone());
                Tool::from(listed)
            })
            .collect())
    }

    /// Call one tool of an npm MCP: start it, call, end it. The result is the
    /// one of the MCP client's `call_tool`: a result with `isError: true` is
    /// a result, not an error. The call waits [`Mcp::tool_timeout`] for its
    /// answer, then the process is ended.
    pub async fn call_tool(
        &self,
        mcp: &Mcp,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<Value, NodeError> {
        let connected = self.connect(mcp).await?;
        let result = connected
            .client
            .call_tool_within(name, Some(arguments), mcp.tool_timeout())
            .await;
        connected.close().await;
        Ok(result?)
    }
}
