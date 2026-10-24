use std::collections::HashMap;
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
/// How long `connect` waits for the daemon to say whether it needs a login.
const LOGIN_PROBES: usize = 40;
const LOGIN_PROBE_INTERVAL: Duration = Duration::from_millis(250);

/// Where the daemons live.
#[derive(Debug, Clone)]
pub struct TailnetsConfig {
    /// The `tailscaled` executable.
    pub binary: PathBuf,
    /// Holds `tailnets/<id>/` for every tailnet: its state, and its socket.
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
        config: TailnetsConfig,
        backoff: Backoff,
        status_ttl: Duration,
    ) -> Self {
        Tailnets {
            inner: Arc::new(Inner {
                config,
                backoff,
                status_ttl,
                entries: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Entry>> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
    pub async fn ensure_running(&self, id: i64) -> Result<(), Error> {
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
    /// URL, which `state` reports as `NeedsLogin`. Either way a node that has
    /// no login yet is asked to start one: `start` only sets the key and the
    /// preferences, and tailscaled 1.104 does not contact the control server
    /// until a login is started. Calling it again on a node that is logged in
    /// applies the new options without a new login.
    pub async fn connect(&self, id: i64, options: ConnectOptions) -> Result<(), Error> {
        let options = validate(&options)?;
        let (client, _) = self.lookup(id).ok_or(Error::NotRunning(id))?;
        self.wait_ready(id, &client).await?;

        let body = start_options(&options);
        let secret = options.auth_key.as_deref().unwrap_or_default();
        client
            .start(&body)
            .await
            .map_err(|error| error.scrubbed(secret))?;
        // A node that has no key is logged out; one that has is already on its
        // way, and asking for a login would throw its key away. With an auth
        // key, the login uses it and no URL is shown.
        for _ in 0..LOGIN_PROBES {
            let status = client.status().await?;
            match status.backend_state.as_str() {
                "NeedsLogin" => return client.login_interactive().await,
                "NoState" | "" => tokio::time::sleep(LOGIN_PROBE_INTERVAL).await,
                _ => return Ok(()),
            }
        }
        Ok(())
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
        let entry = self.entries().remove(&id);
        if let Some(entry) = entry {
            entry.supervisor.stop().await;
        }
    }

    /// Logs the node out of the tailnet, stops its daemon and deletes its
    /// directory. The node is gone from the tailnet unless the logout failed,
    /// which is logged: the removal goes on, an unreachable control server must
    /// not keep a tailnet from being deleted.
    pub async fn remove(&self, id: i64) -> Result<(), Error> {
        let dir = self.tailnet_dir(id);
        if !self.is_supervised(id) && dir.join("tailscaled.state").exists() {
            // A node that was joined before this process started: the daemon
            // has to run to log it out.
            self.ensure_running(id).await?;
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
        self.stop(id).await;
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
