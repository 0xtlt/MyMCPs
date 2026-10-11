//! Port of `tests/functional/vine_gateway_oauth.spec.ts`, through the HTTP
//! endpoints: what a request is answered with, parameter by parameter, as
//! the body parser and the query string hand them to the OAuth server.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::StatusCode;
use mymcps_core::models::{OauthClient, User};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::oauth::GatewayAuthorizationRequest;
use mymcps_web::testing::factories::create_admin;
use mymcps_web::testing::{TestApp, TestResponse};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use url::Url;

const RESOURCE: &str = "http://localhost:3333/mcp";
const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const REMOTE_REDIRECT_URI: &str = "https://client.example/callback";
const CODE_VERIFIER: &str = "vine-oauth-code-verifier-for-mymcps-gateway-tests-123";
const FORM: &str = "application/x-www-form-urlencoded";

fn code_challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(CODE_VERIFIER.as_bytes()))
}

fn registration() -> Value {
    json!({
        "client_name": "Vine client",
        "redirect_uris": ["http://127.0.0.1/callback"],
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp:tools",
    })
}

/// The registration with some of its fields replaced or added.
fn registration_with(overrides: Value) -> Value {
    let mut metadata = registration();
    if let (Some(metadata), Some(overrides)) = (metadata.as_object_mut(), overrides.as_object()) {
        for (field, value) in overrides {
            metadata.insert(field.clone(), value.clone());
        }
    }
    metadata
}

async fn post_registration(app: &TestApp, metadata: Value) -> TestResponse {
    app.post("/register").api().json(metadata).send().await
}

