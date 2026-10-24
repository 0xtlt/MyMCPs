use std::collections::{HashMap, HashSet};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use mymcps_vine as vine;
use serde_json::{Value, json};
use tokio::time::Instant;
use url::Url;

use crate::error::Error;
use crate::handle::TailnetHandle;
use crate::localapi::{LocalClient, Status};
use crate::supervisor::{Backoff, Process, Supervisor, SupervisorConfig};

/// The longest path `sockaddr_un` holds on the systems this runs on: 104 bytes
/// on macOS and the BSDs, 108 on Linux, one of them for the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;
/// How long `connect` waits for a fresh daemon to open its socket.
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Where the daemons live.
#[derive(Debug, Clone)]
pub struct TailnetsConfig {
    /// The `tailscaled` executable.
    pub binary: PathBuf,
    /// Holds `tailnets/<id>/` for every tailnet: its state, and its socket. A
    /// relative path is made absolute, against the working directory of the
    /// server, when the registry is built: a daemon runs in the directory of
    /// its tailnet and would read a relative `--socket` from there.
    pub data_dir: PathBuf,
}

/// What `connect` asks of a tailnet.
#[derive(Clone)]
pub struct ConnectOptions {
    /// The name of this node in the tailnet.
    pub hostname: String,
    /// The coordination server, for a Headscale: `None` is Tailscale's own.
    pub control_url: Option<Url>,
    /// A pre-authorised key, to join without a browser. Never logged, never
    /// stored: it is sent to the daemon once.
    pub auth_key: Option<String>,
}

