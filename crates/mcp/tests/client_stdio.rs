//! The client over stdio, with small shell scripts as MCP servers.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mymcps_mcp::client::{
    Client, ClientError, ClientOptions, StdioClientTransport, StdioServerParameters, StdioStderr,
    Transport, TransportError, TransportHandler, get_default_environment,
};
use mymcps_mcp::{Implementation, JsonRpcMessage, json};
use serde_json::json;
use tokio::io::AsyncReadExt;

/// An MCP server in POSIX shell. It keeps what it reads in `stdin.log`, logs
/// a line that is not JSON-RPC, and describes its own process in its one tool.
const SERVER: &str = r#"
echo "starting up, this line is not JSON-RPC"
while IFS= read -r line; do
  printf '%s\n' "$line" >> stdin.log
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"sh","version":"1"}}}\r\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"describe","description":"%s","inputSchema":{"type":"object"},"vendor":1}]}}\n' "$id" "$PWD|${GREETING-unset}|${CARGO_MANIFEST_DIR-unset}|${HOME-unset}" ;;
    *'"method":"tools/call"'*)
      printf '\n{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"called","extra":true}]}}\n' "$id" ;;
  esac
done
"#;

fn shell(script: &str) -> StdioServerParameters {
    StdioServerParameters::new("/bin/sh").args(["-c", script])
}

fn client() -> Client {
    Client::new(Implementation::new("mymcps-gateway", "0.4.1"))
}

fn is_running(pid: u32) -> bool {
    // `kill` as the shell has it built in: a minimal system, such as a slim
    // container image, has no `kill` program of its own.
    Command::new("/bin/sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_until_gone(pid: u32) {
    let started = Instant::now();
    while is_running(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "process {pid} is still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Collects what a transport reports, for the tests that use one without a client.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
}

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl TransportHandler for Recorder {
    fn on_message(&self, message: JsonRpcMessage) {
        self.events
            .lock()
            .unwrap()
            .push(format!("message {}", message.to_json()));
    }

    fn on_error(&self, error: &TransportError) {
        self.events.lock().unwrap().push(format!("error {error}"));
    }

    fn on_close(&self) {
        self.events.lock().unwrap().push("close".to_owned());
    }
}

#[tokio::test]
async fn talks_to_a_child_process_in_its_own_directory_and_environment() {
    let directory = tempfile::tempdir().unwrap();
    let transport = StdioClientTransport::new(
        shell(SERVER)
            .cwd(directory.path())
            .env([("GREETING", "hello")])
            .stderr(StdioStderr::Null),
    );
    assert_eq!(transport.pid(), None);
    let client = client();
    client.connect(transport.clone()).await.unwrap();
    let pid = transport.pid().unwrap();
    assert!(is_running(pid));
    assert_eq!(
        client.server_version(),
        Some(json!({ "name": "sh", "version": "1" }))
    );

    let listed = client.list_tools().await.unwrap();
    assert_eq!(listed.tools.len(), 1);
    let tool = &listed.tools[0];
    assert_eq!(tool.name(), "describe");
    assert_eq!(tool.input_schema(), &json!({ "type": "object" }));
    // Only what the protocol defines for a tool is kept.
    assert_eq!(tool.get("vendor"), None);

    let described: Vec<&str> = tool.description().unwrap().split('|').collect();
    let directory_path = directory.path().canonicalize().unwrap();
    assert_eq!(
        Path::new(described[0]).canonicalize().unwrap(),
        directory_path
    );
    assert_eq!(described[1], "hello");
    // Set for this test process by cargo, and not handed down.
    assert!(std::env::var("CARGO_MANIFEST_DIR").is_ok());
    assert_eq!(described[2], "unset");
    // The few variables the SDK always hands down.
    assert_eq!(
        Some(described[3]),
        get_default_environment().get("HOME").map(String::as_str)
    );

    let result = client
        .call_tool("describe", Some(serde_json::Map::new()))
        .await
        .unwrap();
    assert_eq!(
        json::to_string(&result),
        r#"{"content":[{"type":"text","text":"called"}]}"#
    );

    let started = Instant::now();
    client.close().await;
    transport.close().await;
    // The server exits when its stdin closes: no signal was needed.
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(transport.pid(), None);
    assert!(!is_running(pid));
    assert!(matches!(
        client.list_tools().await,
        Err(ClientError::NotConnected)
    ));

    // One line per message, written as the SDK writes them.
    let received = std::fs::read_to_string(directory.path().join("stdin.log")).unwrap();
    assert_eq!(
        received.lines().collect::<Vec<_>>(),
        vec![
            r#"{"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"mymcps-gateway","version":"0.4.1"}},"jsonrpc":"2.0","id":0}"#,
            r#"{"method":"notifications/initialized","jsonrpc":"2.0"}"#,
            r#"{"method":"tools/list","jsonrpc":"2.0","id":1}"#,
            r#"{"method":"tools/call","params":{"name":"describe","arguments":{}},"jsonrpc":"2.0","id":2}"#,
        ]
    );
}

#[tokio::test]
async fn starts_the_child_with_nothing_inherited_when_asked() {
    let directory = tempfile::tempdir().unwrap();
    let mut parameters = shell(SERVER)
        .cwd(directory.path())
        .stderr(StdioStderr::Null);
    parameters.inherit_default_environment = false;
    let transport = StdioClientTransport::new(parameters);
    let client = client();
    client.connect(transport.clone()).await.unwrap();

    let listed = client.list_tools().await.unwrap();
    let described: Vec<&str> = listed.tools[0].description().unwrap().split('|').collect();
    assert_eq!(&described[1..], ["unset", "unset", "unset"]);
    client.close().await;
}

#[tokio::test]
async fn reports_a_command_that_cannot_be_started_the_way_node_does() {
    let transport = StdioClientTransport::new(StdioServerParameters::new("/nonexistent/bin/deno"));
    let error = client().connect(transport.clone()).await.unwrap_err();
    assert_eq!(error.to_string(), "spawn /nonexistent/bin/deno ENOENT");
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::Spawn { .. })
    ));
    assert_eq!(transport.pid(), None);

    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("not-executable");
    std::fs::write(&script, "#!/bin/sh\n").unwrap();
    let error = client()
        .connect(StdioClientTransport::new(StdioServerParameters::new(
            &script,
        )))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("spawn {} EACCES", script.display())
    );

    let missing = directory.path().join("missing");
    let error = client()
        .connect(StdioClientTransport::new(shell("exit 0").cwd(&missing)))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "spawn /bin/sh ENOENT");
}