async fn register(app: &TestApp, overrides: Value) -> OauthClient {
    let response = post_registration(app, registration_with(overrides)).await;
    assert_eq!(response.status, StatusCode::CREATED);
    sqlx::query_as("select * from `oauth_clients` where `client_id` = ?")
        .bind(response.json()["client_id"].as_str().unwrap())
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

type Pairs = Vec<(&'static str, String)>;

fn authorization(client_id: &str, redirect_uri: &str) -> Pairs {
    vec![
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("response_type", "code".to_string()),
        ("code_challenge", code_challenge()),
        ("code_challenge_method", "S256".to_string()),
        ("scope", "mcp:tools".to_string()),
        ("resource", RESOURCE.to_string()),
        ("state", "state-from-client".to_string()),
    ]
}

/// What a change makes of a parameter: left out, sent once (empty or not),
/// or sent several times.
#[derive(Clone)]
enum To {
    Omit,
    One(String),
    Many(Vec<String>),
}

fn one(value: &str) -> To {
    To::One(value.to_string())
}

fn many(values: &[&str]) -> To {
    To::Many(values.iter().map(|value| value.to_string()).collect())
}

/// A query string in which a parameter can be left out, sent empty or sent
/// several times.
fn query_string(base: &[(&'static str, String)], changes: &[(&'static str, To)]) -> String {
    let mut pairs: Vec<(&str, String)> = base
        .iter()
        .filter(|(key, _)| !changes.iter().any(|(changed, _)| changed == key))
        .cloned()
        .collect();
    for (key, change) in changes {
        match change {
            To::Omit => {}
            To::One(value) => pairs.push((key, value.clone())),
            To::Many(values) => pairs.extend(values.iter().map(|value| (*key, value.clone()))),
        }
    }
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", utf8_percent_encode(value, NON_ALPHANUMERIC)))
        .collect::<Vec<_>>()
        .join("&")
}

fn assert_oauth_error(response: &TestResponse, status: StatusCode, error: &str, description: &str) {
    assert_eq!(response.status, status, "{}", response.text());
    assert_eq!(
        response.json(),
        json!({ "error": error, "error_description": description })
    );
    assert_eq!(response.header("cache-control"), Some("no-store"));
}

fn between<'a>(text: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let start = text.find(before)? + before.len();
    let end = start + text[start..].find(after)?;
    Some(&text[start..end])
}

/// Where an answer sends the browser: a redirect, or the page that moves on.
fn destination(response: &TestResponse) -> Url {
    let location = match response.location() {
        Some(location) => location.to_string(),
        None => {
            let page = response.text();
            let refresh = between(
                &page,
                "<meta http-equiv=\"refresh\" content=\"0;url=",
                "\">",
            )
            .unwrap_or_else(|| panic!("not sent anywhere: {} {page}", response.status));
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

/// The error a loopback client is sent back with, as [error, description, state].
fn redirected_error(response: &TestResponse) -> [Option<String>; 3] {
    let callback = destination(response);
    assert_eq!(
        format!(
            "{}{}",
            callback.origin().ascii_serialization(),
            callback.path()
        ),
        LOOPBACK_REDIRECT_URI
    );
    [
        param(&callback, "error"),
        param(&callback, "error_description"),
        param(&callback, "state"),
    ]
}

fn text(value: &str) -> Option<String> {
    Some(value.to_string())
}

async fn get_authorize(app: &TestApp, query: &str) -> TestResponse {
    app.get(&format!("/authorize?{query}")).api().send().await
}

async fn post_token(app: &TestApp, body: String) -> TestResponse {
    app.post("/token").api().raw_body(body, FORM).send().await
}

async fn post_revoke(app: &TestApp, body: String) -> TestResponse {
    app.post("/revoke").api().raw_body(body, FORM).send().await
}

async fn is_usable(app: &TestApp, plaintext: &str) -> bool {
    access_token::find_usable_by_plaintext(&app.core.db, plaintext)
        .await
        .unwrap()
        .is_some()
}

fn is_secret(value: &Value, prefix: &str) -> bool {
    value
        .as_str()
        .and_then(|text| text.strip_prefix(prefix))
        .is_some_and(|random| {
            random.len() == 43
                && random
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

// ------------------------------------ vine: gateway OAuth client registration

#[tokio::test]
async fn answers_with_the_first_rule_a_registration_breaks() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let expected = [
        (
            "redirect_uris",
            json!(["http://example.com/callback"]),
            "invalid_redirect_uri",
            "Redirect URIs must use HTTPS, HTTP on an exact loopback host, or an approved native-app callback",
        ),
        (
            "token_endpoint_auth_method",
            json!("private_key_jwt"),
            "invalid_client_metadata",
            "Unsupported token endpoint authentication method",
        ),
        (
            "grant_types",
            json!(["implicit"]),
            "invalid_client_metadata",
            "Unsupported OAuth grant type",
        ),
        (
            "response_types",
            json!(["token"]),
            "invalid_client_metadata",
            "Only the code response type is supported",
        ),
        (
            "scope",
            json!("other"),
            "invalid_client_metadata",
            "Unsupported OAuth scope",
        ),
        (
            "client_name",
            json!("x".repeat(121)),
            "invalid_client_metadata",
            "Client name is too long",
        ),
    ];

    // Everything from one field onwards is wrong: that field decides.
    for (index, (_, _, error, description)) in expected.iter().enumerate() {
        let wrong: Map<String, Value> = expected[index..]
            .iter()
            .map(|(field, value, _, _)| (field.to_string(), value.clone()))
            .collect();
        let response = post_registration(&app, registration_with(Value::Object(wrong))).await;
        assert_oauth_error(&response, StatusCode::BAD_REQUEST, error, description);
        assert_eq!(response.header("pragma"), Some("no-cache"));
    }
    let clients: i64 = sqlx::query_scalar("select count(*) from `oauth_clients`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(clients, 0);
}

#[tokio::test]
async fn leaves_the_shape_of_the_metadata_to_the_mcp_sdk_schema() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    for overrides in [
        json!({ "redirect_uris": "https://client.example/callback" }),
        json!({ "redirect_uris": ["not a url"] }),
        json!({ "redirect_uris": [""] }),
        json!({ "token_endpoint_auth_method": ["none"] }),
        json!({ "grant_types": "authorization_code" }),
        json!({ "response_types": [1] }),
        json!({ "scope": ["mcp:tools"] }),
        json!({ "client_name": 5 }),
    ] {
        let response = post_registration(&app, registration_with(overrides.clone())).await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "Invalid OAuth client metadata",
        );
    }
}

#[tokio::test]
async fn accepts_between_one_and_ten_redirect_uris_of_at_most_2048_characters() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let uri = |length: usize| {
        format!(
            "https://client.example/{}",
            "a".repeat(length - "https://client.example/".len())
        )
    };
    let uris = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("https://client.example/{index}"))
            .collect()
    };

    for (redirect_uris, status) in [
        (vec![], StatusCode::BAD_REQUEST),
        (uris(10), StatusCode::CREATED),
        (uris(11), StatusCode::BAD_REQUEST),
        (vec![uri(2048)], StatusCode::CREATED),
        (vec![uri(2049)], StatusCode::BAD_REQUEST),
        (
            vec![
                "https://client.example/callback".to_string(),
                "http://example.com/callback".to_string(),
            ],
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = post_registration(
            &app,
            registration_with(json!({ "redirect_uris": redirect_uris })),
        )
        .await;
        assert_eq!(response.status, status);
        if status == StatusCode::CREATED {
            assert_eq!(response.json()["redirect_uris"], json!(redirect_uris));
        } else {
            assert_eq!(response.json()["error"], "invalid_redirect_uri");
        }
    }
}

#[tokio::test]
async fn fills_in_the_defaults_and_echoes_the_metadata_it_was_sent() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    let response = post_registration(
        &app,
        json!({
            "redirect_uris": [REMOTE_REDIRECT_URI],
            "client_uri": "https://client.example",
            "contacts": ["ops@client.example"],
            "software_id": "vine-client",
            "software_version": "1.2.3",
            "not_in_the_registry": "dropped",
        }),
    )
    .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let mut body = response.json();
    assert!(is_secret(&body["client_secret"], "mcp_secret_"));
    assert!(
        body["client_id"]
            .as_str()
            .unwrap()
            .starts_with("mcp_client_")
    );
    assert!(body["client_id_issued_at"].is_i64());
    assert!(body["client_secret_expires_at"].is_i64());
    for issued in [
        "client_id",
        "client_secret",
        "client_id_issued_at",
        "client_secret_expires_at",
    ] {
        body[issued] = Value::Null;
    }
    assert_eq!(
        body,
        json!({
            "redirect_uris": [REMOTE_REDIRECT_URI],
            "client_uri": "https://client.example",
            "contacts": ["ops@client.example"],
            "software_id": "vine-client",
            "software_version": "1.2.3",
            "client_name": "MCP client",
            "token_endpoint_auth_method": "client_secret_basic",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "scope": "mcp:tools",
            "client_id": null,
            "client_id_issued_at": null,
            "client_secret": null,
            "client_secret_expires_at": null,
        })
    );

    // The body parser trims what it reads, before the server sees it.
    let padded = post_registration(
        &app,
        registration_with(json!({
            "client_name": format!("  {}  ", "x".repeat(120)),
            "scope": "  mcp:tools  ",
            "grant_types": ["refresh_token", "authorization_code", "refresh_token"],
        })),
    )
    .await;
    assert_eq!(padded.status, StatusCode::CREATED);
    let padded = padded.json();
    assert_eq!(padded["client_name"], "x".repeat(120));
    assert_eq!(padded["scope"], "mcp:tools");
    assert_eq!(
        padded["grant_types"],
        json!(["refresh_token", "authorization_code"])
    );
}

// ---------------------------------- vine: gateway OAuth authorization request

#[tokio::test]
async fn tells_a_missing_client_id_from_one_that_names_no_client() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for client_id in [
        To::Omit,
        one(""),
        many(&[&oauth_client.client_id, &oauth_client.client_id]),
    ] {
        let response = get_authorize(&app, &query_string(&base, &[("client_id", client_id)])).await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "client_id is required",
        );
    }

    // A blank value is a client id, and names no client.
    for client_id in [
        " ".to_string(),
        format!(" {}", oauth_client.client_id),
        "mcp_client_unknown".to_string(),
    ] {
        let response = get_authorize(
            &app,
            &query_string(&base, &[("client_id", To::One(client_id))]),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_client",
            "Unknown OAuth client",
        );
        assert_eq!(
            response.header("www-authenticate"),
            Some("Basic realm=\"MyMCPs OAuth\"")
        );
    }
}

