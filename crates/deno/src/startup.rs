//! Why an npm MCP did not start: what Deno wrote to stderr, redacted.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mymcps_core::crypto::Encryption;
use mymcps_core::models::Mcp;
use mymcps_core::redaction::sanitize_mcp_diagnostic_with;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::JoinHandle;

use crate::error::DenoError;
use crate::text::{collapse_whitespace, strip_vt_control_characters, utf16_len, utf16_suffix};

/// stderr kept while an npm MCP starts, to explain a failed start.
pub const STARTUP_STDERR_LIMIT_BYTES: usize = 32 * 1024;
const STARTUP_STDERR_TAIL_CHARS: usize = 300;
/// How long a failed start waits for the last stderr chunks of an exited child.
const STARTUP_STDERR_SETTLE: Duration = Duration::from_millis(250);

/// What a process wrote to stderr while it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupStderr {
    pub text: String,
    /// `false` when the process wrote more than
    /// [`STARTUP_STDERR_LIMIT_BYTES`]: nothing of it was kept.
    pub complete: bool,
}

struct Captured {
    bytes: Vec<u8>,
    size: usize,
    complete: bool,
    capturing: bool,
}

/// Read a child's stderr so it never blocks on a full pipe, keeping the output
/// of its start. Output past the limit is dropped and marks the capture as
/// incomplete: a cut can split a secret, which could then not be redacted.
pub(crate) struct StartupStderrCapture {
    captured: Arc<Mutex<Captured>>,
    reader: Option<JoinHandle<()>>,
}

fn lock(captured: &Mutex<Captured>) -> MutexGuard<'_, Captured> {
    captured
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl StartupStderrCapture {
    pub(crate) fn start<R>(stream: Option<R>) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let captured = Arc::new(Mutex::new(Captured {
            bytes: Vec::new(),
            size: 0,
            complete: true,
            capturing: true,
        }));
        let reader = stream.map(|mut stream| {
            let captured = captured.clone();
            // Runs until the child closes its stderr, whatever becomes of the capture.
            tokio::spawn(async move {
                let mut chunk = vec![0_u8; 16 * 1024];
                loop {
                    let read = match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    let mut captured = lock(&captured);
                    if !captured.capturing {
                        continue;
                    }
                    captured.size += read;
                    if captured.size > STARTUP_STDERR_LIMIT_BYTES {
                        captured.complete = false;
                        captured.capturing = false;
                        captured.bytes = Vec::new();
                        continue;
                    }
                    captured.bytes.extend_from_slice(&chunk[..read]);
                }
            })
        });
        Self { captured, reader }
    }

    /// The start succeeded: keep draining, stop keeping.
    pub(crate) fn discard(self) {
        let mut captured = lock(&self.captured);
        captured.capturing = false;
        captured.bytes = Vec::new();
    }

    /// What the child wrote, once it has exited or after a short wait.
    pub(crate) async fn read(mut self) -> StartupStderr {
        if let Some(reader) = self.reader.as_mut() {
            let _ = tokio::time::timeout(STARTUP_STDERR_SETTLE, reader).await;
        }
        let captured = lock(&self.captured);
        StartupStderr {
            text: String::from_utf8_lossy(&captured.bytes).into_owned(),
            complete: captured.complete,
        }
    }
}

fn startup_output(
    encryption: &Encryption,
    mcp: &Mcp,
    stderr: Option<&StartupStderr>,
) -> Option<String> {
    let stderr = stderr?;
    if !stderr.complete {
        return Some(format!(
            "Output exceeded {} KiB and is not shown.",
            STARTUP_STDERR_LIMIT_BYTES / 1024
        ));
    }
    // Redact first, on the output as written: joining lines or keeping only the
    // end would otherwise break up a secret before it is recognized.
    let redacted =
        sanitize_mcp_diagnostic_with(encryption, &stderr.text, mcp, utf16_len(&stderr.text), []);
    let text = collapse_whitespace(&strip_vt_control_characters(&redacted));
    if text.is_empty() {
        return None;
    }
    Some(if utf16_len(&text) > STARTUP_STDERR_TAIL_CHARS {
        format!(
            "Output: …{}",
            utf16_suffix(&text, STARTUP_STDERR_TAIL_CHARS)
        )
    } else {
        format!("Output: {text}")
    })
}

