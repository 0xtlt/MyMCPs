//! Port of `tests/functional/hardening_gateway_oauth.spec.ts`: the cases
//! that go through the HTTP endpoints. The others (how stored type lists
//! are read, which clients are pruned) are tests of the gateway crate.

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{OauthClient, User};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::oauth::MAX_OAUTH_CLIENTS;
use mymcps_web::testing::factories::create_admin;
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::field::{Field, Visit};
use tracing::span;
use url::Url;

const RESOURCE: &str = "http://localhost:3333/mcp";
const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const REMOTE_REDIRECT_URI: &str = "https://client.example/callback";

fn code_challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(
        b"hardening-oauth-code-verifier-for-mymcps-tests-123",
    ))
}

async fn register_client(app: &TestApp, redirect_uri: &str) -> String {
    let response = app
        .post("/register")
        .api()
        .json(json!({
            "client_name": "Hardening client",
            "redirect_uris": [redirect_uri],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        }))
        .send()
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    response.json()["client_id"].as_str().unwrap().to_string()
}

fn authorization_payload(client_id: &str, redirect_uri: &str) -> Vec<(String, String)> {
    [
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("response_type", "code".to_string()),
        ("code_challenge", code_challenge()),
        ("code_challenge_method", "S256".to_string()),
        ("scope", "mcp:tools".to_string()),
        ("resource", RESOURCE.to_string()),
        ("state", "state-from-client".to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// The payload with some of its parameters replaced or added.
fn changed(payload: &[(String, String)], changes: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = payload
        .iter()
        .filter(|(name, _)| !changes.iter().any(|(changed, _)| changed == name))
        .cloned()
        .collect();
    all.extend(
        changes
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
    all
}

fn authorization_path(payload: &[(String, String)]) -> String {
    let mut url = Url::parse("http://localhost/authorize").unwrap();
    url.query_pairs_mut().extend_pairs(payload);
    format!("/authorize?{}", url.query().unwrap())
}

fn fields(payload: &[(String, String)]) -> Vec<(&str, &str)> {
    payload
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn between<'a>(text: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let start = text.find(before)? + before.len();
    let end = start + text[start..].find(after)?;
    Some(&text[start..end])
}

/// Where an answer sends the browser: a redirect, the address given to the
/// page's script, or the page that moves on.
fn destination(response: &TestResponse) -> Url {
    let location = match response
        .location()
        .or_else(|| response.header("x-location"))
    {
        Some(location) => location.to_string(),
        None => {
            let page = response.text();
            let refresh = between(
                &page,
                "<meta http-equiv=\"refresh\" content=\"0;url=",
                "\">",
            )
            .expect("a page that moves on");
            let link =
                between(&page, "id=\"oauth-continue\" href=\"", "\"").expect("a link to follow");
            assert_eq!(refresh, link);
            link.replace("&amp;", "&")
        }
    };
    Url::parse(&location).unwrap_or_else(|_| panic!("not an absolute URL: {location}"))
}

fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

async fn authorization_codes(app: &TestApp) -> i64 {
    sqlx::query_scalar("select count(*) from `oauth_authorization_codes`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

async fn client_count(app: &TestApp) -> i64 {
    sqlx::query_scalar("select count(*) from `oauth_clients`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

async fn stored_client(app: &TestApp, name: &str) -> OauthClient {
    let mut client = OauthClient {
        client_id: format!("mcp_client_{name}"),
        client_name: name.to_string(),
        redirect_uris: json!([LOOPBACK_REDIRECT_URI]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: "[\"authorization_code\",\"refresh_token\"]".into(),
        response_types: "[\"code\"]".into(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&*app.core.db).await.unwrap();
    client
}

async fn stored_grant(app: &TestApp, client: &OauthClient, user: &User) {
    access_token::create_oauth_grant(
        &*app.core.db,
        NewOauthGrant {
            name: &client.client_name,
            client_id: client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: RESOURCE,
            created_by: user.id,
        },
    )
    .await
    .unwrap();
}

// -------------------------------------- hardening: OAuth authorization endpoint

#[tokio::test]
async fn answers_head_authorize_with_405_and_never_issues_a_code() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let client_id = register_client(&app, REMOTE_REDIRECT_URI).await;
    let payload = changed(
        &authorization_payload(&client_id, REMOTE_REDIRECT_URI),
        &[("decision", "approve")],
    );

    let response = app
        .request(http::Method::HEAD, &authorization_path(&payload))
        .login_as(&admin)
        .api()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.header("allow"), Some("GET, POST"));
    assert_eq!(response.location(), None);
    assert_eq!(response.header("x-location"), None);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(authorization_codes(&app).await, 0);
}

#[tokio::test]
async fn reads_the_decision_from_the_form_body_not_from_the_query_string() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let client_id = register_client(&app, LOOPBACK_REDIRECT_URI).await;
    let authorization = authorization_payload(&client_id, LOOPBACK_REDIRECT_URI);

    for script in [false, true] {
        let mut request = app
            .post("/authorize?decision=approve")
            .login_as(&admin)
            .csrf()
            .form(&fields(&authorization));
        if script {
            request = request.header("x-requested-with", "fetch");
        }
        let response = request.send().await;

        let callback = destination(&response);
        assert_eq!(param(&callback, "error").as_deref(), Some("access_denied"));
        assert_eq!(param(&callback, "code"), None);
        assert_eq!(authorization_codes(&app).await, 0);
    }

    let query_only = app
        .post(&authorization_path(&changed(
            &authorization,
            &[("decision", "approve")],
        )))
        .login_as(&admin)
        .csrf()
        .form(&[])
        .send()
        .await;

    assert_eq!(query_only.status, StatusCode::BAD_REQUEST);
    assert_eq!(query_only.json()["error"], "invalid_request");
    assert_eq!(authorization_codes(&app).await, 0);
}

#[tokio::test]
async fn shows_a_rejected_request_here_instead_of_redirecting_to_a_remote_client() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let client_id = register_client(&app, REMOTE_REDIRECT_URI).await;
    let payload = changed(
        &authorization_payload(&client_id, REMOTE_REDIRECT_URI),
        &[("response_type", "token")],
    );

    let response = app.get(&authorization_path(&payload)).api().send().await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.location(), None);
    assert_eq!(response.json()["error"], "unsupported_response_type");
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(response.header("pragma"), Some("no-cache"));

    // Nor is a browser sent there, by the script or by a page that moves on.
    let admin = create_admin(&app).await;
    for script in [false, true] {
        let mut request = app
            .post("/authorize")
            .login_as(&admin)
            .csrf()
            .form(&fields(&changed(&payload, &[("decision", "approve")])));
        if script {
            request = request.header("x-requested-with", "fetch");
        }
        let posted = request.send().await;
        assert_eq!(posted.status, StatusCode::BAD_REQUEST);
        assert_eq!(posted.location(), None);
        assert_eq!(posted.header("x-location"), None);
        assert_eq!(posted.json()["error"], "unsupported_response_type");
        assert!(!posted.text().contains("client.example"));
    }
}

#[tokio::test]
async fn returns_a_rejected_request_to_a_loopback_client_with_its_state_intact() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let client_id = register_client(&app, LOOPBACK_REDIRECT_URI).await;
    let payload = changed(
        &authorization_payload(&client_id, LOOPBACK_REDIRECT_URI),
        &[("response_type", "token")],
    );

    let response = app.get(&authorization_path(&payload)).api().send().await;

    assert_eq!(response.status, StatusCode::FOUND);
    let callback = destination(&response);
    assert_eq!(
        callback.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    let keys: Vec<String> = callback
        .query_pairs()
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(keys, ["error", "error_description", "state"]);
    assert_eq!(
        param(&callback, "error").as_deref(),
        Some("unsupported_response_type")
    );
    assert_eq!(
        param(&callback, "state").as_deref(),
        Some("state-from-client")
    );

    // The same request from the consent form is returned the same way, as
    // forms are: without a redirect.
    let admin = create_admin(&app).await;
    let posted = app
        .post("/authorize")
        .login_as(&admin)
        .csrf()
        .form(&fields(&changed(&payload, &[("decision", "approve")])))
        .send()
        .await;
    assert_eq!(posted.status, StatusCode::OK);
    assert_eq!(posted.location(), None);
    assert_eq!(destination(&posted), callback);
    assert_eq!(authorization_codes(&app).await, 0);
}

#[tokio::test]
async fn sends_signed_out_users_to_the_login_page_without_the_authorization_query() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let client_id = register_client(&app, REMOTE_REDIRECT_URI).await;

    let response = app
        .get(&authorization_path(&authorization_payload(
            &client_id,
            REMOTE_REDIRECT_URI,
        )))
        .api()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some("/login"));
}

#[tokio::test]
async fn still_returns_the_operator_decision_to_a_remote_client() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let client_id = register_client(&app, REMOTE_REDIRECT_URI).await;
    let authorization = authorization_payload(&client_id, REMOTE_REDIRECT_URI);
    let decide = |decision: &'static str, script: bool| {
        let mut request = app
            .post("/authorize")
            .login_as(&admin)
            .csrf()
            .form(&fields(&changed(&authorization, &[("decision", decision)])));
        if script {
            request = request.header("x-requested-with", "fetch");
        }
        request.send()
    };

    for script in [false, true] {
        let denial = decide("deny", script).await;
        assert_eq!(
            denial.status,
            if script {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::OK
            }
        );
        let denied = destination(&denial);
        assert_eq!(
            format!("{}{}", denied.origin().ascii_serialization(), denied.path()),
            REMOTE_REDIRECT_URI
        );
        assert_eq!(param(&denied, "error").as_deref(), Some("access_denied"));
        assert_eq!(
            param(&denied, "state").as_deref(),
            Some("state-from-client")
        );
        assert_eq!(authorization_codes(&app).await, 0);
    }

    for (script, issued) in [(false, 1), (true, 2)] {
        let approval = decide("approve", script).await;
        let approved = destination(&approval);
        assert_eq!(
            format!(
                "{}{}",
                approved.origin().ascii_serialization(),
                approved.path()
            ),
            REMOTE_REDIRECT_URI
        );
        let code = param(&approved, "code").unwrap_or_default();
        assert!(
            code.len() == 43
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "{code}"
        );
        assert_eq!(
            param(&approved, "state").as_deref(),
            Some("state-from-client")
        );
        assert_eq!(authorization_codes(&app).await, issued);
    }
}

/// The fields of one log entry, by name.
type Entry = Vec<(String, String)>;

/// What a handler logged at the error level while this was the subscriber.
#[derive(Clone, Default)]
struct ErrorLog {
    entries: Arc<Mutex<Vec<Entry>>>,
}

struct Fields(Entry);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }
}

