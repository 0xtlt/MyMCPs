//! `tests/functional/mcp_call_logging.spec.ts`: what a call through `/mcp`
//! leaves in the call log. What the server log says of a call that failed
//! is checked in the gateway crate, which can read it.

#[path = "support/gateway.rs"]
mod support;

use std::sync::Mutex;

use http::StatusCode;
use mymcps_core::models::{
    CallErrorCategory, CallOutcome, InstanceSetting, Mcp, McpLogLevel, ScopeMode,
};
use mymcps_gateway::call_log::MAX_CAPTURE_BYTES;
use mymcps_web::testing::TestResponse;
use mymcps_web::testing::factories::{create_admin, create_mcp};
use serde_json::{Value, json};

use support::*;

const EXPLOSION: &str = "Bearer very-secret-token failed at https://user:pass@example.test {\"client_secret\":\"oauth-secret\"} opaque-custom-value";

/// The MCP of the TypeScript tests: `fail` returns an error, `explode`
/// fails altogether, and any other tool answers `ok`.
fn tool_server(message: &UpstreamMessage) -> Reply {
    match (message.method.as_str(), message.tool()) {
        ("tools/list", _) => Reply::Result(json!({
            "tools": [
                { "name": "echo", "inputSchema": { "type": "object" } },
                { "name": "fail", "inputSchema": { "type": "object" } },
                { "name": "explode", "inputSchema": { "type": "object" } },
            ],
        })),
        (_, "explode") => Reply::Error(EXPLOSION.into()),
        (_, "fail") => Reply::Result(json!({
            "content": [{ "type": "text", "text": "private tool failure" }],
            "isError": true,
        })),
        _ => Reply::Result(json!({ "content": [{ "type": "text", "text": "ok" }] })),
    }
}

fn tool_call(name: &str, arguments: Option<Value>) -> Value {
    let mut params = json!({ "name": name });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params })
}

async fn set_log_level(gateway: &TestGateway, level: McpLogLevel) {
    let db = gateway.db();
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.mcp_log_level = level;
    settings.save(&**db).await.unwrap();
}

/// An MCP named `logging` and the token of an agent that may use it.
async fn logging_setup(gateway: &TestGateway, adjust: impl FnOnce(&mut Mcp)) -> (Mcp, String) {
    let admin = create_admin(gateway).await;
    let mcp = create_mcp(gateway, admin.id, |mcp| {
        mcp.name = "Logging MCP".into();
        mcp.slug = "logging".into();
        mcp.http_url = Some("https://logging.example/mcp".into());
        adjust(mcp);
    })
    .await;
    let plaintext = create_access_token(gateway, admin.id, ScopeMode::Selected, &[mcp.id])
        .await
        .plaintext;
    (mcp, plaintext)
}

/// Call a tool as an agent would, and wait for its record.
async fn call_gateway(
    gateway: &TestGateway,
    plaintext: &str,
    name: &str,
    arguments: Option<Value>,
    headers: &[(&str, &str)],
) -> TestResponse {
    let response = gateway
        .post_message(plaintext, tool_call(name, arguments), headers)
        .await;
    gateway.flush().await;
    response
}

// MCP call capture

#[tokio::test]
async fn captures_successful_calls_as_metadata_without_arguments() {
    let gateway = TestGateway::new(tool_server).await;
    let (mcp, plaintext) = logging_setup(&gateway, |_| {}).await;

    let response = call_gateway(
        &gateway,
        &plaintext,
        "logging__echo",
        Some(json!({ "message": "secret" })),
        &[("x-forwarded-for", "192.0.2.10")],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        result_of(&response),
        json!({ "content": [{ "type": "text", "text": "ok" }] })
    );

    let logs = gateway.call_logs().await;
    assert_eq!(logs.len(), 1);
    let log = &logs[0];
    assert_eq!(log.outcome, CallOutcome::Success);
    assert_eq!(log.error_category, None);
    assert_eq!(log.error_summary, None);
    assert_eq!(log.mcp_id, Some(mcp.id));
    assert_eq!(log.mcp_name.as_deref(), Some("Logging MCP"));
    assert_eq!(log.mcp_slug.as_deref(), Some("logging"));
    assert_eq!(log.requested_tool_name, "logging__echo");
    assert_eq!(log.tool_name.as_deref(), Some("echo"));
    assert_eq!(log.caller_ip.as_deref(), Some("192.0.2.10"));
    assert!(!log.arguments_captured);
    assert_eq!(log.arguments, None);
    assert!(!log.response_captured);
    assert_eq!(log.response, None);
    assert!(log.duration_ms >= 0);
    // The arguments reached the tool as they were sent.
    assert_eq!(
        gateway.upstreams.calls(),
        [json!({ "name": "echo", "arguments": { "message": "secret" } })]
    );
}