#[tokio::test]
async fn never_redirects_to_a_uri_the_client_did_not_register() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for redirect_uri in [
        To::Omit,
        one(""),
        one(" "),
        many(&[LOOPBACK_REDIRECT_URI, LOOPBACK_REDIRECT_URI]),
        one("http://127.0.0.1:49152/other"),
        one("http://localhost:49152/callback"),
        one("https://127.0.0.1:49152/callback"),
        one("http://127.0.0.1:49152/callback?x=1"),
        one(REMOTE_REDIRECT_URI),
    ] {
        let response = get_authorize(
            &app,
            &query_string(
                &base,
                &[
                    ("redirect_uri", redirect_uri),
                    ("response_type", one("token")),
                ],
            ),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Unregistered redirect_uri",
        );
        assert_eq!(response.location(), None);
    }

    let remote = register(&app, json!({ "redirect_uris": [REMOTE_REDIRECT_URI] })).await;
    let other_port = get_authorize(
        &app,
        &query_string(
            &authorization(&remote.client_id, "https://client.example:8443/callback"),
            &[],
        ),
    )
    .await;
    assert_oauth_error(
        &other_port,
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Unregistered redirect_uri",
    );
}

#[tokio::test]
async fn answers_with_the_first_parameter_an_authorization_request_gets_wrong() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);
    let long_state = "s".repeat(2049);
    let expected = [
        (
            "response_type",
            "token".to_string(),
            "unsupported_response_type",
            "Only the code response type is supported",
        ),
        (
            "code_challenge",
            "too-short".to_string(),
            "invalid_request",
            "PKCE with the S256 method is required",
        ),
        (
            "scope",
            "other".to_string(),
            "invalid_scope",
            "Unsupported OAuth scope",
        ),
        (
            "resource",
            "https://other.example/mcp".to_string(),
            "invalid_target",
            "The OAuth resource must be the MyMCPs gateway",
        ),
        (
            "state",
            long_state.clone(),
            "invalid_request",
            "OAuth state is too long",
        ),
    ];

    // Everything from one parameter onwards is wrong: that parameter decides.
    for (index, (_, _, error, description)) in expected.iter().enumerate() {
        let changes: Vec<(&'static str, To)> = expected[index..]
            .iter()
            .map(|(field, value, _, _)| (*field, To::One(value.clone())))
            .collect();
        let response = get_authorize(&app, &query_string(&base, &changes)).await;
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(
            redirected_error(&response),
            [text(error), text(description), Some(long_state.clone())]
        );
    }
}

