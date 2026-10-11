//! The setup and the sign-in of the MCPs MyMCPs runs itself, through the
//! MCPs page. Port of the setup, OAuth and write access groups of
//! `tests/functional/builtin_strava_mcp.spec.ts`,
//! `builtin_icloud_mail_mcp.spec.ts` and `builtin_google_ads_mcp.spec.ts`,
//! and of what the browser specs `builtin_strava_setup`,
//! `builtin_strava_app_icon` and `builtin_icloud_mail_setup` check that the
//! server decides. What agents then reach through the gateway is tested
//! with the gateway and with each provider.

mod mcps_support;

use http::StatusCode;
use mcps_support::*;
use mymcps_builtin::BuiltinEnv;
use mymcps_core::Timestamp;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport, User};
use mymcps_core::secrets::{EnvironmentInput, decrypt_environment, merge_environment};
use mymcps_google_ads::testing::{FakeGoogleAds, GOOGLE_ADS_SCOPE};
use mymcps_icloud_mail::testing::{FakeIcloud, FakeOptions, PASSWORD, SignInAttempt, USERNAME};
use mymcps_net::{CannedResponse, Fetcher};
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use mymcps_web::state::builtins;
use mymcps_web::testing::factories::{create_admin, create_mcp};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::{Value, json};
use url::Url;

fn with<'a>(base: &[(&'a str, &'a str)], fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form: Vec<(&str, &str)> = base
        .iter()
        .filter(|(name, _)| !fields.iter().any(|(replaced, _)| replaced == name))
        .copied()
        .collect();
    form.extend_from_slice(fields);
    form
}

async fn post(app: &TestApp, user: &User, form: &[(&str, &str)]) -> TestResponse {
    app.post("/mcps")
        .login_as(user)
        .csrf()
        .form(form)
        .send()
        .await
}

async fn put(app: &TestApp, user: &User, id: i64, form: &[(&str, &str)]) -> TestResponse {
    app.put(&format!("/mcps/{id}"))
        .login_as(user)
        .csrf()
        .form(form)
        .send()
        .await
}

/// A refused form as the page script gets it back: the dialog, with its errors.
async fn refused(app: &TestApp, user: &User, path: &str, form: &[(&str, &str)]) -> String {
    let request = if path == "/mcps" {
        app.post(path)
    } else {
        app.put(path)
    };
    let response = from_script(request)
        .login_as(user)
        .csrf()
        .form(form)
        .send()
        .await;
    assert_eq!(
        response.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        response.text()
    );
    response.text()
}

/// The messages of the fields a refused dialog marks, in the order of the dialog.
fn field_errors(dialog: &str) -> Vec<String> {
    dialog
        .split("<p class=\"field__error\"")
        .skip(1)
        .map(|rest| {
            let text = &rest[rest.find('>').unwrap() + 1..];
            text[..text.find("</p>").unwrap()]
                .replace("&quot;", "\"")
                .replace("&amp;", "&")
        })
        .collect()
}

async fn edit_dialog(app: &TestApp, user: &User, id: i64) -> String {
    let response = from_script(app.get(&format!("/mcps/{id}/edit")))
        .header("x-fragment", "edit-mcp")
        .login_as(user)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    response.text()
}

// ----------------------------------------------------------------- Strava

const STRAVA_FORM: [(&str, &str); 8] = [
    ("name", "Strava"),
    ("description", "Training log"),
    ("transport", "builtin"),
    ("builtinKey", "strava"),
    ("authType", "auto"),
    ("oauthClientId", "123456"),
    ("oauthClientSecret", "strava-client-secret"),
    ("enabled", "on"),
];

fn strava_json(body: Value, status: u16) -> CannedResponse {
    json_status(status, body)
}

/// Shapes follow real Strava API v3 responses, including the fields agents never need.
fn strava_default(call: &Call) -> CannedResponse {
    match call.path().as_str() {
        "/api/v3/oauth/token" => strava_json(
            json!({
                "token_type": "Bearer",
                "expires_at": 1791127000,
                "expires_in": 21600,
                "refresh_token": "strava-refresh-token",
                "access_token": "strava-access-token",
                "athlete": { "id": 4242 },
            }),
            200,
        ),
        "/api/v3/athlete" => strava_json(
            json!({
                "id": 4242,
                "resource_state": 3,
                "firstname": "Test",
                "lastname": "Athlete",
                "city": "Lyon",
                "country": "France",
                "weight": 70.5,
            }),
            200,
        ),
        _ => strava_json(
            json!({
                "message": "Record Not Found",
                "errors": [{ "resource": "Resource", "field": "", "code": "not found" }],
            }),
            404,
        ),
    }
}

/// The app in front of a fake Strava. `respond` handles the cases a test
/// cares about and returns `None` to fall back to the defaults.
async fn app_with_strava(
    respond: impl Fn(&Call) -> Option<CannedResponse> + Send + Sync + 'static,
) -> (TestApp, Calls) {
    app_answering(move |call| {
        if call.origin() != "https://www.strava.com" {
            return not_found();
        }
        respond(call).unwrap_or_else(|| strava_default(call))
    })
    .await
}

fn token_requests(calls: &Calls) -> Vec<Call> {
    calls.with_path("/api/v3/oauth/token")
}

fn api_requests(calls: &Calls) -> Vec<Call> {
    calls
        .all()
        .into_iter()
        .filter(|call| call.path() != "/api/v3/oauth/token")
        .collect()
}

/// A built-in Strava MCP as the setup form leaves it, optionally already authorized.
async fn create_strava_mcp(app: &TestApp, created_by: i64, connected: bool, write: bool) -> Mcp {
    let secret = encrypt(app, "strava-client-secret");
    let access = encrypt(app, "strava-access-token");
    let refresh = encrypt(app, "strava-refresh-token");
    create_mcp(app, created_by, |mcp| {
        mcp.name = "Strava".into();
        mcp.slug = "strava".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("strava".into());
        mcp.status = if connected {
            McpStatus::Ready
        } else {
            McpStatus::Draft
        };
        mcp.oauth_required = !connected;
        mcp.builtin_write_enabled = write;
        mcp.oauth_client_id = Some("123456".into());
        mcp.oauth_client_secret = secret;
        if connected {
            mcp.oauth_access_token = access;
            mcp.oauth_refresh_token = refresh;
            mcp.oauth_token_type = Some("Bearer".into());
            mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::hours(5));
            mcp.oauth_scopes = Some(format!(
                "read read_all profile:read_all activity:read_all{}",
                if write {
                    " activity:write profile:write"
                } else {
                    ""
                }
            ));
        }
    })
    .await
}

