//! One Tailscale node: the `tailscaled` process of a proxy, kept running, and
//! the cached picture of what it says about itself.
//!
//! Two tasks work for a node. The supervisor starts the process, restarts it
//! with a growing delay when it exits, and stops it. While a process runs, the
//! poller asks its LocalAPI for the status, applies the prefs, logs in when the
//! node needs it, and publishes the result for [`Node::status`].

use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use crate::launcher::{LaunchSpec, Launcher};
use crate::local_api::{ApiError, LocalApi, LocalStatus};
use crate::types::{NodeConfig, NodeState, NodeStatus};

/// The delays and limits of the supervisor and the poller. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// First wait before a restart; doubled after each failure.
    pub backoff_start: Duration,
    pub backoff_max: Duration,
    /// A process that ran this long was healthy: its end resets the backoff.
    pub healthy_run: Duration,
    /// How many quick failures in a row make the node `Failed`.
    pub quick_failures: u32,
    /// Between polls while the process has not answered yet.
    pub poll_startup: Duration,
    /// Between polls while the node is not connected.
    pub poll_pending: Duration,
    /// Between polls while the node is connected.
    pub poll_connected: Duration,
    /// How long a running process may stay silent on its socket.
    pub startup_limit: Duration,
    /// How long control has to accept an auth key.
    pub key_grace: Duration,
    /// How long before a login link is asked for again, when none came.
    pub interactive_retry: Duration,
    /// One LocalAPI call.
    pub request: Duration,
    /// The logout of a node that is being removed.
    pub logout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            healthy_run: Duration::from_secs(30),
            quick_failures: 3,
            poll_startup: Duration::from_millis(250),
            poll_pending: Duration::from_secs(2),
            poll_connected: Duration::from_secs(30),
            startup_limit: Duration::from_secs(60),
            key_grace: Duration::from_secs(90),
            interactive_retry: Duration::from_secs(30),
            request: Duration::from_secs(10),
            logout: Duration::from_secs(5),
        }
    }
}

