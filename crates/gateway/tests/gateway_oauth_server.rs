//! `tests/functional/gateway_oauth_server.spec.ts`, at the level of the
//! functions the endpoints call.

mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mymcps_core::models::TokenSource;
use mymcps_gateway::access_token;
use mymcps_gateway::bearer::{BearerError, BearerRejection};
use mymcps_gateway::oauth::{
    authorization_server_metadata, is_loopback_redirect_uri, oauth_redirect, oauth_token_response,
    protected_resource_metadata,
};
use serde_json::{Value, json};
use support::*;
use url::Url;

const REGISTERED_REDIRECT_URI: &str = "http://127.0.0.1/callback";
const RUNTIME_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const CODE_VERIFIER: &str = "oauth-code-verifier-for-mymcps-tests-1234567890";

async fn register_public_client(gateway: &TestGateway, client_name: &str) -> Value {
    register(gateway, &registration(client_name, REGISTERED_REDIRECT_URI)).await
}

fn authorization_payload(client_id: &str) -> Value {
    authorization(
        client_id,
        RUNTIME_REDIRECT_URI,
        &code_challenge(CODE_VERIFIER),
    )
}

fn code_grant(client_id: &str, code: &str, code_verifier: &str) -> Value {
    json!({
        "grant_type": "authorization_code",
        "client_id": client_id,
        "code": code,
        "code_verifier": code_verifier,
        "redirect_uri": RUNTIME_REDIRECT_URI,
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

fn is_base64url(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether the gateway lets a request with this access token in.
async fn accepts(gateway: &TestGateway, access_token: &str) -> bool {
    match gateway
        .authenticate_bearer(Some(&format!("Bearer {access_token}")))
        .await
    {
        Ok(_) => true,
        Err(BearerError::Rejected(BearerRejection::Invalid)) => false,
        Err(other) => panic!("unexpected answer: {other:?}"),
    }
}

#[tokio::test]
async fn publishes_mcp_protected_resource_and_authorization_server_metadata() {
    let gateway = TestGateway::new().await;
    let config = &gateway.core.config;

    let resource = protected_resource_metadata(config).unwrap();
    assert_eq!(
        resource.to_string(),
        json!({
            "resource": RESOURCE,
            "authorization_servers": ["http://localhost:3333"],
            "scopes_supported": ["mcp:tools"],
            "bearer_methods_supported": ["header"],
            "resource_name": "MyMCPs gateway",
        })
        .to_string()
    );

    let metadata = authorization_server_metadata(config).unwrap();
    assert_eq!(metadata["issuer"], "http://localhost:3333");
    assert_eq!(
        metadata["authorization_endpoint"],
        "http://localhost:3333/authorize"
    );
    assert_eq!(metadata["token_endpoint"], "http://localhost:3333/token");
    assert_eq!(
        metadata["registration_endpoint"],
        "http://localhost:3333/register"
    );
    assert_eq!(
        metadata["revocation_endpoint"],
        "http://localhost:3333/revoke"
    );
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert_eq!(
        metadata.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "issuer",
            "authorization_endpoint",
            "token_endpoint",
            "registration_endpoint",
            "revocation_endpoint",
            "scopes_supported",
            "response_types_supported",
            "grant_types_supported",
            "token_endpoint_auth_methods_supported",
            "revocation_endpoint_auth_methods_supported",
            "code_challenge_methods_supported",
        ]
    );
}

#[tokio::test]
async fn publishes_no_metadata_when_app_url_is_not_a_public_https_origin() {
    let gateway = TestGateway::with_config(|config| {
        config.environment = mymcps_core::Environment::Production;
    })
    .await;
    let config = &gateway.core.config;

    assert!(protected_resource_metadata(config).is_err());
    assert!(authorization_server_metadata(config).is_err());

    // Every endpoint of the OAuth server says so, whatever it was asked.
    let unconfigured = gateway.oauth.ensure_configured().unwrap_err();
    assert_eq!(unconfigured.status(), 503);
    assert_eq!(
        unconfigured.body(),
        json!({
            "error": "temporarily_unavailable",
            "error_description": "OAuth requires APP_URL to be a public HTTPS origin",
        })
    );
    assert!(!unconfigured.is_failure());
    assert_eq!(
        mymcps_gateway::Error::from(authorization_server_metadata(config).unwrap_err()).status(),
        503
    );

    // Bearer authentication stays available, without the insecure metadata.
    assert_eq!(
        BearerRejection::Missing.www_authenticate(config).as_deref(),
        Some("Bearer scope=\"mcp:tools\"")
    );
}

#[tokio::test]
async fn challenges_unauthenticated_mcp_calls_with_oauth_discovery_details() {
    let gateway = TestGateway::new().await;

    let Err(BearerError::Rejected(rejection)) = gateway.authenticate_bearer(None).await else {
        panic!("a request without a token was let in");
    };

    assert_eq!(rejection.status(), 401);
    assert_eq!(
        rejection.www_authenticate(&gateway.core.config).as_deref(),
        Some(
            "Bearer resource_metadata=\"http://localhost:3333/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:tools\""
        )
    );
    assert_eq!(
        BearerRejection::Invalid
            .www_authenticate(&gateway.core.config)
            .as_deref(),
        Some(
            "Bearer error=\"invalid_token\", resource_metadata=\"http://localhost:3333/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:tools\""
        )
    );
}

#[tokio::test]
async fn registers_public_clients_and_rejects_unsafe_redirect_uris() {
    let gateway = TestGateway::new().await;

    let registered = register_public_client(&gateway, "Codex test client").await;
    let client_id = registered["client_id"].as_str().unwrap();
    assert!(
        client_id
            .strip_prefix("mcp_client_")
            .is_some_and(|random| is_base64url(random, 32))
    );
    assert_eq!(registered["token_endpoint_auth_method"], "none");
    assert!(registered.get("client_secret").is_none());

    let cursor = register(
        &gateway,
        &registration("Cursor", "cursor://anysphere.cursor-mcp/oauth/callback"),
    )
    .await;
    assert_eq!(
        cursor["redirect_uris"],
        json!(["cursor://anysphere.cursor-mcp/oauth/callback"])
    );

    for (client_name, redirect_uri) in [
        ("Unsafe client", "http://example.com/callback"),
        (
            "Cursor lookalike",
            "cursor://attacker.example/oauth/callback",
        ),
        (
            "Credentialed redirect client",
            "https://user:password@example.com/callback",
        ),
    ] {
        let refused = gateway
            .oauth
            .register_client(&json!({
                "client_name": client_name,
                "redirect_uris": [redirect_uri],
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code"],
                "response_types": ["code"],
            }))
            .await;
        let refusal = oauth_error(refused);
        assert_eq!(
            (refusal.status, refusal.code),
            (400, "invalid_redirect_uri")
        );
    }

    let confidential = register(
        &gateway,
        &json!({
            "client_name": "Confidential client",
            "redirect_uris": ["https://client.example.com/callback"],
            "token_endpoint_auth_method": "client_secret_basic",
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
        }),
    )
    .await;
    let basic_credentials = STANDARD.encode(format!(
        "{}:{}",
        confidential["client_id"].as_str().unwrap(),
        confidential["client_secret"].as_str().unwrap()
    ));
    let lowercase_basic = gateway
        .oauth
        .issue_tokens(Some(&format!("basic {basic_credentials}")), &json!({}))
        .await;
    assert_oauth_error(
        lowercase_basic,
        400,
        "invalid_request",
        "grant_type is required",
    );
}

#[tokio::test]
async fn returns_users_to_the_authorization_request_after_credential_login() {
    let gateway = TestGateway::new().await;
    let registered = register_public_client(&gateway, "Codex test client").await;
    let client_id = registered["client_id"].as_str().unwrap();

    let request = gateway
        .oauth
        .parse_authorization_request(&authorization_payload(client_id))
        .await
        .unwrap();
    let return_path = request.return_path().unwrap();
    let returned = Url::parse(&format!("http://localhost{return_path}")).unwrap();
    assert_eq!(returned.path(), "/authorize");

    // The path asks for the same authorization once the user is signed in.
    let query: serde_json::Map<String, Value> = returned
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), Value::from(value.into_owned())))
        .collect();
    let resumed = gateway
        .oauth
        .parse_authorization_request(&Value::Object(query))
        .await
        .unwrap();
    assert_eq!(resumed.client.client_name, "Codex test client");
    assert_eq!(resumed.redirect_uri, RUNTIME_REDIRECT_URI);
    assert_eq!(resumed.state.as_deref(), Some("state-from-client"));
    assert_eq!(resumed.code_challenge, code_challenge(CODE_VERIFIER));
    // What the consent screen names as the place the client is sent back to.
    let redirect_url = Url::parse(&resumed.redirect_uri).unwrap();
    assert_eq!(
        (redirect_url.host_str(), redirect_url.port()),
        (Some("127.0.0.1"), Some(49152))
    );
}

