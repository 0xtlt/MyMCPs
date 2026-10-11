//! `tests/functional/gateway_auth.spec.ts` and the request allowance of
//! `tests/functional/hardening_gateway_mcp.spec.ts`, at the level of
//! [`Gateway::authenticate_bearer`](mymcps_gateway::Gateway::authenticate_bearer).

mod support;

use chrono::Duration;
use mymcps_core::Timestamp;
use mymcps_core::models::ScopeMode;
use mymcps_gateway::access_token;
use mymcps_gateway::bearer::{BearerAccess, BearerError, BearerRejection};
use mymcps_gateway::rate_limiter::GATEWAY_REQUESTS_PER_MINUTE;
use serde_json::json;
use support::*;

fn rejection(result: Result<BearerAccess, BearerError>) -> BearerRejection {
    match result {
        Err(BearerError::Rejected(rejection)) => rejection,
        other => panic!("expected the request to be refused, got {other:?}"),
    }
}

// gateway bearer authentication

#[tokio::test]
async fn rejects_requests_without_a_bearer_token() {
    let gateway = TestGateway::new().await;

    for header in [
        None,
        Some(""),
        Some("Bearer"),
        Some("Basic abc"),
        Some("Bearerabc"),
    ] {
        let refused = rejection(gateway.authenticate_bearer(header).await);
        assert_eq!(refused.status(), 401);
        assert_eq!(
            refused.body(),
            json!({ "error": "unauthorized", "message": "Missing Bearer access token" })
        );
    }

    for header in ["Bearer ", "bearer   ", "BEARER \t"] {
        let refused = rejection(gateway.authenticate_bearer(Some(header)).await);
        assert_eq!(refused.status(), 401);
        assert_eq!(
            refused.body(),
            json!({ "error": "unauthorized", "message": "Empty Bearer access token" })
        );
    }
}

#[tokio::test]
async fn rejects_invalid_expired_and_revoked_token_values() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    create_stored_access_token(db, admin.id, |token| {
        token.token_hash = access_token::hash("mcp_expired");
        token.expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    create_stored_access_token(db, admin.id, |token| {
        token.token_hash = access_token::hash("mcp_revoked");
        token.revoked_at = Some(Timestamp::now());
    })
    .await;

    for value in ["not-a-token", "mcp_expired", "mcp_revoked"] {
        let refused = rejection(
            gateway
                .authenticate_bearer(Some(&format!("Bearer {value}")))
                .await,
        );
        assert_eq!(refused.status(), 401);
        assert_eq!(
            refused.body(),
            json!({
                "error": "unauthorized",
                "message": "Invalid, expired, or revoked access token",
            })
        );
    }
}

#[tokio::test]
async fn lets_a_usable_token_in_with_the_mcps_it_may_use() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let allowed = create_mcp(db, admin.id, |mcp| mcp.name = "Allowed".into()).await;
    create_mcp(db, admin.id, |mcp| mcp.name = "Other".into()).await;
    let created = create_access_token(db, admin.id, ScopeMode::Selected, &[allowed.id]).await;
    assert_eq!(created.token.last_used_at, None);

    // The scheme is read whatever its case, and the token without the
    // whitespace around it.
    for header in [
        format!("Bearer {}", created.plaintext),
        format!("bearer   {} ", created.plaintext),
        format!("BEARER {}", created.plaintext),
    ] {
        let access = gateway.authenticate_bearer(Some(&header)).await.unwrap();
        assert_eq!(access.access_token.id, created.token.id);
        assert_eq!(
            access
                .allowed_mcps
                .iter()
                .map(|mcp| mcp.id)
                .collect::<Vec<_>>(),
            [allowed.id]
        );
    }
    assert!(
        find_token(db, created.token.id)
            .await
            .last_used_at
            .is_some()
    );
}

// hardening: gateway requests

#[tokio::test]
async fn limits_the_requests_of_one_access_token_without_affecting_another() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let busy = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let other = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    for _ in 0..GATEWAY_REQUESTS_PER_MINUTE - 1 {
        gateway
            .rate_limiter
            .increment(&format!("mcp:{}", busy.token.id))
            .await
            .unwrap();
    }
    let busy_header = format!("Bearer {}", busy.plaintext);

    gateway
        .authenticate_bearer(Some(&busy_header))
        .await
        .unwrap();

    let refused = rejection(gateway.authenticate_bearer(Some(&busy_header)).await);
    assert_eq!(refused.status(), 429);
    assert_eq!(
        refused.body(),
        json!({
            "error": "rate_limited",
            "message": "Too many requests for this access token",
        })
    );
    let retry_after = refused.retry_after().unwrap();
    assert!(retry_after > 0 && retry_after <= 60);
    assert_eq!(refused.www_authenticate(&gateway.core.config), None);

    gateway
        .authenticate_bearer(Some(&format!("Bearer {}", other.plaintext)))
        .await
        .unwrap();
}

#[tokio::test]
async fn counts_requests_for_each_gateway_on_its_own() {
    let first = TestGateway::new().await;
    let second = TestGateway::new().await;
    assert_eq!(GATEWAY_REQUESTS_PER_MINUTE, 600);
    assert_eq!(first.rate_limiter.requests(), 600);
    for _ in 0..GATEWAY_REQUESTS_PER_MINUTE {
        first.rate_limiter.increment("mcp:1").await.unwrap();
    }

    assert_eq!(first.rate_limiter.remaining("mcp:1").await.unwrap(), 0);
    assert_eq!(
        second.rate_limiter.remaining("mcp:1").await.unwrap(),
        GATEWAY_REQUESTS_PER_MINUTE
    );
}
