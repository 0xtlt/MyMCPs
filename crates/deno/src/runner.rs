use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mymcps_core::Core;
use mymcps_core::models::Mcp;
use mymcps_mcp::client::{
    Client, StdioClientTransport, StdioServerParameters, StdioStderr, Transport,
    get_default_environment,
};
use mymcps_mcp::{Implementation, Tool};
use serde_json::{Map, Value};

use crate::cache::read_cached_npm_package_version;
use crate::environment_policy::is_reserved_environment_name;
use crate::error::DenoError;
use crate::exec::{ExecOptions, exec_file};
use crate::paths::{is_within, remove_recursively, resolve};
use crate::runtime::DenoRuntime;
use crate::startup::{StartupStderr, StartupStderrCapture, create_startup_error};
use crate::text::js_trim;

const DENO_CACHE_RELOAD_TIMEOUT: Duration = Duration::from_millis(120_000);
const DENO_CACHE_RELOAD_MAX_BUFFER: usize = 1024 * 1024;
/// With a package.json in the working directory or above it (the Node app had
/// one, and a package can write one into its sandbox), Deno would default to
/// `nodeModules: "manual"` and refuse `npm:` specifier entrypoints.
/// `none` keeps packages in `$DENO_DIR` instead of creating a local node_modules.
const DENO_NODE_MODULES_DIR: &str = "--node-modules-dir=none";
const DENO_NO_LOCK: &str = "--no-lock";

/// The largest id JavaScript holds exactly, `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// `deno cache --reload` args for an npm package at `@latest`.
/// `--node-modules-dir=none` and `--no-lock` keep Deno from adopting or writing
/// project files next to a package.json in the working directory.
pub fn build_deno_cache_reload_args(npm_package: &str) -> Result<Vec<String>, DenoError> {
    let package = js_trim(npm_package);
    if package.is_empty() {
        return Err(DenoError::MissingPackage);
    }
    Ok(vec![
        "cache".to_owned(),
        "--reload".to_owned(),
        "--quiet".to_owned(),
        DENO_NODE_MODULES_DIR.to_owned(),
        DENO_NO_LOCK.to_owned(),
        format!("npm:{package}@latest"),
    ])
}

/// An npm MCP that started and answered the MCP handshake.
pub struct ConnectedDenoUpstream {
    pub client: Client,
    pub transport: StdioClientTransport,
}

impl ConnectedDenoUpstream {
    /// End the process: its stdin is closed, and it is signalled when it
    /// does not exit by itself.
    pub async fn close(&self) {
        self.client.close().await;
        self.transport.close().await;
    }
}

/// Runs the npm MCPs of one server. Clones share its configuration.
#[derive(Clone, Debug)]
pub struct DenoRunner {
    core: Arc<Core>,
    runtime: Arc<DenoRuntime>,
}

impl DenoRunner {
    /// The runner of the server: Deno is looked up from `DENO_PATH`, the
    /// server's `PATH` and the usual install locations.
    pub fn new(core: Arc<Core>) -> Self {
        let runtime = DenoRuntime::new(&core.config);
        Self::with_runtime(core, runtime)
    }

    /// A runner with another Deno binary or another environment than the
    /// server's own: see [`DenoRuntime`].
    pub fn with_runtime(core: Arc<Core>, runtime: DenoRuntime) -> Self {
        Self {
            core,
            runtime: Arc::new(runtime),
        }
    }

    pub fn runtime(&self) -> &DenoRuntime {
        &self.runtime
    }

    /// The Deno cache directory of every npm MCP: see
    /// [`crate::HostEnvironment::resolve_deno_dir`].
    pub fn resolve_deno_dir(&self) -> PathBuf {
        self.runtime.host().resolve_deno_dir()
    }

    /// The data directory, which the configuration may name relative to the
    /// working directory.
    fn data_dir(&self) -> PathBuf {
        resolve(&self.runtime.host().current_dir, &self.core.config.data_dir)
    }

    fn sandboxes_root(&self) -> PathBuf {
        self.data_dir().join("mcp-sandboxes")
    }

    /// The directory an npm MCP runs in and may write: its working
    /// directory, its `HOME` and its `TMPDIR`.
    pub fn sandbox_root_for(&self, mcp_id: i64) -> PathBuf {
        self.sandboxes_root().join(mcp_id.to_string())
    }

