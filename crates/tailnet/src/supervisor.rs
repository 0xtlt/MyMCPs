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

/// The log of the daemon, line by line, at debug level, with what must not be
/// kept in a log taken out.
fn forward(id: i64, stream: impl AsyncRead + Unpin + Send + 'static) {
    forward_to(stream, move |line| {
        tracing::debug!(target: "mymcps_tailnet::tailscaled", tailnet = id, "{line}");
    });
}

fn forward_to(
    stream: impl AsyncRead + Unpin + Send + 'static,
    sink: impl Fn(&str) + Send + 'static,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink(&redact(&line));
        }
    });
}

/// Words that name a secret in a `name=value` or `name: value` of a log line.
const SECRET_NAMES: [&str; 6] = ["key", "token", "secret", "password", "auth", "code"];
/// What a secret of Tailscale starts with: an auth key, or a key of the node.
const SECRET_PREFIXES: [&str; 5] = ["tskey-", "nodekey:", "privkey:", "mkey:", "discokey:"];

/// A log line of the daemon without its links and keys. The sign-in link of the
/// control server (`AuthURL`) is a credential for as long as it lives: whoever
/// reads it can sign a device in to the tailnet. The daemon logs links in
/// several shapes, so every URL goes, whatever surrounds it, with the keys of
/// Tailscale and the value of anything called a key, token, secret, password,
/// auth or code.
pub(crate) fn redact(line: &str) -> String {
    const URL: &str = "<redacted url>";
    const SECRET: &str = "<redacted>";
    let mut redacted = String::with_capacity(line.len());
    let mut hide_next = false;
    for (index, word) in line.split(' ').enumerate() {
        if index > 0 {
            redacted.push(' ');
        }
        let lower = word.to_ascii_lowercase();
        if let Some(start) = lower.find("://") {
            // The scheme starts after the last character that is not one.
            let scheme = lower[..start]
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
                .map_or(0, |position| position + 1);
            redacted.push_str(&word[..scheme]);
            redacted.push_str(URL);
            hide_next = false;
            continue;
        }
        if let Some(position) = SECRET_PREFIXES
            .iter()
            .filter_map(|prefix| lower.find(prefix))
            .min()
        {
            redacted.push_str(&word[..position]);
            redacted.push_str(SECRET);
            hide_next = false;
            continue;
        }
        if hide_next {
            redacted.push_str(SECRET);
            hide_next = false;
            continue;
        }
        // `name=value`, `"name":"value"`, and `name: value` (the value is the next word).
        let separator = word.find(['=', ':']);
        let named = separator.map(|at| (&lower[..at], at));
        match named {
            Some((name, at)) if names_a_secret(name) => {
                if word[at + 1..].is_empty() {
                    redacted.push_str(word);
                    hide_next = true;
                } else {
                    redacted.push_str(&word[..=at]);
                    redacted.push_str(SECRET);
                }
            }
            _ => redacted.push_str(word),
        }
    }
    redacted
}

fn names_a_secret(name: &str) -> bool {
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    !name.is_empty() && SECRET_NAMES.iter().any(|secret| name.contains(secret))
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
    fn links_and_keys_of_the_daemon_log_are_taken_out() {
        for (line, expected) in [
            (
                "AuthURL is https://login.tailscale.com/a/0123456789abcd",
                "AuthURL is <redacted url>",
            ),
            (
                "To authenticate, visit:\thttps://headscale.example.com/register/mkey:abc123",
                "To authenticate, visit:\t<redacted url>",
            ),
            (
                "control: AuthURL=\"http://10.0.0.5:8080/register/abc\" state=NeedsLogin",
                "control: AuthURL=\"<redacted url> state=NeedsLogin",
            ),
            (
                "visit (https://login.tailscale.com/a/xyz) now",
                "visit (<redacted url> now",
            ),
            ("HTTPS://LOGIN.TAILSCALE.COM/A/XYZ", "<redacted url>"),
            (
                "using authkey tskey-auth-kS3cr3t-abcdef to join",
                "using authkey <redacted> to join",
            ),
            (
                "peer nodekey:0123abcd and key=hunter2 token: s3cr3t",
                "peer <redacted> and key=<redacted> token: <redacted>",
            ),
            ("AuthKey=tskey-auth-xyz", "AuthKey=<redacted>"),
            (
                "{\"AuthURL\":\"https://login.tailscale.com/a/xyz\",\"x\":1}",
                "{\"AuthURL\":\"<redacted url>",
            ),
        ] {
            let redacted = redact(line);
            assert_eq!(redacted, expected, "{line}");
            for secret in [
                "a/0123456789abcd",
                "abc123",
                "hunter2",
                "s3cr3t",
                "kS3cr3t",
                "0123abcd",
                "/a/xyz",
                "register/abc",
            ] {
                assert!(!redacted.contains(secret), "{redacted} still has {secret}");
            }
        }
    }

    #[test]
    fn an_ordinary_line_is_left_as_it_is() {
        for line in [
            "",
            "tailscaled: starting userspace networking",
            "magicsock: endpoints changed: 192.0.2.1:41641 (stun)",
            "Switching ipn state NoState -> Starting (WantRunning=true, nm=false)",
            "health(warnable=login-state): error: You are logged out.",
            "  indented  line  ",
        ] {
            assert_eq!(redact(line), line);
        }
    }

    #[tokio::test]
    async fn the_forwarded_output_has_no_link_in_it() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = tokio::io::duplex(1024);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        forward_to(reader, move |line| {
            let _ = sender.send(line.to_owned());
        });
        writer
            .write_all(
                b"ipnserver: AuthURL https://login.tailscale.com/a/secretlink1\nplain line\n",
            )
            .await
            .unwrap();
        drop(writer);
        let mut seen = Vec::new();
        while let Some(line) = receiver.recv().await {
            seen.push(line);
        }
        assert_eq!(
            seen,
            [
                "ipnserver: AuthURL <redacted url>".to_owned(),
                "plain line".to_owned()
            ]
        );
    }

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
