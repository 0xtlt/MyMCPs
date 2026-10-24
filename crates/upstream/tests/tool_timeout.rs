//! The tool call timeout of an MCP, with a real HTTP server that takes its
//! time and with an npm package that does not answer (in `npm.rs`).

mod support;

use std::time::{Duration, Instant};

use axum::body::Body;
use mymcps_core::TestCore;
use mymcps_net::{AddressGuard, Fetcher};
use mymcps_upstream::Upstream;
use serde_json::json;
use support::*;

/// An MCP server on 127.0.0.1 whose tools answer after `delay`.
async fn slow_server(delay: Duration) -> LocalServer {
    local_server(move |request: Received| async move {
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
                        "serverInfo": { "name": "slow", "version": "1.0.0" },
                    },
                }),
            );
        }
        if message.get("id").is_none() {
            return respond(202, &[], Body::empty());
        }
        tokio::time::sleep(delay).await;
        respond_json(
            200,
            json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": { "content": [{ "type": "text", "text": "done" }] },
            }),
        )
    })
    .await
}

#[tokio::test]
async fn waits_for_a_remote_mcp_as_long_as_its_tool_call_timeout_says() {
    let core = TestCore::new().await;
    let server = slow_server(Duration::from_millis(1500)).await;
    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .fetcher(Fetcher::shared())
        .address_guard(AddressGuard::system())
        .build();
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some(server.url("/mcp"));
        mcp.tool_timeout_seconds = Some(1);
    })
    .await;

    let started = Instant::now();
    let error = upstream
        .call_tool(&mut mcp, "work", None)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(error.is_tool_timeout(), "{error}");
    assert_eq!(
        error.to_string(),
        "The remote MCP server did not answer within 1 second, the tool call timeout of this MCP."
    );
    // With more time, the same call gets its answer.
    mcp.tool_timeout_seconds = Some(5);
    let result = upstream.call_tool(&mut mcp, "work", None).await.unwrap();
    assert_eq!(result["content"][0]["text"], "done");
}
