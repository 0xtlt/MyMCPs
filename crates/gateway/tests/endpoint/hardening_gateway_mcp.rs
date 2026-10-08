//! `tests/functional/hardening_gateway_mcp.spec.ts`, for what goes through
//! `/mcp`: when the gateway contacts the MCPs behind it, and what a refused
//! call leaves in the call log. The request allowance is in `gateway_auth`.

use std::collections::BTreeSet;

use futures::StreamExt;
use http::StatusCode;
use mymcps_core::models::{CallErrorCategory, InstanceSetting, McpLogLevel, ScopeMode};
use mymcps_gateway::McpRequest;
use serde_json::{Value, json};

use crate::support::mcp::*;
use crate::support::*;

fn rpc(body: Value) -> Value {
    let mut message = json!({ "jsonrpc": "2.0" });
    for (key, value) in body.as_object().unwrap() {
        message[key] = value.clone();
    }
    message
}

fn tool_call(name: &str, arguments: Option<Value>) -> Value {
    let mut params = json!({ "name": name });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    rpc(json!({ "id": 7, "method": "tools/call", "params": params }))
}

fn initialize() -> Value {
    rpc(json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "hardening-test", "version": "1.0.0" },
        },
    }))
}

/// Every MCP has one tool, `echo`, which answers `ok`.
fn echo(message: &UpstreamMessage) -> Reply {
    Reply::Result(match message.method.as_str() {
        "tools/list" => {
            json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] })
        }
        _ => json!({ "content": [{ "type": "text", "text": "ok" }] }),
    })
}

// hardening: gateway requests

#[tokio::test]
async fn contacts_upstreams_in_eager_mode_only_to_list_tools_or_to_call_one() {
    let gateway = TestMcpGateway::new(echo).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    create_http_mcp(db, admin.id, "Issues", "issues").await;
    create_http_mcp(db, admin.id, "Calendar", "calendar").await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let initialized = gateway.post(&plaintext, initialize(), &[]).await;
    assert_eq!(initialized.status, StatusCode::OK);
    let notified = gateway
        .post(
            &plaintext,
            rpc(json!({ "method": "notifications/initialized" })),
            &[],
        )
        .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    assert_eq!(notified.text, "");
    let pinged = gateway
        .post(&plaintext, rpc(json!({ "id": 2, "method": "ping" })), &[])
        .await;
    assert_eq!(pinged.status, StatusCode::OK);
    assert_eq!(pinged.result(), json!({}));
    assert!(gateway.upstreams.requests().is_empty());

    let called = gateway
        .post(&plaintext, tool_call("issues__echo", None), &[])
        .await;
    assert_eq!(called.status, StatusCode::OK);
    assert!(called.text.contains("ok"));
    let methods = gateway.upstreams.methods();
    assert!(methods.contains(&"tools/call".to_owned()));
    assert!(!methods.contains(&"tools/list".to_owned()));
    assert!(
        gateway
            .upstreams
            .hosts()
            .iter()
            .all(|host| host == "issues.example")
    );

    gateway.upstreams.clear();
    let listed = gateway
        .post(
            &plaintext,
            rpc(json!({ "id": 3, "method": "tools/list" })),
            &[],
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert!(listed.text.contains("issues__echo"));
    assert!(listed.text.contains("calendar__echo"));
    assert_eq!(
        gateway
            .upstreams
            .hosts()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        ["calendar.example", "issues.example"]
    );
}

#[tokio::test]
async fn answers_as_the_stateless_server_of_an_agent() {
    let gateway = TestMcpGateway::new(echo).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let initialized = gateway.post(&plaintext, initialize(), &[]).await;
    assert_eq!(
        initialized.header("content-type"),
        Some("text/event-stream")
    );
    assert_eq!(initialized.header("mcp-session-id"), None);
    assert_eq!(
        initialized.result(),
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mymcps", "version": mymcps_core::VERSION },
        })
    );

    // A method the gateway does not have is a JSON-RPC error, not a failure.
    let unknown = gateway
        .post(
            &plaintext,
            rpc(json!({ "id": 4, "method": "resources/list" })),
            &[],
        )
        .await;
    assert_eq!(unknown.status, StatusCode::OK);
    assert_eq!(
        unknown.rpc()["error"],
        json!({ "code": -32601, "message": "Method not found" })
    );

    // What the web server read in a body that was not one message.
    let batch = gateway
        .post(&plaintext, json!({ "0": initialize() }), &[])
        .await;
    assert_eq!(batch.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        batch.json()["error"]["message"],
        "Parse error: Invalid JSON-RPC message"
    );

    let authorization = format!("Bearer {plaintext}");
    let not_json = gateway
        .request(
            http::Method::POST,
            &[
                ("authorization", &authorization),
                ("accept", "application/json, text/event-stream"),
                ("content-type", "text/plain"),
            ],
            None,
        )
        .await;
    assert_eq!(not_json.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let no_stream = gateway
        .request(
            http::Method::GET,
            &[("authorization", &authorization), ("accept", "text/html")],
            None,
        )
        .await;
    assert_eq!(no_stream.status, StatusCode::NOT_ACCEPTABLE);
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn opens_a_stream_on_get_that_only_the_agent_ends() {
    let gateway = TestMcpGateway::new(echo).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {plaintext}").parse().unwrap(),
    );
    headers.insert("accept", "text/event-stream".parse().unwrap());

    let response = gateway
        .mcp_gateway
        .handle(McpRequest {
            method: &http::Method::GET,
            headers: &headers,
            body: json!({}),
            caller_ip: None,
        })
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    // A stateless server has nothing to say on it, and does not end it.
    let mut body = response.into_body();
    let silent = tokio::time::timeout(std::time::Duration::from_millis(100), body.next()).await;
    assert!(silent.is_err());
    assert!(gateway.upstreams.requests().is_empty());
}

