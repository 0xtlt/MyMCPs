//! Starting `tailscaled`. The supervisor only knows the two traits, so a test
//! replaces the binary with an in-process server on the same socket path.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// SIGTERM first, then SIGKILL: how long `tailscaled` has to save its state and exit.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// How long to wait for the last lines of a process that has just exited.
const DRAIN_WAIT: Duration = Duration::from_millis(500);

/// How many of the last lines the process wrote to stderr are kept.
const TAIL_LINES: usize = 20;

/// A longer line is cut: `tailscaled` writes short lines, and a pipe nobody
/// reads would block it.
const MAX_LINE_BYTES: u64 = 8 * 1024;

/// Where one node lives: everything is inside its own directory.
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub id: i64,
    pub dir: PathBuf,
    pub state_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub socket: PathBuf,
}

impl LaunchSpec {
    pub fn new(id: i64, dir: PathBuf) -> Self {
        Self {
            id,
            state_dir: dir.join("state"),
            logs_dir: dir.join("logs"),
            socket: dir.join("tailscaled.sock"),
            dir,
        }
    }
}

#[async_trait]
pub(crate) trait Launcher: Send + Sync {
    async fn launch(&self, spec: &LaunchSpec) -> io::Result<Box<dyn Process>>;
}

#[async_trait]
pub(crate) trait Process: Send {
    /// Resolves when the process has exited, with how it ended. Safe to drop and call again.
    async fn wait(&mut self) -> String;
    /// SIGTERM, a grace period, then SIGKILL. Returns once the process is gone.
    async fn stop(&mut self);
    /// The last thing the process said on stderr, for the sentence that says why it stopped.
    async fn last_error(&mut self) -> Option<String>;
}

/// Runs the real `tailscaled`.
pub(crate) struct TailscaledLauncher {
    binary: PathBuf,
}

impl TailscaledLauncher {
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }
}

fn path_flag(flag: &str, path: &Path) -> OsString {
    let mut argument = OsString::from(flag);
    argument.push(path);
    argument
}

#[async_trait]
impl Launcher for TailscaledLauncher {
    async fn launch(&self, spec: &LaunchSpec) -> io::Result<Box<dyn Process>> {
        // A socket left by a process that was killed would make the new one fail to listen.
        match tokio::fs::remove_file(&spec.socket).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let mut command = Command::new(&self.binary);
        command
            // No TUN device and no privilege: the node is a library in a process.
            .arg("--tun=userspace-networking")
            .arg(path_flag("--statedir=", &spec.state_dir))
            .arg(path_flag("--socket=", &spec.socket))
            // Any free UDP port. The node is behind NAT or a container anyway.
            .arg("--port=0")
            // Nothing is uploaded to Tailscale's log service.
            .arg("--no-logs-no-support")
            .current_dir(&spec.dir)
            // Nothing of the server's environment reaches it: not the app key, not a token.
            .env_clear()
            .env("TS_LOGS_DIR", &spec.logs_dir)
            .env("HOME", &spec.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stdout_reader = child
            .stdout
            .take()
            .map(|stdout| read_lines(stdout, spec.id, None));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stderr| read_lines(stderr, spec.id, Some(tail.clone())));
        Ok(Box::new(TailscaledProcess {
            child,
            tail,
            stdout_reader,
            stderr_reader,
        }))
    }
}

/// Reads a pipe line by line into the debug log, and keeps the last lines when asked.
/// Its output can be chatty, so it never goes above debug.
fn read_lines(
    stream: impl AsyncRead + Unpin + Send + 'static,
    id: i64,
    tail: Option<Arc<Mutex<VecDeque<String>>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader)
                .take(MAX_LINE_BYTES)
                .read_until(b'\n', &mut line)
                .await
            {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let text = strip_log_prefix(text.trim());
            if text.is_empty() {
                continue;
            }
            tracing::debug!(proxy = id, "tailscaled: {text}");
            if let Some(tail) = &tail {
                let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(text.to_owned());
            }
        }
    })
}

/// Go's logger starts each line with `2026/10/18 12:00:00 `.
fn strip_log_prefix(line: &str) -> &str {
    let bytes = line.as_bytes();
    let dated = bytes.len() > 20
        && bytes[4] == b'/'
        && bytes[7] == b'/'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..4].iter().all(u8::is_ascii_digit);
    match line.get(19..) {
        Some(rest) if dated => rest.trim_start(),
        _ => line,
    }
}

struct TailscaledProcess {
    child: Child,
    tail: Arc<Mutex<VecDeque<String>>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
}

#[async_trait]
impl Process for TailscaledProcess {
    async fn wait(&mut self) -> String {
        match self.child.wait().await {
            Ok(status) => status.to_string(),
            Err(error) => format!("it could not be waited for ({error})"),
        }
    }

    async fn stop(&mut self) {
        terminate(&self.child);
        if timeout(STOP_GRACE, self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }

    async fn last_error(&mut self) -> Option<String> {
        // The reader ends when the pipe closes; give it the time to hand over the last line.
        for reader in [self.stderr_reader.take(), self.stdout_reader.take()]
            .into_iter()
            .flatten()
        {
            if timeout(DRAIN_WAIT, reader).await.is_err() {
                break;
            }
        }
        let tail = self.tail.lock().unwrap_or_else(PoisonError::into_inner);
        tail.back().map(|line| line.chars().take(300).collect())
    }
}

#[cfg(unix)]
fn terminate(child: &Child) {
    // `id()` is `None` once the child was reaped, so the signal cannot reach another process.
    let pid = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw);
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
}

