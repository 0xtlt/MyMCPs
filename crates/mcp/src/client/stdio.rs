//! stdio, client side: the SDK's `StdioClientTransport`. The server is a
//! child process that reads JSON-RPC messages on stdin and writes them on
//! stdout, one per line.
//!
//! The child never outlives the transport: it is killed when the last handle
//! is dropped, closed or not.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::Instant;

use super::transport::{Transport, TransportError, TransportHandler};
use crate::json;
use crate::types::JsonRpcMessage;

/// The most a child may write without ending a line.
pub const STDIO_DEFAULT_MAX_BUFFER_SIZE: usize = 10 * 1024 * 1024;

/// How long the child gets to exit after its stdin closes, and again after SIGTERM.
const CLOSE_GRACE: Duration = Duration::from_millis(2000);

/// What may sit between the child's stderr and a reader that is behind.
const STDERR_PIPE_BYTES: usize = 64 * 1024;

/// Environment variables the child inherits whatever `env` says, as far as
/// the server itself has them. The list is inspired by what sudo keeps.
#[cfg(not(windows))]
pub const DEFAULT_INHERITED_ENV_VARS: &[&str] =
    &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];

#[cfg(windows)]
pub const DEFAULT_INHERITED_ENV_VARS: &[&str] = &[
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "PROGRAMFILES",
];

/// The variables of [`DEFAULT_INHERITED_ENV_VARS`] that this process has.
pub fn get_default_environment() -> HashMap<String, String> {
    DEFAULT_INHERITED_ENV_VARS
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_owned(), value))
        })
        // Exported shell functions are a security risk.
        .filter(|(_, value)| !value.starts_with("()"))
        .collect()
}

/// Where the child's stderr goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdioStderr {
    /// To the stderr of this process.
    #[default]
    Inherit,
    /// To [`StdioClientTransport::stderr`], which then has to be read: a
    /// child that writes more than a pipe holds waits for its reader.
    Pipe,
    Null,
}

#[derive(Debug, Clone)]
pub struct StdioServerParameters {
    /// Looked up in the `PATH` of the child's environment when it is not a path.
    pub command: OsString,
    pub args: Vec<OsString>,
    /// The child's environment, on top of [`get_default_environment`] unless
    /// `inherit_default_environment` is off. Nothing else is inherited.
    pub env: HashMap<String, String>,
    /// The SDK always merges its default environment in. Turn this off to
    /// start the child with `env` alone.
    pub inherit_default_environment: bool,
    pub stderr: StdioStderr,
    pub cwd: Option<PathBuf>,
    pub max_buffer_size: usize,
}

