//! `tests/functional/gateway_auth.spec.ts`, and the request allowance of
//! `tests/functional/hardening_gateway_mcp.spec.ts`, as `/mcp` answers them.

use chrono::Duration;
use http::{Method, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::ScopeMode;
use mymcps_gateway::access_token;
use mymcps_gateway::rate_limiter::GATEWAY_REQUESTS_PER_MINUTE;
use serde_json::json;

use crate::support::mcp::*;
use crate::support::*;

const CHALLENGE: &str = "resource_metadata=\"http://localhost:3333/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:tools\"";

fn initialize() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "hardening-test", "version": "1.0.0" },
        },
    })
}

// gateway bearer authentication

#[tokio::test]
async fn rejects_requests_without_a_bearer_token() {
    let gateway = TestMcpGateway::offline().await;

    for headers in [
        &[][..],
        &[("authorization", "Basic abc")],
        &[("authorization", "Bearerabc")],
    ] {
        let response = gateway.request(Method::GET, headers, None).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json(),
            json!({ "error": "unauthorized", "message": "Missing Bearer access token" })
        );
        assert_eq!(
            response.header("content-type"),
            Some("application/json; charset=utf-8")
        );
        // Tells an MCP client where to find the OAuth server.
        assert_eq!(
            response.header("www-authenticate"),
            Some(format!("Bearer {CHALLENGE}").as_str())
        );
    }

    let empty = gateway
        .request(Method::POST, &[("authorization", "Bearer ")], None)
        .await;
    assert_eq!(empty.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        empty.json(),
        json!({ "error": "unauthorized", "message": "Empty Bearer access token" })
    );
}

#[tokio::test]
async fn rejects_invalid_expired_and_revoked_token_values() {
    let gateway = TestMcpGateway::offline().await;
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
        let authorization = format!("Bearer {value}");
        let response = gateway
            .request(Method::GET, &[("authorization", &authorization)], None)
            .await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json()["message"],
            "Invalid, expired, or revoked access token"
        );
        assert_eq!(
            response.header("www-authenticate"),
            Some(format!("Bearer error=\"invalid_token\", {CHALLENGE}").as_str())
        );
    }
}

#[tokio::test]
async fn reads_the_first_authorization_header_as_node_does() {
    let gateway = TestMcpGateway::offline().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let created = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let valid = format!("bearer   {} ", created.plaintext);

    let first_wins = gateway
        .request(
            Method::POST,
            &[
                ("authorization", &valid),
                ("authorization", "Bearer not-a-token"),
                ("accept", "application/json, text/event-stream"),
                ("content-type", "application/json"),
            ],
            Some(initialize()),
        )
        .await;
    assert_eq!(first_wins.status, StatusCode::OK);

    let first_loses = gateway
        .request(
            Method::POST,
            &[
                ("authorization", "Bearer not-a-token"),
                ("authorization", &valid),
            ],
            Some(initialize()),
        )
        .await;
    assert_eq!(first_loses.status, StatusCode::UNAUTHORIZED);
}

// hardening: gateway requests

#[tokio::test]
async fn limits_the_requests_of_one_access_token_without_affecting_another() {
    let gateway = TestMcpGateway::offline().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let busy = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let other = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    for _ in 0..GATEWAY_REQUESTS_PER_MINUTE - 1 {
        gateway
            .gateway
            .rate_limiter
            .increment(&format!("mcp:{}", busy.token.id))
            .await
            .unwrap();
    }

    let last = gateway.post(&busy.plaintext, initialize(), &[]).await;
    assert_eq!(last.status, StatusCode::OK);

    let refused = gateway.post(&busy.plaintext, initialize(), &[]).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.json(),
        json!({
            "error": "rate_limited",
            "message": "Too many requests for this access token",
        })
    );
    let retry_after: u64 = refused.header("retry-after").unwrap().parse().unwrap();
    assert!(retry_after > 0 && retry_after <= 60);
    assert_eq!(refused.header("www-authenticate"), None);

    let unaffected = gateway.post(&other.plaintext, initialize(), &[]).await;
    assert_eq!(unaffected.status, StatusCode::OK);
}