    /// Packages read the whole Deno cache directory and write their sandbox. A
    /// cache directory holding the app would expose the database and `.env`, and
    /// one inside a sandbox could be rewritten by the package it serves.
    fn sandbox_safe_deno_dir(&self) -> Result<PathBuf, DenoError> {
        let deno_dir = self.resolve_deno_dir();
        let application = &self.runtime.host().current_dir;
        if is_within(application, &deno_dir)
            // The database is not below the application when DATA_DIR points elsewhere.
            || is_within(&self.data_dir(), &deno_dir)
            || is_within(&deno_dir, &self.sandboxes_root())
        {
            return Err(DenoError::UnsafeCacheDirectory(deno_dir));
        }
        Ok(deno_dir)
    }

    /// Build Deno permission flags for an npm MCP subprocess.
    ///
    /// Filesystem is deny-by-default outside `sandbox_dir` and the Deno npm cache
    /// (no database / `.env` / app files). The cache must be readable because
    /// Node packages often `readFileSync` their own packaged assets (for example
    /// `@shopify/dev-mcp`). Network and env remain allowed because many MCP packages
    /// need outbound HTTP and process env.
    /// `homedir` sys access is required by Node packages that call `os.homedir()` at import time
    /// (for example `@shopify/dev-mcp` via `env-paths`); HOME/TMPDIR still point at `sandbox_dir`.
    /// Treat upstream packages as trusted software, not a full multi-tenant isolation boundary.
    pub fn build_args(&self, mcp: &Mcp, sandbox_dir: &Path) -> Result<Vec<String>, DenoError> {
        let package = mcp
            .npm_package
            .as_deref()
            .filter(|package| !package.is_empty())
            .ok_or(DenoError::MissingPackage)?;

        let version = Some(js_trim(mcp.npm_version.as_deref().unwrap_or_default()))
            .filter(|version| !version.is_empty())
            .unwrap_or("latest");
        let deno_dir = self.sandbox_safe_deno_dir()?;
        let sandbox_dir = sandbox_dir.display();

        let mut args = vec![
            "run".to_owned(),
            "--quiet".to_owned(),
            DENO_NODE_MODULES_DIR.to_owned(),
            DENO_NO_LOCK.to_owned(),
            format!("--allow-read={sandbox_dir},{}", deno_dir.display()),
            format!("--allow-write={sandbox_dir}"),
            "--allow-net".to_owned(),
            "--allow-env".to_owned(),
            "--allow-sys=homedir".to_owned(),
            "--no-prompt".to_owned(),
            format!("npm:{package}@{version}"),
        ];
        args.extend(mcp.npm_args_list());
        Ok(args)
    }

