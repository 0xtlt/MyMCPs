//! The cases of `tests/functional/changed_rows.spec.ts` in which a second
//! request gets its turn between the moment the first has read a grant or a
//! code and the moment it writes it.

#[path = "../tests/support/mod.rs"]
mod support;

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};
use support::*;
use tokio::sync::oneshot;

use crate::access_token::{self, CreatedOauthTokens};
use crate::db::concurrency::before_next_transaction;
use crate::error::Result;

const REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const CODE_VERIFIER: &str = "changed-rows-code-verifier-for-mymcps-tests-123456";

async fn register_client(gateway: &TestGateway, client_name: &str) -> String {
    let registered = register(
        gateway,
        &registration(client_name, "http://127.0.0.1/callback"),
    )
    .await;
    registered["client_id"].as_str().unwrap().to_owned()
}

async fn authorization_code(gateway: &TestGateway, client_id: &str) -> String {
    let admin = create_admin(gateway.db()).await;
    approve(
        gateway,
        &authorization(client_id, REDIRECT_URI, &code_challenge(CODE_VERIFIER)),
        admin.id,
    )
    .await
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

fn refresh_grant(client_id: &str, refresh_token: &str) -> Value {
    json!({
        "grant_type": "refresh_token",
        "client_id": client_id,
        "refresh_token": refresh_token,
        "resource": RESOURCE,
    })
}

/// A registered client with a grant: its `client_id` and its refresh token.
async fn connect(gateway: &TestGateway, client_name: &str) -> (String, String) {
    let client_id = register_client(gateway, client_name).await;
    let code = authorization_code(gateway, &client_id).await;
    let exchange = gateway
        .oauth
        .issue_tokens(None, &code_grant(&client_id, &code))
        .await
        .unwrap();
    (client_id, exchange.refresh_token.unwrap())
}

/// Send `grant` to the token endpoint, and once more just before the first
/// request opens its transaction. Returns the answers to the request that
/// was let through first and to the one that was overtaken.
async fn race(
    gateway: &Arc<TestGateway>,
    grant: &Value,
) -> (Result<CreatedOauthTokens>, Result<CreatedOauthTokens>) {
    let (answer, answered) = oneshot::channel();
    let overtaking = {
        let gateway = gateway.clone();
        let grant = grant.clone();
        async move {
            let _ = answer.send(gateway.oauth.issue_tokens(None, &grant).await);
        }
    };
    let overtaken = overtake(overtaking, gateway.oauth.issue_tokens(None, grant)).await;
    (answered.await.unwrap(), overtaken)
}

async fn overtake<T>(
    overtaking: impl Future<Output = ()> + Send + 'static,
    request: impl Future<Output = T>,
) -> T {
    before_next_transaction(overtaking, request).await
}

async fn refresh_history(gateway: &TestGateway, grant_id: i64) -> i64 {
    sqlx::query_scalar(
        "select count(*) from `oauth_refresh_token_history` where `access_token_id` = ?",
    )
    .bind(grant_id)
    .fetch_one(&**gateway.db())
    .await
    .unwrap()
}

#[tokio::test]
async fn treats_a_refresh_that_loses_a_concurrent_rotation_as_a_replay() {
    let gateway = Arc::new(TestGateway::new().await);
    let (client_id, refresh_token) = connect(&gateway, "Racing refresh client").await;

    let (winner, loser) = race(&gateway, &refresh_grant(&client_id, &refresh_token)).await;

    winner.unwrap();
    assert_oauth_error(
        loser,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    let grant = find_token_by_name(gateway.db(), "Racing refresh client").await;
    assert!(grant.is_revoked());
    assert_eq!(refresh_history(&gateway, grant.id).await, 1);
}

#[tokio::test]
async fn issues_no_tokens_when_the_grant_is_revoked_while_its_refresh_token_rotates() {
    let gateway = Arc::new(TestGateway::new().await);
    let (client_id, refresh_token) = connect(&gateway, "Revoked refresh client").await;
    let before = find_token_by_name(gateway.db(), "Revoked refresh client").await;

    let revocation = {
        let gateway = gateway.clone();
        let mut grant = before.clone();
        async move {
            access_token::revoke(gateway.db(), &mut grant)
                .await
                .unwrap();
        }
    };
    let response = overtake(
        revocation,
        gateway
            .oauth
            .issue_tokens(None, &refresh_grant(&client_id, &refresh_token)),
    )
    .await;

    assert_oauth_error(
        response,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    let grant = find_token(gateway.db(), before.id).await;
    assert!(grant.is_revoked());
    assert_eq!(grant.token_hash, before.token_hash);
    assert_eq!(
        grant.oauth_refresh_token_hash,
        Some(access_token::hash(&refresh_token))
    );
    assert_eq!(refresh_history(&gateway, grant.id).await, 0);
}

#[tokio::test]
async fn issues_one_grant_when_an_authorization_code_is_exchanged_twice_at_once() {
    let gateway = Arc::new(TestGateway::new().await);
    let client_id = register_client(&gateway, "Racing code client").await;
    let code = authorization_code(&gateway, &client_id).await;

    let (winner, loser) = race(&gateway, &code_grant(&client_id, &code)).await;

    winner.unwrap();
    assert_oauth_error(
        loser,
        400,
        "invalid_grant",
        "Authorization code was already used",
    );
    let grants: i64 = sqlx::query_scalar(
        "select count(*) from `access_tokens` where `name` = 'Racing code client'",
    )
    .fetch_one(&**gateway.db())
    .await
    .unwrap();
    assert_eq!(grants, 1);
}