#[tokio::test]
async fn requires_pkce_with_a_well_formed_s256_challenge() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);
    let challenge = code_challenge();

    for change in [
        ("code_challenge", To::Omit),
        ("code_challenge", one("")),
        ("code_challenge", To::One("a".repeat(42))),
        ("code_challenge", To::One("a".repeat(129))),
        ("code_challenge", To::One(format!("{}.", "a".repeat(42)))),
        ("code_challenge", many(&[&challenge, &challenge])),
        ("code_challenge_method", To::Omit),
        ("code_challenge_method", one("plain")),
        ("code_challenge_method", one("s256")),
        ("code_challenge_method", many(&["S256", "S256"])),
    ] {
        let response = get_authorize(&app, &query_string(&base, &[change])).await;
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(
            redirected_error(&response),
            [
                text("invalid_request"),
                text("PKCE with the S256 method is required"),
                text("state-from-client"),
            ]
        );
    }

    for challenge in ["a".repeat(43), "a".repeat(128)] {
        let response = get_authorize(
            &app,
            &query_string(&base, &[("code_challenge", To::One(challenge))]),
        )
        .await;
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(response.location(), Some("/login"));
    }
}

#[tokio::test]
async fn accepts_the_gateway_scope_and_resource_in_the_spellings_clients_use() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for change in [
        ("scope", To::Omit),
        ("scope", one(" mcp:tools ")),
        // Sent twice, the parameter is not a scope string and counts as left out.
        ("scope", many(&["other", "another"])),
        ("resource", one("http://LOCALHOST:3333/mcp")),
        ("resource", one("HTTP://localhost:3333/a/../mcp")),
    ] {
        let response = get_authorize(&app, &query_string(&base, &[change])).await;
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(response.location(), Some("/login"));
    }

    for scope in [
        "",
        " ",
        "mcp:tools mcp:tools",
        "mcp:tools other",
        "MCP:TOOLS",
    ] {
        let response = get_authorize(&app, &query_string(&base, &[("scope", one(scope))])).await;
        assert_eq!(
            redirected_error(&response)[..2],
            [text("invalid_scope"), text("Unsupported OAuth scope")]
        );
    }

    for target in [
        To::Omit,
        one(""),
        one("http://localhost:3333/mcp/"),
        one("http://localhost:3333/mcp?x=1"),
        one("http://localhost:3333"),
        one("not a url"),
        many(&[RESOURCE, RESOURCE]),
    ] {
        let response = get_authorize(&app, &query_string(&base, &[("resource", target)])).await;
        assert_eq!(
            redirected_error(&response)[..2],
            [
                text("invalid_target"),
                text("The OAuth resource must be the MyMCPs gateway"),
            ]
        );
    }
}