#[tokio::test]
async fn strava_creates_the_mcp_with_encrypted_credentials_and_waits_for_authorization() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;

    let response = post(&app, &admin, &STRAVA_FORM).await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "strava").await.unwrap();
    assert_eq!(mcp.transport, McpTransport::Builtin);
    assert_eq!(mcp.builtin_key.as_deref(), Some("strava"));
    assert_eq!(mcp.auth_type, McpAuthType::Auto);
    assert_eq!(mcp.http_url, None);
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("123456"));
    assert!(
        !mcp.oauth_client_secret
            .clone()
            .unwrap()
            .contains("strava-client-secret")
    );
    assert_eq!(
        decrypt(&app, &mcp.oauth_client_secret).as_deref(),
        Some("strava-client-secret")
    );
    assert_eq!(mcp.status, McpStatus::Draft);
    assert!(mcp.oauth_required);
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert!(calls.is_empty());

    // The dialog opens again on what is left to do: connect the account.
    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(response.session())
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("<p class=\"banner__title\">Authorization required</p><p>Connect opens Strava so you can approve access with your account.</p>"));
    assert!(dialog.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\">Connect</a>",
        mcp.id
    )));
    assert!(dialog.contains("name=\"oauthClientId\" type=\"text\" value=\"123456\""));
    assert!(dialog.contains(">Client Secret (leave blank to keep) <span class=\"field__optional\">Optional</span></label><input class=\"input\" id=\"edit-mcp-client-secret\" name=\"oauthClientSecret\" type=\"password\" autocomplete=\"off\" aria-describedby=\"edit-mcp-client-secret-help\">"));
    // Credentials are saved: connecting is the step to do.
    assert_eq!(
        dialog.matches("<li class=\"step\" data-complete>").count(),
        2
    );
    assert!(dialog.contains("<li class=\"step\" aria-current=\"step\"><div class=\"step__body\"><h3 class=\"step__title\">Connect your Strava account</h3>"));
    assert!(!page.contains("strava-client-secret"));
    // The registry lists it as a built-in endpoint.
    let row = between(&page, "<tbody>", "</tbody>");
    assert!(row.contains("<td class=\"cell-secondary\">Built-in · Strava API</td>"));
    assert!(row.contains("<code class=\"code\">oauth</code>"));
    assert!(row.contains("<span class=\"status status--warning\">draft</span>"));
}

#[tokio::test]
async fn strava_asks_for_both_application_credentials_and_a_numeric_client_id() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;

    let missing = refused(
        &app,
        &admin,
        "/mcps",
        &with(
            &STRAVA_FORM,
            &[("oauthClientId", ""), ("oauthClientSecret", "")],
        ),
    )
    .await;
    assert_eq!(
        field_errors(&missing),
        [
            "Enter the Client ID of your Strava API application",
            "Enter the Client Secret of your Strava API application",
        ]
    );
    assert!(missing.contains("id=\"new-mcp-client-id\" name=\"oauthClientId\" type=\"text\" placeholder=\"123456\" autocomplete=\"off\" autocapitalize=\"off\" spellcheck=\"false\" aria-invalid=\"true\" aria-describedby=\"new-mcp-client-id-error\">"));
    // A plain post is told the first of them.
    let plain = post(
        &app,
        &admin,
        &with(
            &STRAVA_FORM,
            &[("oauthClientId", ""), ("oauthClientSecret", "")],
        ),
    )
    .await;
    assert_eq!(plain.status, StatusCode::FOUND);
    assert_eq!(
        plain.flashed("error"),
        Some(json!("Enter the Client ID of your Strava API application"))
    );

    // A secret pasted into the Client ID field is caught before Strava sees it.
    let swapped = refused(
        &app,
        &admin,
        "/mcps",
        &with(&STRAVA_FORM, &[("oauthClientId", "a1b2c3d4e5")]),
    )
    .await;
    assert_eq!(
        field_errors(&swapped),
        ["The Strava Client ID is a number, such as 123456"]
    );
    assert!(swapped.contains(">Set up Strava</h2>"));

    let unknown = refused(
        &app,
        &admin,
        "/mcps",
        &with(&STRAVA_FORM, &[("builtinKey", "garmin")]),
    )
    .await;
    assert!(unknown.contains("The selected builtinKey is invalid"));
    assert!(unknown.contains("<p class=\"banner__title\">Unknown built-in MCP</p><p>This version of MyMCPs does not include this built-in MCP.</p>"));

    assert_eq!(count_mcps(&app).await, 0);
    assert!(calls.is_empty());
}

#[tokio::test]
async fn strava_shares_the_callback_domain_and_never_the_client_secret_with_the_page() {
    let (app, _calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, true, false).await;

    let page = app
        .get(&format!("/mcps/{}/edit", mcp.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    // What Strava asks for when the application is registered.
    assert!(dialog.contains("<span class=\"copy-field__value\">http://localhost:3333</span><button type=\"button\" class=\"icon-button\" data-copy=\"http://localhost:3333\" aria-label=\"Copy website\">"));
    assert!(dialog.contains("<span class=\"copy-field__value\">localhost</span><button type=\"button\" class=\"icon-button\" data-copy=\"localhost\" aria-label=\"Copy authorization callback domain\">"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"transport\" value=\"builtin\"><input type=\"hidden\" name=\"builtinKey\" value=\"strava\"><input type=\"hidden\" name=\"authType\" value=\"auto\">"));
    assert!(dialog.contains("name=\"oauthClientId\" type=\"text\" value=\"123456\""));
    assert!(dialog.contains("Client Secret (leave blank to keep)"));
    // Connected: the three steps are done, and the account can be authorized again.
    assert_eq!(
        dialog.matches("<li class=\"step\" data-complete>").count(),
        3
    );
    assert!(dialog.contains("<span class=\"visually-hidden\">Done: </span>Connect your Strava account</h3><p class=\"text-body-sm\">Connected. MyMCPs reads your profile"));
    assert!(dialog.contains(&format!(
        "href=\"/mcps/{}/oauth/start\">Re-authorize</a>",
        mcp.id
    )));
    assert!(!dialog.contains("Authorization required"));
    assert!(!dialog.contains("Write access not granted yet"));
    assert!(dialog.contains("name=\"builtinWriteEnabled\"><span class=\"choice__text\"><span class=\"choice__label\">Allow write access</span>"));
    assert!(dialog.contains("Turning it on applies the next time you connect or re-authorize."));

    for secret in [
        "strava-client-secret",
        "strava-access-token",
        "strava-refresh-token",
        mcp.oauth_client_secret.as_deref().unwrap(),
        mcp.oauth_access_token.as_deref().unwrap(),
    ] {
        assert!(!page.contains(secret), "{secret}");
    }
}

#[tokio::test]
async fn strava_keeps_the_connection_when_saving_without_new_credentials_and_drops_it_when_they_change()
 {
    let (app, _calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, true, false).await;
    let save = async |fields: &[(&str, &str)]| {
        let form = with(&with(&STRAVA_FORM, &[("oauthClientSecret", "")]), fields);
        put(&app, &admin, mcp.id, &form).await
    };

    let renamed = save(&[("name", "Strava training")]).await;
    assert_eq!(renamed.flashed("success"), Some(json!("MCP updated")));
    let kept = find_mcp(&app, mcp.id).await;
    assert_eq!(kept.name, "Strava training");
    assert_eq!(
        decrypt(&app, &kept.oauth_client_secret).as_deref(),
        Some("strava-client-secret")
    );
    assert_eq!(
        decrypt(&app, &kept.oauth_access_token).as_deref(),
        Some("strava-access-token")
    );
    assert_eq!(kept.status, McpStatus::Ready);
    assert_eq!(renamed.flashed("editingMcpId"), None);

    save(&[
        ("name", "Strava training"),
        ("oauthClientSecret", "strava-client-secret"),
    ])
    .await;
    let same_secret = find_mcp(&app, mcp.id).await;
    assert!(same_secret.oauth_access_token.is_some());

    let changed = save(&[("name", "Strava training"), ("oauthClientId", "654321")]).await;
    let disconnected = find_mcp(&app, mcp.id).await;
    assert_eq!(disconnected.oauth_client_id.as_deref(), Some("654321"));
    assert_eq!(
        decrypt(&app, &disconnected.oauth_client_secret).as_deref(),
        Some("strava-client-secret")
    );
    assert_eq!(disconnected.oauth_access_token, None);
    assert_eq!(disconnected.oauth_refresh_token, None);
    assert_eq!(disconnected.oauth_scopes, None);
    assert_eq!(disconnected.status, McpStatus::Draft);
    assert!(disconnected.oauth_required);
    // The account has to be connected again: the dialog reopens for it.
    assert_eq!(changed.flashed("editingMcpId"), Some(json!(mcp.id)));
}

/// Follow the start route of a Strava MCP that is not connected yet.
async fn start_strava_authorization(app: &TestApp, admin: &User, mcp: &Mcp) -> (TestResponse, Url) {
    let start = app
        .get(&format!("/mcps/{}/oauth/start", mcp.id))
        .login_as(admin)
        .send()
        .await;
    assert_eq!(start.status, StatusCode::FOUND);
    let authorization_url = Url::parse(start.location().unwrap()).unwrap();
    (start, authorization_url)
}

#[tokio::test]
async fn strava_sends_the_admin_to_strava_and_stores_the_tokens_and_granted_scopes_on_return() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, false).await;

    let (start, authorization_url) = start_strava_authorization(&app, &admin, &mcp).await;
    assert_eq!(
        format!(
            "{}{}",
            authorization_url.origin().ascii_serialization(),
            authorization_url.path()
        ),
        "https://www.strava.com/oauth/authorize"
    );
    assert_eq!(
        query(&authorization_url, "client_id").as_deref(),
        Some("123456")
    );
    assert_eq!(
        query(&authorization_url, "redirect_uri").as_deref(),
        Some(CALLBACK)
    );
    assert_eq!(
        query(&authorization_url, "scope").as_deref(),
        Some("read,read_all,profile:read_all,activity:read_all")
    );
    assert_eq!(
        query(&authorization_url, "approval_prompt").as_deref(),
        Some("force")
    );
    assert!(!authorization_url.as_str().contains("strava-client-secret"));
    assert!(calls.is_empty());

    // Strava separates the granted scopes with literal commas, which the
    // query parser of the Node app turned into a list.
    let state = query(&authorization_url, "state").unwrap();
    let callback = app
        .get(&format!(
            "/mcps/oauth/callback?state={state}&code=strava-code&scope=read,activity:read_all"
        ))
        .session(start.session())
        .send()
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
    assert_eq!(callback.location(), Some("/mcps"));

    let exchange = &token_requests(&calls)[0];
    assert_eq!(exchange.method, "POST");
    assert_eq!(
        exchange.form_pairs(),
        [
            ("client_id".to_string(), "123456".to_string()),
            (
                "client_secret".to_string(),
                "strava-client-secret".to_string()
            ),
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), "strava-code".to_string()),
        ]
    );
    let api: Vec<(String, Option<String>)> = api_requests(&calls)
        .iter()
        .map(|call| (call.path(), call.header("authorization")))
        .collect();
    assert_eq!(
        api,
        [(
            "/api/v3/athlete".to_string(),
            Some("Bearer strava-access-token".to_string())
        )]
    );

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(
        decrypt(&app, &saved.oauth_access_token).as_deref(),
        Some("strava-access-token")
    );
    assert_eq!(
        decrypt(&app, &saved.oauth_refresh_token).as_deref(),
        Some("strava-refresh-token")
    );
    assert!(
        !saved
            .oauth_access_token
            .clone()
            .unwrap()
            .contains("strava-access-token")
    );
    assert_eq!(
        saved.oauth_scopes.as_deref(),
        Some("read activity:read_all")
    );
    assert!(saved.oauth_token_expires_at.is_some());
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);

    // Back on the registry, the dialog shows the account as connected.
    let page = app
        .get("/mcps")
        .session(callback.session())
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(page.contains("<p class=\"toast__message\">OAuth connected</p>"));
    assert!(dialog.contains("Connected. MyMCPs reads your profile"));
    assert!(dialog.contains(">Re-authorize</a>"));
    assert!(!dialog.contains("Authorization required"));
}

