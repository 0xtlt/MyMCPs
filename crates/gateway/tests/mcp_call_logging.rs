//! `tests/functional/mcp_call_logging.spec.ts`, at the level of the call log
//! service: what a record keeps of a call at each capture level.

mod support;

use std::time::Duration;

use mymcps_core::models::{
    AccessToken, CallErrorCategory, CallOutcome, InstanceSetting, Mcp, McpLogLevel, ScopeMode,
};
use mymcps_core::{Db, Timestamp};
use mymcps_gateway::call_log::{
    MAX_CAPTURE_BYTES, MAX_PENDING_WRITES, McpCallLogInput, sanitize_error_summary,
    serialize_captured_value,
};
use serde_json::{Value, json};
use support::*;

async fn set_log_level(db: &Db, level: McpLogLevel) {
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.mcp_log_level = level;
    settings.save(&**db).await.unwrap();
}

/// A token allowed to use one MCP, `logging`.
async fn logging_setup(gateway: &TestGateway) -> (AccessToken, Mcp) {
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Logging MCP".into();
        mcp.slug = "logging".into();
        mcp.http_url = Some("https://logging.example/mcp".into());
    })
    .await;
    let created = create_access_token(db, admin.id, ScopeMode::Selected, &[mcp.id]).await;
    (created.token, mcp)
}

/// The record of a call to a tool of the `logging` MCP.
fn call(token: &AccessToken, mcp: &Mcp, tool: &str, args: Option<Value>) -> McpCallLogInput {
    McpCallLogInput {
        access_token: token.clone(),
        mcp: Some(mcp.clone()),
        requested_tool_name: format!("{}__{tool}", mcp.slug),
        tool_name: Some(tool.to_owned()),
        args,
        outcome: CallOutcome::Success,
        duration: Duration::from_millis(3),
        ..Default::default()
    }
}

/// The record of a tool that answered with an error.
fn failed_call(token: &AccessToken, mcp: &Mcp, args: Value) -> McpCallLogInput {
    McpCallLogInput {
        response: Some(json!({
            "content": [{ "type": "text", "text": "private tool failure" }],
            "isError": true,
        })),
        outcome: CallOutcome::Error,
        error_category: Some(CallErrorCategory::ToolError),
        error_summary: Some("Upstream tool returned an error".into()),
        ..call(token, mcp, "fail", Some(args))
    }
}

/// The record of a call the gateway refused before it reached an MCP.
fn refused_call(
    token: &AccessToken,
    requested_tool_name: &str,
    category: CallErrorCategory,
    summary: &str,
) -> McpCallLogInput {
    McpCallLogInput {
        access_token: token.clone(),
        requested_tool_name: requested_tool_name.to_owned(),
        outcome: CallOutcome::Error,
        error_category: Some(category),
        error_summary: Some(summary.to_owned()),
        ..Default::default()
    }
}

// MCP call capture

#[tokio::test]
async fn captures_successful_calls_as_metadata_without_arguments() {
    let gateway = TestGateway::new().await;
    let (token, mcp) = logging_setup(&gateway).await;

    gateway.call_log.record(McpCallLogInput {
        caller_ip: Some("192.0.2.10".into()),
        response: Some(json!({ "content": [{ "type": "text", "text": "ok" }] })),
        ..call(&token, &mcp, "echo", Some(json!({ "message": "secret" })))
    });
    gateway.call_log.flush().await;

    let logs = call_logs(gateway.db()).await;
    assert_eq!(logs.len(), 1);
    let log = &logs[0];
    assert_eq!(log.outcome, CallOutcome::Success);
    assert_eq!(log.access_token_id, Some(token.id));
    assert_eq!(log.access_token_name, token.name);
    assert_eq!(log.access_token_prefix, token.token_prefix);
    assert_eq!(log.mcp_id, Some(mcp.id));
    assert_eq!(log.mcp_name.as_deref(), Some("Logging MCP"));
    assert_eq!(log.mcp_slug.as_deref(), Some("logging"));
    assert_eq!(log.requested_tool_name, "logging__echo");
    assert_eq!(log.tool_name.as_deref(), Some("echo"));
    assert_eq!(log.caller_ip.as_deref(), Some("192.0.2.10"));
    assert_eq!(log.error_category, None);
    assert_eq!(log.error_summary, None);
    assert!(!log.arguments_captured);
    assert_eq!(log.arguments, None);
    assert!(!log.response_captured);
    assert_eq!(log.response, None);
    assert_eq!(log.duration_ms, 3);
    assert!((Timestamp::now() - log.created_at).num_seconds() < 60);
}

