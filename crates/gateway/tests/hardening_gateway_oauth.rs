//! `tests/functional/hardening_gateway_oauth.spec.ts`, at the level of the
//! functions the endpoints call.

mod support;

use chrono::Duration;
use mymcps_core::models::{AccessToken, OauthAuthorizationCode, OauthClient};
use mymcps_core::{Db, Timestamp};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::oauth::{
    MAX_OAUTH_CLIENTS, UNUSED_CLIENT_RETENTION_DAYS, is_loopback_redirect_uri, oauth_redirect,
};
use mymcps_gateway::{Error, GatewayOauthError};
use serde_json::{Value, json};
use support::*;
use url::Url;

const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const REMOTE_REDIRECT_URI: &str = "https://client.example/callback";
const CODE_VERIFIER: &str = "hardening-oauth-code-verifier-for-mymcps-tests-123";

async fn register_client(gateway: &TestGateway, redirect_uri: &str) -> String {
    let registered = register(
        gateway,
        &json!({
            "client_name": "Hardening client",
            "redirect_uris": [redirect_uri],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        }),
    )
    .await;
    registered["client_id"].as_str().unwrap().to_owned()
}

fn authorization_payload(client_id: &str, redirect_uri: &str) -> Value {
    authorization(client_id, redirect_uri, &code_challenge(CODE_VERIFIER))
}

/// The error of an authorization request, as the endpoint sends it to a
/// client it may redirect to.
fn redirect_of(error: &GatewayOauthError) -> Url {
    let location = oauth_redirect(
        error.redirect_uri.as_deref().unwrap(),
        &[
            ("error", Some(error.code)),
            ("error_description", Some(&error.message)),
            ("state", error.state.as_deref()),
        ],
    )
    .unwrap();
    Url::parse(&location).unwrap()
}

