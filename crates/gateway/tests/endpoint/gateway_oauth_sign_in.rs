//! What an agent is told, and what the pages are left showing, when the OAuth
//! sign-in of an HTTP MCP no longer works: the call or the search is answered
//! with a sentence that says what to do, and the MCP waits to be authorized
//! again.

use chrono::Duration;
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{CallErrorCategory, Mcp, McpAuthType, McpStatus, ScopeMode};
use mymcps_net::{CannedResponse, Fetcher};
use serde_json::{Value, json};

use crate::support::mcp::*;
use crate::support::*;

const RE_AUTHORIZE: &str = "MCP \"t3-code\" needs to be re-authorized in MyMCPs: its OAuth sign-in has expired or was revoked. Re-authorize it on the MCPs page of MyMCPs, then try again.";

/// A gateway behind which every server answers HTTP 401 to a listing or a
/// call, once the handshake is done.
async fn refusing_gateway() -> TestMcpGateway {
    let layer = |fetcher: Fetcher| {
        fetcher.answering(|request| {
            let body: Value = serde_json::from_str(&request.text()).unwrap_or(Value::Null);
            matches!(body["method"].as_str(), Some("tools/list" | "tools/call")).then(|| {
                CannedResponse::new(StatusCode::UNAUTHORIZED).body("The access token expired")
            })
        })
    };
    TestMcpGateway::with(
        |_| {},
        Default::default(),
        layer,
        listing(&[("create_task", Some("Create a task."))]),
    )
    .await
}

/// A gateway behind which every server accepts what it is sent.
async fn accepting_gateway() -> TestMcpGateway {
    TestMcpGateway::new(listing(&[("create_task", Some("Create a task."))])).await
}

/// T3 Code: signed in with OAuth, and the token ends `expires_in` from now
/// (before now when it is negative) with no refresh token to renew it.
async fn t3_code(gateway: &TestMcpGateway, created_by: i64, expires_in: Option<Duration>) -> Mcp {
    let token = gateway.core.encrypt_secret(Some("t3-access-token"));
    create_mcp(gateway.db(), created_by, |mcp| {
        mcp.name = "T3 Code".into();
        mcp.slug = "t3-code".into();
        mcp.http_url = Some("https://t3.example/mcp".into());
        mcp.oauth_access_token = token;
        mcp.oauth_token_expires_at = expires_in.map(|expires_in| Timestamp::now() + expires_in);
    })
    .await
}

async fn saved(gateway: &TestMcpGateway, mcp: &Mcp) -> Mcp {
    Mcp::find(&**gateway.db(), mcp.id).await.unwrap().unwrap()
}

#[tokio::test]
async fn tells_an_agent_to_re_authorize_an_mcp_whose_server_refuses_the_call() {
    let gateway = refusing_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = t3_code(&gateway, admin.id, Some(Duration::days(10))).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({ "title": "x" }))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), RE_AUTHORIZE);
    // The log keeps what happened, not what the agent was told.
    let log = call_logs(db).await.pop().unwrap();
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::UpstreamException)
    );
    let summary = log.error_summary.as_deref().unwrap();
    assert!(summary.contains("The access token expired"), "{summary}");
    assert!(!summary.contains("t3-access-token"));
    // The pages show the MCP as waiting for a new authorization.
    let row = saved(&gateway, &mcp).await;
    assert_eq!(row.status, McpStatus::Error);
    assert!(row.oauth_required);
    assert_eq!(
        row.last_error.as_deref(),
        Some("OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP.")
    );
}