    /// Environment of the `deno` process, in the order it is put together. The
    /// variables of the MCP reach Deno and its loader, not only the package, so
    /// reserved names are dropped here again: rows saved before the validator
    /// refused them may still hold some.
    ///
    /// The process is started with these on top of what an MCP client hands
    /// every stdio server from its own environment (`LOGNAME`, `SHELL`, `TERM`
    /// and `USER`; its `HOME` and `PATH` are replaced here), and nothing else.
    pub fn build_environment(
        &self,
        mcp: &Mcp,
        sandbox_dir: &Path,
    ) -> Result<Vec<(String, String)>, DenoError> {
        let mut environment: Vec<(String, String)> = mcp
            .npm_environment(&self.core.encryption)?
            .into_iter()
            .filter(|(name, _)| !is_reserved_environment_name(name))
            .collect();

        let sandbox_dir = sandbox_dir.display().to_string();
        let deno_dir = self.sandbox_safe_deno_dir()?;
        // Last, so nothing stored can replace what the sandbox depends on.
        environment.extend([
            (
                "PATH".to_owned(),
                self.runtime.host().path.clone().unwrap_or_default(),
            ),
            ("HOME".to_owned(), sandbox_dir.clone()),
            ("TMPDIR".to_owned(), sandbox_dir),
            ("DENO_DIR".to_owned(), deno_dir.display().to_string()),
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
    ) -> DenoError {
        create_startup_error(&self.core.encryption, mcp, error, stderr)
    }

    /// Semver currently present in the Deno npm cache for this package.
    /// For `latest`, uses the cached `dist-tags.latest` when that version folder exists.
    /// Pinned versions are returned only when that exact folder is cached.
    pub fn cached_npm_package_version(
        &self,
        npm_package: &str,
        npm_version: Option<&str>,
    ) -> Option<String> {
        read_cached_npm_package_version(&self.resolve_deno_dir(), npm_package, npm_version)
    }

    /// Delete what an npm MCP wrote to its sandbox. Called when the MCP is deleted
    /// or runs another package, so nothing is inherited by different code or by a
    /// later MCP that reuses the id.
    pub async fn remove_sandbox(&self, mcp_id: i64) -> Result<(), DenoError> {
        if !(1..=MAX_SAFE_INTEGER).contains(&mcp_id) {
            return Ok(());
        }
        let sandbox_dir = self.sandbox_root_for(mcp_id);
        remove_recursively(&sandbox_dir)
            .await
            .map_err(|error| DenoError::file_system("rm", sandbox_dir, error))
    }

    async fn create_sandbox(&self, mcp: &Mcp) -> Result<PathBuf, DenoError> {
        let sandbox_dir = self.sandbox_root_for(mcp.id);
        tokio::fs::create_dir_all(&sandbox_dir)
            .await
            .map_err(|error| DenoError::file_system("mkdir", &sandbox_dir, error))?;
        Ok(sandbox_dir)
    }

    /// Reload the Deno npm cache for an MCP's package at `@latest` without changing
    /// the MCP row. It runs in the directory and environment the MCP itself starts
    /// with, so it refreshes the cache and the registry that process reads.
    pub async fn reload_npm_package_cache(&self, mcp: &Mcp) -> Result<(), DenoError> {
        let package = js_trim(mcp.npm_package.as_deref().unwrap_or_default());
        if package.is_empty() {
            return Err(DenoError::MissingPackage);
        }

        let deno = self.runtime.resolve_binary()?;
        let sandbox_dir = self.create_sandbox(mcp).await?;
        let mut environment: HashMap<String, String> = get_default_environment();
        environment.extend(self.build_environment(mcp, &sandbox_dir)?);

        exec_file(
            &deno,
            &build_deno_cache_reload_args(package)?,
            ExecOptions {
                cwd: &sandbox_dir,
                env: &environment,
                timeout: DENO_CACHE_RELOAD_TIMEOUT,
                max_buffer: DENO_CACHE_RELOAD_MAX_BUFFER,
            },
        )
        .await
        .map_err(|error| DenoError::CacheReload {
            package: package.to_owned(),
            detail: error.detail(),
        })
    }

    /// Start the package of an npm MCP in its sandbox and connect to it.
    pub async fn connect(&self, mcp: &Mcp) -> Result<ConnectedDenoUpstream, DenoError> {
        let sandbox_dir = self.create_sandbox(mcp).await?;
        remove_legacy_sandbox_caches(&sandbox_dir).await;

        let deno = self.runtime.resolve_binary()?;
        let args = self.build_args(mcp, &sandbox_dir)?;
        let transport = StdioClientTransport::new(
            StdioServerParameters::new(deno)
                .args(args)
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

        Ok(ConnectedDenoUpstream { client, transport })
    }

    /// The tools of an npm MCP: start it, ask, end it.
    pub async fn list_tools(&self, mcp: &Mcp) -> Result<Vec<Tool>, DenoError> {
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
    /// a result, not an error.
    pub async fn call_tool(
        &self,
        mcp: &Mcp,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<Value, DenoError> {
        let connected = self.connect(mcp).await?;
        let result = connected.client.call_tool(name, Some(arguments)).await;
        connected.close().await;
        Ok(result?)
    }
}

/// Until the cache directory was passed explicitly, Deno derived it from the
/// child's HOME and cached each package inside its own sandbox, where the
/// package could rewrite its code. Those copies are no longer read.
async fn remove_legacy_sandbox_caches(sandbox_dir: &Path) {
    let linux = sandbox_dir.join(".cache").join("deno");
    let macos = sandbox_dir.join("Library").join("Caches").join("deno");
    let _ = tokio::join!(remove_recursively(&linux), remove_recursively(&macos));
}
