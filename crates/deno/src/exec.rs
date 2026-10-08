//! Run a command to its end, as the Node app did with `child_process.execFile`:
//! a time limit, a limit on what it may print, and an error that reads as
//! Node's.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::paths::spawn_error_name;
use crate::text::{js_trim, utf16_prefix};

/// How long a command that was told to stop gets before it is killed. Node
/// waits without limit; Deno ends at once on SIGTERM.
const TERMINATE_GRACE: Duration = Duration::from_millis(2000);

pub(crate) struct ExecOptions<'a> {
    pub cwd: &'a Path,
    /// The whole environment of the command: nothing is inherited.
    pub env: &'a HashMap<String, String>,
    pub timeout: Duration,
    /// The most the command may write to stdout, and to stderr.
    pub max_buffer: usize,
}

/// A command that could not be started, failed, was stopped for running too
/// long, or wrote too much.
#[derive(Debug)]
pub(crate) struct ExecError {
    /// The `message` of the error Node rejects with.
    pub message: String,
    /// What the command wrote to stderr, up to the limit.
    pub stderr: String,
}

impl ExecError {
    /// What the app showed of such an error: stderr when there is any, else
    /// the message, cut to 300 characters.
    pub(crate) fn detail(&self) -> String {
        let detail = if !self.stderr.is_empty() {
            &self.stderr
        } else if !self.message.is_empty() {
            &self.message
        } else {
            "Unknown error"
        };
        let detail = utf16_prefix(js_trim(detail), 300);
        if detail.is_empty() {
            "Unknown error".to_owned()
        } else {
            detail.to_owned()
        }
    }
}

type Output = Arc<Mutex<Vec<u8>>>;