pub(crate) struct Node {
    pub id: i64,
    pub spec: LaunchSpec,
    /// The password of the node's SOCKS front. In memory only: it changes with
    /// every start of the server and is shown to nobody.
    secret: String,
    api: LocalApi,
    status: RwLock<NodeStatus>,
    config: watch::Sender<NodeConfig>,
    timing: Timing,
    cancel: CancellationToken,
    /// Restart the process now and start the backoff over.
    reconnect: Notify,
    /// Look at the config now instead of at the next poll.
    poke: Notify,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Node {
    /// Creates the node and starts its supervisor. The directory already exists.
    pub fn spawn(
        config: NodeConfig,
        spec: LaunchSpec,
        launcher: Arc<dyn Launcher>,
        timing: Timing,
    ) -> Arc<Self> {
        let mut secret = [0u8; 32];
        rand::rng().fill(secret.as_mut_slice());
        let (config, _) = watch::channel(config);
        let node = Arc::new(Self {
            id: spec.id,
            api: LocalApi::new(spec.socket.clone(), timing.request),
            spec,
            secret: URL_SAFE_NO_PAD.encode(secret),
            status: RwLock::new(NodeStatus::new(NodeState::Starting)),
            config,
            timing,
            cancel: CancellationToken::new(),
            reconnect: Notify::new(),
            poke: Notify::new(),
            task: Mutex::new(None),
        });
        let task = tokio::spawn(supervise(node.clone(), launcher));
        *node.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
        node
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn api(&self) -> &LocalApi {
        &self.api
    }

    pub fn status(&self) -> NodeStatus {
        self.status
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn config(&self) -> NodeConfig {
        self.config.borrow().clone()
    }

    /// Takes the hostname and the key of a changed proxy; the poller applies them.
    pub fn update(&self, config: NodeConfig) {
        self.config.send_if_modified(|current| {
            let changed =
                current.hostname != config.hostname || current.auth_key != config.auth_key;
            if changed {
                *current = config;
            }
            changed
        });
        self.poke.notify_one();
    }

    /// Restarts the process, and logs in again from the start.
    pub fn reconnect(&self) {
        self.reconnect.notify_one();
    }

    /// Stops the process (SIGTERM, grace, kill) and ends the supervisor.
    pub async fn stop(&self) {
        self.cancel.cancel();
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// Cancels without waiting, for a runtime that is dropped.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    fn set_state(&self, state: NodeState) {
        self.publish(|status| status.state = state);
    }

    fn publish(&self, change: impl FnOnce(&mut NodeStatus)) {
        let mut status = self.status.write().unwrap_or_else(PoisonError::into_inner);
        let before = std::mem::discriminant(&status.state);
        change(&mut status);
        if before != std::mem::discriminant(&status.state) {
            // The link of a login is not logged: it is a bearer of the login.
            match &status.state {
                NodeState::Failed { error } => {
                    tracing::warn!(proxy = self.id, "Tailscale proxy failed: {error}");
                }
                state => tracing::info!(
                    proxy = self.id,
                    state = label(state),
                    "Tailscale proxy changed state"
                ),
            }
        }
    }
}

fn label(state: &NodeState) -> &'static str {
    match state {
        NodeState::Starting => "starting",
        NodeState::NeedsLogin { .. } => "needs login",
        NodeState::NeedsApproval => "needs approval",
        NodeState::Connected => "connected",
        NodeState::Failed { .. } => "failed",
        NodeState::Stopped => "stopped",
    }
}

enum Ended {
    Exited(String),
    Cancelled,
    Reconnect,
}

/// Keeps one `tailscaled` running until the node is cancelled.
async fn supervise(node: Arc<Node>, launcher: Arc<dyn Launcher>) {
    let timing = node.timing;
    let mut delay = timing.backoff_start;
    let mut quick_failures = 0u32;

    while !node.cancel.is_cancelled() {
        let started = Instant::now();
        let failure = match launcher.launch(&node.spec).await {
            Err(error) => format!("it could not be started ({error})"),
            Ok(mut process) => {
                let poller = tokio::spawn(Poller::new(node.clone()).run());
                let ended = tokio::select! {
                    exit = process.wait() => Ended::Exited(exit),
                    () = node.cancel.cancelled() => Ended::Cancelled,
                    () = node.reconnect.notified() => Ended::Reconnect,
                };
                poller.abort();
                match ended {
                    Ended::Cancelled => {
                        process.stop().await;
                        break;
                    }
                    Ended::Reconnect => {
                        process.stop().await;
                        delay = timing.backoff_start;
                        quick_failures = 0;
                        node.set_state(NodeState::Starting);
                        continue;
                    }
                    Ended::Exited(exit) => match process.last_error().await {
                        Some(line) => line,
                        None => format!("it exited with {exit}"),
                    },
                }
            }
        };

        if started.elapsed() >= timing.healthy_run {
            quick_failures = 0;
            delay = timing.backoff_start;
        } else {
            quick_failures += 1;
        }
        tracing::warn!(
            proxy = node.id,
            "tailscaled stopped, restarting in {} ms: {failure}",
            delay.as_millis()
        );
        node.set_state(if quick_failures >= timing.quick_failures {
            NodeState::Failed {
                error: format!("tailscaled keeps stopping: {failure}"),
            }
        } else {
            NodeState::Starting
        });

        tokio::select! {
            () = sleep(delay) => {}
            () = node.cancel.cancelled() => break,
            () = node.reconnect.notified() => {
                delay = timing.backoff_start;
                quick_failures = 0;
                continue;
            }
        }
        delay = (delay * 2).min(timing.backoff_max);
    }
    node.set_state(NodeState::Stopped);
}

/// Asks a running process what it is doing, and moves it along.
struct Poller {
    node: Arc<Node>,
    started: Instant,
    /// The hostname the prefs were last applied with.
    prefs_for: Option<String>,
    /// The key this process was given, and when. A changed key is tried again.
    tried_key: Option<String>,
    tried_at: Option<Instant>,
    /// Why `start` refused the key; kept until the key changes.
    start_error: Option<String>,
    interactive_at: Option<Instant>,
    /// Since when a node without a key has been logged out because control cannot be reached.
    unreachable_since: Option<Instant>,
    failures: u32,
}

impl Poller {
    fn new(node: Arc<Node>) -> Self {
        Self {
            node,
            started: Instant::now(),
            prefs_for: None,
            tried_key: None,
            tried_at: None,
            start_error: None,
            interactive_at: None,
            unreachable_since: None,
            failures: 0,
        }
    }

    async fn run(mut self) {
        let timing = self.node.timing;
        loop {
            let wait = match self.poll().await {
                Poll::NotReady => timing.poll_startup,
                Poll::Pending => timing.poll_pending,
                Poll::Connected => timing.poll_connected,
            };
            tokio::select! {
                () = sleep(wait) => {}
                () = self.node.poke.notified() => {}
            }
        }
    }

    async fn poll(&mut self) -> Poll {
        let config = self.node.config();
        let api = self.node.api.clone();
        let status = match api.status().await {
            Ok(status) => status,
            Err(error) => {
                self.status_failed(&error);
                return Poll::NotReady;
            }
        };
        self.failures = 0;

        // The prefs come first: a node that logs in with them unset would register with the
        // machine's name and without the shields.
        if self.prefs_for.as_deref() != Some(config.hostname.as_str()) {
            match api.apply_prefs(&config.hostname).await {
                Ok(()) => self.prefs_for = Some(config.hostname.clone()),
                Err(error) => {
                    tracing::warn!(
                        proxy = self.node.id,
                        "Tailscale prefs were not applied: {error}"
                    );
                    self.publish(&status, NodeState::Starting);
                    return Poll::NotReady;
                }
            }
        }

        let state = match status.backend_state.as_str() {
            "Running" => {
                self.start_error = None;
                NodeState::Connected
            }
            "NeedsMachineAuth" => NodeState::NeedsApproval,
            "NeedsLogin" => self.needs_login(&config, &status).await,
            "Stopped" => {
                // The node is meant to run: say so again at the next poll.
                self.prefs_for = None;
                NodeState::Starting
            }
            _ => NodeState::Starting,
        };
        let connected = state == NodeState::Connected;
        self.publish(&status, state);
        if connected {
            Poll::Connected
        } else {
            Poll::Pending
        }
    }

    fn publish(&self, status: &LocalStatus, state: NodeState) {
        self.node.publish(|node| {
            node.state = state;
            node.tailnet = status.tailnet();
            node.dns_name = status.dns_name();
            node.addresses = status.addresses();
            node.health = status.health();
        });
    }

    fn status_failed(&mut self, error: &ApiError) {
        self.failures += 1;
        let state = self.node.status().state;
        let silent_for = self.started.elapsed();
        if matches!(state, NodeState::Connected) {
            // One missed answer is not a loss of the tailnet.
            if self.failures >= 3 {
                self.node.set_state(NodeState::Starting);
            }
        } else if silent_for >= self.node.timing.startup_limit
            && !matches!(state, NodeState::Failed { .. })
        {
            self.node.set_state(NodeState::Failed {
                error: format!("tailscaled runs but does not answer: {error}"),
            });
        }
    }

    /// `NeedsLogin`: log in with the key, or ask for a link.
    async fn needs_login(&mut self, config: &NodeConfig, status: &LocalStatus) -> NodeState {
        let timing = self.node.timing;
        let key = config.auth_key.as_deref().filter(|key| !key.is_empty());

        let Some(key) = key else {
            if !status.auth_url.is_empty() {
                self.unreachable_since = None;
                return NodeState::NeedsLogin {
                    auth_url: Some(status.auth_url.clone()),
                };
            }
            let due = self
                .interactive_at
                .is_none_or(|asked| asked.elapsed() >= timing.interactive_retry);
            if due {
                match self.node.api.login_interactive().await {
                    Ok(()) => self.interactive_at = Some(Instant::now()),
                    Err(error) => {
                        tracing::warn!(
                            proxy = self.node.id,
                            "Tailscale login link was not asked for: {error}"
                        );
                    }
                }
            }
            // Without a key there is no answer to wait for, only a link: when control cannot be
            // reached no link comes, and the node says so once the grace period has passed.
            // It keeps asking for the link, so it recovers as soon as the network is back.
            let reason = last_login_error(&status.health())
                .filter(|reason| reason.starts_with(TRANSPORT_ERROR));
            let Some(reason) = reason else {
                self.unreachable_since = None;
                return NodeState::NeedsLogin { auth_url: None };
            };
            let since = *self.unreachable_since.get_or_insert_with(Instant::now);
            return if since.elapsed() >= timing.key_grace {
                NodeState::Failed {
                    error: format!(
                        "Tailscale could not reach its control server to log in: {reason}"
                    ),
                }
            } else {
                NodeState::NeedsLogin { auth_url: None }
            };
        };

        let mut just_tried = false;
        if self.tried_key.as_deref() != Some(key) {
            self.start_error = None;
            match self
                .node
                .api
                .login_with_key(key, status.have_node_key)
                .await
            {
                Ok(()) => {
                    self.tried_key = Some(key.to_owned());
                    self.tried_at = Some(Instant::now());
                    just_tried = true;
                }
                Err(error @ ApiError::Http { .. }) => {
                    // The key is not tried again until it changes.
                    self.tried_key = Some(key.to_owned());
                    self.start_error =
                        Some(format!("Tailscale could not start the login: {error}"));
                }
                Err(error) => {
                    tracing::warn!(
                        proxy = self.node.id,
                        "Tailscale login did not start: {error}"
                    );
                    return NodeState::Starting;
                }
            }
        }
        if let Some(error) = &self.start_error {
            return NodeState::Failed {
                error: error.clone(),
            };
        }
        if just_tried {
            // The health in hand was read before the key was sent: it says nothing about it.
            return NodeState::Starting;
        }

        let login_error = last_login_error(&status.health());
        // A refusal comes from the control server itself. An error of the request means it
        // was not reached, which Tailscale retries on its own.
        if let Some(reason) = login_error
            .as_deref()
            .filter(|reason| !reason.starts_with(TRANSPORT_ERROR))
        {
            return NodeState::Failed {
                error: format!("Tailscale refused the auth key: {reason}"),
            };
        }
        match self.tried_at {
            Some(at) if at.elapsed() >= timing.key_grace => NodeState::Failed {
                error: match login_error {
                    Some(reason) => {
                        format!("Tailscale could not reach its control server to log in: {reason}")
                    }
                    None => {
                        "Tailscale did not log in with the auth key: check that the key is valid \
                             and that this server can reach Tailscale"
                            .into()
                    }
                },
            },
            _ => NodeState::Starting,
        }
    }
}

enum Poll {
    /// The process does not answer yet.
    NotReady,
    Pending,
    Connected,
}

/// How the error of a request that never got an answer starts, in `tailscaled`'s own words.
const TRANSPORT_ERROR: &str = "register request:";

/// While a login fails, the health of the node says
/// "You are logged out. The last login error was: …".
fn last_login_error(health: &[String]) -> Option<String> {
    const MARKER: &str = "The last login error was:";
    health.iter().find_map(|line| {
        let (_, reason) = line.split_once(MARKER)?;
        let reason = reason.trim();
        (!reason.is_empty()).then(|| reason.chars().take(300).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_error_is_read_from_the_health() {
        let health = vec![
            "Tailscale is not connected".to_owned(),
            "You are logged out. The last login error was: invalid key: API key abc not valid"
                .to_owned(),
        ];
        assert_eq!(
            last_login_error(&health).as_deref(),
            Some("invalid key: API key abc not valid")
        );
        assert_eq!(last_login_error(&health[..1]), None);
        assert_eq!(
            last_login_error(&["You are logged out. The last login error was: ".to_owned()]),
            None
        );
    }
}