impl fmt::Debug for ConnectOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectOptions")
            .field("hostname", &self.hostname)
            .field("control_url", &self.control_url)
            .field("auth_key", &self.auth_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Where a tailnet stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetState {
    /// The daemon is starting, or has no answer yet.
    Starting,
    /// The node must be logged in: open `auth_url` in a browser. The URL is
    /// there a moment after `connect` without a key.
    NeedsLogin { auth_url: Option<Url> },
    /// Logged in, waiting for an admin to approve the machine.
    NeedsMachineAuth,
    /// Connected.
    Running {
        /// `tail1234.ts.net`: the domain of the nodes of the tailnet.
        magic_dns_suffix: String,
        /// The name of this node, `mymcps.tail1234.ts.net`.
        dns_name: String,
        /// The Tailscale addresses of this node.
        addresses: Vec<std::net::IpAddr>,
    },
    /// Logged in, and switched off.
    Stopped,
    /// There is no daemon: it cannot be started, or keeps exiting.
    Failed(String),
}

/// A set of tailnets, one `tailscaled` each. Cheap to clone: clones share the
/// daemons, which are killed when the last clone is dropped.
#[derive(Clone)]
pub struct Tailnets {
    inner: Arc<Inner>,
}

struct Inner {
    config: TailnetsConfig,
    backoff: Backoff,
    status_ttl: Duration,
    entries: Mutex<HashMap<i64, Entry>>,
    /// One turn at a time for the lifecycle of a tailnet (start, stop,
    /// remove): `stop` takes the daemon out of `entries` before it is gone, and
    /// a start that came in meanwhile would run a second one.
    turns: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// The tailnets that were removed. A request that began before the removal
    /// (the list of a page, say) must not start one of them again.
    removed: Mutex<HashSet<i64>>,
}

struct Entry {
    supervisor: Supervisor,
    client: LocalClient,
}

impl Tailnets {
    pub fn new(config: TailnetsConfig) -> Self {
        Self::with_settings(config, Backoff::default(), crate::localapi::STATUS_TTL)
    }

    pub(crate) fn with_settings(
        mut config: TailnetsConfig,
        backoff: Backoff,
        status_ttl: Duration,
    ) -> Self {
        config.data_dir = absolute(&config.data_dir);
        Tailnets {
            inner: Arc::new(Inner {
                config,
                backoff,
                status_ttl,
                entries: Mutex::new(HashMap::new()),
                turns: Mutex::new(HashMap::new()),
                removed: Mutex::new(HashSet::new()),
            }),
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Entry>> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The turn of tailnet `id`: held while its daemon is started, stopped or
    /// removed, so that these never overlap.
    async fn turn(&self, id: i64) -> tokio::sync::OwnedMutexGuard<()> {
        let turn = Arc::clone(
            self.inner
                .turns
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(id)
                .or_default(),
        );
        turn.lock_owned().await
    }

    fn removed(&self) -> std::sync::MutexGuard<'_, HashSet<i64>> {
        self.inner
            .removed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Allows tailnet `id` to be started again after `remove`. A removed id is
    /// never started, so that a request that was already under way cannot
    /// bring a deleted tailnet back; call this when the id is given to a tailnet
    /// again (a restored database, a new row), before `ensure_running`.
    pub fn forget_removal(&self, id: i64) {
        self.removed().remove(&id);
    }

    fn tailnet_dir(&self, id: i64) -> PathBuf {
        self.inner
            .config
            .data_dir
            .join("tailnets")
            .join(id.to_string())
    }

    /// Starts the daemon of tailnet `id` if it is not running. It returns once
    /// the process is spawned, not once it answers: `state` tells. It is kept
    /// alive until `stop`, `remove`, or the drop of the last `Tailnets`.
    ///
    /// It waits for a `stop` or a `remove` of the tailnet that is under way,
    /// and fails with [`Error::Removed`] for a tailnet that was removed.
    pub async fn ensure_running(&self, id: i64) -> Result<(), Error> {
        let _turn = self.turn(id).await;
        if self.removed().contains(&id) {
            return Err(Error::Removed(id));
        }
        self.start(id).await
    }

    /// `ensure_running`, with the turn of `id` already held.
    async fn start(&self, id: i64) -> Result<(), Error> {
        if self.is_supervised(id) {
            return Ok(());
        }
        let dir = self.tailnet_dir(id);
        let socket = dir.join("ts.sock");
        if socket.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
            return Err(Error::SocketPathTooLong(socket.display().to_string()));
        }
        if let Some(tailnets) = dir.parent() {
            private_dir(tailnets).await?;
        }
        private_dir(&dir).await?;

        let mut entries = self.entries();
        if entries
            .get(&id)
            .is_some_and(|entry| !entry.supervisor.is_finished())
        {
            return Ok(());
        }
        let client = LocalClient::with_ttl(socket.clone(), self.inner.status_ttl);
        let supervisor = Supervisor::spawn(SupervisorConfig {
            id,
            binary: self.inner.config.binary.clone(),
            dir,
            socket,
            backoff: self.inner.backoff,
        });
        entries.insert(id, Entry { supervisor, client });
        Ok(())
    }

    fn is_supervised(&self, id: i64) -> bool {
        self.entries()
            .get(&id)
            .is_some_and(|entry| !entry.supervisor.is_finished())
    }

    fn lookup(&self, id: i64) -> Option<(LocalClient, Process)> {
        self.entries()
            .get(&id)
            .map(|entry| (entry.client.clone(), entry.supervisor.process()))
    }

    /// Joins the tailnet, or leaves its login to the user: with an auth key the
    /// node is authorised at once; without one the daemon is asked for a login
    /// URL, which `state` reports as `NeedsLogin`. Calling it again on a node
    /// that is logged in applies the new options without a new login.
    pub async fn connect(&self, id: i64, options: ConnectOptions) -> Result<(), Error> {
        let (options, client) = self.start_node(id, options).await?;
        if options.auth_key.is_some() {
            return Ok(());
        }
        // A node that has no key is logged out; one that has is already on its
        // way, and asking for a login would throw its key away.
        if client.settled_status().await?.backend_state == "NeedsLogin" {
            client.login_interactive().await?;
        }
        Ok(())
    }

    /// Asks the daemon for a new interactive login, whatever the node holds: a
    /// sign-in link that expired, a node key that expired, a node restored
    /// without its key. The new link is in `state` a moment later, as
    /// `NeedsLogin`. Any `auth_key` of the options is ignored: this is the way
    /// back for a tailnet that has no usable key, not a new way to join.
    pub async fn sign_in_again(&self, id: i64, options: ConnectOptions) -> Result<(), Error> {
        let (_, client) = self
            .start_node(
                id,
                ConnectOptions {
                    auth_key: None,
                    ..options
                },
            )
            .await?;
        // The login is asked for once the daemon has said where it stands: a
        // request made while it is still loading its state is lost.
        client.settled_status().await?;
        client.login_interactive().await
    }

    /// Validates the options, waits for the daemon, and sends it the options.
    async fn start_node(
        &self,
        id: i64,
        options: ConnectOptions,
    ) -> Result<(ConnectOptions, LocalClient), Error> {
        let options = validate(&options)?;
        let (client, _) = self.lookup(id).ok_or(Error::NotRunning(id))?;
        self.wait_ready(id, &client).await?;

        let body = start_options(&options);
        let secret = options.auth_key.as_deref().unwrap_or_default();
        client
            .start(&body)
            .await
            .map_err(|error| error.scrubbed(secret))?;
        Ok((options, client))
    }

    /// Waits for the daemon of `id` to answer on its socket.
    async fn wait_ready(&self, id: i64, client: &LocalClient) -> Result<(), Error> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let error = match client.status().await {
                Ok(_) => return Ok(()),
                Err(error) => error,
            };
            match self.lookup(id) {
                None => return Err(Error::NotRunning(id)),
                Some((_, Process::Failed(reason))) => return Err(Error::Unreachable(reason)),
                Some(_) => {}
            }
            if Instant::now() >= deadline {
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Where the tailnet stands. A tailnet nobody started is `Stopped`.
    pub async fn state(&self, id: i64) -> TailnetState {
        let Some((client, process)) = self.lookup(id) else {
            return TailnetState::Stopped;
        };
        if let Process::Failed(reason) = process {
            return TailnetState::Failed(reason);
        }
        match client.status().await {
            Ok(status) => state_of(&status),
            // No socket yet, or it stopped answering a moment ago.
            Err(Error::Unreachable(_)) => TailnetState::Starting,
            Err(error) => TailnetState::Failed(error.to_string()),
        }
    }

    /// The way to reach the peers of tailnet `id`, if its daemon is supervised.
    pub fn handle(&self, id: i64) -> Option<TailnetHandle> {
        self.lookup(id)
            .map(|(client, _)| TailnetHandle::new(id, client))
    }

    /// Stops the daemon of tailnet `id`, and keeps its state: `ensure_running`
    /// brings the node back without a login.
    pub async fn stop(&self, id: i64) {
        let _turn = self.turn(id).await;
        self.stop_daemon(id).await;
    }

    /// `stop`, with the turn of `id` already held.
    async fn stop_daemon(&self, id: i64) {
        let entry = self.entries().remove(&id);
        if let Some(entry) = entry {
            entry.supervisor.stop().await;
        }
    }

    /// Logs the node out of the tailnet, stops its daemon and deletes its
    /// directory. The node is gone from the tailnet unless the logout failed,
    /// which is logged: the removal goes on, an unreachable control server must
    /// not keep a tailnet from being deleted.
    ///
    /// The tailnet stays removed: `ensure_running` refuses it until
    /// [`forget_removal`](Self::forget_removal). Whatever was waiting for its
    /// turn finds it removed, and so does a request that comes later.
    pub async fn remove(&self, id: i64) -> Result<(), Error> {
        // Before the turn: what is queued behind this removal, and what is
        // queued before it, must not start a daemon once it has run.
        self.removed().insert(id);
        let _turn = self.turn(id).await;
        let dir = self.tailnet_dir(id);
        if !self.is_supervised(id) && dir.join("tailscaled.state").exists() {
            // A node that was joined before this process started: the daemon
            // has to run to log it out.
            self.start(id).await?;
        }
        if let Some((client, _)) = self.lookup(id) {
            let logout = async {
                self.wait_ready(id, &client).await?;
                client.logout().await
            };
            if let Err(error) = logout.await {
                tracing::warn!(tailnet = id, "could not log the node out: {error}");
            }
        }
        self.stop_daemon(id).await;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Each `Supervisor` aborts its task, which kills its daemon.
        self.entries
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// `path` as an absolute path, against the working directory of the process,
/// with its `.` and `..` folded away: the result is the path of a unix socket,
/// which has little room. A path that cannot be made absolute (no working
/// directory) is left as it is.
pub(crate) fn absolute(path: &std::path::Path) -> PathBuf {
    use std::path::Component;

    let Ok(joined) = std::path::absolute(path) else {
        return path.to_path_buf();
    };
    let mut folded = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    folded
}

/// A directory only this user can enter: it holds the node key.
async fn private_dir(path: &std::path::Path) -> Result<(), Error> {
    tokio::fs::create_dir_all(path).await?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

fn state_of(status: &Status) -> TailnetState {
    match status.backend_state.as_str() {
        "NoState" | "Starting" | "" => TailnetState::Starting,
        "NeedsLogin" => TailnetState::NeedsLogin {
            auth_url: Url::parse(&status.auth_url).ok(),
        },
        "NeedsMachineAuth" => TailnetState::NeedsMachineAuth,
        "Stopped" => TailnetState::Stopped,
        "Running" => TailnetState::Running {
            magic_dns_suffix: status.magic_dns_suffix().to_owned(),
            dns_name: status
                .self_node
                .as_ref()
                .map(|node| node.dns_name.trim_end_matches('.').to_owned())
                .unwrap_or_default(),
            addresses: status.own_addresses(),
        },
        other => TailnetState::Failed(format!("tailscaled is in the unknown state {other}")),
    }
}

/// The `ipn.Options` of `POST /localapi/v0/start`.
///
/// `UpdatePrefs` replaces the stored preferences as a whole, so every one that
/// matters is spelled out: the node runs (`WantRunning`), takes the subnet
/// routes of its peers (`RouteAll`), resolves MagicDNS (`CorpDNS`), refuses
/// connections from the tailnet (`ShieldsUp`: this node only dials out) and
/// does not update itself.
fn start_options(options: &ConnectOptions) -> Value {
    let mut body = json!({
        "UpdatePrefs": {
            "ControlURL": options.control_url.as_ref().map_or("", Url::as_str),
            "RouteAll": true,
            "WantRunning": true,
            "Hostname": options.hostname,
            "CorpDNS": true,
            "ShieldsUp": true,
            "AutoUpdate": {"Check": false},
        }
    });
    if let Some(key) = &options.auth_key {
        body["AuthKey"] = Value::String(key.clone());
    }
    body
}

static CONNECT_OPTIONS: LazyLock<vine::Validator> = LazyLock::new(|| {
    let hostname =
        vine::js::regex(r"^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$", "").expect("static regex");
    let url = vine::js::regex(r"^https?://", "i").expect("static regex");
    vine::global().create(vine::object! {
        "hostname" => vine::string().trim().min_length(1).max_length(63).regex(hostname),
        "control_url" => vine::string().trim().regex(url).optional(),
        "auth_key" => vine::string().trim().min_length(1).max_length(1024).optional(),
    })
});

/// The options as the validator cleaned them: trimmed, and checked.
fn validate(options: &ConnectOptions) -> Result<ConnectOptions, Error> {
    let input = json!({
        "hostname": options.hostname,
        "control_url": options.control_url.as_ref().map(Url::as_str),
        "auth_key": options.auth_key,
    });
    let output = CONNECT_OPTIONS.validate(&input).map_err(|error| {
        let messages: Vec<&str> = error
            .messages
            .iter()
            .map(|message| message.message.as_str())
            .collect();
        Error::InvalidOptions(messages.join("; "))
    })?;
    let text = |name: &str| output.get(name).and_then(Value::as_str).map(str::to_owned);
    Ok(ConnectOptions {
        hostname: text("hostname").unwrap_or_default(),
        control_url: options.control_url.clone(),
        auth_key: text("auth_key"),
    })
}
