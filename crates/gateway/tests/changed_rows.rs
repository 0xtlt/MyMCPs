//! The cases of `tests/functional/changed_rows.spec.ts` that need no hold on
//! the order of two requests: how many records a pruning removed, and two
//! requests that really run at once. Those that give a concurrent request
//! its turn at a chosen moment are in `src/concurrency_tests.rs`.
//!
//! Its first case compares the two query builders of Lucid, which answer
//! differently for the same write. Here every write answers with
//! `rows_affected()`.

mod support;

use std::sync::Arc;

use chrono::Duration;
use mymcps_core::Timestamp;
use mymcps_core::models::InstanceSetting;
use mymcps_gateway::Error;
use serde_json::{Value, json};
use support::*;

const REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const CODE_VERIFIER: &str = "changed-rows-code-verifier-for-mymcps-tests-123456";

#[tokio::test]
async fn reports_how_many_expired_call_logs_were_pruned() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_stored_access_token(db, admin.id, |_| {}).await;
    let expired = Timestamp::now() - Duration::days(60);
    create_mcp_call_log(db, &token, Some(expired)).await;
    create_mcp_call_log(db, &token, Some(expired)).await;
    create_mcp_call_log(db, &token, Some(expired)).await;
    create_mcp_call_log(db, &token, None).await;

    assert_eq!(gateway.call_log.prune_expired(true).await, 3);
    assert_eq!(gateway.call_log.prune_expired(true).await, 0);
    assert_eq!(count(db, "mcp_call_logs").await, 1);
}

#[tokio::test]
async fn prunes_call_logs_past_the_retention_of_the_instance_at_most_once_an_hour() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let token = create_stored_access_token(db, admin.id, |_| {}).await;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    assert_eq!(settings.mcp_log_retention_days, 14);
    settings.mcp_log_retention_days = 3;
    settings.save(&**db).await.unwrap();

    let old = |days: i64| Some(Timestamp::now() - Duration::days(days));
    create_mcp_call_log(db, &token, old(4)).await;
    create_mcp_call_log(db, &token, old(2)).await;
    assert_eq!(gateway.call_log.prune_expired(false).await, 1);

    create_mcp_call_log(db, &token, old(4)).await;
    assert_eq!(gateway.call_log.prune_expired(false).await, 0);
    assert_eq!(count(db, "mcp_call_logs").await, 2);
    assert_eq!(gateway.call_log.prune_expired(true).await, 1);
    assert_eq!(count(db, "mcp_call_logs").await, 1);
}

/// A registered client with a grant: its `client_id` and its refresh token.
async fn connect(gateway: &TestGateway, client_name: &str) -> (String, String) {
    let admin = create_admin(gateway.db()).await;
    let registered = register(
        gateway,
        &registration(client_name, "http://127.0.0.1/callback"),
    )
    .await;
    let client_id = registered["client_id"].as_str().unwrap().to_owned();
    let code = approve(
        gateway,
        &authorization(&client_id, REDIRECT_URI, &code_challenge(CODE_VERIFIER)),
        admin.id,
    )
    .await;
    let exchange = gateway
        .oauth
        .issue_tokens(None, &code_grant(&client_id, &code))
        .await
        .unwrap();
    (client_id, exchange.refresh_token.unwrap())
}

fn code_grant(client_id: &str, code: &str) -> Value {
    json!({
        "grant_type": "authorization_code",
        "client_id": client_id,
        "code": code,
        "code_verifier": CODE_VERIFIER,
        "redirect_uri": REDIRECT_URI,
        "resource": RESOURCE,
    })
}

fn is_invalid_grant<T>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(Error::Oauth(error)) if error.code == "invalid_grant" && error.status == 400)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rotates_a_grant_once_when_its_refresh_token_is_presented_many_times_at_once() {
    let gateway = Arc::new(TestGateway::new().await);
    let (client_id, refresh_token) = connect(&gateway, "Racing refresh client").await;
    let grant = json!({
        "grant_type": "refresh_token",
        "client_id": client_id,
        "refresh_token": refresh_token,
        "resource": RESOURCE,
    });

    let requests: Vec<_> = (0..8)
        .map(|_| {
            let gateway = gateway.clone();
            let grant = grant.clone();
            tokio::spawn(async move { gateway.oauth.issue_tokens(None, &grant).await })
        })
        .collect();
    let mut answers = Vec::new();
    for request in requests {
        answers.push(request.await.unwrap());
    }

    assert_eq!(answers.iter().filter(|answer| answer.is_ok()).count(), 1);
    assert_eq!(
        answers
            .iter()
            .filter(|answer| is_invalid_grant(answer))
            .count(),
        7
    );

    // The grant was rotated once. A request that came after that is a replay
    // and revokes the grant; `src/concurrency_tests.rs` has that case.
    let db = gateway.db();
    assert_eq!(count(db, "oauth_refresh_token_history").await, 1);
    assert_eq!(count(db, "access_tokens").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issues_one_grant_when_an_authorization_code_is_exchanged_many_times_at_once() {
    let gateway = Arc::new(TestGateway::new().await);
    let admin = create_admin(gateway.db()).await;
    let registered = register(
        &gateway,
        &registration("Racing code client", "http://127.0.0.1/callback"),
    )
    .await;
    let client_id = registered["client_id"].as_str().unwrap().to_owned();
    let code = approve(
        &gateway,
        &authorization(&client_id, REDIRECT_URI, &code_challenge(CODE_VERIFIER)),
        admin.id,
    )
    .await;

    let requests: Vec<_> = (0..8)
        .map(|_| {
            let gateway = gateway.clone();
            let grant = code_grant(&client_id, &code);
            tokio::spawn(async move { gateway.oauth.issue_tokens(None, &grant).await })
        })
        .collect();
    let mut answers = Vec::new();
    for request in requests {
        answers.push(request.await.unwrap());
    }

    assert_eq!(answers.iter().filter(|answer| answer.is_ok()).count(), 1);
    assert_eq!(
        answers
            .iter()
            .filter(|answer| is_invalid_grant(answer))
            .count(),
        7
    );
    let db = gateway.db();
    assert_eq!(count(db, "access_tokens").await, 1);
    assert_eq!(count(db, "oauth_authorization_codes").await, 0);
}