#[tokio::test]
async fn strava_explains_rejected_application_credentials_without_echoing_them() {
    let (app, calls) = app_with_strava(|call| {
        (call.path() == "/api/v3/oauth/token").then(|| {
            strava_json(
                json!({
                    "message": "Authorization Error",
                    "errors": [{ "resource": "Application", "field": "", "code": "invalid" }],
                }),
                401,
            )
        })
    })
    .await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, false).await;

    let (start, authorization_url) = start_strava_authorization(&app, &admin, &mcp).await;
    let state = query(&authorization_url, "state").unwrap();
    let callback = app
        .get(&format!(
            "/mcps/oauth/callback?state={state}&code=strava-sensitive-code"
        ))
        .session(start.session())
        .send()
        .await;

    let error = callback.flashed("error").unwrap();
    let error = error.as_str().unwrap();
    assert!(
        error.contains("Strava rejected the token request (HTTP 401)"),
        "{error}"
    );
    assert!(error.contains("Check the Client ID and Client Secret"));
    assert!(!error.contains("strava-client-secret"));
    assert!(!error.contains("strava-sensitive-code"));

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert_eq!(saved.oauth_access_token, None);
    assert!(api_requests(&calls).is_empty());
}

#[tokio::test]
async fn strava_leaves_the_mcp_disconnected_when_access_is_denied() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, false).await;

    let (start, authorization_url) = start_strava_authorization(&app, &admin, &mcp).await;
    let state = query(&authorization_url, "state").unwrap();
    let callback = app
        .get(&format!(
            "/mcps/oauth/callback?state={state}&error=access_denied"
        ))
        .session(start.session())
        .send()
        .await;

    assert_eq!(
        callback.flashed("error"),
        Some(json!("OAuth error: access_denied"))
    );
    assert_eq!(callback.flashed("editingMcpId"), Some(json!(mcp.id)));
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert!(calls.is_empty());
}

#[tokio::test]
async fn strava_reports_a_malformed_callback_instead_of_returning_without_a_message() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, false).await;
    let (start, _) = start_strava_authorization(&app, &admin, &mcp).await;

    // A comma makes the query parser hand the validator a list.
    let callback = app
        .get("/mcps/oauth/callback?state=one,two&code=strava-code")
        .session(start.session())
        .send()
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    assert_eq!(
        callback.flashed("error"),
        Some(json!("Invalid OAuth callback"))
    );
    assert_eq!(callback.location(), Some("/mcps"));
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert!(calls.is_empty());
}

#[tokio::test]
async fn strava_rejects_a_callback_whose_state_was_not_issued_to_this_session() {
    let (app, calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, false).await;
    let (start, _) = start_strava_authorization(&app, &admin, &mcp).await;

    let callback = app
        .get("/mcps/oauth/callback?state=forged-state&code=strava-code")
        .session(start.session())
        .send()
        .await;

    assert_eq!(
        callback.flashed("error"),
        Some(json!("Invalid OAuth callback"))
    );
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert!(calls.is_empty());
}

