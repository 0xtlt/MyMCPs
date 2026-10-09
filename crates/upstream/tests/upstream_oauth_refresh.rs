//! `tests/unit/upstream_oauth_refresh.spec.ts`: one rotation of the refresh
//! token per MCP, whoever asks and however many ask at once.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::Duration;
use futures::future::join_all;
use mymcps_core::models::Mcp;
use mymcps_core::{TestCore, Timestamp};
use mymcps_net::{AddressGuard, CannedResponse, Fetcher};
use mymcps_upstream::{Upstream, UpstreamError};
use serde_json::{Value, json};
use support::*;
use tokio::sync::Notify;

async fn connection(core: &TestCore) -> Mcp {
    let access_token = encrypt(core, "access-old");
    let refresh_token = encrypt(core, "refresh-old");
    // The saved resource indicator has to be the MCP URL or a parent of it.
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

fn oauth_metadata() -> Value {
    json!({
        "issuer": "https://oauth.example",
        "authorization_endpoint": "https://oauth.example/authorize",
        "token_endpoint": "https://oauth.example/token",
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
    })
}

/// The provider at oauth.example, whose token endpoint answers with `token`.
fn oauth_server(
    core: &TestCore,
    token: impl Fn(&Call) -> CannedResponse + Send + Sync + 'static,
) -> (Arc<Upstream>, Calls) {
    upstream(core, move |call| match call.url.as_str() {
        "https://oauth.example/.well-known/oauth-authorization-server" => {
            json_response(oauth_metadata())
        }
        "https://oauth.example/token" => token(call),
        other => panic!("Unexpected test request: {other}"),
    })
}

#[tokio::test]
async fn uses_the_reloaded_endpoint_with_the_reloaded_credentials() {
    let core = TestCore::new().await;
    let mut mcp = connection(&core).await;
    let mut stale = find_mcp(&core, mcp.id).await;
    mcp.http_url = Some("https://new-mcp.example/mcp".into());
    mcp.oauth_access_token = encrypt(&core, "new-endpoint-access");
    mcp.oauth_token_expires_at = Some(Timestamp::now() + Duration::minutes(10));
    mcp.save(&*core.db).await.unwrap();

    let (upstream, calls) = upstream(&core, |call| mcp_answer(call, &json!([]), &json!({})));

    let connected = upstream.connect_http_upstream(&mut stale).await.unwrap();
    connected.close().await;

    let destinations = calls.all();
    assert!(!destinations.is_empty());
    for request in &destinations {
        assert_eq!(request.url, "https://new-mcp.example/mcp");
        assert_eq!(
            request.header("authorization").as_deref(),
            Some("Bearer new-endpoint-access")
        );
    }
    // The caller goes on with the row the refresh read.
    assert_eq!(
        stale.http_url.as_deref(),
        Some("https://new-mcp.example/mcp")
    );
}

#[tokio::test]
async fn rotates_once_for_concurrent_calls_and_reloads_waiting_and_stale_model_instances() {
    let core = TestCore::new().await;
    let mcp = connection(&core).await;
    let mut callers = Vec::new();
    for _ in 0..8 {
        callers.push(find_mcp(&core, mcp.id).await);
    }
    let mut stale = find_mcp(&core, mcp.id).await;
    let token_calls = Arc::new(AtomicUsize::new(0));
    let (upstream, _) = oauth_server(&core, {
        let token_calls = token_calls.clone();
        move |call| {
            let calls = token_calls.fetch_add(1, Ordering::SeqCst) + 1;
            assert_eq!(
                call.form("resource").as_deref(),
                Some("https://mcp.example/mcp")
            );
            assert_eq!(call.form("refresh_token").as_deref(), Some("refresh-old"));
            assert_eq!(call.form("grant_type").as_deref(), Some("refresh_token"));
            assert_eq!(call.form("client_id").as_deref(), Some("client-test"));
            // Like Cygnus, reject a repeated use of the rotating refresh token.
            if calls > 1 {
                return json_status(400, json!({ "error": "invalid_grant" }));
            }
            json_response(json!({
                "access_token": "access-new",
                "refresh_token": "refresh-new",
                "token_type": "Bearer",
                "expires_in": 600,
            }))
        }
    });

    let outcomes = join_all(
        callers
            .iter_mut()
            .map(|caller| upstream.refresh_oauth_access_token(caller)),
    )
    .await;
    for outcome in outcomes {
        outcome.unwrap();
    }
    upstream
        .refresh_oauth_access_token(&mut stale)
        .await
        .unwrap();

    assert_eq!(token_calls.load(Ordering::SeqCst), 1);
    for caller in callers.iter().chain([&stale]) {
        assert_eq!(
            decrypt(&core, &caller.oauth_access_token).as_deref(),
            Some("access-new")
        );
        assert_eq!(
            decrypt(&core, &caller.oauth_refresh_token).as_deref(),
            Some("refresh-new")
        );
    }
}

#[tokio::test]
async fn shares_refresh_failures_then_releases_the_failed_operation_for_a_later_attempt() {
    let core = TestCore::new().await;
    let mut mcp = connection(&core).await;
    let mut callers = Vec::new();
    for _ in 0..3 {
        callers.push(find_mcp(&core, mcp.id).await);
    }
    let token_calls = Arc::new(AtomicUsize::new(0));
    let reject = Arc::new(AtomicBool::new(true));
    let (upstream, _) = oauth_server(&core, {
        let (token_calls, reject) = (token_calls.clone(), reject.clone());
        move |_| {
            token_calls.fetch_add(1, Ordering::SeqCst);
            if reject.load(Ordering::SeqCst) {
                return json_status(503, json!({ "error": "temporarily_unavailable" }));
            }
            json_response(json!({
                "access_token": "recovered-access",
                "refresh_token": "recovered-refresh",
                "token_type": "Bearer",
                "expires_in": 600,
            }))
        }
    });

    for caller in &mut callers {
        caller.description = Some("changed, not saved".into());
    }
    let results = join_all(
        callers
            .iter_mut()
            .map(|caller| upstream.refresh_oauth_access_token(caller)),
    )
    .await;
    assert!(results.iter().all(Result::is_err));
    assert_eq!(token_calls.load(Ordering::SeqCst), 1);
    // Every caller is told why the one renewal failed.
    for result in &results {
        let error = result.as_ref().unwrap_err();
        assert!(matches!(error, UpstreamError::OAuth(_)), "{error:?}");
    }
    // Nobody goes on with the token that could not be renewed, and it stays saved.
    for caller in &callers {
        assert_eq!(
            decrypt(&core, &caller.oauth_refresh_token).as_deref(),
            Some("refresh-old")
        );
    }
    // The caller that started the renewal had its row read again for it,
    // as in the Node app. The ones that joined keep the copy they came with.
    assert_eq!(callers[0].description, None);
    assert_eq!(
        callers[1].description.as_deref(),
        Some("changed, not saved")
    );

    reject.store(false, Ordering::SeqCst);
    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    assert_eq!(token_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("recovered-access")
    );
}

#[tokio::test]
async fn does_not_block_a_different_mcp_while_one_token_endpoint_is_pending() {
    let core = TestCore::new().await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    // A provider that really waits, which an answer from a closure cannot do.
    let provider = local_server({
        let (started, release) = (started.clone(), release.clone());
        move |request: Received| {
            let (started, release) = (started.clone(), release.clone());
            async move {
                if request.path() != "/token" {
                    return respond(404, &[], "not found");
                }
                if request.form("refresh_token").as_deref() == Some("refresh-old") {
                    started.notify_one();
                    release.notified().await;
                }
                respond_json(
                    200,
                    json!({
                        "access_token": "access-new",
                        "refresh_token": "refresh-new",
                        "token_type": "Bearer",
                        "expires_in": 600,
                    }),
                )
            }
        }
    })
    .await;

    // The MCP and its provider are on the loopback address, where the
    // operator put them: nothing of theirs is refused.
    let on_provider = |mcp: &mut Mcp| {
        mcp.http_url = Some(provider.url("/mcp"));
        mcp.oauth_issuer = Some(provider.origin());
        mcp.oauth_authorize_url = Some(provider.url("/authorize"));
        mcp.oauth_token_url = Some(provider.url("/token"));
        mcp.oauth_resource = Some(provider.url("/mcp"));
    };
    let mut first = connection(&core).await;
    on_provider(&mut first);
    first.save(&*core.db).await.unwrap();
    let mut second = connection(&core).await;
    on_provider(&mut second);
    second.oauth_refresh_token = encrypt(&core, "second-refresh");
    second.save(&*core.db).await.unwrap();

    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .fetcher(Fetcher::shared())
        .address_guard(AddressGuard::system())
        .build();

    let pending = tokio::spawn({
        let upstream = upstream.clone();
        async move {
            upstream
                .refresh_oauth_access_token(&mut first)
                .await
                .map(|()| first)
        }
    });
    started.notified().await;
    upstream
        .refresh_oauth_access_token(&mut second)
        .await
        .unwrap();
    assert_eq!(
        decrypt(&core, &second.oauth_access_token).as_deref(),
        Some("access-new")
    );
    assert!(!pending.is_finished());

    release.notify_one();
    let first = pending.await.unwrap().unwrap();
    assert_eq!(
        decrypt(&core, &first.oauth_access_token).as_deref(),
        Some("access-new")
    );
    assert_eq!(
        provider
            .requests()
            .iter()
            .filter(|request| request.path() == "/token")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_rotation_that_reached_the_provider_is_saved_when_its_caller_is_gone() {
    let core = TestCore::new().await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let provider = local_server({
        let (started, release) = (started.clone(), release.clone());
        move |request: Received| {
            let (started, release) = (started.clone(), release.clone());
            async move {
                if request.path() != "/token" {
                    return respond(404, &[], "not found");
                }
                started.notify_one();
                release.notified().await;
                respond_json(
                    200,
                    json!({
                        "access_token": "access-new",
                        "refresh_token": "refresh-new",
                        "token_type": "Bearer",
                        "expires_in": 600,
                    }),
                )
            }
        }
    })
    .await;
    let mut mcp = connection(&core).await;
    mcp.http_url = Some(provider.url("/mcp"));
    mcp.oauth_issuer = Some(provider.origin());
    mcp.oauth_authorize_url = Some(provider.url("/authorize"));
    mcp.oauth_token_url = Some(provider.url("/token"));
    mcp.oauth_resource = Some(provider.url("/mcp"));
    mcp.save(&*core.db).await.unwrap();
    let id = mcp.id;
    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .fetcher(Fetcher::shared())
        .address_guard(AddressGuard::system())
        .build();

    // The request that asked for the renewal is dropped while the provider
    // answers, as when its client disconnects.
    let caller = tokio::spawn({
        let upstream = upstream.clone();
        async move { upstream.refresh_oauth_access_token(&mut mcp).await }
    });
    started.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    release.notify_one();
    let mut saved = find_mcp(&core, id).await;
    for _ in 0..500 {
        if decrypt(&core, &saved.oauth_refresh_token).as_deref() == Some("refresh-new") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        saved = find_mcp(&core, id).await;
    }
    assert_eq!(
        decrypt(&core, &saved.oauth_refresh_token).as_deref(),
        Some("refresh-new")
    );
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-new")
    );

    // The next caller starts from the saved pair, without another rotation.
    upstream
        .refresh_oauth_access_token(&mut saved)
        .await
        .unwrap();
    assert_eq!(
        provider
            .requests()
            .iter()
            .filter(|request| request.path() == "/token")
            .count(),
        1
    );
}

#[tokio::test]
async fn fails_for_an_mcp_that_was_deleted_while_a_caller_held_it() {
    let core = TestCore::new().await;
    let mut mcp = connection(&core).await;
    let (upstream, calls) = oauth_server(&core, |_| status(500));
    mcp.delete(&*core.db).await.unwrap();

    let error = upstream
        .refresh_oauth_access_token(&mut mcp)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "\"Model.refresh\" failed. Unable to lookup \"mcps\" table where \"id\" = {}",
            mcp.id
        )
    );
    assert_eq!(calls.len(), 0);

    // The failed renewal is released: the next call asks again.
    assert!(upstream.refresh_oauth_access_token(&mut mcp).await.is_err());
}

