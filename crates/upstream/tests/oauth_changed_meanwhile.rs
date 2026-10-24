//! The sign-in of a built-in MCP, when it is changed while a step of the
//! OAuth flow is at the provider. The exchange of a code and the renewal of
//! a token save what the provider issued; once the sign-in they started from
//! was cleared or replaced, that is not theirs to save. (The HTTP MCPs, whose
//! URL and network can change too, are tested through the page, in
//! `crates/web/tests/mcp_oauth_changed_meanwhile.rs`.)

mod support;

use std::sync::Arc;

use chrono::Duration;
use mymcps_core::models::Mcp;
use mymcps_core::{TestCore, Timestamp};
use mymcps_net::CannedResponse;
use mymcps_upstream::{Upstream, read_oauth_session};
use serde_json::json;
use support::*;
use url::Url;

const ISSUED: &str = "issued-while-the-sign-in-changed";

/// The example provider, which answers its token endpoint late.
fn late_provider(core: &TestCore, gate: Arc<Gate>) -> (Arc<Upstream>, Calls) {
    upstream_with(
        core,
        public_names(),
        providers::registry(),
        move |call| -> CannedResponse {
            match call.url.as_str() {
                "https://www.example.test/oauth/token" => {
                    gate.hold();
                    json_response(json!({
                        "token_type": "Bearer",
                        "access_token": ISSUED,
                        "refresh_token": ISSUED,
                        "expires_in": 21600,
                    }))
                }
                _ => not_found(),
            }
        },
    )
}

/// A connected MCP of the example provider.
async fn connected(core: &TestCore, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    create_mcp(core, |mcp| {
        mcp.name = "Example".into();
        mcp.transport = mymcps_core::models::McpTransport::Builtin;
        mcp.builtin_key = Some("example".into());
        mcp.oauth_client_id = Some("123456".into());
        mcp.oauth_client_secret = encrypt(core, "example-client-secret");
        mcp.oauth_access_token = encrypt(core, "example-access-token");
        mcp.oauth_refresh_token = encrypt(core, "example-refresh-token");
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
        adjust(mcp);
    })
    .await
}

/// Edit the row as another request does, while the step is at the provider.
async fn edit(core: &TestCore, id: i64, change: impl FnOnce(&mut Mcp)) {
    let mut mcp = find_mcp(core, id).await;
    change(&mut mcp);
    mcp.save(&*core.db).await.unwrap();
}

fn assert_nothing_issued_was_saved(core: &TestCore, saved: &Mcp) {
    for token in [&saved.oauth_access_token, &saved.oauth_refresh_token] {
        assert_ne!(decrypt(core, token).as_deref(), Some(ISSUED));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_a_renewal_answered_after_the_sign_in_was_cleared() {
    let core = TestCore::new().await;
    let gate = Arc::new(Gate::default());
    let (upstream, _) = late_provider(&core, gate.clone());
    let mcp = connected(&core, |_| {}).await;

    let renewing = {
        let upstream = upstream.clone();
        let mut mcp = find_mcp(&core, mcp.id).await;
        gate.arm();
        tokio::spawn(async move {
            upstream
                .refresh_oauth_access_token(&mut mcp)
                .await
                .map_err(|error| error.to_string())
        })
    };
    gate.reached().await;
    edit(&core, mcp.id, |mcp| {
        mcp.oauth_access_token = None;
        mcp.oauth_refresh_token = None;
        mcp.oauth_token_expires_at = None;
        mcp.oauth_client_id = None;
        mcp.oauth_client_secret = None;
    })
    .await;
    gate.release();

    let error = renewing.await.unwrap().unwrap_err();
    assert!(error.contains("changed while"), "{error}");
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.oauth_refresh_token, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_a_renewal_answered_after_the_sign_in_was_made_again() {
    let core = TestCore::new().await;
    let gate = Arc::new(Gate::default());
    let (upstream, _) = late_provider(&core, gate.clone());
    let mcp = connected(&core, |_| {}).await;

    let renewing = {
        let upstream = upstream.clone();
        let mut mcp = find_mcp(&core, mcp.id).await;
        gate.arm();
        tokio::spawn(async move { upstream.refresh_oauth_access_token(&mut mcp).await })
    };
    gate.reached().await;
    // Another sign-in finished first: its tokens are the ones to keep.
    let replaced = encrypt(&core, "replaced-refresh-token");
    edit(&core, mcp.id, |mcp| {
        mcp.oauth_refresh_token = replaced;
    })
    .await;
    gate.release();

    renewing.await.unwrap().unwrap_err();
    let saved = find_mcp(&core, mcp.id).await;
    assert_nothing_issued_was_saved(&core, &saved);
    assert_eq!(
        decrypt(&core, &saved.oauth_refresh_token).as_deref(),
        Some("replaced-refresh-token")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_a_code_exchanged_after_the_application_credentials_were_replaced() {
    let core = TestCore::new().await;
    let gate = Arc::new(Gate::default());
    let (upstream, calls) = late_provider(&core, gate.clone());
    let mcp = connected(&core, |mcp| {
        mcp.oauth_access_token = None;
        mcp.oauth_refresh_token = None;
        mcp.oauth_token_expires_at = None;
        mcp.oauth_required = true;
    })
    .await;
    let session = MemorySession::new();
    let mut starting = find_mcp(&core, mcp.id).await;
    let redirect = upstream
        .start_oauth_flow(&session, &mut starting)
        .await
        .unwrap();
    let state = query(&Url::parse(&redirect).unwrap(), "state").unwrap();
    let oauth = read_oauth_session(&session, Some(&state)).unwrap();

    let exchanging = {
        let upstream = upstream.clone();
        let mut mcp = find_mcp(&core, mcp.id).await;
        gate.arm();
        tokio::spawn(async move {
            upstream
                .exchange_authorization_code(&mut mcp, &oauth, "the-code", None)
                .await
                .map_err(|error| error.to_string())
        })
    };
    gate.reached().await;
    // The admin entered the secret of another application of the provider.
    let replaced = encrypt(&core, "another-client-secret");
    edit(&core, mcp.id, |mcp| {
        mcp.oauth_client_secret = replaced;
    })
    .await;
    gate.release();

    let error = exchanging.await.unwrap().unwrap_err();
    assert!(error.contains("changed while"), "{error}");
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.oauth_refresh_token, None);
    assert!(saved.oauth_required);
    assert_eq!(
        calls.to("https://www.example.test/oauth/token").len(),
        1,
        "the code was spent once"
    );
}