/// The error of a failed start. `error` is the message of what the MCP client
/// failed with (`spawn /usr/local/bin/deno ENOENT`, `MCP error -32000:
/// Connection closed`...); `stderr` is what the process wrote, when it ran.
pub(crate) fn create_startup_error(
    encryption: &Encryption,
    mcp: &Mcp,
    error: &str,
    stderr: Option<&StartupStderr>,
) -> DenoError {
    // An MCP without a package never gets this far; `null` is what the Node app printed.
    let package = mcp.npm_package.as_deref().unwrap_or("null");
    let Some(output) = startup_output(encryption, mcp, stderr) else {
        let detail = sanitize_mcp_diagnostic_with(encryption, error, mcp, 300, []);
        return DenoError::Startup(format!(
            "Failed to start Deno npm MCP \"{package}\". Is Deno installed? {detail}"
        ));
    };

    // Deno ran and said why, so its output gets the room.
    let detail = sanitize_mcp_diagnostic_with(encryption, error, mcp, 80, []);
    DenoError::Startup(format!(
        "Failed to start Deno npm MCP \"{package}\". {detail}. {output}"
    ))
}

#[cfg(test)]
mod tests {
    use mymcps_core::config::TEST_APP_KEY;
    use mymcps_core::secrets::{EnvironmentInput, merge_environment};
    use tokio::io::AsyncWriteExt;

    use super::*;

    fn encryption() -> Encryption {
        Encryption::new(TEST_APP_KEY)
    }

    fn npm_mcp(encryption: &Encryption, environment: &[(&str, &str)]) -> Mcp {
        let entries: Vec<EnvironmentInput> = environment
            .iter()
            .map(|(name, value)| EnvironmentInput {
                name: (*name).to_owned(),
                value: Some((*value).to_owned()),
            })
            .collect();
        Mcp {
            npm_package: Some("@example/failing-mcp".to_owned()),
            npm_env: merge_environment(encryption, None, &entries),
            ..Default::default()
        }
    }

    fn stderr(text: &str) -> StartupStderr {
        StartupStderr {
            text: text.to_owned(),
            complete: true,
        }
    }

    // tests/unit/security.spec.ts
    #[test]
    fn redacts_long_npm_environment_secrets_before_truncating_startup_errors() {
        let encryption = encryption();
        let secret = format!("opaque-{}-tail", "x".repeat(400));
        let mcp = npm_mcp(&encryption, &[("OPAQUE_VALUE", &secret)]);

        let error =
            create_startup_error(&encryption, &mcp, &format!("startup echoed {secret}"), None);

        let message = error.to_string();
        assert!(message.contains("[REDACTED]"), "{message}");
        assert!(!message.contains(&secret));
        assert!(!message.contains(&secret[..300]));
        assert!(error.is_startup_failure());
        assert_eq!(
            message,
            "Failed to start Deno npm MCP \"@example/failing-mcp\". Is Deno installed? startup echoed [REDACTED]"
        );
    }

    #[test]
    fn asks_whether_deno_is_installed_only_when_deno_said_nothing() {
        let encryption = encryption();
        let mcp = npm_mcp(&encryption, &[]);

        for silent in [None, Some(stderr("")), Some(stderr(" \n\t\u{1B}[0m\n"))] {
            assert_eq!(
                create_startup_error(
                    &encryption,
                    &mcp,
                    "spawn /usr/local/bin/deno ENOENT",
                    silent.as_ref()
                )
                .to_string(),
                "Failed to start Deno npm MCP \"@example/failing-mcp\". Is Deno installed? spawn /usr/local/bin/deno ENOENT"
            );
        }

        assert_eq!(
            create_startup_error(
                &encryption,
                &mcp,
                "MCP error -32000: Connection closed",
                Some(&stderr(
                    "\u{1B}[0m\u{1B}[1m\u{1B}[31merror\u{1B}[0m: npm package '@example/failing-mcp' does not exist.\n"
                ))
            )
            .to_string(),
            "Failed to start Deno npm MCP \"@example/failing-mcp\". MCP error -32000: Connection closed. Output: error: npm package '@example/failing-mcp' does not exist."
        );
    }