#[tokio::test]
async fn rejects_authorization_return_paths_too_large_for_the_cookie_session() {
    let gateway = TestGateway::new().await;
    let registered = register_public_client(&gateway, "Codex test client").await;
    let state = "s".repeat(1400);

    let request = gateway
        .oauth
        .parse_authorization_request(&with(
            &authorization_payload(registered["client_id"].as_str().unwrap()),
            &[("state", Some(json!(state)))],
        ))
        .await
        .unwrap();
    let refusal = request.return_path().unwrap_err();

    // Only a loopback client is sent its error; see hardening_gateway_oauth.rs.
    assert_eq!((refusal.status, refusal.code), (400, "invalid_request"));
    assert_eq!(
        refusal.message,
        "The OAuth authorization request is too large"
    );
    assert_eq!(refusal.redirect_uri.as_deref(), Some(RUNTIME_REDIRECT_URI));
    assert_eq!(refusal.state.as_deref(), Some(state.as_str()));
    assert!(is_loopback_redirect_uri(RUNTIME_REDIRECT_URI));
}

#[tokio::test]
async fn issues_refreshes_lists_and_revokes_an_oauth_connection() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let registered = register_public_client(&gateway, "Claude Desktop").await;
    let client_id = registered["client_id"].as_str().unwrap();
    let authorization = authorization_payload(client_id);

    // What the consent screen shows.
    let request = gateway
        .oauth
        .parse_authorization_request(&authorization)
        .await
        .unwrap();
    assert_eq!(request.client.client_name, "Claude Desktop");
    assert!(is_loopback_redirect_uri(&request.redirect_uri));
    assert_eq!(request.scopes, "mcp:tools");
    assert_eq!(request.resource, RESOURCE);

    let code = gateway
        .oauth
        .create_authorization_code(&request, admin.id)
        .await
        .unwrap();
    let callback = oauth_redirect(
        &request.redirect_uri,
        &[
            ("code", Some(code.as_str())),
            ("state", request.state.as_deref()),
        ],
    )
    .unwrap();
    assert_eq!(
        callback,
        format!("http://127.0.0.1:49152/callback?code={code}&state=state-from-client")
    );

    let created = gateway
        .oauth
        .issue_tokens(None, &code_grant(client_id, &code, CODE_VERIFIER))
        .await
        .unwrap();
    let exchange = oauth_token_response(&created);
    assert_eq!(exchange["token_type"], "Bearer");
    assert_eq!(exchange["expires_in"], 3600);
    assert_eq!(exchange["scope"], "mcp:tools");
    let oauth_access_token = exchange["access_token"].as_str().unwrap().to_owned();
    let old_refresh_token = exchange["refresh_token"].as_str().unwrap().to_owned();
    assert!(
        oauth_access_token
            .strip_prefix("mcp_")
            .is_some_and(|random| is_base64url(random, 43))
    );
    assert!(
        old_refresh_token
            .strip_prefix("mcp_refresh_")
            .is_some_and(|random| is_base64url(random, 43))
    );
    assert_eq!(
        exchange.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "access_token",
            "token_type",
            "expires_in",
            "scope",
            "refresh_token"
        ]
    );

    let mut oauth_token = find_token_by_name(db, "Claude Desktop").await;
    assert_eq!(oauth_token.source, TokenSource::Oauth);
    assert_eq!(oauth_token.created_by, admin.id);
    assert_ne!(oauth_token.token_hash, oauth_access_token);
    assert_ne!(
        oauth_token.oauth_refresh_token_hash.as_deref(),
        Some(old_refresh_token.as_str())
    );
    assert_eq!(oauth_token.token_prefix, &oauth_access_token[..12]);
    assert_eq!(
        oauth_token.oauth_refresh_token_prefix.as_deref(),
        Some("mcp_refresh_")
    );
    assert!(oauth_token.is_active());

    assert!(accepts(&gateway, &oauth_access_token).await);

    oauth_token.oauth_resource = Some("https://old-gateway.example.com/mcp".into());
    oauth_token.save(&**db).await.unwrap();
    assert!(!accepts(&gateway, &oauth_access_token).await);

    oauth_token.oauth_resource = Some(RESOURCE.into());
    oauth_token.oauth_scopes = Some("unsupported:scope".into());
    oauth_token.save(&**db).await.unwrap();
    assert!(!accepts(&gateway, &oauth_access_token).await);

    oauth_token.oauth_scopes = Some("mcp:tools".into());
    oauth_token.save(&**db).await.unwrap();

    let refreshed = gateway
        .oauth
        .issue_tokens(None, &refresh_grant(client_id, &old_refresh_token))
        .await
        .unwrap();
    let refresh = oauth_token_response(&refreshed);
    let new_access_token = refresh["access_token"].as_str().unwrap();
    let new_refresh_token = refresh["refresh_token"].as_str().unwrap();
    assert_ne!(new_access_token, oauth_access_token);
    assert_ne!(new_refresh_token, old_refresh_token);
    assert_eq!(refreshed.token.id, oauth_token.id);
    assert!(
        access_token::find_usable_by_plaintext(db, &oauth_access_token)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        access_token::find_usable_by_plaintext(db, new_access_token)
            .await
            .unwrap()
            .is_some()
    );

    // What the Tokens page does to revoke the connection.
    let mut listed = find_token(db, oauth_token.id).await;
    access_token::revoke(db, &mut listed).await.unwrap();

    let revoked = find_token(db, oauth_token.id).await;
    assert!(revoked.is_revoked());
    assert!(
        access_token::find_usable_by_plaintext(db, new_access_token)
            .await
            .unwrap()
            .is_none()
    );

    let refresh_after_revoke = gateway
        .oauth
        .issue_tokens(None, &refresh_grant(client_id, new_refresh_token))
        .await;
    assert_oauth_error(
        refresh_after_revoke,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );
}