#[tokio::test]
async fn strava_stays_read_only_unless_write_access_is_allowed_in_the_form() {
    let (app, _calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;

    post(
        &app,
        &admin,
        &with(&STRAVA_FORM, &[("name", "Strava read")]),
    )
    .await;
    post(
        &app,
        &admin,
        &with(
            &STRAVA_FORM,
            &[("name", "Strava write"), ("builtinWriteEnabled", "on")],
        ),
    )
    .await;

    let read_only = mcp_by_slug(&app, "strava-read").await.unwrap();
    let writable = mcp_by_slug(&app, "strava-write").await.unwrap();
    assert!(!read_only.builtin_write_enabled);
    assert!(writable.builtin_write_enabled);

    // Write access only exists for a built-in MCP.
    post(
        &app,
        &admin,
        &[
            ("name", "Not built in"),
            ("transport", "http"),
            ("httpUrl", "https://www.strava.com/mcp"),
            ("authType", "auto"),
            ("builtinWriteEnabled", "on"),
            ("builtinKey", "strava"),
        ],
    )
    .await;
    let http = mcp_by_slug(&app, "not-built-in").await.unwrap();
    assert!(!http.builtin_write_enabled);
    assert_eq!(http.builtin_key, None);
}

#[tokio::test]
async fn strava_requests_the_write_scopes_at_connect_once_write_access_is_allowed() {
    let (app, _calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, false, true).await;

    let (start, authorization_url) = start_strava_authorization(&app, &admin, &mcp).await;
    assert_eq!(
        query(&authorization_url, "scope").as_deref(),
        Some("read,read_all,profile:read_all,activity:read_all,activity:write,profile:write")
    );

    let state = query(&authorization_url, "state").unwrap();
    let granted = "read,activity:write,activity:read_all,profile:write,profile:read_all,read_all";
    let callback = app
        .get(&format!(
            "/mcps/oauth/callback?state={state}&code=strava-code&scope={granted}"
        ))
        .session(start.session())
        .send()
        .await;
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(
        saved.oauth_scopes.as_deref(),
        Some("read activity:write activity:read_all profile:write profile:read_all read_all")
    );
    assert!(saved.builtin_write_enabled);
    let dialog = edit_dialog(&app, &admin, mcp.id).await;
    assert!(!dialog.contains("Write access not granted yet"));
    assert!(dialog.contains("name=\"builtinWriteEnabled\" checked>"));
}

#[tokio::test]
async fn strava_keeps_the_account_connected_when_write_access_is_allowed_later_and_asks_to_re_authorize()
 {
    let (app, _calls) = app_with_strava(|_| None).await;
    let admin = create_admin(&app).await;
    let mcp = create_strava_mcp(&app, admin.id, true, false).await;

    let before = edit_dialog(&app, &admin, mcp.id).await;
    assert!(before.contains("name=\"builtinWriteEnabled\"><span"));
    assert!(!before.contains("Write access not granted yet"));

    let saved = put(
        &app,
        &admin,
        mcp.id,
        &with(
            &STRAVA_FORM,
            &[("oauthClientSecret", ""), ("builtinWriteEnabled", "on")],
        ),
    )
    .await;
    assert_eq!(saved.flashed("success"), Some(json!("MCP updated")));
    // The account stays connected: nothing reopens the dialog.
    assert_eq!(saved.flashed("editingMcpId"), None);

    let updated = find_mcp(&app, mcp.id).await;
    assert!(updated.builtin_write_enabled);
    assert_eq!(updated.status, McpStatus::Ready);
    assert_eq!(
        decrypt(&app, &updated.oauth_access_token).as_deref(),
        Some("strava-access-token")
    );

    // The saved authorization is still read-only: the dialog says how to change that.
    let after = edit_dialog(&app, &admin, mcp.id).await;
    assert!(after.contains("name=\"builtinWriteEnabled\" checked>"));
    assert!(after.contains("<div class=\"banner banner--warning\" role=\"status\">"));
    assert!(after.contains("<p class=\"banner__title\">Write access not granted yet</p><p>Select Re-authorize and keep the write permissions checked on Strava. Until then, only the read tools are available.</p>"));
    assert!(after.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\">Re-authorize</a>",
        mcp.id
    )));

    // Turning it off again needs no authorization.
    put(
        &app,
        &admin,
        mcp.id,
        &with(&STRAVA_FORM, &[("oauthClientSecret", "")]),
    )
    .await;
    assert!(!find_mcp(&app, mcp.id).await.builtin_write_enabled);
    assert!(
        !edit_dialog(&app, &admin, mcp.id)
            .await
            .contains("Write access not granted yet")
    );
}

// --- tests/browser/builtin_strava_setup.spec.ts and builtin_strava_app_icon.spec.ts ---

#[tokio::test]
async fn strava_guides_the_admin_from_the_template_to_the_setup_form() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = app
        .get("/mcps/new?template=strava")
        .login_as(&admin)
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"new-mcp\"", "</dialog>");

    assert!(dialog.contains(">Set up Strava</h2><p class=\"dialog__subtitle\">Runs inside MyMCPs with your own API application</p>"));
    assert!(dialog.contains("<ol class=\"steps steps--detailed\" aria-label=\"Strava setup\">"));
    // Nothing is saved yet: the first step is the one to do.
    assert!(dialog.contains("<li class=\"step\" aria-current=\"step\"><div class=\"step__body\"><h3 class=\"step__title\">Create a Strava API application</h3>"));
    assert!(!dialog.contains("data-complete"));
    assert!(dialog.contains("<a class=\"link\" href=\"https://www.strava.com/settings/api\" target=\"_blank\" rel=\"noopener noreferrer\">Strava API settings"));
    assert!(dialog.contains("Authorization Callback Domain"));
    assert!(dialog.contains("<span class=\"copy-field__value\">http://localhost:3333</span>"));
    // A built-in MCP has no transport or authentication to choose.
    assert!(!dialog.contains("type=\"radio\""));
    assert!(dialog.contains("<input type=\"hidden\" name=\"transport\" value=\"builtin\">"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"builtinKey\" value=\"strava\">"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"template\" value=\"strava\">"));
    assert!(dialog.contains("name=\"name\" type=\"text\" value=\"Strava\""));
    assert!(dialog.contains(
        "value=\"Read Strava activities, training totals, zones, segments, and routes.\""
    ));
    assert!(dialog.contains("<h3 class=\"step__title\">Paste its Client ID and Client Secret</h3><p class=\"text-body-sm\">Both are shown on the My API Application page once the application exists.</p>"));
    assert!(dialog.contains(">Client Secret</label>"));
    assert!(dialog.contains("Encrypted at rest and only sent to the provider."));
    assert!(
        dialog.contains("Once this MCP is saved, select Connect and approve access on Strava.")
    );
    // Write access is off until the admin allows it.
    assert!(dialog.contains("name=\"builtinWriteEnabled\"><span"));
    assert!(dialog.contains("Lets agents create manual activities, edit activity details, star segments, and update your weight. Turning it on applies the next time you connect or re-authorize."));
    assert!(dialog.contains("name=\"enabled\" checked>"));

    // The icon Strava asks for is a file of the instance, ready to download.
    let icon = between(dialog, "<div class=\"cluster gap-300\">", "</div>");
    let href = between(icon, "href=\"", "\"")["href=\"".len()..].to_string();
    assert!(icon.contains("download=\"mymcps-app-icon.png\">"));
    assert!(icon.contains("Download app icon"));
    let file = app.get(&href).send().await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.header("content-type"), Some("image/png"));
    // PNG signature, then the IHDR chunk: 512 by 512, and no alpha channel,
    // so that a provider which converts it to JPG does not show it on black.
    assert_eq!(&file.body[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        u32::from_be_bytes(file.body[16..20].try_into().unwrap()),
        512
    );
    assert_eq!(
        u32::from_be_bytes(file.body[20..24].try_into().unwrap()),
        512
    );
    assert_eq!(file.body[25], 2, "colour type: RGB");
}