impl StdioServerParameters {
    pub fn new(command: impl Into<OsString>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            inherit_default_environment: true,
            stderr: StdioStderr::default(),
            cwd: None,
            max_buffer_size: STDIO_DEFAULT_MAX_BUFFER_SIZE,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn env<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env = env
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    pub fn stderr(mut self, stderr: StdioStderr) -> Self {
        self.stderr = stderr;
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// The child's stderr, for [`StdioStderr::Pipe`]. It ends when the child closes its stderr.
pub struct ChildStderr(DuplexStream);

impl AsyncRead for ChildStderr {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

enum Request {
    Close(oneshot::Sender<()>),
}

/// What the handles and the tasks of a running child both see.
struct Running {
    /// Lines for the child's stdin. Gone once the transport is closing or the child is.
    lines: Mutex<Option<mpsc::UnboundedSender<Bytes>>>,
    /// 0 when no child is running.
    pid: AtomicU32,
    /// The read buffer overflowed: the reader asks for the transport to close.
    overflow: Notify,
}

enum State {
    Idle,
    Started {
        running: Arc<Running>,
        /// Held by the handles only: when the last one goes, the supervisor
        /// sees the channel close and lets go of the child, which kills it.
        requests: mpsc::UnboundedSender<Request>,
    },
}

struct Inner {
    parameters: StdioServerParameters,
    state: Mutex<State>,
    stderr_reader: Mutex<Option<ChildStderr>>,
    stderr_writer: Mutex<Option<DuplexStream>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// See the module documentation. Clones share one child process.
#[derive(Clone)]
pub struct StdioClientTransport {
    inner: Arc<Inner>,
}

impl StdioClientTransport {
    /// Nothing is started yet: [`Transport::start`], which `Client::connect`
    /// calls, spawns the process.
    pub fn new(parameters: StdioServerParameters) -> Self {
        let (reader, writer) = match parameters.stderr {
            StdioStderr::Pipe => {
                let (reader, writer) = tokio::io::duplex(STDERR_PIPE_BYTES);
                (Some(ChildStderr(reader)), Some(writer))
            }
            StdioStderr::Inherit | StdioStderr::Null => (None, None),
        };
        Self {
            inner: Arc::new(Inner {
                parameters,
                state: Mutex::new(State::Idle),
                stderr_reader: Mutex::new(reader),
                stderr_writer: Mutex::new(writer),
            }),
        }
    }

    /// The child's stderr, once, when it was asked for with
    /// [`StdioStderr::Pipe`]. Available before the process starts, so that
    /// nothing it writes early is missed.
    pub fn stderr(&self) -> Option<ChildStderr> {
        lock(&self.inner.stderr_reader).take()
    }

    /// The process id of the child while it runs.
    pub fn pid(&self) -> Option<u32> {
        match &*lock(&self.inner.state) {
            State::Started { running, .. } => {
                Some(running.pid.load(Ordering::SeqCst)).filter(|pid| *pid != 0)
            }
            State::Idle => None,
        }
    }

    fn spawn(&self, handler: Arc<dyn TransportHandler>) -> Result<(), TransportError> {
        let parameters = &self.inner.parameters;
        let mut state = lock(&self.inner.state);
        if matches!(*state, State::Started { .. }) {
            return Err(TransportError::AlreadyStarted(
                "StdioClientTransport already started! If using Client class, note that connect() calls start() automatically.",
            ));
        }

        let mut command = Command::new(&parameters.command);
        command.args(&parameters.args).env_clear();
        if parameters.inherit_default_environment {
            command.envs(get_default_environment());
        }
        command
            .envs(&parameters.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match parameters.stderr {
                StdioStderr::Inherit => Stdio::inherit(),
                StdioStderr::Pipe => Stdio::piped(),
                StdioStderr::Null => Stdio::null(),
            })
            .kill_on_drop(true);
        if let Some(cwd) = &parameters.cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(|source| TransportError::Spawn {
            command: parameters.command.to_string_lossy().into_owned(),
            code: errno_name(&source),
            source,
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(TransportError::NotConnected);
        };

        let (lines, queued_lines) = mpsc::unbounded_channel();
        let running = Arc::new(Running {
            lines: Mutex::new(Some(lines)),
            pid: AtomicU32::new(child.id().unwrap_or_default()),
            overflow: Notify::new(),
        });
        let (requests, received_requests) = mpsc::unbounded_channel();

        tokio::spawn(write_lines(stdin, queued_lines, handler.clone()));
        let (stdout_closed, stdout_done) = oneshot::channel();
        tokio::spawn(read_messages(
            stdout,
            parameters.max_buffer_size,
            handler.clone(),
            running.clone(),
            stdout_closed,
        ));
        let stderr_done = match (child.stderr.take(), lock(&self.inner.stderr_writer).take()) {
            (Some(stderr), Some(writer)) => {
                let (closed, done) = oneshot::channel();
                tokio::spawn(forward_stderr(stderr, writer, closed));
                Some(done)
            }
            _ => None,
        };
        tokio::spawn(supervise(
            child,
            running.clone(),
            received_requests,
            stdout_done,
            stderr_done,
            handler,
        ));

        *state = State::Started { running, requests };
        Ok(())
    }
}

impl Transport for StdioClientTransport {
    fn start(
        &self,
        handler: Arc<dyn TransportHandler>,
    ) -> BoxFuture<'static, Result<(), TransportError>> {
        Box::pin(std::future::ready(self.spawn(handler)))
    }

    fn send(&self, message: JsonRpcMessage) -> BoxFuture<'static, Result<(), TransportError>> {
        let queued = match &*lock(&self.inner.state) {
            State::Started { running, .. } => match &*lock(&running.lines) {
                Some(lines) => {
                    let mut line = message.to_json();
                    line.push('\n');
                    lines
                        .send(Bytes::from(line))
                        .map_err(|_| TransportError::NotConnected)
                }
                None => Err(TransportError::NotConnected),
            },
            State::Idle => Err(TransportError::NotConnected),
        };
        Box::pin(std::future::ready(queued))
    }

    /// End the child as the SDK does: close its stdin and give it two seconds
    /// to exit, send SIGTERM and give it two more, then kill it.
    fn close(&self) -> BoxFuture<'static, ()> {
        let closed = match &*lock(&self.inner.state) {
            State::Started { requests, .. } => {
                let (done, closed) = oneshot::channel();
                requests.send(Request::Close(done)).ok().map(|()| closed)
            }
            State::Idle => None,
        };
        Box::pin(async move {
            if let Some(closed) = closed {
                // An error means the child was gone already.
                let _ = closed.await;
            }
        })
    }
}

async fn write_lines(
    mut stdin: ChildStdin,
    mut lines: mpsc::UnboundedReceiver<Bytes>,
    handler: Arc<dyn TransportHandler>,
) {
    while let Some(line) = lines.recv().await {
        if let Err(error) = stdin.write_all(&line).await {
            handler.on_error(&TransportError::Io(error));
            return;
        }
    }
    // Every handle to the queue is gone: what was queued is written, stdin closes.
    let _ = stdin.shutdown().await;
}

async fn read_messages(
    mut stdout: ChildStdout,
    max_buffer_size: usize,
    handler: Arc<dyn TransportHandler>,
    running: Arc<Running>,
    closed: oneshot::Sender<()>,
) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = match stdout.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                handler.on_error(&TransportError::Io(error));
                break;
            }
        };
        if buffer.len() + read > max_buffer_size {
            buffer.clear();
            handler.on_error(&TransportError::ReadBufferOverflow(max_buffer_size));
            running.overflow.notify_one();
            continue;
        }
        buffer.extend_from_slice(&chunk[..read]);