#[tokio::test]
async fn captures_exact_arguments_and_tool_returned_errors_at_arguments_level() {
    let gateway = TestGateway::new(tool_server).await;
    let (_, plaintext) = logging_setup(&gateway, |_| {}).await;
    set_log_level(&gateway, McpLogLevel::Arguments).await;
    let args = json!({ "password": "stored-exactly", "nested": { "token": "also-stored" } });

    let response = call_gateway(
        &gateway,
        &plaintext,
        "logging__fail",
        Some(args.clone()),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    // The agent is told what the tool said.
    assert_eq!(
        result_of(&response),
        json!({
            "content": [{ "type": "text", "text": "private tool failure" }],
            "isError": true,
        })
    );

    let logs = gateway.call_logs().await;
    let log = &logs[0];
    assert_eq!(log.outcome, CallOutcome::Error);
    assert_eq!(log.error_category, Some(CallErrorCategory::ToolError));
    assert_eq!(
        log.error_summary.as_deref(),
        Some("Upstream tool returned an error")
    );
    assert!(log.arguments_captured);
    assert_eq!(log.arguments.as_deref(), Some(args.to_string().as_str()));
    assert!(!log.response_captured);
    assert_eq!(log.response, None);
}

#[tokio::test]
async fn captures_exact_arguments_and_mcp_responses_only_at_responses_level() {
    let gateway = TestGateway::new(tool_server).await;
    let (_, plaintext) = logging_setup(&gateway, |_| {}).await;
    set_log_level(&gateway, McpLogLevel::Responses).await;
    let args = json!({ "query": "stored request" });

    let response = call_gateway(
        &gateway,
        &plaintext,
        "logging__fail",
        Some(args.clone()),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);

    let logs = gateway.call_logs().await;
    let log = &logs[0];
    assert!(log.arguments_captured);
    assert_eq!(
        serde_json::from_str::<Value>(log.arguments.as_deref().unwrap()).unwrap(),
        args
    );
    assert!(log.response_captured);
    assert_eq!(
        serde_json::from_str::<Value>(log.response.as_deref().unwrap()).unwrap(),
        json!({
            "content": [{ "type": "text", "text": "private tool failure" }],
            "isError": true,
        })
    );
}

#[tokio::test]
async fn sanitizes_upstream_exceptions_and_preserves_the_gateway_response() {
    let gateway = TestGateway::new(tool_server).await;
    let secret = gateway.core.encrypt_secret(Some("opaque-custom-value"));
    let (_, plaintext) = logging_setup(&gateway, |mcp| mcp.auth_header_value = secret).await;
    set_log_level(&gateway, McpLogLevel::Responses).await;

    let response = call_gateway(&gateway, &plaintext, "logging__explode", None, &[]).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        result_of(&response),
        json!({
            "content": [{ "type": "text", "text": "Upstream tool call failed" }],
            "isError": true,
        })
    );

    let records = gateway.call_logs().await;
    let log = &records[0];
    assert_eq!(log.outcome, CallOutcome::Error);
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::UpstreamException)
    );
    let summary = log.error_summary.as_deref().unwrap();
    assert!(summary.contains("Bearer [REDACTED]"), "{summary}");
    assert!(summary.contains("https://[REDACTED]@example.test"));
    for secret in [
        "very-secret-token",
        "user:pass",
        "opaque-custom-value",
        "oauth-secret",
    ] {
        assert!(!summary.contains(secret), "{summary}");
    }
    // Nothing was answered, so there is no response to keep.
    assert!(log.response_captured);
    assert_eq!(log.response, None);
}

#[tokio::test]
async fn caps_oversized_argument_captures() {
    let gateway = TestGateway::new(tool_server).await;
    let (_, plaintext) = logging_setup(&gateway, |_| {}).await;
    set_log_level(&gateway, McpLogLevel::Arguments).await;

    let response = call_gateway(
        &gateway,
        &plaintext,
        "logging__echo",
        Some(json!({ "payload": "x".repeat(MAX_CAPTURE_BYTES + 1) })),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);

    let logs = gateway.call_logs().await;
    let stored = logs[0].arguments.as_deref().unwrap();
    let capture: Value = serde_json::from_str(stored).unwrap();
    assert_eq!(capture["truncated"], true);
    assert!(capture["originalBytes"].as_u64().unwrap() > MAX_CAPTURE_BYTES as u64);
    assert!(stored.len() < MAX_CAPTURE_BYTES);
    // The tool itself received every byte.
    assert_eq!(
        gateway.upstreams.calls()[0]["arguments"]["payload"]
            .as_str()
            .unwrap()
            .len(),
        MAX_CAPTURE_BYTES + 1
    );
}