// hardening: MCP call log

#[tokio::test]
async fn stores_only_a_well_formed_slug_and_bounded_names_for_refused_calls() {
    let gateway = TestMcpGateway::new(echo).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let long_address = "a".repeat(300);
    let forwarded_for = ("x-forwarded-for", long_address.as_str());

    for name in [
        format!("{}__tool", "x".repeat(4000)),
        "Not A Slug__tool".to_owned(),
        "missing-mcp__tool".to_owned(),
    ] {
        gateway
            .post(&plaintext, tool_call(&name, None), &[forwarded_for])
            .await;
    }
    gateway
        .post(
            &plaintext,
            tool_call(
                "call_tool",
                Some(json!({ "mcp": "Not A Slug", "tool": "tool" })),
            ),
            &[forwarded_for, ("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    gateway.gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    assert_eq!(
        logs.iter()
            .map(|log| log.error_category)
            .collect::<Vec<_>>(),
        [Some(CallErrorCategory::DisallowedMcp); 4]
    );
    assert_eq!(
        logs.iter()
            .map(|log| log.mcp_slug.as_deref())
            .collect::<Vec<_>>(),
        [None, None, Some("missing-mcp"), None]
    );
    assert_eq!(logs[0].requested_tool_name.len(), 512);
    assert_eq!(logs[3].requested_tool_name, "Not A Slug__tool");
    assert_eq!(logs[3].tool_name.as_deref(), Some("tool"));
    for log in &logs {
        assert_eq!(log.caller_ip.as_ref().map(String::len), Some(64));
    }
}

#[tokio::test]
async fn logs_a_failed_log_write_without_the_statement_or_its_values() {
    let gateway = TestMcpGateway::new(echo).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.mcp_log_level = McpLogLevel::Arguments;
    settings.save(&**db).await.unwrap();
    sqlx::query(
        "create trigger `refuse_call_logs` before insert on `mcp_call_logs` \
         begin select raise(abort, 'database or disk is full'); end",
    )
    .execute(&**db)
    .await
    .unwrap();
    // Requests prune what expired: let the first one do it before logs are read.
    gateway.post(&plaintext, initialize(), &[]).await;

    let logs = CapturedLogs::start();
    let response = gateway
        .post(
            &plaintext,
            tool_call(
                "invalid",
                Some(json!({ "password": "captured-argument-secret" })),
            ),
            &[],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    gateway.gateway.call_log.flush().await;

    let logged = logs.text();
    assert_eq!(logged.lines().count(), 1, "{logged}");
    assert!(logged.contains("WARN"));
    assert!(logged.contains("MCP call log could not be persisted"));
    assert!(logged.contains("code="));
    assert!(logged.contains("database or disk is full"));
    assert!(!logged.contains("captured-argument-secret"));
    assert_eq!(count(db, "mcp_call_logs").await, 0);
}