impl tracing::Subscriber for ErrorLog {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::ERROR
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        self.entries.lock().unwrap().push(fields.0);
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

#[tokio::test]
async fn logs_an_unexpected_oauth_failure_without_the_raw_error() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    // The database refuses the new client, with a message that quotes a secret.
    sqlx::query(
        "create trigger `refuse_oauth_clients` before insert on `oauth_clients` begin \
         select raise(abort, 'insert into `oauth_clients` (`client_secret_hash`) values (''Bearer raw-secret-value'') - SQLITE_BUSY'); end",
    )
    .execute(&*app.core.db)
    .await
    .unwrap();

    let log = ErrorLog::default();
    let response = {
        let _capturing = tracing::subscriber::set_default(log.clone());
        app.post("/register")
            .api()
            .json(json!({
                "client_name": "Failing client",
                "redirect_uris": [LOOPBACK_REDIRECT_URI],
                "token_endpoint_auth_method": "none",
            }))
            .send()
            .await
    };

    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.json(),
        json!({
            "error": "server_error",
            "error_description": "The OAuth request could not be completed",
        })
    );
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert!(!response.text().contains("raw-secret-value"));

    let logged = log.entries.lock().unwrap().clone();
    assert_eq!(logged.len(), 1, "{logged:?}");
    let field = |name: &str| {
        logged[0]
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(field("message").as_deref(), Some("OAuth request failed"));
    assert!(field("error").unwrap().contains("Bearer [REDACTED]"));
    assert!(!format!("{logged:?}").contains("raw-secret-value"));
}

