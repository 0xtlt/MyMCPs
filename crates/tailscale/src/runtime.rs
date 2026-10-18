//! The runtime the server holds: every node of the instance, and the SOCKS
//! front they share.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use url::Url;

use crate::error::TailscaleError;
use crate::launcher::{LaunchSpec, Launcher, TailscaledLauncher};
use crate::node::{Node, Timing};
use crate::socks::SocksFront;
use crate::types::{Availability, NodeConfig, NodeState, NodeStatus, TailscaleOptions};

/// `tailscaled --version` prints at once; a binary that takes longer is not one.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// `sun_path` holds 108 bytes on Linux and 104 on macOS, with the final NUL.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// The nodes of the instance by id. The SOCKS front reads it for every connection.
#[derive(Default)]
pub(crate) struct Registry {
    nodes: RwLock<HashMap<i64, Arc<Node>>>,
}

impl Registry {
    pub fn get(&self, id: i64) -> Option<Arc<Node>> {
        self.read().get(&id).cloned()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<i64, Arc<Node>>> {
        self.nodes.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<i64, Arc<Node>>> {
        self.nodes.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The Tailscale nodes of this server.
///
/// Cloning gives another handle to the same runtime. [`Tailscale::start`] never
/// fails: when nodes cannot run, [`Tailscale::availability`] says why and every
/// call that would start one answers with that reason.
#[derive(Clone)]
pub struct Tailscale {
    inner: Arc<Inner>,
}

struct Inner {
    availability: Availability,
    root: PathBuf,
    max_nodes: usize,
    launcher: Option<Arc<dyn Launcher>>,
    timing: Timing,
    registry: Arc<Registry>,
    front: Option<SocksFront>,
    /// One lock per node id: `ensure`, `reconnect` and `remove` of the same node run one at a time.
    locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// Why a node that is not in the registry could not be started, as the sentence to show. A node that never
    /// registered has no status of its own, so this is all the page can tell about it.
    start_failures: Mutex<HashMap<i64, String>>,
    closed: AtomicBool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Handles of a runtime nobody shut down: the processes die with their supervisors.
        if let Some(front) = &self.front {
            front.stop();
        }
        for node in self.registry.read().values() {
            node.cancel();
        }
    }
}

impl Tailscale {
    /// Checks the binary, creates the root directory and starts the SOCKS front.
    pub async fn start(options: TailscaleOptions) -> Self {
        if options.max_nodes == 0 {
            return Self::unavailable(Availability::Disabled, &options);
        }
        let unavailable =
            |reason: String| Self::unavailable(Availability::Unavailable { reason }, &options);

        let binary = match find_binary(options.binary.as_deref()) {
            Ok(binary) => binary,
            Err(reason) => return unavailable(reason),
        };
        let version = match probe_version(&binary).await {
            Ok(version) => version,
            Err(reason) => return unavailable(reason),
        };
        let root = match std::path::absolute(&options.root) {
            Ok(root) => root,
            Err(error) => {
                return unavailable(format!(
                    "the Tailscale directory {} cannot be resolved: {error}",
                    options.root.display()
                ));
            }
        };
        if let Err(error) = create_private_dir(&root) {
            return unavailable(format!(
                "the Tailscale directory {} cannot be created: {error}",
                root.display()
            ));
        }
        let launcher: Arc<dyn Launcher> = Arc::new(TailscaledLauncher::new(binary));
        match Self::build(
            root,
            options.max_nodes,
            Availability::Available { version },
            Some(launcher),
            Timing::default(),
        )
        .await
        {
            Ok(tailscale) => tailscale,
            Err(error) => unavailable(format!(
                "the Tailscale SOCKS front cannot listen on the loopback interface: {error}"
            )),
        }
    }

    /// A runtime with nothing running, for tests of other crates.
    pub fn disabled() -> Self {
        Self::unavailable(
            Availability::Disabled,
            &TailscaleOptions {
                binary: None,
                root: PathBuf::new(),
                max_nodes: 0,
            },
        )
    }

    fn unavailable(availability: Availability, options: &TailscaleOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                availability,
                root: options.root.clone(),
                max_nodes: options.max_nodes,
                launcher: None,
                timing: Timing::default(),
                registry: Arc::default(),
                front: None,
                locks: Mutex::default(),
                start_failures: Mutex::default(),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) async fn build(
        root: PathBuf,
        max_nodes: usize,
        availability: Availability,
        launcher: Option<Arc<dyn Launcher>>,
        timing: Timing,
    ) -> std::io::Result<Self> {
        let registry = Arc::new(Registry::default());
        let front = SocksFront::bind(registry.clone()).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                availability,
                root,
                max_nodes,
                launcher,
                timing,
                registry,
                front: Some(front),
                locks: Mutex::default(),
                start_failures: Mutex::default(),
                closed: AtomicBool::new(false),
            }),
        })
    }

    pub fn availability(&self) -> &Availability {
        &self.inner.availability
    }

    /// The loopback port of the SOCKS front, when there is one.
    pub fn socks_port(&self) -> Option<u16> {
        self.inner.front.as_ref().map(SocksFront::port)
    }

    /// Idempotent. Starts the node if it is not running, applies a changed hostname, logs in with the key when the
    /// node needs a login and a key is given, otherwise asks Tailscale for a login link. Refuses past `max_nodes`.
    ///
    /// It returns once the node is started, not once it is connected: follow [`Tailscale::status`]. When the node
    /// cannot be started at all (socket path too long, no place left, directory not usable), the error is returned
    /// and also kept: [`Tailscale::status`] answers [`NodeState::Failed`] with its sentence until a later call
    /// succeeds or the node is removed.
    pub async fn ensure(&self, config: NodeConfig) -> Result<(), TailscaleError> {
        self.ensure_node(config, false).await
    }

    /// Restart the process and log in again (key, or a fresh login link).
    pub async fn reconnect(&self, config: NodeConfig) -> Result<(), TailscaleError> {
        self.ensure_node(config, true).await
    }

    async fn ensure_node(&self, config: NodeConfig, restart: bool) -> Result<(), TailscaleError> {
        let launcher = self.launcher()?;
        let _turn = self.lock(config.id).await;
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(TailscaleError::ShuttingDown);
        }

        let id = config.id;
        if let Some(node) = self.inner.registry.get(id) {
            node.update(config);
            if restart {
                node.reconnect();
            }
            self.set_start_failure(id, None);
            return Ok(());
        }

        match self.spawn_node(config, launcher) {
            Ok(()) => {
                self.set_start_failure(id, None);
                Ok(())
            }
            Err(error) => {
                self.set_start_failure(id, Some(error.to_string()));
                Err(error)
            }
        }
    }

    /// Creates the directories of a node that is not in the registry and starts it.
    fn spawn_node(
        &self,
        config: NodeConfig,
        launcher: Arc<dyn Launcher>,
    ) -> Result<(), TailscaleError> {
        let spec = LaunchSpec::new(config.id, self.node_dir(config.id));
        check_socket_path(&spec.socket)?;
        // The count and the insert happen under one lock, so two nodes cannot both take the last place.
        let mut nodes = self.inner.registry.write();
        if nodes.len() >= self.inner.max_nodes {
            return Err(TailscaleError::TooManyNodes {
                max: self.inner.max_nodes,
            });
        }
        for dir in [&spec.dir, &spec.state_dir, &spec.logs_dir] {
            create_private_dir(dir).map_err(|error| TailscaleError::Io {
                path: dir.clone(),
                message: error.to_string(),
            })?;
        }
        let node = Node::spawn(config, spec, launcher, self.inner.timing);
        nodes.insert(node.id, node);
        Ok(())
    }

    /// Cached status, refreshed in the background (every ~2 s while not connected, ~30 s when connected).
    ///
    /// A node that [`Tailscale::ensure`] could not start is `Failed` with the reason, not `Stopped`.
    pub fn status(&self, id: i64) -> NodeStatus {
        if let Some(node) = self.inner.registry.get(id) {
            return node.status();
        }
        NodeStatus::new(
            self.start_failure(id)
                .map_or(NodeState::Stopped, |error| NodeState::Failed { error }),
        )
    }

    /// Log out (best effort), stop the process, delete `<root>/<id>`.
    pub async fn remove(&self, id: i64) -> Result<(), TailscaleError> {
        let _turn = self.lock(id).await;
        self.set_start_failure(id, None);
        let node = self.inner.registry.write().remove(&id);
        if let Some(node) = node {
            if !matches!(node.status().state, NodeState::Stopped) {
                // Best effort: the node may never have logged in, or its process may be gone.
                let api = node.api().with_timeout(self.inner.timing.logout);
                if let Err(error) = api.logout().await {
                    tracing::debug!(proxy = id, "Tailscale logout did not complete: {error}");
                }
            }
            node.stop().await;
        }
        self.delete_state(id)
    }

    /// Ensure every listed node; stop and delete the state of nodes (running or on disk) that are not listed.
    ///
    /// The state of a node that is only on disk is deleted without a logout: that needs the process.
    pub async fn reconcile(&self, configs: Vec<NodeConfig>) {
        if self.launcher().is_err() {
            // Proxies are off or unavailable: the state on disk waits for the day they are back.
            return;
        }
        let wanted: HashSet<i64> = configs.iter().map(|config| config.id).collect();
        let mut stale: HashSet<i64> = self
            .inner
            .registry
            .read()
            .keys()
            .copied()
            .filter(|id| !wanted.contains(id))
            .collect();
        stale.extend(
            self.ids_on_disk()
                .into_iter()
                .filter(|id| !wanted.contains(id)),
        );
        // Nodes that never started leave no trace but their failure.
        stale.extend(
            self.start_failure_ids()
                .into_iter()
                .filter(|id| !wanted.contains(id)),
        );
        // First, so that the room they took is free for the nodes below.
        for id in stale {
            if let Err(error) = self.remove(id).await {
                tracing::warn!(
                    proxy = id,
                    "A Tailscale node that is no longer used was not removed: {error}"
                );
            }
        }
        for config in configs {
            let id = config.id;
            if let Err(error) = self.ensure(config).await {
                tracing::warn!(proxy = id, "Tailscale node was not started: {error}");
            }
        }
    }

    /// `socks5h://<id>:<secret>@127.0.0.1:<port>` when the node is connected; otherwise the sentence to show, e.g.
    /// "it needs a Tailscale login" / "it is waiting for approval in the Tailscale admin console" / "it is starting",
    /// or the reason the node could not be started.
    pub fn proxy_url(&self, id: i64) -> Result<Url, String> {
        match &self.inner.availability {
            Availability::Disabled => return Err("Tailscale proxies are off on this server".into()),
            Availability::Unavailable { reason } => {
                return Err(format!("Tailscale proxies are not available: {reason}"));
            }
            Availability::Available { .. } => {}
        }
        let Some(node) = self.inner.registry.get(id) else {
            return Err(self
                .start_failure(id)
                .unwrap_or_else(|| "it is not running".to_owned()));
        };
        let port = self
            .socks_port()
            .ok_or_else(|| "it is not running".to_owned())?;
        match node.status().state {
            NodeState::Connected => {}
            NodeState::Starting => return Err("it is starting".into()),
            NodeState::NeedsLogin { .. } => return Err("it needs a Tailscale login".into()),
            NodeState::NeedsApproval => {
                return Err("it is waiting for approval in the Tailscale admin console".into());
            }
            NodeState::Failed { error } => return Err(error),
            NodeState::Stopped => return Err("it is not running".into()),
        }
        // The secret is made of letters, digits, `-` and `_`: it needs no escaping.
        Url::parse(&format!(
            "socks5h://{id}:{}@127.0.0.1:{port}",
            node.secret()
        ))
        .map_err(|_| "its address cannot be built".to_owned())
    }

    /// Stop every process (SIGTERM, 2 s grace, kill). State stays on disk.
    pub async fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        if let Some(front) = &self.inner.front {
            front.stop();
        }
        let nodes: Vec<Arc<Node>> = self.inner.registry.read().values().cloned().collect();
        // Cancel them all first, so the graces run side by side.
        for node in &nodes {
            node.cancel();
        }
        for node in nodes {
            node.stop().await;
        }
    }

    fn launcher(&self) -> Result<Arc<dyn Launcher>, TailscaleError> {
        match &self.inner.availability {
            Availability::Disabled => Err(TailscaleError::Disabled),
            Availability::Unavailable { reason } => {
                Err(TailscaleError::Unavailable(reason.clone()))
            }
            Availability::Available { .. } => self
                .inner
                .launcher
                .clone()
                .ok_or_else(|| TailscaleError::Unavailable("no tailscaled binary".into())),
        }
    }

    async fn lock(&self, id: i64) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .inner
            .locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(id)
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    fn start_failure(&self, id: i64) -> Option<String> {
        self.start_failures().get(&id).cloned()
    }

    fn start_failure_ids(&self) -> Vec<i64> {
        self.start_failures().keys().copied().collect()
    }

    fn set_start_failure(&self, id: i64, error: Option<String>) {
        let mut failures = self.start_failures();
        match error {
            Some(error) => failures.insert(id, error),
            None => failures.remove(&id),
        };
    }

    fn start_failures(&self) -> std::sync::MutexGuard<'_, HashMap<i64, String>> {
        self.inner
            .start_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn node_dir(&self, id: i64) -> PathBuf {
        self.inner.root.join(id.to_string())
    }

    fn delete_state(&self, id: i64) -> Result<(), TailscaleError> {
        if self.inner.root.as_os_str().is_empty() {
            return Ok(());
        }
        let dir = self.node_dir(id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TailscaleError::Io {
                path: dir,
                message: error.to_string(),
            }),
        }
    }

    /// The directories of the root that are named like a node id. Anything else is not ours.
    fn ids_on_disk(&self) -> Vec<i64> {
        let Ok(entries) = std::fs::read_dir(&self.inner.root) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i64>().ok())
            .collect()
    }
}

