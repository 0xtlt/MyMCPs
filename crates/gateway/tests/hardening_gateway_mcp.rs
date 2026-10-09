//! The cases of `tests/functional/hardening_gateway_mcp.spec.ts` about the
//! call log. Its request allowance is in `gateway_auth.rs`.

mod support;

use mymcps_core::models::InstanceSetting;
use mymcps_core::models::{AccessToken, CallErrorCategory, CallOutcome, McpLogLevel, ScopeMode};
use mymcps_gateway::call_log::McpCallLogInput;
use serde_json::json;
use support::*;

/// The record of a call for an MCP the token may not use, named `slug`.
fn disallowed_call(token: &AccessToken, slug: &str, tool: &str) -> McpCallLogInput {
    McpCallLogInput {
        access_token: token.clone(),
        caller_ip: Some("a".repeat(300)),
        mcp_slug: Some(slug.to_owned()),
        requested_tool_name: format!("{slug}__{tool}"),
        tool_name: Some(tool.to_owned()),
        outcome: CallOutcome::Error,
        error_category: Some(CallErrorCategory::DisallowedMcp),
        error_summary: Some("MCP not allowed for this token".into()),
        ..Default::default()
    }
}

// hardening: MCP call log

#[tokio::test]
async fn stores_only_a_well_formed_slug_and_bounded_names_for_refused_calls() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    for slug in [
        "x".repeat(4000).as_str(),
        "Not A Slug",
        "missing-mcp",
        "Not A Slug",
    ] {
        gateway
            .call_log
            .record(disallowed_call(&token, slug, "tool"));
    }
    gateway.call_log.record(McpCallLogInput {
        tool_name: Some("t".repeat(4000)),
        ..disallowed_call(&token, "missing-mcp", "tool")
    });
    gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    assert!(
        logs.iter()
            .all(|log| log.error_category == Some(CallErrorCategory::DisallowedMcp))
    );
    assert_eq!(
        logs.iter()
            .map(|log| log.mcp_slug.as_deref())
            .collect::<Vec<_>>(),
        [None, None, Some("missing-mcp"), None, Some("missing-mcp")]
    );
    assert_eq!(logs[0].requested_tool_name.len(), 512);
    assert_eq!(logs[4].tool_name.as_ref().map(String::len), Some(254));
    for log in &logs {
        assert_eq!(log.caller_ip.as_ref().map(String::len), Some(64));
    }
}

#[tokio::test]
async fn logs_a_failed_log_write_without_the_arguments_of_the_call() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.mcp_log_level = McpLogLevel::Arguments;
    settings.save(&**db).await.unwrap();
    // The record of a token that no longer exists is one the database refuses.
    let deleted_token = AccessToken {
        id: 999_999,
        ..token
    };

    let logs = CapturedLogs::start();
    gateway.call_log.record(McpCallLogInput {
        access_token: deleted_token,
        requested_tool_name: "invalid".into(),
        args: Some(json!({ "password": "captured-argument-secret" })),
        outcome: CallOutcome::Error,
        error_category: Some(CallErrorCategory::InvalidTool),
        error_summary: Some("Invalid tool name".into()),
        ..Default::default()
    });
    gateway.call_log.flush().await;

    let logged = logs.text();
    assert_eq!(logged.lines().count(), 1, "{logged}");
    assert!(logged.contains("WARN"));
    assert!(logged.contains("MCP call log could not be persisted"));
    assert!(logged.contains("code="));
    assert!(logged.contains("FOREIGN KEY constraint failed"));
    assert!(!logged.contains("captured-argument-secret"));
    assert_eq!(count(db, "mcp_call_logs").await, 0);
}
