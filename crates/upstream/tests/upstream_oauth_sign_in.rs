//! An HTTP MCP whose OAuth sign-in stopped working is marked as waiting to be
//! authorized again by the requests that find out, not only by a test of its
//! connection, and a call that goes through again clears the mark.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Duration;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_core::{TestCore, Timestamp};
use mymcps_net::CannedResponse;
use mymcps_upstream::{Upstream, UpstreamError, authorization_needed_message};
use serde_json::{Value, json};
use support::*;

fn tools() -> Value {
    json!([{ "name": "search", "inputSchema": { "type": "object" } }])
}

/// An MCP server that lists `tools()` and answers every call with a result.
fn mcp_server(core: &TestCore) -> (Arc<Upstream>, Calls) {
    upstream(core, |call| mcp_answer(call, &tools(), &json!({})))
}

/// A token its provider issued with a lifetime and without a refresh token,
/// which expires `expires_in` from now (before now when it is negative).
async fn without_refresh_token(core: &TestCore, expires_in: Duration) -> Mcp {
    let access_token = encrypt(core, "access-old");
    create_mcp(core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.oauth_access_token = access_token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + expires_in);
    })
    .await
}

fn expired_message(mcp: &Mcp) -> String {
    format!(
        "OAuth sign-in expired on {} and cannot be renewed: the provider issues no refresh token.",
        mcp.oauth_token_expires_at
            .unwrap()
            .as_datetime()
            .format("%Y-%m-%d %H:%M UTC")
    )
}

#[tokio::test]
async fn marks_an_mcp_whose_token_expired_without_a_refresh_token_and_sends_nothing() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core);
    let mut mcp = without_refresh_token(&core, Duration::minutes(-1)).await;
    let message = expired_message(&mcp);

    let error = upstream.probe(&mut mcp).await.unwrap_err();

    assert!(matches!(error, UpstreamError::OAuthSignInExpired(_)));
    assert_eq!(error.to_string(), message);
    assert_eq!(calls.len(), 0);
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert!(saved.oauth_required);
    assert_eq!(saved.last_error.as_deref(), Some(message.as_str()));
    // The token stays: the page knows the MCP signed in, and says when.
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-old")
    );
    // The caller holds the row as it was saved.
    assert_eq!(mcp.status, McpStatus::Error);
    assert!(mcp.oauth_required);

    // A call is refused the same way, and the mark stays as it is.
    let error = upstream.call_tool(&mut mcp, "search", None).await;
    assert!(matches!(error, Err(UpstreamError::OAuthSignInExpired(_))));
    assert_eq!(calls.len(), 0);
    assert_eq!(find_mcp(&core, mcp.id).await.last_error, Some(message));
}

#[tokio::test]
async fn sends_a_token_that_has_not_reached_its_stated_expiry() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core);
    let mut mcp = without_refresh_token(&core, Duration::seconds(30)).await;
    let before = find_mcp(&core, mcp.id).await;

    let listed = upstream.probe(&mut mcp).await.unwrap();

    assert_eq!(listed.len(), 1);
    assert!(calls.len() > 0);
    for request in calls.all() {
        assert_eq!(
            request.header("authorization").as_deref(),
            Some("Bearer access-old")
        );
    }
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);
    assert_eq!(saved.last_error, None);
    assert_eq!(saved.updated_at, before.updated_at);
}

#[tokio::test]
async fn tests_a_connection_whose_token_expired_the_way_a_request_marks_it() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core);
    let mut mcp = without_refresh_token(&core, Duration::minutes(-1)).await;
    let message = expired_message(&mcp);

    upstream.test_and_update_status(&mut mcp).await.unwrap();

    assert_eq!(calls.len(), 0);
    for row in [&mcp, &find_mcp(&core, mcp.id).await] {
        assert_eq!(row.status, McpStatus::Error);
        assert!(row.oauth_required);
        assert_eq!(row.last_error.as_deref(), Some(message.as_str()));
    }
}

/// A server that refuses every call while `refusing` is set.
fn refusing_calls(core: &TestCore, refusing: &Arc<AtomicBool>) -> (Arc<Upstream>, Calls) {
    let refusing = refusing.clone();
    upstream(core, move |call| {
        if refusing.load(Ordering::SeqCst) && call.json()["method"] == "tools/call" {
            return status(401).body("token revoked");
        }
        mcp_answer(call, &tools(), &json!({ "content": [] }))
    })
}