#[cfg(not(unix))]
fn terminate(_child: &Child) {
    // Without signals, only the kill that follows ends the child.
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    #[test]
    fn go_timestamps_are_dropped() {
        assert_eq!(
            strip_log_prefix("2026/10/18 12:00:01 flag provided but not defined: -x"),
            "flag provided but not defined: -x"
        );
        assert_eq!(strip_log_prefix("logtail started"), "logtail started");
        assert_eq!(strip_log_prefix("short"), "short");
        assert_eq!(strip_log_prefix("2026/10/18 12:00:01 é"), "é");
    }

    /// A `tailscaled` made of shell: it records how it was started.
    fn fake_binary(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-tailscaled");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn spec(dir: &Path) -> LaunchSpec {
        let spec = LaunchSpec::new(7, dir.join("7"));
        for path in [&spec.dir, &spec.state_dir, &spec.logs_dir] {
            std::fs::create_dir_all(path).unwrap();
        }
        spec
    }

    /// A script that was just written can be reported busy while another test forks.
    async fn launch(launcher: &TailscaledLauncher, spec: &LaunchSpec) -> Box<dyn Process> {
        for _ in 0..20 {
            match launcher.launch(spec).await {
                Ok(process) => return process,
                Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("{error}"),
            }
        }
        panic!("the script stayed busy");
    }

    const RUN_UNTIL_TERMINATED: &str = "\
printf '%s\\n' \"$@\" > \"$HOME/args.txt\"
env > \"$HOME/env.txt\"
echo '2026/10/18 10:00:00 a harmless line' >&2
echo 'to stdout' 
echo '2026/10/18 10:00:00 boom: it broke' >&2
trap 'echo term > \"$HOME/term.txt\"; exit 0' TERM
echo ready > \"$HOME/ready.txt\"
while :; do sleep 1 & wait $!; done";

    async fn ready(spec: &LaunchSpec) {
        for _ in 0..300 {
            if spec.dir.join("ready.txt").exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the script did not start");
    }

    #[tokio::test]
    async fn tailscaled_gets_its_flags_and_a_cleared_environment() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_binary(dir.path(), RUN_UNTIL_TERMINATED);
        let spec = spec(dir.path());
        let launcher = TailscaledLauncher::new(binary);
        let mut process = launch(&launcher, &spec).await;
        ready(&spec).await;

        let args = std::fs::read_to_string(spec.dir.join("args.txt")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "--tun=userspace-networking".to_owned(),
                format!("--statedir={}", spec.state_dir.display()),
                format!("--socket={}", spec.socket.display()),
                "--port=0".to_owned(),
                "--no-logs-no-support".to_owned(),
            ]
        );
        let env = std::fs::read_to_string(spec.dir.join("env.txt")).unwrap();
        assert!(
            env.lines()
                .any(|line| line == format!("HOME={}", spec.dir.display()))
        );
        assert!(
            env.lines()
                .any(|line| line == format!("TS_LOGS_DIR={}", spec.logs_dir.display()))
        );
        // Nothing of the server's own environment is passed on.
        for name in ["CARGO_MANIFEST_DIR", "PATH", "APP_KEY", "HTTPS_PROXY"] {
            assert!(
                !env.lines()
                    .any(|line| line.starts_with(&format!("{name}="))),
                "{name}"
            );
        }

        // The last line of stderr, without the timestamp of Go's logger.
        process.stop().await;
        assert_eq!(
            process.last_error().await.as_deref(),
            Some("boom: it broke")
        );
    }

    #[tokio::test]
    async fn stop_sends_sigterm_and_waits_for_the_exit() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_binary(dir.path(), RUN_UNTIL_TERMINATED);
        let spec = spec(dir.path());
        let mut process = launch(&TailscaledLauncher::new(binary), &spec).await;
        ready(&spec).await;

        let started = Instant::now();
        process.stop().await;
        assert!(
            started.elapsed() < Duration::from_millis(1900),
            "the grace period was used up"
        );
        assert_eq!(
            std::fs::read_to_string(spec.dir.join("term.txt"))
                .unwrap()
                .trim(),
            "term"
        );
    }

    #[tokio::test]
    async fn a_process_that_ignores_sigterm_is_killed_after_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_binary(
            dir.path(),
            "trap '' TERM\necho ready > \"$HOME/ready.txt\"\nwhile :; do sleep 1 & wait $!; done",
        );
        let spec = spec(dir.path());
        let mut process = launch(&TailscaledLauncher::new(binary), &spec).await;
        ready(&spec).await;

        let started = Instant::now();
        process.stop().await;
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(1900) && took < Duration::from_secs(5),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn an_exit_is_reported_with_the_last_line_of_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_binary(
            dir.path(),
            "echo 'one' >&2\necho '2026/10/18 10:00:00 flag provided but not defined: -x' >&2\nexit 3",
        );
        let spec = spec(dir.path());
        let mut process = launch(&TailscaledLauncher::new(binary), &spec).await;
        assert_eq!(process.wait().await, "exit status: 3");
        assert_eq!(
            process.last_error().await.as_deref(),
            Some("flag provided but not defined: -x")
        );
    }

    #[tokio::test]
    async fn a_stale_socket_is_removed_and_a_missing_binary_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec(dir.path());
        std::fs::write(&spec.socket, "").unwrap();
        let launcher = TailscaledLauncher::new(dir.path().join("missing"));
        let error = launcher.launch(&spec).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!spec.socket.exists());
    }
}