#[tokio::test]
async fn echoes_the_state_exactly_as_the_client_sent_it() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let mut base = authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);
    base.retain(|(key, _)| *key != "response_type");
    base.push(("response_type", "token".to_string()));

    for (state, echoed) in [
        (To::Omit, None),
        (one(""), text("")),
        (one(" "), text(" ")),
        (one("a b&c=d#e%20"), text("a b&c=d#e%20")),
        (To::One("s".repeat(2049)), Some("s".repeat(2049))),
        // Sent twice, the parameter is not a state.
        (many(&["one", "two"]), None),
    ] {
        let response = get_authorize(&app, &query_string(&base, &[("state", state)])).await;
        assert_eq!(
            redirected_error(&response),
            [
                text("unsupported_response_type"),
                text("Only the code response type is supported"),
                echoed,
            ]
        );
    }
}

#[tokio::test]
async fn grants_access_on_an_explicit_approval_only() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let base = query_string(
        &authorization(&oauth_client.client_id, LOOPBACK_REDIRECT_URI),
        &[],
    );
    let decide = |decision: &'static str| {
        let body = if decision.is_empty() {
            base.clone()
        } else {
            format!("{base}&{decision}")
        };
        app.post("/authorize")
            .login_as(&admin)
            .csrf()
            .raw_body(body, FORM)
            .send()
    };

    for decision in [
        "",
        "decision=",
        "decision=deny",
        "decision=APPROVE",
        "decision=true",
        "decision=approve&decision=approve",
        "decision[0]=approve&decision[1]=approve",
        "decision[value]=approve",
    ] {
        let response = decide(decision).await;
        assert_eq!(
            redirected_error(&response),
            [
                text("access_denied"),
                text("The user denied the authorization request"),
                text("state-from-client"),
            ],
            "{decision}"
        );
    }
    let codes: i64 = sqlx::query_scalar("select count(*) from `oauth_authorization_codes`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(codes, 0);

    let approval = decide("decision=approve").await;
    let callback = destination(&approval);
    assert_eq!(param(&callback, "code").map(|code| code.len()), Some(43));
    assert_eq!(
        param(&callback, "state").as_deref(),
        Some("state-from-client")
    );
}

// ------------------------ vine: gateway OAuth token and revocation endpoints

async fn issue_code(app: &TestApp, oauth_client: &OauthClient, admin: &User) -> String {
    app.state
        .gateway
        .oauth
        .create_authorization_code(
            &GatewayAuthorizationRequest {
                client: oauth_client.clone(),
                redirect_uri: LOOPBACK_REDIRECT_URI.to_string(),
                state: None,
                code_challenge: code_challenge(),
                scopes: "mcp:tools".to_string(),
                resource: RESOURCE.to_string(),
            },
            admin.id,
        )
        .await
        .unwrap()
}

/// The access token and the refresh token of a new grant.
async fn issue_grant(app: &TestApp, oauth_client: &OauthClient, admin: &User) -> (String, String) {
    let created = access_token::create_oauth_grant(
        &*app.core.db,
        NewOauthGrant {
            name: &oauth_client.client_name,
            client_id: oauth_client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: RESOURCE,
            created_by: admin.id,
        },
    )
    .await
    .unwrap();
    (created.plaintext, created.refresh_token.unwrap())
}

fn basic(id: &str, secret: &str) -> String {
    // Identifiers and secrets hold nothing that percent-encoding would change.
    format!("Basic {}", STANDARD.encode(format!("{id}:{secret}")))
}

