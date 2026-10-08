//! Port of `tests/functional/gateway_oauth_server.spec.ts`, and of what the
//! server does in the browser spec `gateway_oauth_consent`.
//!
//! Where the Node app answered the consent form with a redirect (or with
//! Inertia's `409` and `X-Inertia-Location`), this server tells the page's
//! script where to go (`X-Location`) and gives a form posted without it a
//! page that moves on: `destination` reads all three.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::StatusCode;
use mymcps_core::models::{AccessToken, TokenSource, User};
use mymcps_gateway::access_token;
use mymcps_gateway::bearer::{BearerError, BearerRejection};
use mymcps_web::testing::factories::{create_admin, create_admin_with};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

const RESOURCE: &str = "http://localhost:3333/mcp";
const REGISTERED_REDIRECT_URI: &str = "http://127.0.0.1/callback";
const RUNTIME_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const CODE_VERIFIER: &str = "oauth-code-verifier-for-mymcps-tests-1234567890";

fn code_challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(CODE_VERIFIER.as_bytes()))
}

async fn register_public_client(app: &TestApp, client_name: &str) -> Value {
    let response = app
        .post("/register")
        .api()
        .json(json!({
            "client_name": client_name,
            "redirect_uris": [REGISTERED_REDIRECT_URI],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "scope": "mcp:tools",
        }))
        .send()
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    response.json()
}

fn client_id(registered: &Value) -> String {
    registered["client_id"].as_str().unwrap().to_string()
}

fn authorization_payload(client_id: &str) -> Vec<(String, String)> {
    [
        ("client_id", client_id.to_string()),
        ("redirect_uri", RUNTIME_REDIRECT_URI.to_string()),
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

fn with<'a>(
    payload: &'a [(String, String)],
    extra: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    let mut all = fields(payload);
    all.extend_from_slice(extra);
    all
}

fn between<'a>(text: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let start = text.find(before)? + before.len();
    let end = start + text[start..].find(after)?;
    Some(&text[start..end])
}

fn unescape(html: &str) -> String {
    html.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Where an answer sends the browser: a redirect, the address given to the
/// page's script, or the page that moves on, whose refresh and whose link
/// must name the same place.
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
            unescape(link)
        }
    };
    Url::parse(&location).unwrap_or_else(|_| panic!("not an absolute URL: {location}"))
}

fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn is_secret(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|random| {
        random.len() == 43
            && random
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

async fn approve(app: &TestApp, admin: &User, authorization: &[(String, String)]) -> TestResponse {
    app.post("/authorize")
        .login_as(admin)
        .csrf()
        .form(&with(authorization, &[("decision", "approve")]))
        .send()
        .await
}

async fn token(app: &TestApp, form: &[(&str, &str)]) -> TestResponse {
    app.post("/token").api().form(form).send().await
}

async fn find_by_name(app: &TestApp, name: &str) -> AccessToken {
    sqlx::query_as("select * from `access_tokens` where `name` = ?")
        .bind(name)
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

async fn is_usable(app: &TestApp, plaintext: &str) -> bool {
    access_token::find_usable_by_plaintext(&app.core.db, plaintext)
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn publishes_mcp_protected_resource_and_authorization_server_metadata() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let resource_response = app.get(path).api().send().await;
        assert_eq!(resource_response.status, StatusCode::OK);
        assert_eq!(
            resource_response.json(),
            json!({
                "resource": RESOURCE,
                "authorization_servers": ["http://localhost:3333"],
                "scopes_supported": ["mcp:tools"],
                "bearer_methods_supported": ["header"],
                "resource_name": "MyMCPs gateway",
            })
        );
        assert_eq!(
            resource_response.header("cache-control"),
            Some("public, max-age=3600")
        );
        assert_eq!(
            resource_response.header("content-type"),
            Some("application/json; charset=utf-8")
        );
    }

    let metadata_response = app
        .get("/.well-known/oauth-authorization-server")
        .api()
        .send()
        .await;
    assert_eq!(metadata_response.status, StatusCode::OK);
    let metadata = metadata_response.json();
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
        metadata_response.header("cache-control"),
        Some("public, max-age=3600")
    );
}

#[tokio::test]
async fn publishes_nothing_while_app_url_cannot_be_the_address_of_the_server() {
    for app_url in [
        None,
        Some("http://mcp.example.com"),
        Some("https://mcp.example.com/app"),
    ] {
        let app = TestApp::with_config(|config| config.app_url = app_url.map(str::to_string)).await;
        let admin = create_admin(&app).await;
        let unavailable = json!({
            "error": "temporarily_unavailable",
            "error_description": "OAuth requires APP_URL to be a public HTTPS origin",
        });

        for response in [
            app.get("/.well-known/oauth-authorization-server")
                .api()
                .send()
                .await,
            app.get("/.well-known/oauth-protected-resource")
                .api()
                .send()
                .await,
            app.get("/.well-known/oauth-protected-resource/mcp")
                .api()
                .send()
                .await,
            app.post("/register")
                .api()
                .json(json!({ "redirect_uris": [RUNTIME_REDIRECT_URI] }))
                .send()
                .await,
            app.post("/token")
                .api()
                .form(&[("grant_type", "authorization_code")])
                .send()
                .await,
            app.post("/revoke")
                .api()
                .form(&[("token", "mcp_any")])
                .send()
                .await,
            app.get("/authorize?client_id=mcp_client_any")
                .api()
                .send()
                .await,
            app.post("/authorize")
                .login_as(&admin)
                .csrf()
                .form(&[("decision", "approve")])
                .send()
                .await,
        ] {
            assert_eq!(
                response.status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{app_url:?}"
            );
            assert_eq!(response.json(), unavailable);
            assert_eq!(response.header("cache-control"), Some("no-store"));
        }
    }
}

#[tokio::test]
async fn answers_installed_clients_from_any_origin() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    let metadata = app
        .get("/.well-known/oauth-authorization-server")
        .api()
        .header("origin", "https://client.example")
        .send()
        .await;
    assert_eq!(metadata.header("access-control-allow-origin"), Some("*"));

    let refused = app
        .post("/token")
        .api()
        .header("origin", "https://client.example")
        .form(&[("client_id", "mcp_client_unknown")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused.header("access-control-allow-origin"), Some("*"));
    assert_eq!(
        refused.header("access-control-expose-headers"),
        Some("WWW-Authenticate")
    );

    let preflight = app
        .request(http::Method::OPTIONS, "/register")
        .api()
        .header("origin", "https://client.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .send()
        .await;
    assert_eq!(preflight.status, StatusCode::NO_CONTENT);
    assert_eq!(preflight.header("access-control-allow-origin"), Some("*"));

    // The consent screen is a page of the instance: no other origin reads it.
    let consent = app
        .get("/authorize?client_id=mcp_client_unknown")
        .api()
        .header("origin", "https://client.example")
        .send()
        .await;
    assert_eq!(consent.header("access-control-allow-origin"), None);
}

#[tokio::test]
async fn registers_public_clients_and_rejects_unsafe_redirect_uris() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let register = |metadata: Value| app.post("/register").api().json(metadata).send();

    let registered = register_public_client(&app, "Codex test client").await;
    let id = client_id(&registered);
    assert!(
        id.strip_prefix("mcp_client_")
            .is_some_and(|random| random.len() == 32),
        "{id}"
    );
    assert_eq!(registered["token_endpoint_auth_method"], "none");

    let cursor = register(json!({
        "client_name": "Cursor",
        "redirect_uris": ["cursor://anysphere.cursor-mcp/oauth/callback"],
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp:tools",
    }))
    .await;
    assert_eq!(cursor.status, StatusCode::CREATED);
    assert_eq!(
        cursor.json()["redirect_uris"],
        json!(["cursor://anysphere.cursor-mcp/oauth/callback"])
    );
    assert_eq!(cursor.header("cache-control"), Some("no-store"));

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
        let refused = register(json!({
            "client_name": client_name,
            "redirect_uris": [redirect_uri],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
        }))
        .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{client_name}");
        assert_eq!(refused.json()["error"], "invalid_redirect_uri");
    }

    let confidential = register(json!({
        "client_name": "Confidential client",
        "redirect_uris": ["https://client.example.com/callback"],
        "token_endpoint_auth_method": "client_secret_basic",
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
    }))
    .await;
    assert_eq!(confidential.status, StatusCode::CREATED);
    let confidential_client = confidential.json();
    // Identifier and secret hold nothing that percent-encoding would change.
    let basic_credentials = base64::engine::general_purpose::STANDARD.encode(format!(
        "{}:{}",
        confidential_client["client_id"].as_str().unwrap(),
        confidential_client["client_secret"].as_str().unwrap()
    ));
    let lowercase_basic = app
        .post("/token")
        .api()
        .header("authorization", &format!("basic {basic_credentials}"))
        .form(&[])
        .send()
        .await;
    assert_eq!(lowercase_basic.status, StatusCode::BAD_REQUEST);
    assert_eq!(lowercase_basic.json()["error"], "invalid_request");
}