#[tokio::test]
async fn asks_for_the_public_address_before_an_application_can_be_registered() {
    let app = TestApp::with_config(|config| config.app_url = None).await;
    let admin = create_admin(&app).await;

    for (template, provider) in [("strava", "Strava"), ("google-ads", "Google Ads")] {
        let page = app
            .get(&format!("/mcps/new?template={template}"))
            .login_as(&admin)
            .send()
            .await
            .text();
        let dialog = between(&page, "id=\"new-mcp\"", "</dialog>");
        assert!(dialog.contains(&format!(
            "<p class=\"banner__title\">Set APP_URL first</p><p>{provider} sends you back to this instance after you approve access. Set APP_URL to its public HTTPS origin and redeploy to see the values to enter.</p>"
        )));
        assert!(!dialog.contains("copy-field"));
    }
}

// ------------------------------------------------------------ iCloud Mail

const SIGN_IN_REJECTED: &str = "iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.";

const PASSWORD_HINT: &str = "Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop. Your Apple Account password does not work here.";

const ICLOUD_FORM: [(&str, &str); 9] = [
    ("name", "iCloud Mail"),
    ("description", "Personal mail"),
    ("transport", "builtin"),
    ("builtinKey", "icloud-mail"),
    ("authType", "auto"),
    ("builtinUsername", USERNAME),
    ("builtinPassword", PASSWORD),
    ("builtinPermissions[]", "read"),
    ("enabled", "on"),
];

/// The app in front of a mail account served on 127.0.0.1.
async fn app_with_icloud(options: FakeOptions) -> (TestApp, FakeIcloud) {
    let icloud = FakeIcloud::start_with(options).await;
    let servers = icloud.servers();
    let app = TestApp::with_state(
        |_| {},
        |core| {
            let env = BuiltinEnv::new(core.clone())
                .with_fetcher(Fetcher::offline())
                .with_extension(servers);
            let upstream = Upstream::builder(core.clone(), builtins())
                .fetcher(Fetcher::offline())
                .builtin_env(env)
                .build();
            AppState::with_upstream(core, upstream)
        },
    )
    .await;
    (app, icloud)
}

async fn create_icloud_mail_mcp(
    app: &TestApp,
    created_by: i64,
    permissions: &str,
    aliases: Option<&str>,
) -> Mcp {
    let password = encrypt(app, PASSWORD);
    create_mcp(app, created_by, |mcp| {
        mcp.name = "iCloud Mail".into();
        mcp.slug = "icloud-mail".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("icloud-mail".into());
        mcp.builtin_username = Some(USERNAME.into());
        mcp.builtin_password = password;
        mcp.builtin_permissions = Some(permissions.into());
        mcp.builtin_aliases = aliases.map(str::to_string);
    })
    .await
}

#[tokio::test]
async fn icloud_creates_the_mcp_with_an_encrypted_password_and_checks_it_by_signing_in() {
    let (app, icloud) = app_with_icloud(FakeOptions::default()).await;
    let admin = create_admin(&app).await;

    let response = post(&app, &admin, &ICLOUD_FORM).await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "icloud-mail").await.unwrap();
    assert_eq!(mcp.transport, McpTransport::Builtin);
    assert_eq!(mcp.builtin_key.as_deref(), Some("icloud-mail"));
    assert_eq!(mcp.auth_type, McpAuthType::Auto);
    assert_eq!(mcp.builtin_username.as_deref(), Some("thomas@icloud.com"));
    assert!(!mcp.builtin_password.clone().unwrap().contains(PASSWORD));
    assert_eq!(
        decrypt(&app, &mcp.builtin_password).as_deref(),
        Some(PASSWORD)
    );
    assert_eq!(mcp.builtin_permissions.as_deref(), Some("read"));
    assert_eq!(mcp.builtin_aliases, None);
    assert!(!mcp.builtin_write_enabled);
    assert_eq!(mcp.oauth_client_id, None);
    assert_eq!(mcp.status, McpStatus::Ready);
    assert_eq!(mcp.last_error, None);
    assert!(!mcp.oauth_required);
    // It works: there is nothing left to do in the dialog.
    assert_eq!(response.flashed("editingMcpId"), None);
    assert_eq!(
        icloud.sign_ins(),
        [SignInAttempt {
            username: USERNAME.into(),
            password: PASSWORD.into(),
        }]
    );

    // The registry lists it under the way it signs in.
    let page = app.get("/mcps").login_as(&admin).send().await.text();
    let row = between(&page, "<tbody>", "</tbody>");
    assert!(
        row.contains("<td class=\"cell-secondary\">Built-in · iCloud Mail over IMAP and SMTP</td>")
    );
    assert!(row.contains("<code class=\"code\">password</code>"));
    // Nothing to authorize on Apple's side.
    assert!(!row.contains("Re-authorize") && !row.contains("Connect"));
}

#[tokio::test]
async fn icloud_asks_for_an_address_an_app_specific_password_and_at_least_one_permission() {
    let (app, icloud) = app_with_icloud(FakeOptions::default()).await;
    let admin = create_admin(&app).await;
    let without_permissions: Vec<(&str, &str)> = ICLOUD_FORM
        .iter()
        .filter(|(name, _)| *name != "builtinPermissions[]")
        .copied()
        .collect();

    let missing = refused(
        &app,
        &admin,
        "/mcps",
        &with(
            &without_permissions,
            &[("builtinUsername", ""), ("builtinPassword", "")],
        ),
    )
    .await;
    assert_eq!(
        field_errors(&missing),
        [
            "Enter your iCloud Mail address, such as name@icloud.com",
            PASSWORD_HINT
        ]
    );
    assert!(missing.contains("<p class=\"banner__title\">Allow at least one permission</p><p>Without a permission, agents would get no tool from this MCP.</p>"));
    // The error is named before the help line of the field.
    assert!(missing.contains(
        "aria-invalid=\"true\" aria-describedby=\"new-mcp-username-error new-mcp-username-help\">"
    ));
    assert!(missing.contains(">Set up iCloud Mail</h2>"));

    // The Apple Account password must never be stored.
    let account_password = refused(
        &app,
        &admin,
        "/mcps",
        &with(
            &ICLOUD_FORM,
            &[("builtinPassword", "Correct-Horse-Battery-9")],
        ),
    )
    .await;
    assert_eq!(field_errors(&account_password), [PASSWORD_HINT]);

    let local_part = refused(
        &app,
        &admin,
        "/mcps",
        &with(&ICLOUD_FORM, &[("builtinUsername", "thomas")]),
    )
    .await;
    assert_eq!(
        field_errors(&local_part),
        ["Enter your iCloud Mail address, such as name@icloud.com"]
    );
    assert!(local_part.contains("name=\"builtinUsername\" type=\"text\" value=\"thomas\""));

    let mut unknown_permission = ICLOUD_FORM.to_vec();
    unknown_permission.push(("builtinPermissions[]", "admin"));
    let unknown = refused(&app, &admin, "/mcps", &unknown_permission).await;
    assert!(field_errors(&unknown).is_empty());
    assert!(unknown.contains(
        "<p class=\"banner__title\">iCloud Mail has no &quot;admin&quot; permission</p>"
    ));

    let alias = refused(
        &app,
        &admin,
        "/mcps",
        &with(
            &ICLOUD_FORM,
            &[("builtinAliases", "hello@thomas.example, thomas")],
        ),
    )
    .await;
    assert_eq!(
        field_errors(&alias),
        [
            "Enter up to 20 other addresses of this iCloud account, such as alias@icloud.com, separated by commas"
        ]
    );
    assert!(
        alias.contains(
            "name=\"builtinAliases\" type=\"text\" value=\"hello@thomas.example, thomas\""
        )
    );

    assert_eq!(count_mcps(&app).await, 0);
    assert!(icloud.sign_ins().is_empty());
}