#[tokio::test]
async fn authenticates_the_client_before_it_reads_the_grant() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let credentials = vec![("client_id", oauth_client.client_id.clone())];

    for body in [
        String::new(),
        "client_id=".to_string(),
        query_string(
            &[],
            &[(
                "client_id",
                many(&[&oauth_client.client_id, &oauth_client.client_id]),
            )],
        ),
    ] {
        let response = post_token(&app, body).await;
        assert_oauth_error(
            &response,
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "OAuth client authentication is required",
        );
        assert_eq!(
            response.header("www-authenticate"),
            Some("Basic realm=\"MyMCPs OAuth\"")
        );
        assert_eq!(response.header("pragma"), Some("no-cache"));
    }

    let unknown = post_token(&app, "client_id=mcp_client_unknown".to_string()).await;
    assert_oauth_error(
        &unknown,
        StatusCode::UNAUTHORIZED,
        "invalid_client",
        "Invalid OAuth client credentials",
    );

    // A public client that sends a secret is not the client that registered.
    let with_secret = post_token(
        &app,
        format!("{}&client_secret=surplus", query_string(&credentials, &[])),
    )
    .await;
    assert_oauth_error(
        &with_secret,
        StatusCode::UNAUTHORIZED,
        "invalid_client",
        "Invalid OAuth client credentials",
    );

    // A secret sent twice is not a secret, and the client is a public one.
    let secret_twice = post_token(
        &app,
        format!(
            "{}&client_secret=surplus&client_secret=surplus",
            query_string(&credentials, &[])
        ),
    )
    .await;
    assert_oauth_error(
        &secret_twice,
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "grant_type is required",
    );
}

#[tokio::test]
async fn accepts_a_client_secret_in_the_body_or_in_a_basic_header() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let posted = post_registration(
        &app,
        registration_with(json!({ "token_endpoint_auth_method": "client_secret_post" })),
    )
    .await
    .json();
    let with_basic = post_registration(
        &app,
        registration_with(json!({ "token_endpoint_auth_method": "client_secret_basic" })),
    )
    .await
    .json();
    let field = |client: &Value, name: &str| client[name].as_str().unwrap().to_string();
    let posted_id = field(&posted, "client_id");
    let posted_secret = field(&posted, "client_secret");
    let basic_id = field(&with_basic, "client_id");
    let basic_secret = field(&with_basic, "client_secret");
    let with_header = |authorization: String| {
        app.post("/token")
            .api()
            .header("authorization", &authorization)
            .raw_body("", FORM)
            .send()
    };

    let via_body = post_token(
        &app,
        format!("client_id={posted_id}&client_secret={posted_secret}"),
    )
    .await;
    assert_oauth_error(
        &via_body,
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "grant_type is required",
    );

    let via_header = with_header(basic(&basic_id, &basic_secret)).await;
    assert_oauth_error(
        &via_header,
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "grant_type is required",
    );

    for response in [
        post_token(&app, format!("client_id={posted_id}")).await,
        post_token(
            &app,
            format!("client_id={posted_id}&client_secret=mcp_secret_wrong"),
        )
        .await,
        with_header(basic(&basic_id, "mcp_secret_wrong")).await,
        // The method a client registered is the only one it may use.
        with_header(basic(&posted_id, &posted_secret)).await,
        post_token(
            &app,
            format!("client_id={basic_id}&client_secret={basic_secret}"),
        )
        .await,
    ] {
        assert_oauth_error(
            &response,
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Invalid OAuth client credentials",
        );
    }
}

#[tokio::test]
async fn names_the_grant_type_it_misses_and_refuses_those_it_does_not_have() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let credentials = vec![("client_id", oauth_client.client_id.clone())];

    for grant_type in [To::Omit, one(""), many(&["refresh_token", "refresh_token"])] {
        let response = post_token(
            &app,
            query_string(&credentials, &[("grant_type", grant_type)]),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "grant_type is required",
        );
    }

    for grant_type in ["client_credentials", "password", "AUTHORIZATION_CODE"] {
        let response = post_token(
            &app,
            query_string(&credentials, &[("grant_type", one(grant_type))]),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Only authorization_code and refresh_token grants are supported",
        );
    }
}