#[tokio::test]
async fn marks_an_mcp_whose_server_refuses_a_call_and_clears_the_mark_when_a_call_goes_through() {
    let core = TestCore::new().await;
    let refusing = Arc::new(AtomicBool::new(true));
    let (upstream, _) = refusing_calls(&core, &refusing);
    let mut mcp = without_refresh_token(&core, Duration::days(10)).await;

    let error = upstream
        .call_tool(&mut mcp, "search", None)
        .await
        .unwrap_err();

    assert!(error.is_unauthorized());
    assert!(error.needs_authorization());
    for row in [&mcp, &find_mcp(&core, mcp.id).await] {
        assert_eq!(row.status, McpStatus::Error);
        assert!(row.oauth_required);
        assert_eq!(
            row.last_error.as_deref(),
            Some("OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP.")
        );
    }

    // Asking again says the same, and the row is the same.
    let marked = find_mcp(&core, mcp.id).await;
    upstream
        .call_tool(&mut mcp, "search", None)
        .await
        .unwrap_err();
    assert_eq!(find_mcp(&core, mcp.id).await.updated_at, marked.updated_at);

    // The same sign-in works again: the mark goes.
    refusing.store(false, Ordering::SeqCst);
    upstream.call_tool(&mut mcp, "search", None).await.unwrap();
    for row in [&mcp, &find_mcp(&core, mcp.id).await] {
        assert_eq!(row.status, McpStatus::Ready);
        assert!(!row.oauth_required);
        assert_eq!(row.last_error, None);
    }
}

#[tokio::test]
async fn marks_an_mcp_whose_server_refuses_the_handshake_with_what_it_said() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |_| status(401).body("expired"));
    let mut mcp = without_refresh_token(&core, Duration::days(10)).await;

    upstream.probe(&mut mcp).await.unwrap_err();

    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert!(saved.oauth_required);
    assert_eq!(
        saved.last_error.as_deref(),
        Some("OAuth token rejected. MCP server returned HTTP 401. Response: expired")
    );
}

#[tokio::test]
async fn leaves_an_mcp_alone_when_a_call_fails_for_another_reason_or_it_does_not_sign_in_with_oauth()
 {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |call| {
        if call.json()["method"] == "tools/call" {
            return status(500).body("boom");
        }
        mcp_answer(call, &tools(), &json!({}))
    });
    let mut failing = without_refresh_token(&core, Duration::days(10)).await;
    upstream
        .call_tool(&mut failing, "search", None)
        .await
        .unwrap_err();
    let saved = find_mcp(&core, failing.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);

    // A static credential that is refused is an error to fix in the form.
    let (refused, _) = upstream_refusing_everything(&core);
    let mut bearer = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = encrypt(&core, "static-token");
    })
    .await;
    refused.probe(&mut bearer).await.unwrap_err();
    let saved = find_mcp(&core, bearer.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);

    // No account connected yet: nothing to mark, the MCP is not set up.
    let mut waiting = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
    })
    .await;
    refused.probe(&mut waiting).await.unwrap_err();
    let saved = find_mcp(&core, waiting.id).await;
    assert_eq!(saved.status, McpStatus::Draft);
    assert_eq!(saved.last_error, None);
}

fn upstream_refusing_everything(core: &TestCore) -> (Arc<Upstream>, Calls) {
    upstream(core, |_| status(401).body("no"))
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_mark_a_sign_in_that_was_replaced_while_the_request_was_in_flight() {
    let core = TestCore::new().await;
    let mut mcp = without_refresh_token(&core, Duration::days(10)).await;

    // The account is connected again while the server is refusing the old token.
    let replaced = Arc::new(AtomicBool::new(false));
    let (upstream, _) = upstream(&core, {
        let db = core.db.clone();
        let mut other = find_mcp(&core, mcp.id).await;
        other.oauth_access_token = encrypt(&core, "access-new");
        other.oauth_token_expires_at = Some(Timestamp::now() + Duration::days(30));
        let replaced = replaced.clone();
        move |call| {
            if call.json()["method"] != "tools/call" {
                return mcp_answer(call, &tools(), &json!({}));
            }
            if !replaced.swap(true, Ordering::SeqCst) {
                let mut other = other.clone();
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(other.save(&*db))
                        .unwrap();
                });
            }
            status(401).body("token revoked")
        }
    });

    let error = upstream
        .call_tool(&mut mcp, "search", None)
        .await
        .unwrap_err();

    assert!(error.is_unauthorized());
    assert!(replaced.load(Ordering::SeqCst));
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);
    assert_eq!(saved.last_error, None);
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-new")
    );
}

fn refused_renewal(
    error: Option<&str>,
) -> impl Fn(&Call) -> CannedResponse + Send + Sync + 'static {
    let answer = match error {
        Some(description) => json!({ "error": "invalid_grant", "error_description": description }),
        None => json!({ "error": "invalid_grant" }),
    };
    move |call| match call.url.as_str() {
        "https://oauth.example/.well-known/oauth-authorization-server" => json_response(json!({
            "issuer": "https://oauth.example",
            "authorization_endpoint": "https://oauth.example/authorize",
            "token_endpoint": "https://oauth.example/token",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": ["none"],
        })),
        "https://oauth.example/token" => json_status(400, answer.clone()),
        other => panic!("Unexpected test request: {other}"),
    }
}