fn query(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

async fn stored_client(db: &Db, name: &str, created_at: Option<Timestamp>) -> OauthClient {
    let mut client = OauthClient {
        client_id: format!("mcp_client_{name}"),
        client_name: name.to_owned(),
        redirect_uris: json!([LOOPBACK_REDIRECT_URI]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: json!(["authorization_code", "refresh_token"]).to_string(),
        response_types: json!(["code"]).to_string(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&**db).await.unwrap();
    let created_at = created_at
        .unwrap_or_else(|| Timestamp::now() - Duration::days(UNUSED_CLIENT_RETENTION_DAYS + 1));
    sqlx::query("update `oauth_clients` set `created_at` = ? where `id` = ?")
        .bind(created_at)
        .bind(client.id)
        .execute(&**db)
        .await
        .unwrap();
    client.refresh(&**db).await.unwrap();
    client
}

async fn stored_grant(db: &Db, oauth_client: &OauthClient, user_id: i64) -> AccessToken {
    access_token::create_oauth_grant(
        &**db,
        NewOauthGrant {
            name: &oauth_client.client_name,
            client_id: oauth_client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: RESOURCE,
            created_by: user_id,
        },
    )
    .await
    .unwrap()
    .token
}

/// Age a grant so that both its tokens have expired and it was last touched at `last_active`.
async fn expire_grant(db: &Db, token: &AccessToken, last_active: Timestamp) {
    sqlx::query(
        "update `access_tokens` set `expires_at` = ?, `oauth_refresh_expires_at` = ?, `updated_at` = ? where `id` = ?",
    )
    .bind(last_active)
    .bind(last_active)
    .bind(last_active)
    .bind(token.id)
    .execute(&**db)
    .await
    .unwrap();
}

async fn pending_code(db: &Db, oauth_client: &OauthClient, user_id: i64) {
    let mut code = OauthAuthorizationCode {
        code_hash: access_token::hash(&format!("pending-code-{}", oauth_client.id)),
        oauth_client_id: oauth_client.id,
        user_id,
        redirect_uri: LOOPBACK_REDIRECT_URI.into(),
        code_challenge: code_challenge(CODE_VERIFIER),
        scopes: "mcp:tools".into(),
        resource: RESOURCE.into(),
        expires_at: Timestamp::now() + Duration::minutes(5),
        ..Default::default()
    };
    code.insert(&**db).await.unwrap();
}

// hardening: OAuth authorization endpoint

#[tokio::test]
async fn shows_a_rejected_request_here_instead_of_redirecting_to_a_remote_client() {
    let gateway = TestGateway::new().await;
    let client_id = register_client(&gateway, REMOTE_REDIRECT_URI).await;

    let refused = gateway
        .oauth
        .parse_authorization_request(&with(
            &authorization_payload(&client_id, REMOTE_REDIRECT_URI),
            &[("response_type", Some(json!("token")))],
        ))
        .await;
    let error = oauth_error(refused);

    assert_eq!(
        (error.status, error.code),
        (400, "unsupported_response_type")
    );
    // Anyone can register a client, so a rejected request is only sent back
    // to a redirect URI on the user's own device.
    assert_eq!(error.redirect_uri.as_deref(), Some(REMOTE_REDIRECT_URI));
    assert!(!is_loopback_redirect_uri(REMOTE_REDIRECT_URI));
    assert_eq!(
        error.body(),
        json!({
            "error": "unsupported_response_type",
            "error_description": "Only the code response type is supported",
        })
    );
}

#[tokio::test]
async fn returns_a_rejected_request_to_a_loopback_client_with_its_state_intact() {
    let gateway = TestGateway::new().await;
    let client_id = register_client(&gateway, LOOPBACK_REDIRECT_URI).await;

    let refused = gateway
        .oauth
        .parse_authorization_request(&with(
            &authorization_payload(&client_id, LOOPBACK_REDIRECT_URI),
            &[("response_type", Some(json!("token")))],
        ))
        .await;
    let error = oauth_error(refused);
    assert!(is_loopback_redirect_uri(
        error.redirect_uri.as_deref().unwrap()
    ));

    let callback = redirect_of(&error);
    assert_eq!(
        callback.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    assert_eq!(
        callback
            .query_pairs()
            .map(|(name, _)| name.into_owned())
            .collect::<Vec<_>>(),
        ["error", "error_description", "state"]
    );
    assert_eq!(
        query(&callback, "error").as_deref(),
        Some("unsupported_response_type")
    );
    assert_eq!(
        query(&callback, "state").as_deref(),
        Some("state-from-client")
    );
}

#[tokio::test]
async fn still_returns_the_operator_decision_to_a_remote_client() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let client_id = register_client(&gateway, REMOTE_REDIRECT_URI).await;
    let request = gateway
        .oauth
        .parse_authorization_request(&authorization_payload(&client_id, REMOTE_REDIRECT_URI))
        .await
        .unwrap();

    let denied = Url::parse(
        &oauth_redirect(
            &request.redirect_uri,
            &[
                ("error", Some("access_denied")),
                (
                    "error_description",
                    Some("The user denied the authorization request"),
                ),
                ("state", request.state.as_deref()),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        format!("{}{}", denied.origin().ascii_serialization(), denied.path()),
        REMOTE_REDIRECT_URI
    );
    assert_eq!(query(&denied, "error").as_deref(), Some("access_denied"));
    assert_eq!(
        query(&denied, "state").as_deref(),
        Some("state-from-client")
    );
    assert_eq!(count(gateway.db(), "oauth_authorization_codes").await, 0);

    let code = gateway
        .oauth
        .create_authorization_code(&request, admin.id)
        .await
        .unwrap();
    let approved = Url::parse(
        &oauth_redirect(
            &request.redirect_uri,
            &[
                ("code", Some(code.as_str())),
                ("state", request.state.as_deref()),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        format!(
            "{}{}",
            approved.origin().ascii_serialization(),
            approved.path()
        ),
        REMOTE_REDIRECT_URI
    );
    let sent_code = query(&approved, "code").unwrap();
    assert_eq!(sent_code.len(), 43);
    assert!(
        sent_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );
    assert_eq!(
        query(&approved, "state").as_deref(),
        Some("state-from-client")
    );
}

#[tokio::test]
async fn logs_an_unexpected_oauth_failure_without_the_raw_error() {
    let failure = Error::Database(sqlx::Error::Protocol(
        "insert into `oauth_clients` (`client_secret_hash`) values ('Bearer raw-secret-value') - SQLITE_BUSY"
            .into(),
    ));

    assert!(failure.is_failure());
    assert_eq!(failure.status(), 500);
    assert_eq!(
        failure.body(),
        json!({
            "error": "server_error",
            "error_description": "The OAuth request could not be completed",
        })
    );
    assert!(failure.diagnostic().contains("Bearer [REDACTED]"));
    assert!(!failure.diagnostic().contains("raw-secret-value"));
}

// hardening: OAuth client registration

#[tokio::test]
async fn stores_each_grant_and_response_type_once() {
    let gateway = TestGateway::new().await;
    let mut grant_types = vec!["authorization_code"; 5000];
    grant_types.extend(vec!["refresh_token"; 5000]);

    let registered = register(
        &gateway,
        &json!({
            "client_name": "Repeating client",
            "redirect_uris": [LOOPBACK_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
            "grant_types": grant_types,
            "response_types": ["code", "code", "code"],
        }),
    )
    .await;

    assert_eq!(
        registered["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(registered["response_types"], json!(["code"]));

    let stored = find_client(gateway.db(), registered["client_id"].as_str().unwrap()).await;
    assert_eq!(
        stored.grant_types,
        r#"["authorization_code","refresh_token"]"#
    );
    assert_eq!(stored.response_types, r#"["code"]"#);
    assert_eq!(
        stored.redirect_uris,
        r#"["http://127.0.0.1:49152/callback"]"#
    );
}

#[tokio::test]
async fn still_rejects_grant_and_response_types_outside_the_allowed_set() {
    let gateway = TestGateway::new().await;
    let metadata = json!({
        "client_name": "Unsupported client",
        "redirect_uris": [LOOPBACK_REDIRECT_URI],
        "token_endpoint_auth_method": "none",
    });

    for (name, value) in [
        (
            "grant_types",
            json!(["authorization_code", "client_credentials"]),
        ),
        ("grant_types", json!(["refresh_token", "refresh_token"])),
        ("response_types", json!(["code", "token"])),
    ] {
        let refused = gateway
            .oauth
            .register_client(&with(&metadata, &[(name, Some(value))]))
            .await;
        let refusal = oauth_error(refused);
        assert_eq!(
            (refusal.status, refusal.code),
            (400, "invalid_client_metadata")
        );
    }
}

#[tokio::test]
async fn ignores_an_oversized_type_list_stored_by_an_earlier_version() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let mut legacy = stored_client(db, "legacy", None).await;
    legacy.grant_types = json!(vec!["refresh_token"; 5000]).to_string();
    legacy.save(&**db).await.unwrap();
    let current = stored_client(db, "current", None).await;

    assert_eq!(legacy.grant_type_list(), Vec::<String>::new());
    assert_eq!(
        current.grant_type_list(),
        ["authorization_code", "refresh_token"]
    );
}

#[tokio::test]
async fn removes_clients_left_unused_for_the_retention_period() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let long_ago = Timestamp::now() - Duration::days(UNUSED_CLIENT_RETENTION_DAYS + 10);

    stored_client(db, "never-used", None).await;
    stored_client(
        db,
        "registered-recently",
        Some(Timestamp::now() - Duration::days(1)),
    )
    .await;

    stored_grant(db, &stored_client(db, "live-grant", None).await, admin.id).await;

    let pending = stored_client(db, "pending-code", None).await;
    pending_code(db, &pending, admin.id).await;

    let idle_grant = stored_grant(
        db,
        &stored_client(db, "grant-expired-recently", None).await,
        admin.id,
    )
    .await;
    expire_grant(db, &idle_grant, Timestamp::now() - Duration::days(10)).await;

    let dead_grant = stored_grant(
        db,
        &stored_client(db, "grant-expired-long-ago", None).await,
        admin.id,
    )
    .await;
    expire_grant(db, &dead_grant, long_ago).await;

    gateway.oauth.prune_unused_clients(true).await;

    let remaining: Vec<String> =
        sqlx::query_scalar("select `client_name` from `oauth_clients` order by `client_name` asc")
            .fetch_all(&**db)
            .await
            .unwrap();
    assert_eq!(
        remaining,
        [
            "grant-expired-recently",
            "live-grant",
            "pending-code",
            "registered-recently"
        ]
    );

    // The expired connection stays in the token list without its client.
    let orphaned = find_token(db, dead_grant.id).await;
    assert_eq!(orphaned.oauth_client_id, None);
}

#[tokio::test]
async fn prunes_unused_clients_at_most_once_an_hour_unless_forced() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();

    stored_client(db, "first", None).await;
    gateway.oauth.prune_unused_clients(false).await;
    assert_eq!(count(db, "oauth_clients").await, 0);

    stored_client(db, "second", None).await;
    gateway.oauth.prune_unused_clients(false).await;
    assert_eq!(count(db, "oauth_clients").await, 1);

    gateway.oauth.prune_unused_clients(true).await;
    assert_eq!(count(db, "oauth_clients").await, 0);
}

#[tokio::test]
async fn evicts_the_oldest_unused_client_when_the_client_limit_is_reached() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let in_use = stored_client(db, "in-use", Some(Timestamp::now())).await;
    stored_grant(db, &in_use, admin.id).await;
    let oldest_unused = stored_client(db, "oldest-unused", Some(Timestamp::now())).await;

    let created_at = Timestamp::now();
    let mut fillers = db.begin().await.unwrap();
    for index in 0..MAX_OAUTH_CLIENTS - 2 {
        sqlx::query(
            "insert into `oauth_clients` (`client_id`, `client_name`, `redirect_uris`, `token_endpoint_auth_method`, `grant_types`, `response_types`, `scope`, `created_at`) \
             values (?, 'Filler', '[]', 'none', '[\"authorization_code\"]', '[\"code\"]', 'mcp:tools', ?)",
        )
        .bind(format!("mcp_client_filler_{index}"))
        .bind(created_at)
        .execute(&mut *fillers)
        .await
        .unwrap();
    }
    fillers.commit().await.unwrap();
    assert_eq!(count(db, "oauth_clients").await, MAX_OAUTH_CLIENTS);

    register_client(&gateway, LOOPBACK_REDIRECT_URI).await;

    assert_eq!(count(db, "oauth_clients").await, MAX_OAUTH_CLIENTS);
    assert!(OauthClient::find(&**db, in_use.id).await.unwrap().is_some());
    assert!(
        OauthClient::find(&**db, oldest_unused.id)
            .await
            .unwrap()
            .is_none()
    );

    // Once every client is in use there is nothing left to evict.
    sqlx::query(
        "insert into oauth_authorization_codes \
           (code_hash, oauth_client_id, user_id, redirect_uri, code_challenge, scopes, resource, expires_at, created_at) \
         select 'pending-' || id, id, ?, ?, ?, 'mcp:tools', ?, ?, ? from oauth_clients",
    )
    .bind(admin.id)
    .bind(LOOPBACK_REDIRECT_URI)
    .bind(code_challenge(CODE_VERIFIER))
    .bind(RESOURCE)
    .bind(Timestamp::now() + Duration::minutes(5))
    .bind(created_at)
    .execute(&**db)
    .await
    .unwrap();

    let refused = gateway
        .oauth
        .register_client(&json!({
            "client_name": "One too many",
            "redirect_uris": [LOOPBACK_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
        }))
        .await;
    assert_oauth_error(
        refused,
        503,
        "temporarily_unavailable",
        "Too many OAuth clients are registered",
    );
    assert_eq!(count(db, "oauth_clients").await, MAX_OAUTH_CLIENTS);
}