#[tokio::test]
async fn returns_users_to_the_authorization_request_after_credential_login() {
    let app = TestApp::new().await;
    create_admin_with(&app, "oauth-user@example.com").await;
    let registered = register_public_client(&app, "Codex test client").await;
    let path = authorization_path(&authorization_payload(&client_id(&registered)));

    let start = app.get(&path).api().send().await;
    assert_eq!(start.status, StatusCode::FOUND);
    assert_eq!(start.location(), Some("/login"));
    assert_eq!(start.header("cache-control"), Some("no-store"));
    assert_eq!(start.session().get("oauthReturnTo"), Some(&json!(path)));

    let login = app
        .post("/login")
        .session(start.session())
        .csrf()
        .form(&[
            ("email", "oauth-user@example.com"),
            ("password", "password123"),
        ])
        .send()
        .await;

    assert_eq!(login.status, StatusCode::FOUND);
    assert_eq!(login.redirect_path().as_deref(), Some("/authorize"));
    assert_eq!(login.location(), Some(path.as_str()));
    assert!(!login.session().contains_key("oauthReturnTo"));

    let consent = app
        .get(login.location().unwrap())
        .session(login.session())
        .send()
        .await;
    assert_eq!(consent.status, StatusCode::OK);
    let page = consent.text();
    assert!(page.contains(
        "<h1 class=\"auth-card__title\" id=\"authorize-title\">Authorize Codex test client</h1>"
    ));
    assert!(
        page.contains("<p class=\"auth-card__subtitle\">Signed in as oauth-user@example.com</p>")
    );
    assert!(page.contains("<p class=\"text-body-sm text-tertiary\">Callback: 127.0.0.1:49152</p>"));
    assert!(page.contains("<title>Authorize MCP client · MyMCPs</title>"));
}

#[tokio::test]
async fn rejects_authorization_return_paths_too_large_for_the_cookie_session() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let registered = register_public_client(&app, "Codex test client").await;
    let state = "s".repeat(1400);
    let mut payload = authorization_payload(&client_id(&registered));
    payload.retain(|(name, _)| name != "state");
    payload.push(("state".to_string(), state.clone()));

    let response = app.get(&authorization_path(&payload)).api().send().await;

    // Only a loopback client is sent its error; see hardening_gateway_oauth.rs.
    assert_eq!(response.status, StatusCode::FOUND);
    let callback = destination(&response);
    assert_eq!(
        callback.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    assert_eq!(
        param(&callback, "error").as_deref(),
        Some("invalid_request")
    );
    assert_eq!(param(&callback, "state"), Some(state));
    assert!(!response.session().contains_key("oauthReturnTo"));
}