#[tokio::test]
async fn records_invalid_and_disallowed_attempts_but_never_tools_list() {
    let gateway = TestGateway::new(tool_server).await;
    let admin = create_admin(&gateway).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let forwarded = [("x-forwarded-for", "192.0.2.11")];

    let invalid = call_gateway(&gateway, &plaintext, "invalid", None, &forwarded).await;
    let disallowed = call_gateway(&gateway, &plaintext, "missing__tool", None, &forwarded).await;
    let listed = gateway
        .post_message(
            &plaintext,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
            &forwarded,
        )
        .await;
    gateway.flush().await;

    assert_eq!(
        result_of(&invalid),
        json!({ "content": [{ "type": "text", "text": "Invalid tool name" }], "isError": true })
    );
    assert_eq!(
        result_of(&disallowed),
        json!({
            "content": [{ "type": "text", "text": "MCP not allowed for this token" }],
            "isError": true,
        })
    );
    assert_eq!(result_of(&listed), json!({ "tools": [] }));

    let logs = gateway.call_logs().await;
    assert_eq!(
        logs.iter()
            .map(|log| (
                log.outcome,
                log.error_category,
                log.error_summary.as_deref()
            ))
            .collect::<Vec<_>>(),
        [
            (
                CallOutcome::Error,
                Some(CallErrorCategory::InvalidTool),
                Some("Invalid tool name")
            ),
            (
                CallOutcome::Error,
                Some(CallErrorCategory::DisallowedMcp),
                Some("MCP not allowed for this token")
            ),
        ]
    );
    assert_eq!(
        logs.iter()
            .map(|log| log.caller_ip.as_deref())
            .collect::<Vec<_>>(),
        [Some("192.0.2.11"), Some("192.0.2.11")]
    );
    assert_eq!(logs[0].mcp_slug, None);
    assert_eq!(logs[0].tool_name, None);
    assert_eq!(logs[0].requested_tool_name, "invalid");
    assert_eq!(logs[1].mcp_slug.as_deref(), Some("missing"));
    assert_eq!(logs[1].tool_name.as_deref(), Some("tool"));
    assert_eq!(logs[1].requested_tool_name, "missing__tool");
    assert!(logs.iter().all(|log| log.mcp_id.is_none()));
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn creates_no_records_when_capture_is_off() {
    let gateway = TestGateway::new(tool_server).await;
    let admin = create_admin(&gateway).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    set_log_level(&gateway, McpLogLevel::Off).await;

    let response = call_gateway(&gateway, &plaintext, "invalid", None, &[]).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(gateway.call_logs().await.is_empty());
}

#[tokio::test]
async fn does_not_alter_mcp_responses_when_log_persistence_fails() {
    let gateway = TestGateway::new(tool_server).await;
    let (_, plaintext) = logging_setup(&gateway, |_| {}).await;
    sqlx::query(
        "create trigger `refuse_call_logs` before insert on `mcp_call_logs` \
         begin select raise(abort, 'database unavailable'); end",
    )
    .execute(&**gateway.db())
    .await
    .unwrap();

    let invalid = call_gateway(&gateway, &plaintext, "invalid", None, &[]).await;
    assert_eq!(invalid.status, StatusCode::OK);
    assert!(invalid.text().contains("Invalid tool name"));

    let called = call_gateway(&gateway, &plaintext, "logging__echo", None, &[]).await;
    assert_eq!(called.status, StatusCode::OK);
    assert_eq!(
        result_of(&called),
        json!({ "content": [{ "type": "text", "text": "ok" }] })
    );
    assert!(gateway.call_logs().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn returns_the_mcp_response_before_a_queued_log_write_completes() {
    // The MCP says when the call reached it, then holds its answer.
    let (reached, mut calls_reached) = tokio::sync::mpsc::unbounded_channel();
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = Mutex::new(held);
    let gateway = TestGateway::new(move |message| {
        if message.method == "tools/call" {
            reached.send(()).unwrap();
            held.lock().unwrap().recv().unwrap();
        }
        tool_server(message)
    })
    .await;
    let (_, plaintext) = logging_setup(&gateway, |_| {}).await;
    let db = gateway.db();

    let call = gateway.post_message(&plaintext, tool_call("logging__echo", None), &[]);
    let answered = async {
        calls_reached.recv().await.unwrap();
        // While this transaction is open nothing else can write to the
        // database: the record of the call has to wait, its answer does not.
        let blocking_write = db.begin().await.unwrap();
        release.send(()).unwrap();
        blocking_write
    };
    let (response, mut blocking_write) =
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(call, answered)
        })
        .await
        .expect("the call reached the MCP and was answered");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        result_of(&response),
        json!({ "content": [{ "type": "text", "text": "ok" }] })
    );
    let written: i64 = sqlx::query_scalar("select count(*) from `mcp_call_logs`")
        .fetch_one(&mut *blocking_write)
        .await
        .unwrap();
    assert_eq!(written, 0);

    blocking_write.commit().await.unwrap();
    gateway.flush().await;
    let logs = gateway.call_logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome, CallOutcome::Success);
}