#[tokio::test]
async fn tells_an_agent_to_re_authorize_a_token_that_expired_without_asking_the_server() {
    let gateway = accepting_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = t3_code(&gateway, admin.id, Some(Duration::minutes(-5))).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({}))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), RE_AUTHORIZE);
    assert!(gateway.upstreams.requests().is_empty());
    let row = saved(&gateway, &mcp).await;
    assert_eq!(row.status, McpStatus::Error);
    assert!(row.oauth_required);
    assert!(
        row.last_error
            .as_deref()
            .unwrap()
            .starts_with("OAuth sign-in expired on ")
    );
    assert!(
        row.last_error
            .as_deref()
            .unwrap()
            .ends_with(" UTC and cannot be renewed: the provider issues no refresh token.")
    );

    // The listing leaves the MCP out, as before, and says nothing new to the agent.
    let listed = gateway
        .rpc(&plaintext, "tools/list", json!({}), "eager")
        .await;
    assert_eq!(listed["tools"], json!([]));

    // Connecting the account again is what makes the next call go through.
    let mut row = saved(&gateway, &mcp).await;
    row.oauth_access_token = gateway.core.encrypt_secret(Some("t3-access-token-2"));
    row.oauth_token_expires_at = Some(Timestamp::now() + Duration::days(30));
    row.status = McpStatus::Ready;
    row.oauth_required = false;
    row.last_error = None;
    row.save(&**db).await.unwrap();
    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({}))
        .await;
    assert_eq!(result["isError"], Value::Null);
    assert_eq!(result_text(&result), "ran create_task");
}

#[tokio::test]
async fn tells_an_agent_to_re_authorize_when_a_lazy_search_finds_the_sign_in_lapsed() {
    let gateway = refusing_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = t3_code(&gateway, admin.id, None).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "mcp": "t3-code", "query": "task" } }),
            "lazy",
        )
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), RE_AUTHORIZE);
    let row = saved(&gateway, &mcp).await;
    assert_eq!(row.status, McpStatus::Error);
    assert!(row.oauth_required);

    // A lazy call says the same.
    let result = gateway
        .call_lazily(&plaintext, "t3-code", "create_task", json!({}))
        .await;
    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), RE_AUTHORIZE);
}

#[tokio::test]
async fn tells_an_agent_to_connect_an_mcp_that_has_no_account_yet() {
    let gateway = refusing_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_mcp(db, admin.id, |mcp| {
        mcp.name = "T3 Code".into();
        mcp.slug = "t3-code".into();
        mcp.http_url = Some("https://t3.example/mcp".into());
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({}))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(
        result_text(&result),
        "MCP \"t3-code\" needs to be connected in MyMCPs: it signs in with OAuth and no account is connected yet. Connect it on the MCPs page of MyMCPs, then try again."
    );
    // Nothing was signed in, so there is no sign-in to mark.
    let row = saved(&gateway, &mcp).await;
    assert_eq!(row.status, McpStatus::Draft);
    assert_eq!(row.last_error, None);
}

#[tokio::test]
async fn tells_an_agent_to_re_authorize_before_asking_anyone_to_approve_a_call() {
    let gateway = accepting_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = t3_code(&gateway, admin.id, Some(Duration::minutes(-5))).await;
    let mut ask = saved(&gateway, &mcp).await;
    ask.tool_approvals = Some(json!({ "create_task": "ask" }).to_string());
    ask.save(&**db).await.unwrap();
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({}))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), RE_AUTHORIZE);
    assert!(gateway.approval_requests().await.is_empty());
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn keeps_the_generic_failure_for_what_a_sign_in_cannot_explain() {
    let gateway = refusing_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    // A static credential that is refused is not something to re-authorize.
    create_mcp(db, admin.id, |mcp| {
        mcp.name = "T3 Code".into();
        mcp.slug = "t3-code".into();
        mcp.http_url = Some("https://t3.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = gateway.core.encrypt_secret(Some("static-token"));
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "t3-code__create_task", json!({}))
        .await;
    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), "Upstream tool call failed");

    let result = gateway
        .rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "mcp": "t3-code", "query": "task" } }),
            "lazy",
        )
        .await;
    assert_eq!(
        result_text(&result),
        "Unable to search tools for MCP \"t3-code\""
    );
}