#[tokio::test]
async fn icloud_reopens_the_dialog_with_the_reason_when_the_sign_in_is_rejected() {
    let (app, _icloud) = app_with_icloud(FakeOptions {
        reject_sign_in: true,
        ..FakeOptions::default()
    })
    .await;
    let admin = create_admin(&app).await;

    let response = post(&app, &admin, &ICLOUD_FORM).await;

    let mcp = mcp_by_slug(&app, "icloud-mail").await.unwrap();
    assert_eq!(mcp.status, McpStatus::Error);
    assert_eq!(mcp.last_error.as_deref(), Some(SIGN_IN_REJECTED));
    // Connect cannot repair a password, so the dialog must not offer it.
    assert!(!mcp.oauth_required);
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));

    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(response.session())
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("<p class=\"banner__title\">Last connection error</p>"));
    assert!(dialog.contains("iCloud Mail rejected the sign-in."));
    assert!(!dialog.contains("Authorization required"));
    assert!(!dialog.contains(">Connect</a>"));
    assert!(!dialog.contains("Re-authorize"));
    assert!(!dialog.contains("Connected."));
    // A saved password that does not work has to be entered again.
    assert!(dialog.contains("<li class=\"step\" data-complete><div class=\"step__body\"><h3 class=\"step__title\"><span class=\"visually-hidden\">Done: </span>Create an app-specific password</h3>"));
    assert!(dialog.contains("<li class=\"step\" aria-current=\"step\"><div class=\"step__body\"><h3 class=\"step__title\">Enter your address and the password</h3>"));
}

#[tokio::test]
async fn icloud_shares_the_address_and_permissions_and_never_the_password_with_the_page() {
    let (app, _icloud) = app_with_icloud(FakeOptions::default()).await;
    let admin = create_admin(&app).await;
    let mcp =
        create_icloud_mail_mcp(&app, admin.id, "read send", Some("hello@thomas.example")).await;

    let page = app
        .get(&format!("/mcps/{}/edit", mcp.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    assert!(dialog.contains("<input type=\"hidden\" name=\"builtinKey\" value=\"icloud-mail\">"));
    assert!(dialog.contains("name=\"builtinUsername\" type=\"text\" value=\"thomas@icloud.com\""));
    assert!(dialog.contains("for=\"edit-mcp-password\">App-specific password (leave blank to keep) <span class=\"field__optional\">Optional</span></label><input class=\"input\" id=\"edit-mcp-password\" name=\"builtinPassword\" type=\"password\" autocomplete=\"off\" aria-describedby=\"edit-mcp-password-help\">"));
    assert!(
        dialog.contains("name=\"builtinAliases\" type=\"text\" value=\"hello@thomas.example\"")
    );
    assert!(dialog.contains("name=\"builtinPermissions[]\" value=\"read\" checked>"));
    assert!(dialog.contains("name=\"builtinPermissions[]\" value=\"draft\">"));
    assert!(dialog.contains("name=\"builtinPermissions[]\" value=\"send\" checked>"));
    assert!(dialog.contains("name=\"builtinPermissions[]\" value=\"organize\">"));
    // The sign-in works: the three steps are done.
    assert_eq!(
        dialog.matches("<li class=\"step\" data-complete>").count(),
        3
    );
    assert!(dialog.contains("Connected. Apple cannot limit what an app-specific password reaches"));
    // Apple has nothing to authorize, and write access is not a thing here.
    assert!(!dialog.contains("Authorization required"));
    assert!(!dialog.contains("Re-authorize"));
    assert!(!dialog.contains("Allow write access"));
    assert!(!dialog.contains("class=\"banner"));

    assert!(!page.contains(PASSWORD));
    assert!(!page.contains(mcp.builtin_password.as_deref().unwrap()));
}

#[tokio::test]
async fn icloud_keeps_the_saved_password_when_none_is_entered_and_applies_new_permissions() {
    let (app, icloud) = app_with_icloud(FakeOptions::default()).await;
    let admin = create_admin(&app).await;
    let mcp = create_icloud_mail_mcp(&app, admin.id, "read", None).await;
    let base: Vec<(&str, &str)> = ICLOUD_FORM
        .iter()
        .filter(|(name, _)| !["builtinPermissions[]", "builtinPassword"].contains(name))
        .copied()
        .collect();
    let save = async |fields: &[(&str, &str)]| {
        let mut form = base.clone();
        form.extend_from_slice(fields);
        put(&app, &admin, mcp.id, &form).await
    };

    let widened = save(&[
        ("builtinPassword", ""),
        ("builtinPermissions[]", "organize"),
        ("builtinPermissions[]", "read"),
    ])
    .await;
    assert_eq!(widened.flashed("success"), Some(json!("MCP updated")));
    let kept = find_mcp(&app, mcp.id).await;
    assert_eq!(
        decrypt(&app, &kept.builtin_password).as_deref(),
        Some(PASSWORD)
    );
    // Saved in the order of the provider, whatever order they were checked in.
    assert_eq!(kept.builtin_permissions.as_deref(), Some("read organize"));
    assert_eq!(kept.status, McpStatus::Ready);

    // A form with one permission checked sends it as a single value.
    save(&[
        ("builtinPassword", "zyxwvutsrqponmlk"),
        ("builtinPermissions", "draft"),
    ])
    .await;
    let replaced = find_mcp(&app, mcp.id).await;
    assert_eq!(
        decrypt(&app, &replaced.builtin_password).as_deref(),
        Some("zyxwvutsrqponmlk")
    );
    assert_eq!(replaced.builtin_permissions.as_deref(), Some("draft"));
    assert_eq!(
        icloud.sign_ins().last().unwrap().password,
        "zyxwvutsrqponmlk"
    );

    // The account's own address is not an alias, and each alias is kept once.
    save(&[
        ("builtinPassword", ""),
        ("builtinPermissions[]", "send"),
        (
            "builtinAliases",
            "hello@thomas.example; THOMAS@icloud.com\ntt@icloud.com, Hello@Thomas.example",
        ),
    ])
    .await;
    let aliased = find_mcp(&app, mcp.id).await;
    assert_eq!(
        aliased.builtin_aliases.as_deref(),
        Some("hello@thomas.example tt@icloud.com")
    );
    // The dialog writes them back separated by commas.
    assert!(edit_dialog(&app, &admin, mcp.id).await.contains(
        "name=\"builtinAliases\" type=\"text\" value=\"hello@thomas.example, tt@icloud.com\""
    ));
    save(&[
        ("builtinPassword", ""),
        ("builtinPermissions[]", "draft"),
        ("builtinAliases", ""),
    ])
    .await;

    let emptied = refused(
        &app,
        &admin,
        &format!("/mcps/{}", mcp.id),
        &with(&base, &[("builtinPassword", "")]),
    )
    .await;
    assert!(emptied.contains("<p class=\"banner__title\">Allow at least one permission</p>"));
    assert!(emptied.contains(">Edit iCloud Mail</h2>"));
    let unchanged = find_mcp(&app, mcp.id).await;
    assert_eq!(unchanged.builtin_permissions.as_deref(), Some("draft"));
    assert_eq!(unchanged.builtin_aliases, None);
}

#[tokio::test]
async fn icloud_has_no_oauth_flow_and_forgets_the_sign_in_when_the_mcp_changes_kind() {
    let (app, _icloud) = app_with_icloud(FakeOptions::default()).await;
    let admin = create_admin(&app).await;
    let mcp = create_icloud_mail_mcp(&app, admin.id, "read", Some("alias@icloud.com")).await;

    let start = app
        .get(&format!("/mcps/{}/oauth/start", mcp.id))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(start.status, StatusCode::FOUND);
    assert_eq!(
        start.flashed("error"),
        Some(json!("This MCP does not require OAuth authorization"))
    );

    put(
        &app,
        &admin,
        mcp.id,
        &[
            ("name", "iCloud Mail"),
            ("transport", "http"),
            ("httpUrl", "http://127.0.0.1:9/mcp"),
            ("authType", "auto"),
            ("enabled", "on"),
        ],
    )
    .await;
    let changed = find_mcp(&app, mcp.id).await;
    assert_eq!(changed.transport, McpTransport::Http);
    assert_eq!(changed.builtin_key, None);
    assert_eq!(changed.builtin_username, None);
    assert_eq!(changed.builtin_password, None);
    assert_eq!(changed.builtin_permissions, None);
    assert_eq!(changed.builtin_aliases, None);
}

// --- tests/browser/builtin_icloud_mail_setup.spec.ts ---

#[tokio::test]
async fn icloud_guides_the_admin_from_the_template_to_the_setup_form() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = app
        .get("/mcps/new?template=icloud-mail")
        .login_as(&admin)
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"new-mcp\"", "</dialog>");

    assert!(dialog.contains(">Set up iCloud Mail</h2><p class=\"dialog__subtitle\">Runs inside MyMCPs with an app-specific password</p>"));
    assert!(
        dialog.contains("<ol class=\"steps steps--detailed\" aria-label=\"iCloud Mail setup\">")
    );
    assert!(dialog.contains("<h3 class=\"step__title\">Create an app-specific password</h3>"));
    assert!(dialog.contains("<a class=\"link\" href=\"https://account.apple.com/account/manage\" target=\"_blank\" rel=\"noopener noreferrer\">your Apple Account"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"transport\" value=\"builtin\">"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"builtinKey\" value=\"icloud-mail\">"));
    assert!(dialog.contains("for=\"new-mcp-username\">iCloud Mail address</label>"));
    assert!(dialog.contains("placeholder=\"name@icloud.com\""));
    assert!(dialog.contains("for=\"new-mcp-password\">App-specific password</label><input class=\"input\" id=\"new-mcp-password\" name=\"builtinPassword\" type=\"password\" placeholder=\"abcd-efgh-ijkl-mnop\""));
    assert!(dialog.contains("Encrypted at rest and only sent to the provider's mail servers."));
    assert!(dialog.contains(
        ">Other sender addresses <span class=\"field__optional\">Optional</span></label>"
    ));

    // Apple cannot scope the password, so the permissions are chosen here.
    // Only reading is allowed until the admin decides otherwise.
    assert!(dialog.contains("Saving this MCP signs in to iCloud Mail to check the password. Apple cannot limit what an app-specific password reaches"));
    assert!(dialog.contains("name=\"builtinPermissions[]\" value=\"read\" checked><span class=\"choice__text\"><span class=\"choice__label\">Read mail</span>"));
    for (permission, label) in [
        ("draft", "Save drafts"),
        ("send", "Send mail"),
        ("organize", "Organize mail"),
    ] {
        assert!(
            dialog.contains(&format!(
                "name=\"builtinPermissions[]\" value=\"{permission}\"><span class=\"choice__text\"><span class=\"choice__label\">{label}</span>"
            )),
            "{label}"
        );
    }
    assert!(!dialog.contains("Allow write access"));
    assert!(!dialog.contains("oauthClientId"));
}