#[tokio::test]
async fn shows_the_consent_screen_of_an_authorization_request() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "owner@example.com").await;
    let registered = register_public_client(&app, "Claude <Desktop>").await;
    let payload = authorization_payload(&client_id(&registered));

    let consent = app
        .get(&authorization_path(&payload))
        .login_as(&admin)
        .send()
        .await;

    assert_eq!(consent.status, StatusCode::OK);
    assert_eq!(consent.header("cache-control"), Some("no-store"));
    let page = consent.text();
    // What a client calls itself is text, never markup.
    assert!(page.contains("Authorize Claude &lt;Desktop&gt;</h1>"));
    assert!(page.contains("<code class=\"code code--info\">mcp:tools</code>"));
    assert!(page.contains("<p class=\"banner__title\">Local callback</p>"));
    assert!(page.contains(
        "After approval, the authorization code will be sent to 127.0.0.1:49152. Only continue if you started this connection from an MCP client on this device."
    ));
    // The form carries the request back, for the page's script to send.
    assert!(page.contains(
        "<form class=\"auth-card__actions\" method=\"post\" action=\"/authorize\" data-async>"
    ));
    assert!(page.contains("data-fragment>"));
    assert!(page.contains("name=\"_csrf\""));
    for (name, value) in &payload {
        assert!(
            page.contains(&format!(
                "<input type=\"hidden\" name=\"{name}\" value=\"{value}\">"
            )),
            "{name}"
        );
    }
    assert!(page.contains(
        "<button type=\"submit\" class=\"button button--secondary\" name=\"decision\" value=\"deny\">Cancel</button>"
    ));
    assert!(page.contains(
        "<button type=\"submit\" class=\"button button--primary\" name=\"decision\" value=\"approve\">Authorize client</button>"
    ));

    // A client somewhere else gets no warning about a local callback, and
    // a request without a state sends none back.
    let remote = app
        .post("/register")
        .api()
        .json(json!({
            "client_name": "Remote client",
            "redirect_uris": ["https://client.example:8443/callback"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .json();
    let mut payload = authorization_payload(&client_id(&remote));
    payload.retain(|(name, _)| name != "state" && name != "redirect_uri");
    payload.push((
        "redirect_uri".to_string(),
        "https://client.example:8443/callback".to_string(),
    ));
    let page = app
        .get(&authorization_path(&payload))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(page.contains("Callback: client.example:8443</p>"));
    assert!(!page.contains("Local callback"));
    assert!(!page.contains("name=\"state\""));
}

#[tokio::test]
async fn sends_the_browser_to_the_client_without_a_redirect_after_the_consent_form() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let registered = register_public_client(&app, "Browser callback client").await;
    let authorization = authorization_payload(&client_id(&registered));

    // The page's script is told where to go, and navigates there itself.
    let scripted = app
        .post("/authorize")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&with(&authorization, &[("decision", "approve")]))
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::NO_CONTENT);
    assert_eq!(scripted.location(), None);
    let callback = Url::parse(scripted.header("x-location").unwrap()).unwrap();
    assert_eq!(
        callback.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    assert_eq!(
        param(&callback, "state").as_deref(),
        Some("state-from-client")
    );
    assert_eq!(param(&callback, "code").map(|code| code.len()), Some(43));
    assert_eq!(scripted.header("cache-control"), Some("no-store"));

    // A form posted without the script gets a page that moves on: the policy
    // of the pages forbids a form to end on another site through a redirect.
    let plain = approve(&app, &admin, &authorization).await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.location(), None);
    assert_eq!(plain.header("cache-control"), Some("no-store"));
    assert!(
        plain
            .header("content-security-policy")
            .unwrap()
            .contains("form-action 'self'")
    );
    let page = plain.text();
    assert!(page.contains("<h1 class=\"auth-card__title\" id=\"leaving-title\">Returning to Browser callback client</h1>"));
    assert!(page.contains("127.0.0.1:49152"));
    assert!(!page.contains("<script"));
    let callback = destination(&plain);
    assert_eq!(callback.path(), "/callback");
    assert_eq!(
        param(&callback, "state").as_deref(),
        Some("state-from-client")
    );
    let code = param(&callback, "code").unwrap();
    assert_eq!(code.len(), 43);

    // Either way the code is one the client can exchange.
    let exchange = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id(&registered)),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
            ("redirect_uri", RUNTIME_REDIRECT_URI),
            ("resource", RESOURCE),
        ],
    )
    .await;
    assert_eq!(exchange.status, StatusCode::OK);

    // A session that ended while the consent screen was open: the script is
    // sent to sign in, and the request waits in the session.
    let signed_out = app
        .post("/authorize")
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&with(&authorization, &[("decision", "approve")]))
        .send()
        .await;
    assert_eq!(signed_out.status, StatusCode::NO_CONTENT);
    assert_eq!(signed_out.header("x-location"), Some("/login"));
    assert!(signed_out.session().contains_key("oauthReturnTo"));

    // Without the token of the form, nothing is decided.
    let forged = app
        .post("/authorize")
        .login_as(&admin)
        .api()
        .form(&with(&authorization, &[("decision", "approve")]))
        .send()
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn issues_refreshes_lists_and_revokes_an_oauth_connection() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "owner@example.com").await;
    let registered = register_public_client(&app, "Claude Desktop").await;
    let id = client_id(&registered);
    let authorization = authorization_payload(&id);

    let consent = app
        .get(&authorization_path(&authorization))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(consent.status, StatusCode::OK);
    let page = consent.text();
    assert!(page.contains("Authorize Claude Desktop</h1>"));
    assert!(page.contains("Callback: 127.0.0.1:49152"));
    assert!(page.contains("Local callback"));
    assert!(page.contains("<code class=\"code code--info\">mcp:tools</code>"));

    let approval = app
        .post("/authorize")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&with(&authorization, &[("decision", "approve")]))
        .send()
        .await;
    assert_eq!(approval.status, StatusCode::NO_CONTENT);
    let callback = destination(&approval);
    assert_eq!(
        callback.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    assert_eq!(
        param(&callback, "state").as_deref(),
        Some("state-from-client")
    );
    let code = param(&callback, "code").expect("an authorization code");

    let exchange = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &id),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
            ("redirect_uri", RUNTIME_REDIRECT_URI),
            ("resource", RESOURCE),
        ],
    )
    .await;
    assert_eq!(exchange.status, StatusCode::OK);
    assert_eq!(exchange.header("cache-control"), Some("no-store"));
    assert_eq!(exchange.header("pragma"), Some("no-cache"));
    let issued = exchange.json();
    assert_eq!(issued["token_type"], "Bearer");
    assert_eq!(issued["expires_in"], 3600);
    assert_eq!(issued["scope"], "mcp:tools");
    let oauth_access_token = issued["access_token"].as_str().unwrap().to_string();
    let old_refresh_token = issued["refresh_token"].as_str().unwrap().to_string();
    assert!(is_secret(&oauth_access_token, "mcp_"));
    assert!(is_secret(&old_refresh_token, "mcp_refresh_"));

    let mut oauth_token = find_by_name(&app, "Claude Desktop").await;
    assert_eq!(oauth_token.source, TokenSource::Oauth);
    assert_eq!(oauth_token.created_by, admin.id);
    assert_ne!(oauth_token.token_hash, oauth_access_token);
    assert_ne!(
        oauth_token.oauth_refresh_token_hash.as_deref(),
        Some(old_refresh_token.as_str())
    );
    assert!(oauth_token.is_active());

    // The gateway lets the token in while it names this gateway and its
    // scope. (`POST /mcp` itself is another module's; this is what it asks.)
    let bearer = format!("Bearer {oauth_access_token}");
    let gateway = &app.state.gateway;
    assert!(gateway.authenticate_bearer(Some(&bearer)).await.is_ok());

    oauth_token.oauth_resource = Some("https://old-gateway.example.com/mcp".into());
    oauth_token.save(&*app.core.db).await.unwrap();
    assert!(matches!(
        gateway.authenticate_bearer(Some(&bearer)).await,
        Err(BearerError::Rejected(BearerRejection::Invalid))
    ));

    oauth_token.oauth_resource = Some(RESOURCE.into());
    oauth_token.oauth_scopes = Some("unsupported:scope".into());
    oauth_token.save(&*app.core.db).await.unwrap();
    assert!(matches!(
        gateway.authenticate_bearer(Some(&bearer)).await,
        Err(BearerError::Rejected(BearerRejection::Invalid))
    ));

    oauth_token.oauth_scopes = Some("mcp:tools".into());
    oauth_token.save(&*app.core.db).await.unwrap();

    let token_list = app.get("/tokens").login_as(&admin).send().await;
    assert_eq!(token_list.status, StatusCode::OK);
    let list = token_list.text();
    let row = list
        .split("<tbody>")
        .nth(1)
        .unwrap()
        .split("</tr>")
        .find(|row| row.contains("Claude Desktop"))
        .expect("a row for the connection");
    assert!(row.contains("<td class=\"cell-strong\">Claude Desktop</td>"));
    assert!(row.contains("<span class=\"badge badge--info badge--no-dot\">OAuth</span>"));
    assert!(row.contains("<span class=\"badge badge--success\">Active</span>"));
    assert!(row.contains(&format!(
        "<form id=\"revoke-token-{}\" method=\"post\" action=\"/tokens/{}/revoke\"",
        oauth_token.id, oauth_token.id
    )));
    assert!(!row.contains(">Edit</a>"));

    let refresh = token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &id),
            ("refresh_token", &old_refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await;
    assert_eq!(refresh.status, StatusCode::OK);
    let refreshed = refresh.json();
    let new_access_token = refreshed["access_token"].as_str().unwrap();
    let new_refresh_token = refreshed["refresh_token"].as_str().unwrap();
    assert_ne!(new_access_token, oauth_access_token);
    assert_ne!(new_refresh_token, old_refresh_token);
    assert!(!is_usable(&app, &oauth_access_token).await);
    assert!(is_usable(&app, new_access_token).await);

    let revoke = app
        .post(&format!("/tokens/{}/revoke", oauth_token.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(revoke.status, StatusCode::FOUND);

    let revoked = AccessToken::find(&*app.core.db, oauth_token.id)
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.is_revoked());
    assert!(!is_usable(&app, new_access_token).await);

    let refresh_after_revoke = token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &id),
            ("refresh_token", new_refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await;
    assert_eq!(refresh_after_revoke.status, StatusCode::BAD_REQUEST);
    assert_eq!(refresh_after_revoke.json()["error"], "invalid_grant");
}

#[tokio::test]
async fn revokes_the_active_grant_when_a_rotated_refresh_token_is_replayed() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let registered = register_public_client(&app, "Refresh replay client").await;
    let id = client_id(&registered);
    let approval = approve(&app, &admin, &authorization_payload(&id)).await;
    let code = param(&destination(&approval), "code").unwrap();
    let exchange = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &id),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
            ("redirect_uri", RUNTIME_REDIRECT_URI),
            ("resource", RESOURCE),
        ],
    )
    .await;
    assert_eq!(exchange.status, StatusCode::OK);

    let old_refresh_token = exchange.json()["refresh_token"]
        .as_str()
        .unwrap()
        .to_string();
    let refresh_with = |refresh_token: String| {
        let id = id.clone();
        let app = &app;
        async move {
            token(
                app,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &id),
                    ("refresh_token", &refresh_token),
                    ("resource", RESOURCE),
                ],
            )
            .await
        }
    };
    let refresh = refresh_with(old_refresh_token.clone()).await;
    assert_eq!(refresh.status, StatusCode::OK);
    let refreshed = refresh.json();

    let replay = refresh_with(old_refresh_token).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.json()["error"], "invalid_grant");

    let grant = find_by_name(&app, "Refresh replay client").await;
    assert!(grant.is_revoked());
    assert!(!is_usable(&app, refreshed["access_token"].as_str().unwrap()).await);

    let attacker_refresh =
        refresh_with(refreshed["refresh_token"].as_str().unwrap().to_string()).await;
    assert_eq!(attacker_refresh.status, StatusCode::BAD_REQUEST);
    assert_eq!(attacker_refresh.json()["error"], "invalid_grant");
}

