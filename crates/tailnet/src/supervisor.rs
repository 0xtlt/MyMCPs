//! One `tailscaled` process, kept alive.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How long a daemon gets between SIGTERM and SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

/// The wait before a restart: it doubles on every failure up to `max`, and
/// starts over once a daemon ran for `healthy_after`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    pub initial: Duration,
    pub max: Duration,
    pub healthy_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
            healthy_after: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    pub(crate) fn next(&self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.max)
    }
}

/// What the supervisor knows of the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Process {
    /// Being started.
    Starting,
    /// Running: it may not answer on its socket yet.
    Running,
    /// Not running: it could not be started, or exited. The supervisor starts
    /// it again after a wait.
    Failed(String),
}

pub(crate) struct SupervisorConfig {
    pub id: i64,
    pub binary: PathBuf,
    /// The state directory, `HOME` of the daemon.
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub backoff: Backoff,
}

pub(crate) struct Supervisor {
    cancel: CancellationToken,
    task: JoinHandle<()>,
    process: Arc<Mutex<Process>>,
}

impl Supervisor {
    pub(crate) fn spawn(config: SupervisorConfig) -> Self {
        let cancel = CancellationToken::new();
        let process = Arc::new(Mutex::new(Process::Starting));
        let task = tokio::spawn(supervise(config, cancel.clone(), Arc::clone(&process)));
        Supervisor {
            cancel,
            task,
            process,
        }
    }

    pub(crate) fn process(&self) -> Process {
        self.process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Stops the daemon: SIGTERM, then SIGKILL after a grace period.
    pub(crate) async fn stop(mut self) {
        self.cancel.cancel();
        let wait = GRACE + Duration::from_secs(2);
        if tokio::time::timeout(wait, &mut self.task).await.is_err() {
            // Dropping `self` aborts the task, and with it the child.
            tracing::warn!("tailscaled did not stop in time");
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        // The child is `kill_on_drop`: aborting the task kills it.
        self.task.abort();
    }
}

fn set(process: &Mutex<Process>, value: Process) {
    *process.lock().unwrap_or_else(PoisonError::into_inner) = value;
}

async fn supervise(
    config: SupervisorConfig,
    cancel: CancellationToken,
    process: Arc<Mutex<Process>>,
) {
    let backoff = config.backoff;
    let mut delay = backoff.initial;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        set(&process, Process::Starting);
        let started = Instant::now();
        let failure = match spawn(&config) {
            Err(error) => format!("could not start {}: {error}", config.binary.display()),
            Ok(mut child) => {
                set(&process, Process::Running);
                tracing::debug!(tailnet = config.id, "tailscaled started");
                let exit = tokio::select! {
                    status = child.wait() => status,
                    () = cancel.cancelled() => {
                        terminate(&mut child).await;
                        return;
                    }
                };
                match exit {
                    Ok(status) => format!("tailscaled exited with {status}"),
                    Err(error) => format!("tailscaled could not be waited for: {error}"),
                }
            }
        };
        if started.elapsed() >= backoff.healthy_after {
            delay = backoff.initial;
        }
        tracing::warn!(tailnet = config.id, "{failure}; restarting in {delay:?}");
        set(&process, Process::Failed(failure));
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancel.cancelled() => return,
        }
        delay = backoff.next(delay);
    }
}

fn spawn(config: &SupervisorConfig) -> std::io::Result<Child> {
    let mut command = Command::new(&config.binary);
    command
        .arg("--tun=userspace-networking")
        .arg(format!("--statedir={}", config.dir.display()))
        .arg(format!("--socket={}", config.socket.display()))
        .arg("--port=0")
        .arg("--no-logs-no-support")
        // Nothing of the environment of the server reaches the daemon: not its
        // secrets, not a proxy or a `TS_*` knob.
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", &config.dir)
        .current_dir(&config.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        forward(config.id, stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        forward(config.id, stderr);
    }
    Ok(child)
}

/// The log of the daemon, line by line, at debug level.
fn forward(id: i64, stream: impl AsyncRead + Unpin + Send + 'static) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "mymcps_tailnet::tailscaled", tailnet = id, "{line}");
        }
    });
}

async fn terminate(child: &mut Child) {
    // `id()` is `None` once the child was reaped, so the signal cannot reach
    // another process.
    let pid = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw);
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_up_to_the_maximum() {
        let backoff = Backoff::default();
        let mut delay = backoff.initial;
        let mut seen = vec![delay];
        for _ in 0..8 {
            delay = backoff.next(delay);
            seen.push(delay);
        }
        let seconds: Vec<f64> = seen.iter().map(Duration::as_secs_f64).collect();
        assert_eq!(seconds, [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0]);
    }
}