// -------------------------------------------------------------- Google Ads

const GOOGLE_ADS_FORM: [(&str, &str); 9] = [
    ("name", "Google Ads"),
    ("transport", "builtin"),
    ("builtinKey", "google-ads"),
    ("authType", "auto"),
    ("oauthClientId", "1234567890-abc.apps.googleusercontent.com"),
    ("oauthClientSecret", "google-client-secret"),
    ("builtinSettings[loginCustomerId]", "987-654-3210"),
    ("builtinSettings[customerIds]", "123-456-7890, 2345678901"),
    ("enabled", "on"),
];

/// The app in front of a fake Google: its token endpoint and the Google Ads API.
async fn app_with_google_ads() -> (TestApp, FakeGoogleAds) {
    let google = FakeGoogleAds::new();
    let fetcher = google.fetcher();
    let app = TestApp::with_upstream(|upstream| upstream.fetcher(fetcher)).await;
    (app, google)
}

fn settings(app: &TestApp, mcp: &Mcp) -> Vec<(String, String)> {
    decrypt_environment(&app.core.encryption, mcp.builtin_settings.as_deref()).unwrap()
}

#[tokio::test]
async fn google_ads_creates_the_mcp_with_its_accounts_and_waits_for_authorization() {
    let (app, google) = app_with_google_ads().await;
    let admin = create_admin(&app).await;

    let response = post(&app, &admin, &GOOGLE_ADS_FORM).await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "google-ads").await.unwrap();
    assert_eq!(mcp.builtin_key.as_deref(), Some("google-ads"));
    assert_eq!(
        decrypt(&app, &mcp.oauth_client_secret).as_deref(),
        Some("google-client-secret")
    );
    // Stored without their dashes, and encrypted like the rest of the dialog.
    assert_eq!(
        settings(&app, &mcp),
        [
            ("loginCustomerId".to_string(), "9876543210".to_string()),
            (
                "customerIds".to_string(),
                "1234567890 2345678901".to_string()
            ),
        ]
    );
    assert!(!mcp.builtin_settings.clone().unwrap().contains("9876543210"));
    assert_eq!(mcp.status, McpStatus::Draft);
    assert!(mcp.oauth_required);
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert!(google.requests().is_empty());

    // The dialog shows the accounts again: they are not credentials.
    let dialog = edit_dialog(&app, &admin, mcp.id).await;
    assert!(dialog.contains("name=\"builtinSettings[loginCustomerId]\" type=\"text\" value=\"9876543210\" placeholder=\"123-456-7890\""));
    assert!(dialog.contains(
        "name=\"builtinSettings[customerIds]\" type=\"text\" value=\"1234567890 2345678901\""
    ));
    assert!(
        dialog.contains("Connect opens Google Ads so you can approve access with your account.")
    );
    assert!(!dialog.contains("google-client-secret"));
    // One scope reads and writes, so there is nothing to grant again for write access.
    assert!(!dialog.contains("Write access not granted yet"));
}

#[tokio::test]
async fn google_ads_reports_every_wrong_field_of_the_dialog_at_once() {
    let (app, _google) = app_with_google_ads().await;
    let admin = create_admin(&app).await;

    let dialog = refused(
        &app,
        &admin,
        "/mcps",
        &with(
            &GOOGLE_ADS_FORM,
            &[
                ("oauthClientId", "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-field"),
                ("builtinSettings[loginCustomerId]", "12345"),
                ("builtinSettings[customerIds]", "123-456-7890, acme"),
            ],
        ),
    )
    .await;

    assert_eq!(
        field_errors(&dialog),
        [
            "The Google Client ID ends in .apps.googleusercontent.com",
            "Enter the ID of the manager account, such as 123-456-7890",
            "Enter up to 50 Google Ads account IDs, such as 123-456-7890, separated by commas",
        ]
    );
    // What was typed is in the fields again, as it was typed.
    assert!(
        dialog.contains("name=\"builtinSettings[loginCustomerId]\" type=\"text\" value=\"12345\"")
    );
    assert!(dialog.contains(
        "name=\"builtinSettings[customerIds]\" type=\"text\" value=\"123-456-7890, acme\""
    ));
    assert!(dialog.contains("id=\"new-mcp-login-customer-id-error\""));
    assert_eq!(count_mcps(&app).await, 0);
}

