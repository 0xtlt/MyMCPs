//! The "MCP client through the limited body" group of
//! `tests/unit/hardening_upstream_fetch.spec.ts`: a real HTTP server, so the
//! MCP client reads through the limited body as in production.

mod support;

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::StreamExt;
use mymcps_core::TestCore;
use mymcps_core::models::McpAuthType;
use mymcps_net::{AddressGuard, Fetcher, MAX_UPSTREAM_ERROR_RESPONSE_BYTES};
use mymcps_upstream::{Upstream, UpstreamError, UpstreamTool};
use serde_json::{Value, json};
use support::*;

/// An MCP server on 127.0.0.1 that runs the handshake itself and leaves
/// every other request to `answer`.
async fn mcp_server<F, Fut>(answer: F) -> LocalServer
where
    F: Fn(Value) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    local_server(move |request: Received| {
        let answer = answer.clone();
        async move {
            if request.method != "POST" {
                return respond(405, &[], Body::empty());
            }
            let message = request.json();
            if message["method"] == "initialize" {
                return respond_json(
                    200,
                    json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "protocolVersion": message["params"]["protocolVersion"],
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "local", "version": "1.0.0" },
                        },
                    }),
                );
            }
            if message.get("id").is_none() {
                return respond(202, &[], Body::empty());
            }
            answer(message).await
        }
    })
    .await
}

fn tools() -> Value {
    json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] })
}

async fn list_http_tools(core: &TestCore, url: String) -> Result<Vec<UpstreamTool>, UpstreamError> {
    let mut mcp = create_mcp(core, |mcp| {
        mcp.http_url = Some(url);
        mcp.auth_type = McpAuthType::Bearer;
    })
    .await;
    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .fetcher(Fetcher::shared())
        .address_guard(AddressGuard::system())
        .build();
    upstream.list_http_tools(&mut mcp).await
}

async fn tool_names(core: &TestCore, url: String) -> Vec<String> {
    list_http_tools(core, url)
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}

#[tokio::test]
async fn reads_json_and_event_stream_answers_as_before() {
    let core = TestCore::new().await;
    let as_json = mcp_server(|message| async move {
        respond_json(
            200,
            json!({ "jsonrpc": "2.0", "id": message["id"], "result": tools() }),
        )
    })
    .await;
    let as_stream = mcp_server(|message| async move {
        // Split mid-event, as a network would.
        let event = format!(
            "event: message\ndata: {}\n\n",
            json!({ "jsonrpc": "2.0", "id": message["id"], "result": tools() })
        );
        let (start, end) = (event[..20].to_owned(), event[20..].to_owned());
        let chunks = futures::stream::once(async move { Ok::<_, Infallible>(start) }).chain(
            futures::stream::once(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(end)
            }),
        );
        respond(
            200,
            &[("Content-Type", "text/event-stream")],
            Body::from_stream(chunks),
        )
    })
    .await;

    assert_eq!(tool_names(&core, as_json.url("/mcp")).await, ["echo"]);
    assert_eq!(tool_names(&core, as_stream.url("/mcp")).await, ["echo"]);
}

#[tokio::test]
async fn gives_up_on_an_answer_larger_than_the_limit() {
    let core = TestCore::new().await;
    let written = Arc::new(AtomicUsize::new(0));
    let oversized = mcp_server({
        let written = written.clone();
        move |_message| {
            let written = written.clone();
            async move {
                // Twice the limit, sent only as fast as the client reads it.
                let chunk = Bytes::from(vec![b'a'; 1024 * 1024]);
                let chunks = futures::stream::iter(0..64).map(move |_| {
                    written.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Infallible>(chunk.clone())
                });
                respond(
                    200,
                    &[("Content-Type", "application/json")],
                    Body::from_stream(chunks),
                )
            }
        }
    })
    .await;

    let error = list_http_tools(&core, oversized.url("/mcp"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("MCP endpoint response exceeded 33554432 bytes"),
        "{error}"
    );
    assert!(!error.is_unauthorized());
    assert!(written.load(Ordering::SeqCst) < 64);
}

#[tokio::test]
async fn quotes_only_the_start_of_an_oversized_error_page() {
    let core = TestCore::new().await;
    let failing = mcp_server(|_message| async move {
        respond(
            500,
            &[("Content-Type", "text/html")],
            vec![b'e'; 4 * 1024 * 1024],
        )
    })
    .await;

    let error = list_http_tools(&core, failing.url("/mcp"))
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("Error POSTing to endpoint"), "{message}");
    assert!(message.len() <= MAX_UPSTREAM_ERROR_RESPONSE_BYTES + 200);
    assert_eq!(
        message.len(),
        "Streamable HTTP error: Error POSTing to endpoint: ".len()
            + MAX_UPSTREAM_ERROR_RESPONSE_BYTES
    );
}