#[tokio::test]
async fn hands_over_what_a_failed_start_wrote_to_stderr() {
    let transport = StdioClientTransport::new(
        shell("echo 'error: cannot start, the key was refused' >&2; exit 3")
            .stderr(StdioStderr::Pipe),
    );
    let mut stderr = transport.stderr().unwrap();
    // Taken once.
    assert!(transport.stderr().is_none());

    let error = client().connect(transport.clone()).await.unwrap_err();
    // The child is gone before it answered `initialize`.
    assert_eq!(error.to_string(), "MCP error -32000: Connection closed");
    assert_eq!(error.mcp_code(), Some(-32000));

    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(2), stderr.read_to_string(&mut output))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output, "error: cannot start, the key was refused\n");
}

#[tokio::test]
async fn keeps_a_chatty_child_running_while_its_stderr_is_read() {
    // 512 KiB before the first MCP message: more than a pipe holds.
    let script = format!("head -c 524288 /dev/zero | tr '\\0' 'e' >&2\n{SERVER}");
    let directory = tempfile::tempdir().unwrap();
    let transport = StdioClientTransport::new(
        shell(&script)
            .cwd(directory.path())
            .stderr(StdioStderr::Pipe),
    );
    let mut stderr = transport.stderr().unwrap();
    let drained = tokio::spawn(async move {
        let mut total = 0;
        let mut chunk = vec![0_u8; 8192];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => return total,
                Ok(read) => total += read,
            }
        }
    });

    let client = client();
    tokio::time::timeout(Duration::from_secs(10), client.connect(transport.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(client.list_tools().await.unwrap().tools.len(), 1);
    client.close().await;
    // The stream ends with the child.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), drained)
            .await
            .unwrap()
            .unwrap(),
        524_288
    );
}

