//! `tests/unit/hardening_upstream_oauth.spec.ts`: what authorizing an MCP
//! again does to the tokens it had, tokens without a stated lifetime, and
//! the pending authorizations of a session.

mod support;

use std::sync::Arc;

use chrono::Duration;
use mymcps_core::models::{Mcp, McpStatus};
use mymcps_core::{TestCore, Timestamp};
use mymcps_upstream::{Upstream, read_oauth_session};
use serde_json::{Value, json};
use support::*;
use url::Url;

/// The MCP at mcp.example names `advertised` as its authorization server. Both
/// auth.example and other.example answer as complete providers.
fn providers(core: &TestCore, advertised: &'static str, tokens: Value) -> (Arc<Upstream>, Calls) {
    upstream(core, move |call| {
        let url = call.parsed();
        let path = url.path();
        if path.starts_with("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": "https://mcp.example/mcp",
                "authorization_servers": [advertised],
            }));
        }
        match path {
            "/.well-known/oauth-authorization-server" => {
                json_response(authorization_server(&origin(&url)))
            }
            "/register" => json_response(registered_client(&format!(
                "client-of-{}",
                url.host_str().unwrap()
            ))),
            "/token" => {
                let mut answer = json!({ "access_token": "access-new", "token_type": "Bearer" });
                for (name, value) in tokens.as_object().unwrap() {
                    answer[name] = value.clone();
                }
                json_response(answer)
            }
            _ => not_found(),
        }
    })
}

async fn connected_mcp(core: &TestCore, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    let access_token = encrypt(core, "access-old");
    let refresh_token = encrypt(core, "refresh-old");
    create_mcp(core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.oauth_issuer = Some("https://auth.example".into());
        mcp.oauth_authorize_url = Some("https://auth.example/authorize".into());
        mcp.oauth_token_url = Some("https://auth.example/token".into());
        mcp.oauth_redirect_uri = Some(CALLBACK.into());
        mcp.oauth_client_id = Some("client-of-auth.example".into());
        mcp.oauth_client_auth_method = Some("none".into());
        mcp.oauth_resource = Some("https://mcp.example/mcp".into());
        mcp.oauth_access_token = access_token;
        mcp.oauth_refresh_token = refresh_token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + Duration::hours(1));
        mcp.oauth_token_type = Some("Bearer".into());
        adjust(mcp);
    })
    .await
}

fn token_requests(calls: &Calls) -> Vec<Call> {
    calls.with_path("/token")
}

// Re-authorizing an upstream MCP

#[tokio::test]
async fn forgets_the_tokens_of_the_previous_provider_when_the_mcp_names_another_one() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core, |_| {}).await;
    let (upstream, calls) = providers(&core, "https://other.example", json!({}));

    let redirect = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();
    assert_eq!(
        origin(&Url::parse(&redirect).unwrap()),
        "https://other.example"
    );

    let mut saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_issuer.as_deref(), Some("https://other.example"));
    assert_eq!(
        saved.oauth_client_id.as_deref(),
        Some("client-of-other.example")
    );
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.oauth_refresh_token, None);
    assert_eq!(saved.oauth_token_expires_at, None);
    assert!(saved.oauth_required);

    // The flow may be abandoned here. Nothing is left to send to the new provider.
    upstream
        .refresh_oauth_access_token(&mut saved)
        .await
        .unwrap();
    assert_eq!(token_requests(&calls).len(), 0);
    assert!(
        !calls
            .all()
            .iter()
            .any(|call| call.body.contains("refresh-old"))
    );
}

#[tokio::test]
async fn forgets_them_too_when_only_an_inferred_provider_was_saved() {
    let core = TestCore::new().await;
    // Rows from before the issuer was stored: it is inferred from the endpoints.
    let mut mcp = connected_mcp(&core, |mcp| {
        mcp.oauth_issuer = None;
        mcp.oauth_authorize_url = Some("https://legacy.example/authorize".into());
        mcp.oauth_token_url = Some("https://legacy.example/token".into());
    })
    .await;
    let (upstream, _) = providers(&core, "https://auth.example", json!({}));

    upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    assert_eq!(mcp.oauth_issuer.as_deref(), Some("https://auth.example"));
    assert_eq!(
        mcp.oauth_client_id.as_deref(),
        Some("client-of-auth.example")
    );
    assert_eq!(mcp.oauth_refresh_token, None);
    assert_eq!(mcp.oauth_access_token, None);
}