#[tokio::test]
async fn captures_exact_arguments_and_tool_returned_errors_at_arguments_level() {
    let gateway = TestGateway::new().await;
    let (token, mcp) = logging_setup(&gateway).await;
    set_log_level(gateway.db(), McpLogLevel::Arguments).await;
    let args = json!({ "password": "stored-exactly", "nested": { "token": "also-stored" } });

    gateway
        .call_log
        .record(failed_call(&token, &mcp, args.clone()));
    gateway.call_log.flush().await;

    let logs = call_logs(gateway.db()).await;
    let log = &logs[0];
    assert_eq!(log.outcome, CallOutcome::Error);
    assert_eq!(log.error_category, Some(CallErrorCategory::ToolError));
    assert_eq!(
        log.error_summary.as_deref(),
        Some("Upstream tool returned an error")
    );
    assert!(log.arguments_captured);
    assert_eq!(
        log.arguments.as_deref(),
        Some(r#"{"password":"stored-exactly","nested":{"token":"also-stored"}}"#)
    );
    assert!(!log.response_captured);
    assert_eq!(log.response, None);
    assert!(
        !log.error_summary
            .as_deref()
            .unwrap()
            .contains("private tool failure")
    );
}

#[tokio::test]
async fn captures_exact_arguments_and_mcp_responses_only_at_responses_level() {
    let gateway = TestGateway::new().await;
    let (token, mcp) = logging_setup(&gateway).await;
    set_log_level(gateway.db(), McpLogLevel::Responses).await;
    let args = json!({ "query": "stored request" });

    gateway
        .call_log
        .record(failed_call(&token, &mcp, args.clone()));
    // A call without arguments or without an answer has none to keep.
    gateway.call_log.record(call(&token, &mcp, "echo", None));
    gateway.call_log.flush().await;

    let logs = call_logs(gateway.db()).await;
    let log = &logs[0];
    assert!(log.arguments_captured);
    assert_eq!(
        serde_json::from_str::<Value>(log.arguments.as_deref().unwrap()).unwrap(),
        args
    );
    assert!(log.response_captured);
    assert_eq!(
        log.response.as_deref(),
        Some(r#"{"content":[{"type":"text","text":"private tool failure"}],"isError":true}"#)
    );

    assert!(logs[1].arguments_captured);
    assert_eq!(logs[1].arguments, None);
    assert!(logs[1].response_captured);
    assert_eq!(logs[1].response, None);
}

#[tokio::test]
async fn sanitizes_upstream_exceptions() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let (token, mut mcp) = logging_setup(&gateway).await;
    mcp.auth_header_value = gateway.core.encrypt_secret(Some("opaque-custom-value"));
    mcp.save(&**db).await.unwrap();

    gateway.call_log.record(McpCallLogInput {
        outcome: CallOutcome::Error,
        error_category: Some(CallErrorCategory::UpstreamException),
        error_summary: Some(
            "Bearer very-secret-token failed at https://user:pass@example.test {\"client_secret\":\"oauth-secret\"} opaque-custom-value"
                .into(),
        ),
        ..call(&token, &mcp, "explode", None)
    });
    gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    let log = &logs[0];
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::UpstreamException)
    );
    let summary = log.error_summary.as_deref().unwrap();
    assert!(summary.contains("Bearer [REDACTED]"));
    assert!(summary.contains("https://[REDACTED]@example.test"));
    assert!(!summary.contains("very-secret-token"));
    assert!(!summary.contains("user:pass"));
    assert!(!summary.contains("opaque-custom-value"));
    assert!(!summary.contains("oauth-secret"));

    // Without the MCP its own credentials are not known, the shapes still are.
    let unknown = sanitize_error_summary(
        &gateway.core,
        Some("Bearer very-secret-token opaque-custom-value"),
        None,
    );
    assert_eq!(
        unknown.as_deref(),
        Some("Bearer [REDACTED] opaque-custom-value")
    );
    assert_eq!(
        sanitize_error_summary(&gateway.core, None, Some(&mcp)),
        None
    );
}