#[tokio::test]
async fn leaves_alone_what_is_not_an_expired_oauth_sign_in() {
    let core = TestCore::new().await;
    let (upstream, calls) = oauth_server(&core, |_| status(500));

    // A static credential: the row is read again, and that is all.
    let mut bearer = connection(&core).await;
    bearer.auth_type = mymcps_core::models::McpAuthType::Bearer;
    bearer.save(&*core.db).await.unwrap();
    upstream
        .refresh_oauth_access_token(&mut bearer)
        .await
        .unwrap();

    // No refresh token, no client, or no provider to ask.
    for incomplete in [
        |mcp: &mut Mcp| mcp.oauth_refresh_token = None,
        |mcp: &mut Mcp| mcp.oauth_refresh_token = Some("not ciphertext".into()),
        |mcp: &mut Mcp| mcp.oauth_client_id = None,
        |mcp: &mut Mcp| {
            mcp.oauth_issuer = None;
            mcp.oauth_authorize_url = None;
            mcp.oauth_token_url = None;
        },
    ] {
        let mut mcp = connection(&core).await;
        incomplete(&mut mcp);
        mcp.save(&*core.db).await.unwrap();
        upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
        assert_eq!(
            decrypt(&core, &mcp.oauth_access_token).as_deref(),
            Some("access-old")
        );
    }

    // More than two minutes left: the token is used as it is.
    let mut fresh = connection(&core).await;
    fresh.oauth_token_expires_at = Some(Timestamp::now() + Duration::minutes(3));
    fresh.save(&*core.db).await.unwrap();
    upstream
        .refresh_oauth_access_token(&mut fresh)
        .await
        .unwrap();
    assert_eq!(calls.len(), 0);

    // Less than two minutes left: it is renewed before it is used.
    let mut expiring = connection(&core).await;
    expiring.oauth_token_expires_at = Some(Timestamp::now() + Duration::seconds(90));
    expiring.save(&*core.db).await.unwrap();
    assert!(
        upstream
            .refresh_oauth_access_token(&mut expiring)
            .await
            .is_err()
    );
    assert_eq!(calls.posts().len(), 1);
}