#[tokio::test]
async fn keeps_a_chatty_child_running_when_its_stderr_reader_is_dropped() {
    let script = format!("head -c 524288 /dev/zero | tr '\\0' 'e' >&2\n{SERVER}");
    let directory = tempfile::tempdir().unwrap();
    let transport = StdioClientTransport::new(
        shell(&script)
            .cwd(directory.path())
            .stderr(StdioStderr::Pipe),
    );
    drop(transport.stderr());

    let client = client();
    tokio::time::timeout(Duration::from_secs(10), client.connect(transport.clone()))
        .await
        .unwrap()
        .unwrap();
    client.close().await;
}

#[tokio::test]
async fn kills_the_child_when_the_transport_is_dropped() {
    let transport = StdioClientTransport::new(shell("exec sleep 300").stderr(StdioStderr::Null));
    transport
        .start(Arc::new(Recorder::default()))
        .await
        .unwrap();
    let pid = transport.pid().unwrap();
    assert!(is_running(pid));

    drop(transport);
    wait_until_gone(pid).await;
}

#[tokio::test]
async fn kills_the_child_when_a_connected_client_is_dropped() {
    let directory = tempfile::tempdir().unwrap();
    // After the handshake it stops reading, so closing its stdin would not end it.
    let script = format!("{SERVER}\nexec sleep 300");
    let transport = StdioClientTransport::new(
        shell(&script.replace(
            "    *'\"method\":\"tools/list\"'*)",
            "    *'\"method\":\"tools/list\"'*) break ;;\n    *nothing*)",
        ))
        .cwd(directory.path())
        .stderr(StdioStderr::Null),
    );
    let client = client();
    client.connect(transport.clone()).await.unwrap();
    let pid = transport.pid().unwrap();

    // The request is abandoned along with the client and the transport.
    let waiting = tokio::spawn(async move { client.list_tools().await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(is_running(pid));
    waiting.abort();
    let _ = waiting.await;
    drop(transport);
    wait_until_gone(pid).await;
}

#[tokio::test]
async fn sends_sigterm_two_seconds_after_closing_stdin() {
    // Never reads stdin, and leaves on SIGTERM.
    let script = "trap 'kill $child; exit 0' TERM; sleep 300 & child=$!; wait";
    let transport = StdioClientTransport::new(shell(script).stderr(StdioStderr::Null));
    let recorder = Arc::new(Recorder::default());
    transport.start(recorder.clone()).await.unwrap();
    let pid = transport.pid().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    transport.close().await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1900),
        "closed after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(3500),
        "closed after {elapsed:?}"
    );
    wait_until_gone(pid).await;
    assert_eq!(recorder.events(), vec!["close"]);
    // Closing again has nothing left to do.
    tokio::time::timeout(Duration::from_millis(200), transport.close())
        .await
        .unwrap();
}

#[tokio::test]
async fn kills_a_child_that_ignores_sigterm_after_two_more_seconds() {
    let script = "trap '' TERM; sleep 300 & wait";
    let transport = StdioClientTransport::new(shell(script).stderr(StdioStderr::Null));
    transport
        .start(Arc::new(Recorder::default()))
        .await
        .unwrap();
    let pid = transport.pid().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    transport.close().await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(3900),
        "closed after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(5500),
        "closed after {elapsed:?}"
    );
    wait_until_gone(pid).await;
}

#[tokio::test]
async fn refuses_to_send_before_the_start_and_after_the_close() {
    let ping = || JsonRpcMessage::Request {
        id: json!(1),
        method: "ping".to_owned(),
        params: None,
    };
    let transport = StdioClientTransport::new(shell("cat > /dev/null").stderr(StdioStderr::Null));
    assert!(matches!(
        transport.send(ping()).await,
        Err(TransportError::NotConnected)
    ));

    let recorder = Arc::new(Recorder::default());
    transport.start(recorder.clone()).await.unwrap();
    transport.send(ping()).await.unwrap();
    let again = transport.start(recorder.clone()).await.unwrap_err();
    assert_eq!(
        again.to_string(),
        "StdioClientTransport already started! If using Client class, note that connect() calls start() automatically."
    );

    transport.close().await;
    assert!(matches!(
        transport.send(ping()).await,
        Err(TransportError::NotConnected)
    ));
    assert_eq!(recorder.events(), vec!["close"]);
}