    #[test]
    fn gives_the_room_to_the_end_of_what_deno_wrote() {
        let encryption = encryption();
        let mcp = npm_mcp(
            &encryption,
            &[("API_KEY", "super-secret-api-key\nsecond-line-of-key")],
        );
        let long_error = format!("MCP error -32001: Request timed out {}", "e".repeat(200));
        let output = format!(
            "Download https://registry.example/a\n{}\nerror: the key super-secret-api-key\nsecond-line-of-key was refused\n",
            "progress ".repeat(60)
        );

        let message = create_startup_error(&encryption, &mcp, &long_error, Some(&stderr(&output)))
            .to_string();

        let (head, tail) = message.split_once(". Output: …").expect(&message);
        // The error of the client keeps 80 characters, the output its last 300.
        assert_eq!(
            head,
            format!(
                "Failed to start Deno npm MCP \"@example/failing-mcp\". {}",
                &long_error[..80]
            )
        );
        assert_eq!(tail.chars().count(), 300);
        assert!(
            tail.ends_with("error: the key [REDACTED] was refused"),
            "{tail}"
        );
        assert!(!message.contains("super-secret-api-key"));
        assert!(!message.contains("second-line-of-key"));
        assert!(!message.contains("Is Deno installed?"));
    }

    #[test]
    fn does_not_quote_an_incomplete_capture() {
        let encryption = encryption();
        let mcp = npm_mcp(&encryption, &[]);
        let incomplete = StartupStderr {
            text: String::new(),
            complete: false,
        };
        assert_eq!(
            create_startup_error(
                &encryption,
                &mcp,
                "MCP error -32000: Connection closed",
                Some(&incomplete)
            )
            .to_string(),
            "Failed to start Deno npm MCP \"@example/failing-mcp\". MCP error -32000: Connection closed. Output exceeded 32 KiB and is not shown."
        );
    }

    #[tokio::test]
    async fn keeps_what_was_written_up_to_the_limit_and_nothing_beyond_it() {
        // Exactly the limit is kept whole.
        let (mut writer, reader) = tokio::io::duplex(1024);
        let capture = StartupStderrCapture::start(Some(reader));
        let written = tokio::spawn(async move {
            writer
                .write_all(&vec![b'a'; STARTUP_STDERR_LIMIT_BYTES])
                .await
                .unwrap();
        });
        let kept = capture.read().await;
        written.await.unwrap();
        assert!(kept.complete);
        assert_eq!(kept.text.len(), STARTUP_STDERR_LIMIT_BYTES);

        // One byte more and nothing is.
        let (mut writer, reader) = tokio::io::duplex(1024);
        let capture = StartupStderrCapture::start(Some(reader));
        let written = tokio::spawn(async move {
            writer
                .write_all(&vec![b'a'; STARTUP_STDERR_LIMIT_BYTES + 1])
                .await
                .unwrap();
            // More than the limit again: the reader goes on reading.
            writer
                .write_all(&vec![b'b'; 4 * STARTUP_STDERR_LIMIT_BYTES])
                .await
                .unwrap();
        });
        let kept = capture.read().await;
        written.await.unwrap();
        assert_eq!(
            kept,
            StartupStderr {
                text: String::new(),
                complete: false
            }
        );
    }

    #[tokio::test]
    async fn waits_a_moment_for_a_stream_that_stays_open() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let capture = StartupStderrCapture::start(Some(reader));
        writer.write_all(b"still starting\n").await.unwrap();

        let started = tokio::time::Instant::now();
        let kept = capture.read().await;

        assert_eq!(kept, stderr("still starting\n"));
        assert!(started.elapsed() >= STARTUP_STDERR_SETTLE);
        drop(writer);

        // Without a stream there is nothing to wait for.
        let none = StartupStderrCapture::start(None::<tokio::io::DuplexStream>);
        assert_eq!(none.read().await, stderr(""));
    }

    #[tokio::test]
    async fn goes_on_draining_after_a_successful_start() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let capture = StartupStderrCapture::start(Some(reader));
        writer.write_all(b"listening\n").await.unwrap();
        capture.discard();

        // Far more than the pipe holds: this would wait forever on a reader that stopped.
        tokio::time::timeout(
            Duration::from_secs(5),
            writer.write_all(&vec![b'x'; 256 * 1024]),
        )
        .await
        .expect("stderr is still read")
        .unwrap();
    }
}