fn take(output: &Output) -> Vec<u8> {
    std::mem::take(
        &mut *output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// Keep what the command writes, up to `max_buffer` bytes, and report the
/// stream that went over.
async fn collect<R: AsyncRead + Unpin>(
    stream: Option<R>,
    name: &'static str,
    max_buffer: usize,
    output: Output,
    overflow: mpsc::Sender<&'static str>,
) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut chunk = vec![0_u8; 16 * 1024];
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let overflowed = {
            let mut output = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let room = max_buffer - output.len();
            output.extend_from_slice(&chunk[..read.min(room)]);
            read > room
        };
        if overflowed {
            let _ = overflow.send(name).await;
            return;
        }
    }
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
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
fn terminate(child: &mut Child) {
    let _ = child.start_kill();
}

/// `execFile(file, args, { cwd, env, timeout, maxBuffer, encoding: 'utf8' })`.
pub(crate) async fn exec_file(
    file: &Path,
    args: &[String],
    options: ExecOptions<'_>,
) -> Result<(), ExecError> {
    let mut command = Command::new(file);
    command
        .args(args)
        .current_dir(options.cwd)
        .env_clear()
        .envs(options.env)
        // Node hands the command a pipe it never writes to, and so never closes.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| ExecError {
        message: format!("spawn {} {}", file.display(), spawn_error_name(&error)),
        stderr: String::new(),
    })?;

    let stdout: Output = Arc::default();
    let stderr: Output = Arc::default();
    let (overflow, mut overflowed) = mpsc::channel(2);
    let mut reading_stdout = tokio::spawn(collect(
        child.stdout.take(),
        "stdout",
        options.max_buffer,
        stdout,
        overflow.clone(),
    ));
    let mut reading_stderr = tokio::spawn(collect(
        child.stderr.take(),
        "stderr",
        options.max_buffer,
        stderr.clone(),
        overflow,
    ));

    let (exited, mut too_much) = tokio::select! {
        // The command has ended once it exited and its output was read to the end.
        status = async {
            let status = child.wait().await;
            let _ = tokio::join!(&mut reading_stdout, &mut reading_stderr);
            status
        } => (Some(status), None),
        Some(stream) = overflowed.recv() => (None, Some(stream)),
        () = tokio::time::sleep(options.timeout) => (None, None),
    };
    let status = match exited {
        Some(status) => {
            // Going over the limit is an error even when the command then exits by itself.
            too_much = overflowed.try_recv().ok();
            status
        }
        // Out of time, or over the limit. What it writes from here on is not kept.
        None => {
            reading_stdout.abort();
            reading_stderr.abort();
            terminate(&mut child);
            match tokio::time::timeout(TERMINATE_GRACE, child.wait()).await {
                Ok(status) => status,
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        }
    };
    // As for Node, a command that was told to stop and still exits with zero has succeeded.
    if too_much.is_none() && matches!(&status, Ok(status) if status.success()) {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&take(&stderr)).into_owned();
    let message = match (too_much, status) {
        (Some(stream), _) => format!("{stream} maxBuffer length exceeded"),
        (None, Err(error)) => error.to_string(),
        (None, Ok(_)) => {
            let mut command = file.display().to_string();
            for argument in args {
                command.push(' ');
                command.push_str(argument);
            }
            format!("Command failed: {command}\n{stderr}")
        }
    };
    Err(ExecError { message, stderr })
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    fn script(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("command");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn run(
        directory: &Path,
        body: &str,
        timeout: Duration,
        max_buffer: usize,
    ) -> Result<(), ExecError> {
        let file = script(directory, body);
        let env = HashMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
        exec_file(
            &file,
            &["cache".to_owned(), "--reload".to_owned()],
            ExecOptions {
                cwd: directory,
                env: &env,
                timeout,
                max_buffer,
            },
        )
        .await
    }

    const MINUTE: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn succeeds_when_the_command_exits_with_zero() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            run(directory.path(), "echo out; echo err >&2", MINUTE, 1024)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn reports_what_a_failed_command_wrote_to_stderr() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(
            directory.path(),
            "echo '  error: no such package  ' >&2; exit 1",
            MINUTE,
            1024,
        )
        .await
        .unwrap_err();

        let command = directory.path().join("command");
        assert_eq!(error.stderr, "  error: no such package  \n");
        assert_eq!(
            error.message,
            format!(
                "Command failed: {} cache --reload\n  error: no such package  \n",
                command.display()
            )
        );
        assert_eq!(error.detail(), "error: no such package");
    }

    #[tokio::test]
    async fn names_the_command_when_it_failed_without_a_word() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(directory.path(), "exit 3", MINUTE, 1024)
            .await
            .unwrap_err();
        assert_eq!(
            error.detail(),
            format!(
                "Command failed: {} cache --reload",
                directory.path().join("command").display()
            )
        );
    }

    #[tokio::test]
    async fn cuts_the_detail_to_300_characters() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(
            directory.path(),
            "head -c 2000 /dev/zero | tr '\\0' e >&2; exit 1",
            MINUTE,
            4096,
        )
        .await
        .unwrap_err();
        assert_eq!(error.detail(), "e".repeat(300));
    }

    #[tokio::test]
    async fn stops_a_command_that_runs_out_of_time() {
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let error = run(
            directory.path(),
            "echo 'Download registry' >&2; exec sleep 30",
            // Long enough for the shell to start and write on a busy machine.
            Duration::from_secs(2),
            1024,
        )
        .await
        .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.detail(), "Download registry");
        assert!(error.message.starts_with("Command failed: "));
    }

    #[tokio::test]
    async fn kills_a_command_that_ignores_being_told_to_stop() {
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let error = run(
            directory.path(),
            "trap '' TERM; echo waiting >&2; exec sleep 30",
            // Long enough for the shell to get to its trap.
            Duration::from_millis(700),
            1024,
        )
        .await
        .unwrap_err();

        assert!(started.elapsed() >= TERMINATE_GRACE);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.detail(), "waiting");
    }

    #[tokio::test]
    async fn takes_an_exit_with_zero_for_a_success_even_after_the_time_limit() {
        let directory = tempfile::tempdir().unwrap();
        let outcome = run(
            directory.path(),
            "trap 'kill $waiting; exit 0' TERM; sleep 30 & waiting=$!; wait",
            Duration::from_millis(700),
            1024,
        )
        .await;
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[tokio::test]
    async fn stops_a_command_that_writes_more_than_the_buffer_holds() {
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let error = run(
            directory.path(),
            "head -c 100000 /dev/zero | tr '\\0' x >&2; exec sleep 30",
            MINUTE,
            1024,
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.message, "stderr maxBuffer length exceeded");
        assert_eq!(error.stderr.len(), 1024);
        assert_eq!(error.detail(), "x".repeat(300));

        let error = run(
            directory.path(),
            "head -c 100000 /dev/zero | tr '\\0' x; exec sleep 30",
            MINUTE,
            1024,
        )
        .await
        .unwrap_err();
        assert_eq!(error.detail(), "stdout maxBuffer length exceeded");
    }

    #[tokio::test]
    async fn reads_as_node_when_the_command_cannot_be_started() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing").join("deno");
        let error = exec_file(
            &missing,
            &[],
            ExecOptions {
                cwd: directory.path(),
                env: &HashMap::new(),
                timeout: MINUTE,
                max_buffer: 1024,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.detail(),
            format!("spawn {} ENOENT", missing.display())
        );
    }

    #[test]
    fn falls_back_when_there_is_nothing_to_show() {
        let blank = ExecError {
            message: "Command failed: deno".to_owned(),
            stderr: " \n".to_owned(),
        };
        // As in the Node app: stderr that is only white space hides the message.
        assert_eq!(blank.detail(), "Unknown error");
        let empty = ExecError {
            message: String::new(),
            stderr: String::new(),
        };
        assert_eq!(empty.detail(), "Unknown error");
    }
}