#[tokio::test]
async fn names_the_first_parameter_an_authorization_code_grant_misses() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let grant: Pairs = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", oauth_client.client_id.clone()),
        ("code", issue_code(&app, &oauth_client, &admin).await),
        ("code_verifier", CODE_VERIFIER.to_string()),
        ("redirect_uri", LOOPBACK_REDIRECT_URI.to_string()),
        ("resource", RESOURCE.to_string()),
    ];
    let parameters = ["code", "code_verifier", "redirect_uri", "resource"];

    for (index, parameter) in parameters.iter().enumerate() {
        // Everything from this parameter onwards is missing, empty or sent twice.
        for value in [To::Omit, one(""), many(&["one", "two"])] {
            let changes: Vec<(&'static str, To)> = parameters[index..]
                .iter()
                .map(|name| (*name, value.clone()))
                .collect();
            let response = post_token(&app, query_string(&grant, &changes)).await;
            assert_oauth_error(
                &response,
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &format!("{parameter} is required"),
            );
        }
    }

    // None of these requests used the code up.
    let exchange = post_token(&app, query_string(&grant, &[])).await;
    assert_eq!(exchange.status, StatusCode::OK);
    assert!(is_secret(&exchange.json()["access_token"], "mcp_"));
}

#[tokio::test]
async fn answers_a_malformed_verifier_or_a_foreign_resource_like_a_code_that_does_not_match() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let grant: Pairs = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", oauth_client.client_id.clone()),
        ("code", issue_code(&app, &oauth_client, &admin).await),
        ("code_verifier", CODE_VERIFIER.to_string()),
        ("redirect_uri", LOOPBACK_REDIRECT_URI.to_string()),
        ("resource", RESOURCE.to_string()),
    ];

    for change in [
        ("code_verifier", To::One("a".repeat(42))),
        ("code_verifier", To::One("a".repeat(129))),
        ("code_verifier", To::One(format!("{CODE_VERIFIER}!"))),
        (
            "code_verifier",
            one("another-verifier-that-is-long-enough-to-be-well-formed"),
        ),
        ("resource", one("https://other.example/mcp")),
        ("resource", one("http://localhost:3333/mcp/")),
        ("resource", one("not a url")),
        ("redirect_uri", one("http://127.0.0.1/callback")),
        ("code", one("unknown-code")),
    ] {
        let response = post_token(&app, query_string(&grant, &[change])).await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Invalid or expired authorization code",
        );
    }

    let exchange = post_token(
        &app,
        query_string(&grant, &[("resource", one("http://LOCALHOST:3333/mcp"))]),
    )
    .await;
    assert_eq!(exchange.status, StatusCode::OK);
}

#[tokio::test]
async fn checks_a_refresh_request_in_the_order_client_scope_resource_token() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let (_, refresh_token) = issue_grant(&app, &oauth_client, &admin).await;
    let grant: Pairs = vec![
        ("grant_type", "refresh_token".to_string()),
        ("client_id", oauth_client.client_id.clone()),
        ("refresh_token", refresh_token),
        ("resource", RESOURCE.to_string()),
    ];

    for (changes, description) in [
        (
            vec![("refresh_token", To::Omit), ("resource", To::Omit)],
            "refresh_token is required",
        ),
        (
            vec![("refresh_token", many(&["one", "two"]))],
            "refresh_token is required",
        ),
        (
            vec![("resource", To::Omit), ("scope", one("other"))],
            "resource is required",
        ),
        (vec![("resource", one(""))], "resource is required"),
    ] {
        let response = post_token(&app, query_string(&grant, &changes)).await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            description,
        );
    }

    for scope in ["other", "mcp:tools mcp:tools", "MCP:TOOLS"] {
        let response = post_token(
            &app,
            query_string(
                &grant,
                &[
                    ("scope", one(scope)),
                    ("resource", one("https://other.example/mcp")),
                ],
            ),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "Unsupported OAuth scope",
        );
    }
    // An empty scope in the query string is a scope, and names none.
    let empty_scope = app
        .post("/token?scope=")
        .api()
        .raw_body(query_string(&grant, &[]), FORM)
        .send()
        .await;
    assert_oauth_error(
        &empty_scope,
        StatusCode::BAD_REQUEST,
        "invalid_scope",
        "Unsupported OAuth scope",
    );
    // In the body it is a parameter left empty, which the parser reads as left out.
    let empty_body_scope = post_token(
        &app,
        query_string(
            &grant,
            &[
                ("scope", one("")),
                ("refresh_token", one("mcp_refresh_unknown")),
            ],
        ),
    )
    .await;
    assert_oauth_error(
        &empty_body_scope,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    for target in [
        "https://other.example/mcp",
        "http://localhost:3333/mcp/",
        "not a url",
    ] {
        let response = post_token(
            &app,
            query_string(
                &grant,
                &[
                    ("resource", one(target)),
                    ("refresh_token", one("mcp_refresh_unknown")),
                ],
            ),
        )
        .await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "The OAuth resource must be the MyMCPs gateway",
        );
    }

    let unknown = post_token(
        &app,
        query_string(&grant, &[("refresh_token", one("mcp_refresh_unknown"))]),
    )
    .await;
    assert_oauth_error(
        &unknown,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    // Sent twice, the scope is not a scope string and counts as left out.
    let refreshed = post_token(
        &app,
        query_string(
            &grant,
            &[
                ("scope", many(&["other", "another"])),
                ("resource", one("http://LOCALHOST:3333/mcp")),
            ],
        ),
    )
    .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    let refreshed = refreshed.json();
    assert!(is_secret(&refreshed["refresh_token"], "mcp_refresh_"));

    let again = post_token(
        &app,
        query_string(
            &grant,
            &[
                (
                    "refresh_token",
                    one(refreshed["refresh_token"].as_str().unwrap()),
                ),
                ("scope", one("mcp:tools")),
            ],
        ),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);
}

