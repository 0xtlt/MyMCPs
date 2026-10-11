//! `tests/functional/hardening_gateway_timezone.spec.ts`: expiries are
//! written and read in UTC, whatever the time zone of the server.

mod support;

use chrono::Utc;
use mymcps_core::models::{OauthAuthorizationCode, OauthClient};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::oauth::GatewayAuthorizationRequest;
use serde_json::json;
use support::*;

#[tokio::test]
async fn reads_token_and_code_expiries_back_from_the_database_unchanged() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut client = OauthClient {
        client_id: "mcp_client_timezone".into(),
        client_name: "Time zone client".into(),
        redirect_uris: json!(["http://127.0.0.1/callback"]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: json!(["authorization_code", "refresh_token"]).to_string(),
        response_types: json!(["code"]).to_string(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&**db).await.unwrap();

    let issued_at = Utc::now();
    let created = access_token::create_oauth_grant(
        &**db,
        NewOauthGrant {
            name: &client.client_name,
            client_id: client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: RESOURCE,
            created_by: admin.id,
        },
    )
    .await
    .unwrap();

    let token = find_token(db, created.token.id).await;
    let minutes_from_issue =
        |time: mymcps_core::Timestamp| (time.as_datetime() - issued_at).num_seconds() as f64 / 60.0;
    assert!(token.is_usable());
    assert!(
        access_token::find_usable_by_plaintext(db, &created.plaintext)
            .await
            .unwrap()
            .is_some()
    );
    assert!((minutes_from_issue(token.expires_at.unwrap()) - 60.0).abs() <= 1.0);
    assert!(
        (minutes_from_issue(token.oauth_refresh_expires_at.unwrap()) / (24.0 * 60.0) - 30.0).abs()
            <= 0.01
    );
    assert!(minutes_from_issue(token.created_at).abs() <= 1.0);

    // The columns hold UTC wall-clock text, as the Node app wrote it.
    let stored: String =
        sqlx::query_scalar("select `expires_at` from `access_tokens` where `id` = ?")
            .bind(token.id)
            .fetch_one(&**db)
            .await
            .unwrap();
    assert_eq!(stored, token.expires_at.unwrap().to_sql());
    assert_eq!(stored.len(), "2026-10-07 12:19:57".len());

    let plaintext = gateway
        .oauth
        .create_authorization_code(
            &GatewayAuthorizationRequest {
                client: client.clone(),
                redirect_uri: "http://127.0.0.1/callback".into(),
                state: None,
                code_challenge: "c".repeat(43),
                scopes: "mcp:tools".into(),
                resource: RESOURCE.into(),
            },
            admin.id,
        )
        .await
        .unwrap();
    let code: OauthAuthorizationCode =
        sqlx::query_as("select * from `oauth_authorization_codes` where `code_hash` = ?")
            .bind(access_token::hash(&plaintext))
            .fetch_one(&**db)
            .await
            .unwrap();
    assert!((minutes_from_issue(code.expires_at) - 5.0).abs() <= 1.0);
}

#[tokio::test]
async fn reads_the_expiries_an_earlier_version_wrote_with_milliseconds() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let live = create_stored_access_token(db, admin.id, |token| {
        token.token_hash = access_token::hash("mcp_live");
    })
    .await;
    let expired = create_stored_access_token(db, admin.id, |token| {
        token.token_hash = access_token::hash("mcp_expired");
    })
    .await;
    // What `toSQL({ includeOffset: false })` wrote on a refresh.
    sqlx::query(
        "update `access_tokens` set `expires_at` = '2999-01-01 00:00:00.123' where `id` = ?",
    )
    .bind(live.id)
    .execute(&**db)
    .await
    .unwrap();
    sqlx::query(
        "update `access_tokens` set `expires_at` = '2020-01-01 00:00:00.123' where `id` = ?",
    )
    .bind(expired.id)
    .execute(&**db)
    .await
    .unwrap();

    assert!(
        access_token::find_usable_by_plaintext(db, "mcp_live")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        access_token::find_usable_by_plaintext(db, "mcp_expired")
            .await
            .unwrap()
            .is_none()
    );
}