#[tokio::test]
async fn caps_oversized_argument_captures() {
    let gateway = TestGateway::new().await;
    let (token, mcp) = logging_setup(&gateway).await;
    set_log_level(gateway.db(), McpLogLevel::Arguments).await;

    gateway.call_log.record(call(
        &token,
        &mcp,
        "echo",
        Some(json!({ "payload": "x".repeat(MAX_CAPTURE_BYTES + 1) })),
    ));
    gateway.call_log.flush().await;

    let logs = call_logs(gateway.db()).await;
    let arguments = logs[0].arguments.as_deref().unwrap();
    let capture: Value = serde_json::from_str(arguments).unwrap();
    assert_eq!(capture["truncated"], true);
    assert!(capture["originalBytes"].as_u64().unwrap() > MAX_CAPTURE_BYTES as u64);
    assert!(arguments.len() < MAX_CAPTURE_BYTES);
}

/// What the Node app stored for these values.
#[test]
fn replaces_a_value_too_large_to_keep_by_its_size_and_its_beginning() {
    assert_eq!(serialize_captured_value(&json!(null)), "null");
    assert_eq!(
        serialize_captured_value(&json!({ "b": [1, "two", null], "a": true })),
        r#"{"b":[1,"two",null],"a":true}"#
    );

    // The limit counts bytes, not characters.
    let at_the_limit = json!("é".repeat(32767));
    assert_eq!(
        serialize_captured_value(&at_the_limit).len(),
        MAX_CAPTURE_BYTES
    );
    let past_the_limit = serialize_captured_value(&json!("é".repeat(32768)));
    assert!(
        past_the_limit.starts_with(r#"{"truncated":true,"originalBytes":65538,"preview":"\"éééé"#)
    );

    let capture =
        serialize_captured_value(&json!({ "payload": "x".repeat(MAX_CAPTURE_BYTES + 1) }));
    assert!(
        capture.starts_with(
            r#"{"truncated":true,"originalBytes":65551,"preview":"{\"payload\":\"xxxx"#
        )
    );
    assert!(capture.ends_with("xxxx\"}"));
    assert_eq!(capture.len(), 8248);

    // The preview is cut after 8192 UTF-16 code units, between characters...
    let capture = serialize_captured_value(&json!({ "p": format!("ab{}", "😀".repeat(40000)) }));
    assert!(
        capture.starts_with(r#"{"truncated":true,"originalBytes":160010,"preview":"{\"p\":\"ab😀"#)
    );
    assert!(capture.ends_with("😀😀\"}"));
    assert_eq!(capture.len(), 16433);

    // ...or in the middle of one, of which JavaScript kept the first half.
    let capture = serialize_captured_value(&json!({ "p": format!("abc{}", "😀".repeat(40000)) }));
    assert!(
        capture
            .starts_with(r#"{"truncated":true,"originalBytes":160011,"preview":"{\"p\":\"abc😀"#)
    );
    assert!(capture.ends_with("😀😀\\ud83d\"}"));
    assert_eq!(capture.len(), 16436);
}

#[tokio::test]
async fn records_invalid_and_disallowed_attempts() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    gateway.call_log.record(McpCallLogInput {
        caller_ip: Some("192.0.2.11".into()),
        ..refused_call(
            &token,
            "invalid",
            CallErrorCategory::InvalidTool,
            "Invalid tool name",
        )
    });
    gateway.call_log.record(McpCallLogInput {
        caller_ip: Some("192.0.2.11".into()),
        mcp_slug: Some("missing".into()),
        tool_name: Some("tool".into()),
        ..refused_call(
            &token,
            "missing__tool",
            CallErrorCategory::DisallowedMcp,
            "MCP not allowed for this token",
        )
    });
    gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    assert_eq!(logs.len(), 2);
    assert_eq!(
        logs.iter()
            .map(|log| log.error_category)
            .collect::<Vec<_>>(),
        [
            Some(CallErrorCategory::InvalidTool),
            Some(CallErrorCategory::DisallowedMcp)
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
    assert_eq!(logs[1].mcp_slug.as_deref(), Some("missing"));
    assert_eq!(logs[1].mcp_id, None);
    assert_eq!(logs[1].mcp_name, None);
    assert_eq!(
        logs[1].error_summary.as_deref(),
        Some("MCP not allowed for this token")
    );
}

#[tokio::test]
async fn creates_no_records_when_capture_is_off() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    set_log_level(db, McpLogLevel::Off).await;

    gateway.call_log.record(refused_call(
        &token,
        "invalid",
        CallErrorCategory::InvalidTool,
        "Invalid tool name",
    ));
    gateway.call_log.flush().await;

    assert_eq!(count(db, "mcp_call_logs").await, 0);
}

#[tokio::test]
async fn keeps_writing_after_a_log_write_fails() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    // A token deleted while its call waits to be recorded: the row it
    // belongs to is gone, and the database refuses the record.
    let deleted_token = AccessToken {
        id: 999_999,
        ..token.clone()
    };

    gateway.call_log.record(refused_call(
        &deleted_token,
        "invalid",
        CallErrorCategory::InvalidTool,
        "Invalid tool name",
    ));
    gateway.call_log.record(refused_call(
        &token,
        "invalid",
        CallErrorCategory::InvalidTool,
        "Invalid tool name",
    ));
    // Recording never fails, and a flush does not wait for a write that did.
    gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].access_token_id, Some(token.id));
}