/// A token that can be renewed, past its expiry.
async fn renewable(core: &TestCore) -> Mcp {
    let access_token = encrypt(core, "access-old");
    let refresh_token = encrypt(core, "refresh-old");
    create_mcp(core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.oauth_issuer = Some("https://oauth.example".into());
        mcp.oauth_authorize_url = Some("https://oauth.example/authorize".into());
        mcp.oauth_token_url = Some("https://oauth.example/token".into());
        mcp.oauth_client_id = Some("client-test".into());
        mcp.oauth_client_auth_method = Some("none".into());
        mcp.oauth_resource = Some("https://mcp.example/mcp".into());
        mcp.oauth_access_token = access_token;
        mcp.oauth_refresh_token = refresh_token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await
}

#[tokio::test]
async fn marks_an_mcp_whose_provider_refuses_to_renew_its_token() {
    let core = TestCore::new().await;

    let (upstream, _) = upstream(&core, refused_renewal(Some("Token has been revoked")));
    let mut mcp = renewable(&core).await;
    upstream.test_and_update_status(&mut mcp).await.unwrap();
    for row in [&mcp, &find_mcp(&core, mcp.id).await] {
        assert_eq!(row.status, McpStatus::Error);
        assert!(row.oauth_required);
        assert_eq!(
            row.last_error.as_deref(),
            Some("OAuth refresh token rejected (invalid_grant): Token has been revoked")
        );
    }

    // A call finds out the same, without a test.
    let mut called = renewable(&core).await;
    let error = upstream
        .call_tool(&mut called, "search", None)
        .await
        .unwrap_err();
    assert!(!error.is_unauthorized());
    assert!(error.needs_authorization());
    let saved = find_mcp(&core, called.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert!(saved.oauth_required);

    // The provider gave no reason.
    let (upstream, _) = upstream_without_reason(&core);
    let mut mcp = renewable(&core).await;
    upstream.test_and_update_status(&mut mcp).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Error);
    assert!(mcp.oauth_required);
    assert_eq!(
        mcp.last_error.as_deref(),
        Some("OAuth refresh token rejected (invalid_grant).")
    );
}

fn upstream_without_reason(core: &TestCore) -> (Arc<Upstream>, Calls) {
    upstream(core, refused_renewal(None))
}

#[tokio::test]
async fn does_not_take_another_failure_of_the_renewal_for_a_lapsed_sign_in() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |call| match call.url.as_str() {
        "https://oauth.example/token" => status(500),
        _ => refused_renewal(None)(call),
    });
    let mut mcp = renewable(&core).await;

    let error = upstream.probe(&mut mcp).await.unwrap_err();

    assert!(!error.needs_authorization());
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);
}

#[test]
fn tells_an_agent_what_to_do_when_the_sign_in_of_an_mcp_no_longer_works() {
    let signed_in = |access_token: Option<&str>| Mcp {
        slug: "t3-code".into(),
        transport: McpTransport::Http,
        auth_type: McpAuthType::Auto,
        oauth_access_token: access_token.map(str::to_owned),
        ..Default::default()
    };
    let expired = UpstreamError::OAuthSignInExpired(Timestamp::now());
    let rejected = UpstreamError::Unauthorized("MCP server returned HTTP 401 Unauthorized.".into());

    for error in [&expired, &rejected] {
        assert_eq!(
            authorization_needed_message(&signed_in(Some("ciphertext")), error).as_deref(),
            Some(
                "MCP \"t3-code\" needs to be re-authorized in MyMCPs: its OAuth sign-in has expired or was revoked. Re-authorize it on the MCPs page of MyMCPs, then try again."
            )
        );
        for no_token in [None, Some("")] {
            assert_eq!(
                authorization_needed_message(&signed_in(no_token), error).as_deref(),
                Some(
                    "MCP \"t3-code\" needs to be connected in MyMCPs: it signs in with OAuth and no account is connected yet. Connect it on the MCPs page of MyMCPs, then try again."
                )
            );
        }
    }

    // Another failure, or an MCP that does not sign in with OAuth.
    let other = UpstreamError::Other("boom".into());
    assert_eq!(
        authorization_needed_message(&signed_in(Some("ciphertext")), &other),
        None
    );
    for adjust in [
        |mcp: &mut Mcp| mcp.auth_type = McpAuthType::Bearer,
        |mcp: &mut Mcp| mcp.auth_type = McpAuthType::Header,
        |mcp: &mut Mcp| mcp.transport = McpTransport::Npm,
        |mcp: &mut Mcp| mcp.transport = McpTransport::Builtin,
    ] {
        let mut mcp = signed_in(Some("ciphertext"));
        adjust(&mut mcp);
        assert_eq!(authorization_needed_message(&mcp, &expired), None);
    }
}