#[tokio::test]
async fn refuses_a_refresh_to_a_client_registered_without_that_grant_whatever_else_is_wrong() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({ "grant_types": ["authorization_code"] })).await;
    let (_, refresh_token) = issue_grant(&app, &oauth_client, &admin).await;

    let response = post_token(
        &app,
        query_string(
            &[
                ("grant_type", "refresh_token".to_string()),
                ("client_id", oauth_client.client_id.clone()),
                ("refresh_token", refresh_token),
                ("scope", "other".to_string()),
                ("resource", "https://other.example/mcp".to_string()),
            ],
            &[],
        ),
    )
    .await;

    assert_oauth_error(
        &response,
        StatusCode::BAD_REQUEST,
        "unauthorized_client",
        "This OAuth client cannot refresh tokens",
    );
}

#[tokio::test]
async fn requires_a_token_to_revoke_and_says_nothing_about_tokens_it_does_not_know() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let oauth_client = register(&app, json!({})).await;
    let (access_token, _) = issue_grant(&app, &oauth_client, &admin).await;
    let credentials = vec![("client_id", oauth_client.client_id.clone())];

    for token in [To::Omit, one(""), many(&["one", "two"])] {
        let response = post_revoke(&app, query_string(&credentials, &[("token", token)])).await;
        assert_oauth_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "token is required",
        );
    }
    let anonymous = post_revoke(&app, format!("token={access_token}")).await;
    assert_oauth_error(
        &anonymous,
        StatusCode::UNAUTHORIZED,
        "invalid_client",
        "OAuth client authentication is required",
    );
    assert!(is_usable(&app, &access_token).await);

    // A blank token in the query string is a token, and one nobody holds.
    for response in [
        post_revoke(
            &app,
            query_string(&credentials, &[("token", one("mcp_unknown"))]),
        )
        .await,
        app.post("/revoke?token=%20")
            .api()
            .raw_body(query_string(&credentials, &[]), FORM)
            .send()
            .await,
    ] {
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.json(), json!({}));
        assert_eq!(response.header("cache-control"), Some("no-store"));
    }
    assert!(is_usable(&app, &access_token).await);

    // Body and query string are read together, the query string first.
    let query_wins = app
        .post("/revoke?token=mcp_unknown")
        .api()
        .raw_body(
            query_string(&credentials, &[("token", To::One(access_token.clone()))]),
            FORM,
        )
        .send()
        .await;
    assert_eq!(query_wins.status, StatusCode::OK);
    assert!(is_usable(&app, &access_token).await);

    let revoked = post_revoke(
        &app,
        query_string(&credentials, &[("token", To::One(access_token.clone()))]),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::OK);
    assert!(!is_usable(&app, &access_token).await);
}