#[tokio::test]
async fn google_ads_needs_neither_a_manager_nor_a_list_of_accounts() {
    let (app, _google) = app_with_google_ads().await;
    let admin = create_admin(&app).await;
    let form: Vec<(&str, &str)> = GOOGLE_ADS_FORM
        .iter()
        .filter(|(name, _)| *name != "builtinSettings[loginCustomerId]")
        .copied()
        .collect();

    post(
        &app,
        &admin,
        &with(&form, &[("builtinSettings[customerIds]", "")]),
    )
    .await;

    let mcp = mcp_by_slug(&app, "google-ads").await.unwrap();
    assert_eq!(mcp.builtin_settings, None);
}

#[tokio::test]
async fn google_ads_asks_google_for_offline_access_and_sends_the_redirect_uri_again_with_the_code()
{
    let (app, google) = app_with_google_ads().await;
    let admin = create_admin(&app).await;
    let secret = encrypt(&app, "google-client-secret");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("google-ads".into());
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
        mcp.oauth_client_id = Some("1234567890-abc.apps.googleusercontent.com".into());
        mcp.oauth_client_secret = secret;
    })
    .await;

    let start = app
        .get(&format!("/mcps/{}/oauth/start", mcp.id))
        .login_as(&admin)
        .send()
        .await;
    let authorization_url = Url::parse(start.location().unwrap()).unwrap();
    assert_eq!(
        format!(
            "{}{}",
            authorization_url.origin().ascii_serialization(),
            authorization_url.path()
        ),
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(
        query(&authorization_url, "scope").as_deref(),
        Some(GOOGLE_ADS_SCOPE)
    );
    assert_eq!(
        query(&authorization_url, "access_type").as_deref(),
        Some("offline")
    );
    assert_eq!(
        query(&authorization_url, "prompt").as_deref(),
        Some("consent")
    );

    let state = query(&authorization_url, "state").unwrap();
    let callback = app
        .get(&format!(
            "/mcps/oauth/callback?state={state}&code=google-code&scope={GOOGLE_ADS_SCOPE}"
        ))
        .session(start.session())
        .send()
        .await;
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));

    let exchange = &google.token_requests()[0];
    assert_eq!(
        exchange.form.clone().unwrap(),
        [
            (
                "client_id".to_string(),
                "1234567890-abc.apps.googleusercontent.com".to_string()
            ),
            (
                "client_secret".to_string(),
                "google-client-secret".to_string()
            ),
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), "google-code".to_string()),
            ("redirect_uri".to_string(), CALLBACK.to_string()),
        ]
    );

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(
        decrypt(&app, &saved.oauth_refresh_token).as_deref(),
        Some("google-refresh-token")
    );
    assert_eq!(saved.oauth_scopes.as_deref(), Some(GOOGLE_ADS_SCOPE));
    assert_eq!(saved.status, McpStatus::Ready);
    // The connection is checked with one request, which carries no developer token.
    let checks: Vec<_> = google
        .requests()
        .into_iter()
        .filter(|request| request.url.host_str() == Some("googleads.googleapis.com"))
        .collect();
    assert!(
        checks[0]
            .url
            .path()
            .ends_with("/customers:listAccessibleCustomers")
    );
    assert_eq!(
        checks[0].header("authorization").as_deref(),
        Some("Bearer google-access-token")
    );
    assert_eq!(checks[0].header("developer-token"), None);
}

#[tokio::test]
async fn google_ads_guides_the_admin_through_its_own_setup() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = app
        .get("/mcps/new?template=google-ads")
        .login_as(&admin)
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"new-mcp\"", "</dialog>");

    assert!(dialog.contains(">Set up Google Ads</h2><p class=\"dialog__subtitle\">Runs inside MyMCPs with an OAuth client from your own Google Cloud project</p>"));
    assert!(dialog.contains("<h3 class=\"step__title\">Create a Google Cloud OAuth client</h3>"));
    assert!(dialog.contains(
        "href=\"https://console.cloud.google.com/apis/library/googleads.googleapis.com\""
    ));
    // The redirect URI Google asks for, and no application icon.
    assert!(dialog.contains("<span class=\"field__label\">Authorized redirect URI</span><div class=\"copy-field copy-field--wrap\"><span class=\"copy-field__value\">http://localhost:3333/mcps/oauth/callback</span>"));
    assert!(dialog.contains("aria-label=\"Copy authorized redirect URI\""));
    assert!(!dialog.contains("Download app icon"));
    assert!(dialog.contains("<h3 class=\"step__title\">Paste its Client ID and Client Secret, and choose the accounts</h3>"));
    assert!(dialog.contains("placeholder=\"1234567890-abc123.apps.googleusercontent.com\""));
    assert!(dialog.contains("for=\"new-mcp-login-customer-id\">Manager account ID <span class=\"field__optional\">Optional</span></label>"));
    assert!(dialog.contains(
        "name=\"builtinSettings[loginCustomerId]\" type=\"text\" placeholder=\"123-456-7890\""
    ));
    assert!(dialog.contains("for=\"new-mcp-customer-ids\">Accounts agents may use <span class=\"field__optional\">Optional</span></label>"));
    assert!(dialog.contains("Limits every tool to these Google Ads accounts. Left blank, agents reach every account your sign-in does."));
    assert!(dialog.contains("<h3 class=\"step__title\">Connect your Google Ads account</h3>"));
    // Its one scope reads and writes: write access needs no new authorization.
    assert!(
        dialog
            .contains("which you can change under Tool approvals. It applies as soon as you save.")
    );
}

#[tokio::test]
async fn google_ads_applies_write_access_without_a_new_authorization() {
    let (app, _google) = app_with_google_ads().await;
    let admin = create_admin(&app).await;
    let secret = encrypt(&app, "google-client-secret");
    let access = encrypt(&app, "google-access-token");
    let accounts = merge_environment(
        &app.core.encryption,
        None,
        &[EnvironmentInput {
            name: "customerIds".into(),
            value: Some("1234567890".into()),
        }],
    );
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("google-ads".into());
        mcp.oauth_client_id = Some("1234567890-abc.apps.googleusercontent.com".into());
        mcp.oauth_client_secret = secret;
        mcp.oauth_access_token = access;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::minutes(50));
        mcp.oauth_scopes = Some(GOOGLE_ADS_SCOPE.into());
        mcp.builtin_settings = accounts;
    })
    .await;

    let saved = put(
        &app,
        &admin,
        mcp.id,
        &with(
            &GOOGLE_ADS_FORM,
            &[
                ("oauthClientSecret", ""),
                ("builtinSettings[loginCustomerId]", ""),
                ("builtinSettings[customerIds]", "1234567890"),
                ("builtinWriteEnabled", "on"),
            ],
        ),
    )
    .await;
    assert_eq!(saved.flashed("success"), Some(json!("MCP updated")));

    let updated = find_mcp(&app, mcp.id).await;
    assert!(updated.builtin_write_enabled);
    assert_eq!(updated.status, McpStatus::Ready);
    assert_eq!(
        settings(&app, &updated),
        [("customerIds".to_string(), "1234567890".to_string())]
    );
    let dialog = edit_dialog(&app, &admin, mcp.id).await;
    assert!(dialog.contains("name=\"builtinWriteEnabled\" checked>"));
    assert!(!dialog.contains("Write access not granted yet"));
    assert!(dialog.contains("Connected. MyMCPs reads the accounts"));
}
