//! `tests/functional/gateway_auth.spec.ts`, and the cases of
//! `gateway_oauth_server.spec.ts` and `public_url_config.spec.ts` that call
//! `/mcp`: who the endpoint lets in, and what it tells the others.

#[path = "support/gateway.rs"]
mod support;

use chrono::Duration;
use http::{Method, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::{OauthClient, ScopeMode};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::create_admin;
use serde_json::{Value, json};

use support::*;

const RESOURCE: &str = "http://localhost:3333/mcp";
const CHALLENGE: &str = "resource_metadata=\"http://localhost:3333/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:tools\"";

fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "gateway-auth-test", "version": "1.0.0" },
        },
    })
}

// gateway bearer authentication

#[tokio::test]
async fn rejects_requests_without_a_bearer_token() {
    let gateway = TestGateway::offline().await;

    for headers in [
        &[][..],
        &[("authorization", "Basic abc")],
        &[("authorization", "Bearerabc")],
    ] {
        let response = gateway.mcp_request(Method::GET, headers, None).await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json(),
            json!({ "error": "unauthorized", "message": "Missing Bearer access token" })
        );
        assert_eq!(
            response.header("content-type"),
            Some("application/json; charset=utf-8")
        );
        // No session is opened for a program.
        assert_eq!(response.header("set-cookie"), None);
    }

    let empty = gateway
        .mcp_request(Method::POST, &[("authorization", "Bearer ")], None)
        .await;
    assert_eq!(empty.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        empty.json(),
        json!({ "error": "unauthorized", "message": "Empty Bearer access token" })
    );
}

#[tokio::test]
async fn rejects_invalid_expired_and_revoked_token_values() {
    let gateway = TestGateway::offline().await;
    let admin = create_admin(&gateway).await;
    create_token_with_secret(&gateway, admin.id, "mcp_expired", |token| {
        token.expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    create_token_with_secret(&gateway, admin.id, "mcp_revoked", |token| {
        token.revoked_at = Some(Timestamp::now());
    })
    .await;

    for value in ["not-a-token", "mcp_expired", "mcp_revoked"] {
        let authorization = format!("Bearer {value}");
        let response = gateway
            .mcp_request(Method::GET, &[("authorization", &authorization)], None)
            .await;

        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{value}");
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
async fn lets_a_usable_token_in_however_its_scheme_is_spelled() {
    let gateway = TestGateway::offline().await;
    let admin = create_admin(&gateway).await;
    let created = create_access_token(&gateway, admin.id, ScopeMode::All, &[]).await;
    assert_eq!(created.token.last_used_at, None);

    let authorization = format!("bearer   {} ", created.plaintext);
    let response = gateway
        .mcp_request(
            Method::POST,
            &[
                ("authorization", &authorization),
                ("accept", "application/json, text/event-stream"),
            ],
            Some(initialize()),
        )
        .await;

    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.header("www-authenticate"), None);
    assert_eq!(
        result_of(&response)["serverInfo"],
        json!({ "name": "mymcps", "version": mymcps_core::VERSION })
    );
    let used = sqlx::query_scalar::<_, Option<Timestamp>>(
        "select `last_used_at` from `access_tokens` where `id` = ?",
    )
    .bind(created.token.id)
    .fetch_one(&**gateway.db())
    .await
    .unwrap();
    assert!(used.is_some());
}

// gateway OAuth server

#[tokio::test]
async fn challenges_unauthenticated_mcp_calls_with_oauth_discovery_details() {
    let gateway = TestGateway::offline().await;
    create_admin(&gateway).await;

    let response = gateway.mcp_request(Method::GET, &[], None).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    // Tells an MCP client where to find the OAuth server.
    assert_eq!(
        response.header("www-authenticate"),
        Some(format!("Bearer {CHALLENGE}").as_str())
    );
}

#[tokio::test]
async fn answers_before_the_instance_has_its_first_administrator() {
    let app = TestApp::new().await;

    // Not sent to onboarding: the endpoint is not a page.
    let response = app.get("/mcp").api().send().await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.location(), None);
}

#[tokio::test]
async fn accepts_an_oauth_access_token_only_for_this_gateway_and_its_scope() {
    let gateway = TestGateway::offline().await;
    let db = gateway.db();
    let admin = create_admin(&gateway).await;
    let mut client = OauthClient {
        client_id: "mcp_client_gateway_auth".into(),
        client_name: "Claude Desktop".into(),
        redirect_uris: json!(["http://127.0.0.1/callback"]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: json!(["authorization_code", "refresh_token"]).to_string(),
        response_types: json!(["code"]).to_string(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&**db).await.unwrap();
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
    let mut token = created.token;

    let accepted = gateway
        .post_message(&created.plaintext, initialize(), &[])
        .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.text());

    token.oauth_resource = Some("https://old-gateway.example.com/mcp".into());
    token.save(&**db).await.unwrap();
    let wrong_audience = gateway
        .post_message(&created.plaintext, initialize(), &[])
        .await;
    assert_eq!(wrong_audience.status, StatusCode::UNAUTHORIZED);

    token.oauth_resource = Some(RESOURCE.into());
    token.oauth_scopes = Some("unsupported:scope".into());
    token.save(&**db).await.unwrap();
    let wrong_scope = gateway
        .post_message(&created.plaintext, initialize(), &[])
        .await;
    assert_eq!(wrong_scope.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong_scope.json()["message"],
        "Invalid, expired, or revoked access token"
    );
}

// public URL configuration

#[tokio::test]
async fn keeps_bearer_authentication_without_publishing_insecure_oauth_metadata() {
    let gateway = TestGateway::with(
        |config| config.app_url = Some("http://mcp.example.com".into()),
        |fetcher| fetcher,
        |_| Reply::Error("no MCP is expected to be reached".into()),
    )
    .await;
    let admin = create_admin(&gateway).await;
    let created = create_access_token(&gateway, admin.id, ScopeMode::All, &[]).await;

    let challenge = gateway.mcp_request(Method::GET, &[], None).await;
    assert_eq!(challenge.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenge.header("www-authenticate"),
        Some("Bearer scope=\"mcp:tools\"")
    );

    let accepted = gateway
        .post_message(&created.plaintext, initialize(), &[])
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
}