// ---------------------------------------- hardening: OAuth client registration

#[tokio::test]
async fn stores_each_grant_and_response_type_once() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let mut grant_types = vec!["authorization_code"; 5000];
    grant_types.extend(vec!["refresh_token"; 5000]);

    let response = app
        .post("/register")
        .api()
        .json(json!({
            "client_name": "Repeating client",
            "redirect_uris": [LOOPBACK_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
            "grant_types": grant_types,
            "response_types": ["code", "code", "code"],
        }))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    assert_eq!(
        body["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(body["response_types"], json!(["code"]));

    let stored: OauthClient = sqlx::query_as("select * from `oauth_clients` where `client_id` = ?")
        .bind(body["client_id"].as_str().unwrap())
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(
        stored.grant_types,
        "[\"authorization_code\",\"refresh_token\"]"
    );
    assert_eq!(stored.response_types, "[\"code\"]");
}

#[tokio::test]
async fn still_rejects_grant_and_response_types_outside_the_allowed_set() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    for (field, value) in [
        (
            "grant_types",
            json!(["authorization_code", "client_credentials"]),
        ),
        ("grant_types", json!(["refresh_token", "refresh_token"])),
        ("response_types", json!(["code", "token"])),
    ] {
        let mut metadata = json!({
            "client_name": "Unsupported client",
            "redirect_uris": [LOOPBACK_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
        });
        metadata[field] = value;
        let response = app.post("/register").api().json(metadata).send().await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(response.json()["error"], "invalid_client_metadata");
        assert_eq!(response.header("cache-control"), Some("no-store"));
    }
    assert_eq!(client_count(&app).await, 0);
}

#[tokio::test]
async fn evicts_the_oldest_unused_client_when_the_client_limit_is_reached() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let in_use = stored_client(&app, "in-use").await;
    stored_grant(&app, &in_use, &admin).await;
    let oldest_unused = stored_client(&app, "oldest-unused").await;

    let created_at = Timestamp::now();
    for index in 0..MAX_OAUTH_CLIENTS - 2 {
        sqlx::query(
            "insert into `oauth_clients` (`client_id`, `client_name`, `redirect_uris`, `token_endpoint_auth_method`, `grant_types`, `response_types`, `scope`, `created_at`) \
             values (?, 'Filler', '[]', 'none', '[\"authorization_code\"]', '[\"code\"]', 'mcp:tools', ?)",
        )
        .bind(format!("mcp_client_filler_{index}"))
        .bind(created_at)
        .execute(&*app.core.db)
        .await
        .unwrap();
    }
    assert_eq!(client_count(&app).await, MAX_OAUTH_CLIENTS);

    register_client(&app, LOOPBACK_REDIRECT_URI).await;

    assert_eq!(client_count(&app).await, MAX_OAUTH_CLIENTS);
    assert!(
        OauthClient::find(&*app.core.db, in_use.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        OauthClient::find(&*app.core.db, oldest_unused.id)
            .await
            .unwrap()
            .is_none()
    );

    // Once every client is in use there is nothing left to evict.
    sqlx::query(
        "insert into `oauth_authorization_codes` \
         (`code_hash`, `oauth_client_id`, `user_id`, `redirect_uri`, `code_challenge`, `scopes`, `resource`, `expires_at`, `created_at`) \
         select 'pending-' || `id`, `id`, ?, ?, ?, 'mcp:tools', ?, ?, ? from `oauth_clients`",
    )
    .bind(admin.id)
    .bind(LOOPBACK_REDIRECT_URI)
    .bind(code_challenge())
    .bind(RESOURCE)
    .bind(Timestamp::now() + chrono::Duration::minutes(5))
    .bind(created_at)
    .execute(&*app.core.db)
    .await
    .unwrap();

    let refused = app
        .post("/register")
        .api()
        .json(json!({
            "client_name": "One too many",
            "redirect_uris": [LOOPBACK_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused.json()["error"], "temporarily_unavailable");
    assert_eq!(client_count(&app).await, MAX_OAUTH_CLIENTS);
}

#[tokio::test]
async fn holds_registration_to_twenty_clients_an_hour_for_one_address() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let register_from = |address: &'static str| {
        app.post("/register")
            .api()
            .header("x-forwarded-for", address)
            .json(json!({
                "redirect_uris": [LOOPBACK_REDIRECT_URI],
                "token_endpoint_auth_method": "none",
            }))
            .send()
    };

    for _ in 0..20 {
        assert_eq!(
            register_from("198.51.100.7").await.status,
            StatusCode::CREATED
        );
    }
    let refused = register_from("198.51.100.7").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.json(),
        json!({ "error": "too_many_requests", "error_description": "Try again later" })
    );
    assert_eq!(refused.header("cache-control"), Some("no-store"));
    assert_eq!(client_count(&app).await, 20);

    assert_eq!(
        register_from("198.51.100.8").await.status,
        StatusCode::CREATED
    );
}