        let mut start = 0;
        while let Some(length) = buffer[start..].iter().position(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&buffer[start..start + length]);
            start += length + 1;
            let line = line.strip_suffix('\r').unwrap_or(&line);
            // A server that logs to stdout only costs these lines.
            let message = json::parse(line)
                .map_err(TransportError::InvalidJson)
                .and_then(|data| {
                    JsonRpcMessage::parse(&data).map_err(TransportError::InvalidMessage)
                });
            match message {
                Ok(message) => handler.on_message(message),
                Err(error) => handler.on_error(&error),
            }
        }
        buffer.drain(..start);
    }
    let _ = closed.send(());
}

/// Keep reading the child's stderr so that it never waits on a full pipe
/// because its reader went away.
async fn forward_stderr(
    mut stderr: tokio::process::ChildStderr,
    writer: DuplexStream,
    closed: oneshot::Sender<()>,
) {
    let mut writer = Some(writer);
    let mut chunk = vec![0_u8; 16 * 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Some(open) = writer.as_mut()
                    && open.write_all(&chunk[..read]).await.is_err()
                {
                    writer = None;
                }
            }
        }
    }
    drop(writer);
    let _ = closed.send(());
}

enum Closing {
    /// stdin was closed; the child has until the deadline to exit.
    AfterStdin(Instant),
    /// SIGTERM was sent; the child has until the deadline to exit.
    AfterTerminate(Instant),
    /// Everything `close()` does has been done.
    Done,
}

/// Owns the child. Ends when the child has exited and its output is read to
/// the end, which is when Node emits `close`, or when the transport is dropped.
async fn supervise(
    mut child: Child,
    running: Arc<Running>,
    mut requests: mpsc::UnboundedReceiver<Request>,
    stdout_done: oneshot::Receiver<()>,
    stderr_done: Option<oneshot::Receiver<()>>,
    handler: Arc<dyn TransportHandler>,
) {
    let mut exited = false;
    let mut output_read = false;
    let mut closing: Option<Closing> = None;
    let mut waiting: Vec<oneshot::Sender<()>> = Vec::new();
    let output = async move {
        let _ = stdout_done.await;
        if let Some(stderr_done) = stderr_done {
            let _ = stderr_done.await;
        }
    };
    tokio::pin!(output);

    while !(exited && output_read) {
        let deadline = match closing {
            Some(Closing::AfterStdin(deadline) | Closing::AfterTerminate(deadline)) => {
                Some(deadline)
            }
            _ => None,
        };
        let begin_closing = tokio::select! {
            _ = child.wait(), if !exited => {
                exited = true;
                false
            }
            () = &mut output, if !output_read => {
                output_read = true;
                false
            }
            request = requests.recv() => match request {
                Some(Request::Close(done)) => {
                    if matches!(closing, Some(Closing::Done)) {
                        let _ = done.send(());
                    } else {
                        waiting.push(done);
                    }
                    true
                }
                // The last handle was dropped: dropping the child kills it.
                None => return,
            },
            () = running.overflow.notified() => true,
            () = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                closing = match closing {
                    Some(Closing::AfterStdin(_)) if !exited => {
                        terminate(&child);
                        Some(Closing::AfterTerminate(Instant::now() + CLOSE_GRACE))
                    }
                    Some(Closing::AfterTerminate(_)) if !exited => {
                        let _ = child.start_kill();
                        Some(Closing::Done)
                    }
                    _ => Some(Closing::Done),
                };
                if matches!(closing, Some(Closing::Done)) {
                    for done in waiting.drain(..) {
                        let _ = done.send(());
                    }
                }
                false
            }
        };
        if begin_closing && closing.is_none() {
            // From here on the transport is "not connected"; what was queued is still written.
            lock(&running.lines).take();
            closing = Some(Closing::AfterStdin(Instant::now() + CLOSE_GRACE));
        }
    }

    lock(&running.lines).take();
    running.pid.store(0, Ordering::SeqCst);
    handler.on_close();
    for done in waiting {
        let _ = done.send(());
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

/// The name Node gives a spawn failure, as in `spawn deno ENOENT`.
fn errno_name(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    let name = match error.kind() {
        ErrorKind::NotFound => "ENOENT",
        ErrorKind::PermissionDenied => "EACCES",
        ErrorKind::NotADirectory => "ENOTDIR",
        ErrorKind::IsADirectory => "EISDIR",
        ErrorKind::ArgumentListTooLong => "E2BIG",
        ErrorKind::OutOfMemory => "ENOMEM",
        ErrorKind::InvalidFilename => "ENAMETOOLONG",
        ErrorKind::ExecutableFileBusy => "ETXTBSY",
        ErrorKind::WouldBlock => "EAGAIN",
        _ => return error.to_string(),
    };
    name.to_owned()
}
