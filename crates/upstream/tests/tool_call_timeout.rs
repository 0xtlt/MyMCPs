//! How long a tool call to an HTTP MCP waits: the configured tool-call wait,
//! restarted by the progress the MCP reports, where every other request keeps
//! the client's own. A real HTTP server, since the waits are real time.

mod support;

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpAuthType};
use mymcps_net::{AddressGuard, Fetcher};
use mymcps_upstream::Upstream;
use serde_json::{Value, json};
use support::*;

const TIMEOUT: Duration = Duration::from_millis(1000);

/// A core that waits `TIMEOUT` for a tool call.
async fn core() -> TestCore {
    TestCore::with_config(|config| {
        config.tool_call_timeout = TIMEOUT;
        config.tool_call_max_duration = Duration::from_secs(60);
    })
    .await
}

async fn http_mcp(core: &TestCore, url: String) -> (Mcp, std::sync::Arc<Upstream>) {
    let mcp = create_mcp(core, |mcp| {
        mcp.http_url = Some(url);
        mcp.auth_type = McpAuthType::Bearer;
    })
    .await;
    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .fetcher(Fetcher::shared())
        .address_guard(AddressGuard::system())
        .build();
    (mcp, upstream)
}

fn event(message: Value) -> String {
    format!("event: message\ndata: {message}\n\n")
}

fn event_stream(body: Body) -> axum::response::Response {
    respond(200, &[("Content-Type", "text/event-stream")], body)
}

fn token_of(message: &Value) -> Value {
    message["params"]["_meta"]["progressToken"].clone()
}

#[tokio::test]
async fn asks_for_progress_on_a_tool_call_only() {
    let core = core().await;
    let server = mcp_server(|message| async move {
        let result = if message["method"] == "tools/list" {
            json!({ "tools": [] })
        } else {
            json!({ "content": [] })
        };
        respond_json(
            200,
            json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
        )
    })
    .await;
    let (mut mcp, upstream) = http_mcp(&core, server.url("/mcp")).await;

    upstream.list_http_tools(&mut mcp).await.unwrap();
    upstream.call_tool(&mut mcp, "echo", None).await.unwrap();

    let bodies: Vec<Value> = server
        .requests()
        .iter()
        .map(Received::json)
        .filter(|body| body["method"] == "tools/list" || body["method"] == "tools/call")
        .collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["method"], "tools/list");
    assert!(bodies[0]["params"].get("_meta").is_none(), "{}", bodies[0]);
    assert_eq!(bodies[1]["method"], "tools/call");
    assert_eq!(token_of(&bodies[1]), bodies[1]["id"]);
}

#[tokio::test]
async fn waits_on_while_the_mcp_reports_progress() {
    let core = core().await;
    let server = mcp_server(|message| async move {
        let (id, token) = (message["id"].clone(), token_of(&message));
        // Longer than the wait in all, with a sign of life more often than it.
        let events = futures::stream::unfold(0, move |step| {
            let (id, token) = (id.clone(), token.clone());
            async move {
                if step > 7 {
                    return None;
                }
                tokio::time::sleep(TIMEOUT / 4).await;
                let message = if step < 7 {
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": { "progressToken": token, "progress": step + 1 },
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [{ "type": "text", "text": "done" }] },
                    })
                };
                Some((Ok::<_, Infallible>(event(message)), step + 1))
            }
        });
        event_stream(Body::from_stream(events))
    })
    .await;
    let (mut mcp, upstream) = http_mcp(&core, server.url("/mcp")).await;

    let started = std::time::Instant::now();
    let result = upstream.call_tool(&mut mcp, "wait", None).await.unwrap();

    assert!(started.elapsed() > TIMEOUT, "{:?}", started.elapsed());
    assert_eq!(result["content"][0]["text"], "done");
}

#[tokio::test]
async fn times_out_a_call_that_is_never_answered_and_cancels_it() {
    let core = core().await;
    let server = mcp_server(|_message| async move {
        // An open stream that never carries the answer.
        event_stream(Body::from_stream(futures::stream::pending::<
            Result<String, Infallible>,
        >()))
    })
    .await;
    let (mut mcp, upstream) = http_mcp(&core, server.url("/mcp")).await;

    let error = upstream
        .call_tool(&mut mcp, "wait", None)
        .await
        .unwrap_err();

    let timed_out = error.timed_out().expect("a timeout");
    assert_eq!(timed_out.method, "tools/call");
    assert_eq!(timed_out.waited, TIMEOUT);
    assert_eq!(error.to_string(), "MCP error -32001: Request timed out");

    // The call is closed right after the failure, yet the MCP was told.
    let requests = server.requests();
    let call = requests
        .iter()
        .map(Received::json)
        .find(|body| body["method"] == "tools/call")
        .unwrap();
    let cancelled: Vec<Value> = requests
        .iter()
        .map(Received::json)
        .filter(|body| body["method"] == "notifications/cancelled")
        .collect();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["params"]["requestId"], call["id"]);
}