#[tokio::test]
async fn forgets_them_when_a_new_client_had_to_be_registered_with_the_same_provider() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core, |mcp| {
        mcp.oauth_redirect_uri = Some("https://old-gateway.example/callback".into());
    })
    .await;
    let (upstream, calls) = providers(&core, "https://auth.example", json!({}));

    upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    assert!(
        calls
            .all()
            .iter()
            .any(|call| call.url == "https://auth.example/register")
    );
    assert_eq!(mcp.oauth_refresh_token, None);
    assert_eq!(mcp.oauth_access_token, None);
    assert!(mcp.oauth_required);
}

#[tokio::test]
async fn keeps_the_connection_while_the_same_provider_and_client_are_authorized_again() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core, |_| {}).await;
    let (upstream, calls) = providers(&core, "https://auth.example", json!({}));

    let redirect = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();
    let redirect = Url::parse(&redirect).unwrap();

    assert_eq!(
        query(&redirect, "client_id").as_deref(),
        Some("client-of-auth.example")
    );
    assert!(calls.posts().is_empty());
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-old")
    );
    assert_eq!(
        decrypt(&core, &saved.oauth_refresh_token).as_deref(),
        Some("refresh-old")
    );
    assert!(!saved.oauth_required);
}

// Upstream OAuth tokens without a stated lifetime

fn minutes_away(expiry: Option<Timestamp>) -> f64 {
    let expiry = expiry.expect("an expiry is saved");
    (expiry - Timestamp::now()).num_seconds() as f64 / 60.0
}

fn assert_about_an_hour_away(expiry: Option<Timestamp>) {
    let minutes = minutes_away(expiry);
    assert!(minutes > 58.0 && minutes <= 60.0, "{minutes} minutes");
}

#[tokio::test]
async fn are_given_an_hour_instead_of_being_refreshed_on_every_connection() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core, |mcp| {
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    // No `expires_in` in the token responses.
    let (upstream, calls) = providers(
        &core,
        "https://auth.example",
        json!({ "refresh_token": "refresh-new" }),
    );

    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    assert_eq!(token_requests(&calls).len(), 1);
    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("access-new")
    );
    assert_about_an_hour_away(mcp.oauth_token_expires_at);

    // Every later connection asks for a refresh first.
    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    upstream
        .refresh_oauth_access_token(&mut find_mcp(&core, mcp.id).await)
        .await
        .unwrap();
    assert_eq!(token_requests(&calls).len(), 1);
}

#[tokio::test]
async fn are_given_the_same_hour_when_first_issued() {
    let core = TestCore::new().await;
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let (upstream, calls) = providers(
        &core,
        "https://auth.example",
        json!({ "refresh_token": "refresh-new" }),
    );
    let session = MemorySession::new();

    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let redirect = Url::parse(&redirect).unwrap();
    let oauth = read_oauth_session(&session, query(&redirect, "state").as_deref()).unwrap();
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "authorization-code", None)
        .await
        .unwrap();

    assert_about_an_hour_away(mcp.oauth_token_expires_at);
    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    assert!(
        !token_requests(&calls)
            .iter()
            .any(|call| call.body.contains("refresh_token"))
    );
}

#[tokio::test]
async fn keep_the_lifetime_the_provider_states() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core, |mcp| {
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    let (upstream, _) = providers(&core, "https://auth.example", json!({ "expires_in": 600 }));

    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    let minutes = minutes_away(mcp.oauth_token_expires_at);
    assert!(minutes > 9.0 && minutes <= 10.0, "{minutes} minutes");
    // The provider did not rotate the refresh token: the one in use is kept.
    assert_eq!(
        decrypt(&core, &mcp.oauth_refresh_token).as_deref(),
        Some("refresh-old")
    );
}

// Pending upstream OAuth authorizations

#[tokio::test]
async fn keeps_the_five_most_recent_starts_of_a_session_and_nothing_else_of_theirs() {
    let core = TestCore::new().await;
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let (upstream, _) = providers(&core, "https://auth.example", json!({}));
    let session = MemorySession::new();
    session.set("auth_web", json!(42));

    let mut states = Vec::new();
    for _ in 0..8 {
        let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
        states.push(query(&Url::parse(&redirect).unwrap(), "state").unwrap());
    }

    let pending = session.pending();
    // At most five, and fewer when five would not fit in the session cookie.
    assert!(pending.len() <= 5);
    assert!(pending.len() > 1);
    let most_recent: Vec<String> = states[states.len() - pending.len()..]
        .iter()
        .map(|state| format!("mcp_oauth:{state}"))
        .collect();
    assert_eq!(pending, most_recent);
    for abandoned in &states[..3] {
        assert_eq!(read_oauth_session(&session, Some(abandoned)), None);
    }
    assert!(read_oauth_session(&session, states.last().map(String::as_str)).is_some());
    assert_eq!(session.value("auth_web"), Some(json!(42)));
}