#[tokio::test]
async fn rejects_authorization_code_replay_and_an_incorrect_pkce_verifier() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let registered = register_public_client(&app, "Codex test client").await;
    let id = client_id(&registered);
    let approval = approve(&app, &admin, &authorization_payload(&id)).await;
    let code = param(&destination(&approval), "code").unwrap();
    let exchange_with = |code_verifier: &'static str| {
        let id = id.clone();
        let code = code.clone();
        let app = &app;
        async move {
            token(
                app,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &id),
                    ("code", &code),
                    ("code_verifier", code_verifier),
                    ("redirect_uri", RUNTIME_REDIRECT_URI),
                    ("resource", RESOURCE),
                ],
            )
            .await
        }
    };

    let invalid_verifier =
        exchange_with("incorrect-verifier-that-is-still-long-enough-123456789").await;
    assert_eq!(invalid_verifier.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_verifier.json()["error"], "invalid_grant");

    let valid = exchange_with(CODE_VERIFIER).await;
    assert_eq!(valid.status, StatusCode::OK);

    let replay = exchange_with(CODE_VERIFIER).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.json()["error"], "invalid_grant");
}

#[tokio::test]
async fn holds_each_client_address_to_its_allowance() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let limited =
        json!({ "error": "temporarily_unavailable", "error_description": "Try again later" });

    // Token requests: 50 in 15 minutes for one address.
    let token_from = |address: &'static str| {
        app.post("/token")
            .api()
            .header("x-forwarded-for", address)
            .form(&[("client_id", "mcp_client_unknown")])
            .send()
    };
    for _ in 0..50 {
        assert_eq!(
            token_from("198.51.100.7").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let refused = token_from("198.51.100.7").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json(), limited);
    assert_eq!(refused.header("cache-control"), Some("no-store"));
    assert_eq!(refused.header("pragma"), Some("no-cache"));
    // Another address has its own allowance, and so does revocation.
    assert_eq!(
        token_from("198.51.100.8").await.status,
        StatusCode::UNAUTHORIZED
    );
    let revoke_from = |address: &'static str| {
        app.post("/revoke")
            .api()
            .header("x-forwarded-for", address)
            .form(&[("token", "mcp_any")])
            .send()
    };
    for _ in 0..50 {
        assert_eq!(
            revoke_from("198.51.100.7").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(revoke_from("198.51.100.7").await.json(), limited);

    // Authorization requests: 100 in 15 minutes.
    let authorize_from = |address: &'static str| {
        app.get("/authorize")
            .api()
            .header("x-forwarded-for", address)
            .send()
    };
    for _ in 0..100 {
        assert_eq!(
            authorize_from("198.51.100.7").await.status,
            StatusCode::BAD_REQUEST
        );
    }
    let refused = authorize_from("198.51.100.7").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json(), limited);
    assert_eq!(
        authorize_from("198.51.100.8").await.status,
        StatusCode::BAD_REQUEST
    );
}