/// The binary the options name, or the first `tailscaled` on `PATH`.
fn find_binary(configured: Option<&Path>) -> Result<PathBuf, String> {
    const NOT_FOUND: &str = "tailscaled was not found: install Tailscale or set TAILSCALED_BINARY";
    match configured {
        Some(path) if path.components().count() > 1 => {
            if is_executable(path) {
                Ok(path.to_path_buf())
            } else {
                Err(format!(
                    "tailscaled was not found at {}: install Tailscale or fix TAILSCALED_BINARY",
                    path.display()
                ))
            }
        }
        named => {
            let name =
                named.map_or_else(|| "tailscaled".into(), |name| name.as_os_str().to_owned());
            let paths = std::env::var_os("PATH").unwrap_or_default();
            std::env::split_paths(&paths)
                .map(|directory| directory.join(&name))
                .find(|candidate| is_executable(candidate))
                .ok_or_else(|| NOT_FOUND.to_owned())
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Runs `tailscaled --version` and returns its first line, e.g. `1.104.1`.
async fn probe_version(binary: &Path) -> Result<String, String> {
    let run = Command::new(binary)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match timeout(VERSION_TIMEOUT, run).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("tailscaled could not be run: {error}")),
        Err(_) => return Err("tailscaled --version did not answer in 10 seconds".into()),
    };
    if !output.status.success() {
        return Err(format!("tailscaled --version failed ({})", output.status));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "tailscaled --version printed nothing".to_owned())
}

fn check_socket_path(socket: &Path) -> Result<(), TailscaleError> {
    let length = socket.as_os_str().len();
    if length > MAX_SOCKET_PATH_BYTES {
        return Err(TailscaleError::SocketPathTooLong {
            path: socket.to_path_buf(),
            length,
            max: MAX_SOCKET_PATH_BYTES,
        });
    }
    Ok(())
}

/// Creates the directory and its parents if missing, readable by this user only.
/// The mode is set on every call, also on a directory that was already there.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