#[tokio::test]
async fn revokes_the_active_grant_when_a_rotated_refresh_token_is_replayed() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let registered = register_public_client(&gateway, "Refresh replay client").await;
    let client_id = registered["client_id"].as_str().unwrap();
    let code = approve(&gateway, &authorization_payload(client_id), admin.id).await;
    let exchange = gateway
        .oauth
        .issue_tokens(None, &code_grant(client_id, &code, CODE_VERIFIER))
        .await
        .unwrap();

    let old_refresh_token = exchange.refresh_token.unwrap();
    let refresh = gateway
        .oauth
        .issue_tokens(None, &refresh_grant(client_id, &old_refresh_token))
        .await
        .unwrap();

    let replay = gateway
        .oauth
        .issue_tokens(None, &refresh_grant(client_id, &old_refresh_token))
        .await;
    assert_eq!(oauth_error(replay).code, "invalid_grant");

    let grant = find_token_by_name(db, "Refresh replay client").await;
    assert!(grant.is_revoked());
    assert!(
        access_token::find_usable_by_plaintext(db, &refresh.plaintext)
            .await
            .unwrap()
            .is_none()
    );

    let attacker_refresh = gateway
        .oauth
        .issue_tokens(
            None,
            &refresh_grant(client_id, &refresh.refresh_token.unwrap()),
        )
        .await;
    assert_eq!(oauth_error(attacker_refresh).code, "invalid_grant");
}

#[tokio::test]
async fn rejects_authorization_code_replay_and_an_incorrect_pkce_verifier() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let registered = register_public_client(&gateway, "Codex test client").await;
    let client_id = registered["client_id"].as_str().unwrap();
    let code = approve(&gateway, &authorization_payload(client_id), admin.id).await;
    assert!(is_base64url(&code, 43));

    let invalid_verifier = gateway
        .oauth
        .issue_tokens(
            None,
            &code_grant(
                client_id,
                &code,
                "incorrect-verifier-that-is-still-long-enough-123456789",
            ),
        )
        .await;
    assert_eq!(oauth_error(invalid_verifier).code, "invalid_grant");

    gateway
        .oauth
        .issue_tokens(None, &code_grant(client_id, &code, CODE_VERIFIER))
        .await
        .unwrap();

    let replay = gateway
        .oauth
        .issue_tokens(None, &code_grant(client_id, &code, CODE_VERIFIER))
        .await;
    assert_oauth_error(
        replay,
        400,
        "invalid_grant",
        "Invalid or expired authorization code",
    );
    assert_eq!(count(gateway.db(), "access_tokens").await, 1);
}