#[tokio::test]
async fn writes_what_was_queued_before_closing_stdin() {
    let directory = tempfile::tempdir().unwrap();
    let transport = StdioClientTransport::new(
        shell("cat > received.log")
            .cwd(directory.path())
            .stderr(StdioStderr::Null),
    );
    transport
        .start(Arc::new(Recorder::default()))
        .await
        .unwrap();
    // Queued at once, in this order, whenever the futures are polled.
    let cancelled = transport.send(JsonRpcMessage::Notification {
        method: "notifications/cancelled".to_owned(),
        params: Some(
            json!({ "requestId": 4, "reason": "McpError: MCP error -32001: Request timed out" }),
        ),
    });
    let closed = transport.close();
    closed.await;
    cancelled.await.unwrap();

    assert_eq!(
        std::fs::read_to_string(directory.path().join("received.log")).unwrap(),
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":4,\"reason\":\"McpError: MCP error -32001: Request timed out\"}}\n"
    );
}

#[tokio::test]
async fn skips_lines_that_are_not_messages_and_reads_the_rest() {
    let script = r#"
printf 'plain log line\n\n{"hello":1}\n{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}\r\n'
printf '{"jsonrpc":"2.0","id":7,"res'
printf 'ult":{"ok":true}}\n{"jsonrpc":"2.0","id":8,"error":{"code":-1,"message":"no"}}\nunfinished'
"#;
    let transport = StdioClientTransport::new(shell(script).stderr(StdioStderr::Null));
    let recorder = Arc::new(Recorder::default());
    transport.start(recorder.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while recorder.events().last().map(String::as_str) != Some("close") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let events = recorder.events();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.split(' ').next().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "error", "error", "error", "message", "message", "message", "close"
        ]
    );
    assert_eq!(
        &events[3..6],
        [
            r#"message {"method":"notifications/message","params":{"level":"info"},"jsonrpc":"2.0"}"#,
            r#"message {"result":{"ok":true},"jsonrpc":"2.0","id":7}"#,
            r#"message {"jsonrpc":"2.0","id":8,"error":{"code":-1,"message":"no"}}"#,
        ]
    );
}

#[tokio::test]
async fn closes_when_the_child_writes_a_line_longer_than_the_read_buffer() {
    let mut parameters =
        shell("head -c 20000 /dev/zero | tr '\\0' 'x'; cat > /dev/null").stderr(StdioStderr::Null);
    parameters.max_buffer_size = 4096;
    let notices = Arc::new(Mutex::new(Vec::new()));
    let listener = notices.clone();
    let client = Client::with_options(
        Implementation::new("test-client", "1.0.0"),
        ClientOptions {
            on_error: Some(Arc::new(move |error: &TransportError| {
                listener.lock().unwrap().push(error.to_string());
            })),
            ..ClientOptions::default()
        },
    );

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(StdioClientTransport::new(parameters)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.to_string(), "MCP error -32000: Connection closed");
    assert!(
        notices
            .lock()
            .unwrap()
            .contains(&"ReadBuffer exceeded maximum size of 4096 bytes".to_owned()),
        "{notices:?}"
    );
}

#[tokio::test]
async fn times_out_a_request_the_child_never_answers() {
    let directory = tempfile::tempdir().unwrap();
    let script = SERVER.replace(
        "    *'\"method\":\"tools/call\"'*)",
        "    *'\"method\":\"tools/call\"'*) : ;;\n    *nothing*)",
    );
    let transport = StdioClientTransport::new(
        shell(&script)
            .cwd(directory.path())
            .stderr(StdioStderr::Null),
    );
    let client = Client::with_options(
        Implementation::new("test-client", "1.0.0"),
        ClientOptions {
            request_timeout: Duration::from_millis(200),
            ..ClientOptions::default()
        },
    );
    client.connect(transport.clone()).await.unwrap();

    let error = client.call_tool("describe", None).await.unwrap_err();
    assert_eq!(error.to_string(), "MCP error -32001: Request timed out");
    assert_eq!(error.mcp_code(), Some(-32001));
    client.close().await;

    // The cancellation was queued before the close, so the child still got it.
    let received = std::fs::read_to_string(directory.path().join("stdin.log")).unwrap();
    assert_eq!(
        received.lines().last().unwrap(),
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"reason":"McpError: MCP error -32001: Request timed out"}}"#
    );
}