#[tokio::test]
async fn returns_before_a_queued_log_write_completes() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    // While this transaction is open nothing else can write to the database.
    let mut blocking_write = db.begin().await.unwrap();
    for index in 0..3 {
        gateway.call_log.record(refused_call(
            &token,
            &format!("invalid-{index}"),
            CallErrorCategory::InvalidTool,
            "Invalid tool name",
        ));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let written: i64 = sqlx::query_scalar("select count(*) from `mcp_call_logs`")
        .fetch_one(&mut *blocking_write)
        .await
        .unwrap();
    assert_eq!(written, 0);

    blocking_write.commit().await.unwrap();
    gateway.call_log.flush().await;
    let logs = call_logs(db).await;
    assert_eq!(
        logs.iter()
            .map(|log| log.requested_tool_name.as_str())
            .collect::<Vec<_>>(),
        ["invalid-0", "invalid-1", "invalid-2"]
    );

    // Nothing is left to wait for.
    gateway.call_log.flush().await;
}

#[tokio::test]
async fn drops_records_once_too_many_wait_to_be_written() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    let record = |name: String| {
        gateway.call_log.record(refused_call(
            &token,
            &name,
            CallErrorCategory::InvalidTool,
            "Invalid tool name",
        ))
    };

    for index in 0..MAX_PENDING_WRITES + 5 {
        record(format!("call-{index}"));
    }
    gateway.call_log.flush().await;
    assert_eq!(count(db, "mcp_call_logs").await, MAX_PENDING_WRITES as i64);

    // The queue takes records again once it was written down.
    record("after".into());
    gateway.call_log.flush().await;
    let logs = call_logs(db).await;
    assert_eq!(logs.len(), MAX_PENDING_WRITES + 1);
    assert_eq!(logs.last().unwrap().requested_tool_name, "after");
    assert_eq!(
        logs[MAX_PENDING_WRITES - 1].requested_tool_name,
        format!("call-{}", MAX_PENDING_WRITES - 1)
    );
}

#[tokio::test]
async fn rounds_the_duration_of_a_call_to_milliseconds() {
    let gateway = TestGateway::new().await;
    let (token, mcp) = logging_setup(&gateway).await;

    for micros in [0, 499, 500, 1_499, 2_500] {
        gateway.call_log.record(McpCallLogInput {
            duration: Duration::from_micros(micros),
            ..call(&token, &mcp, "echo", None)
        });
    }
    gateway.call_log.flush().await;

    assert_eq!(
        call_logs(gateway.db())
            .await
            .iter()
            .map(|log| log.duration_ms)
            .collect::<Vec<_>>(),
        [0, 0, 1, 1, 3]
    );
}
