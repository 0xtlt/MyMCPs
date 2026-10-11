//! Port of `tests/functional/access_tokens.spec.ts`,
//! `hardening_auth_history.spec.ts` and the token cases of
//! `vine_route_params.spec.ts`, and of what the server does in the browser
//! specs `access_token_cleanup`, `access_token_edit_modal`,
//! `mcp_install_modal` and `date_formatting`.

use chrono::{DateTime, Duration, DurationRound, SubsecRound, Utc};
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{AccessToken, OauthClient, ScopeMode, User};
use mymcps_gateway::access_token::{self, NewAccessToken, NewOauthGrant};
use mymcps_web::install_config::{McpClient, McpInstallAuthMode, create_mcp_install_config};
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;

const CREATED: &str = "Access token created — copy it now, it will not be shown again";

async fn create_token(
    app: &TestApp,
    user: &User,
    name: &str,
    mcp_ids: &[i64],
    expires_at: Option<DateTime<Utc>>,
) -> AccessToken {
    access_token::create(
        &app.core.db,
        NewAccessToken {
            name,
            scope_mode: if mcp_ids.is_empty() {
                ScopeMode::All
            } else {
                ScopeMode::Selected
            },
            mcp_ids,
            expires_at: expires_at.map(Timestamp::from),
            created_by: user.id,
        },
    )
    .await
    .unwrap()
    .token
}

async fn create_revoked_token(app: &TestApp, user: &User, name: &str) -> AccessToken {
    let mut token = create_token(app, user, name, &[], None).await;
    token.revoked_at = Some(Timestamp::now());
    token.save(&*app.core.db).await.unwrap();
    token
}

async fn create_oauth_connection(app: &TestApp, user: &User, name: &str) -> AccessToken {
    let mut client = OauthClient {
        client_id: format!("mcp_client_{name}"),
        client_name: name.to_string(),
        redirect_uris: "[\"http://127.0.0.1/callback\"]".into(),
        token_endpoint_auth_method: "none".into(),
        grant_types: "[\"authorization_code\",\"refresh_token\"]".into(),
        response_types: "[\"code\"]".into(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&*app.core.db).await.unwrap();
    access_token::create_oauth_grant(
        &*app.core.db,
        NewOauthGrant {
            name,
            client_id: client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: "http://localhost:3333/mcp",
            created_by: user.id,
        },
    )
    .await
    .unwrap()
    .token
}

async fn find_by_name(app: &TestApp, name: &str) -> Option<AccessToken> {
    sqlx::query_as("select * from `access_tokens` where `name` = ?")
        .bind(name)
        .fetch_optional(&*app.core.db)
        .await
        .unwrap()
}

async fn find(app: &TestApp, id: i64) -> Option<AccessToken> {
    AccessToken::find(&*app.core.db, id).await.unwrap()
}

async fn mcp_ids(app: &TestApp, token: &AccessToken) -> Vec<i64> {
    sqlx::query_scalar(
        "select `mcp_id` from `access_token_mcps` where `access_token_id` = ? order by `mcp_id` asc",
    )
    .bind(token.id)
    .fetch_all(&*app.core.db)
    .await
    .unwrap()
}

fn flashed_text(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

/// `prefix` followed by 32 random bytes in base64url.
fn is_secret(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|random| {
        random.len() == 43
            && random
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

/// The page the browser lands on after a response, with what it flashed.
async fn follow(app: &TestApp, response: &TestResponse) -> TestResponse {
    let location = response
        .location()
        .or_else(|| response.header("x-location"))
        .expect("a response that sends the browser on");
    app.get(location).session(response.session()).send().await
}

fn unescape(html: &str) -> String {
    html.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The text of the element that carries this id, without its markup.
fn text_of(page: &str, id: &str) -> String {
    let marker = format!("id=\"{id}\"");
    let start = page.find(&marker).unwrap_or_else(|| panic!("no #{id}"));
    let open = start + page[start..].find('>').unwrap() + 1;
    let tag_start = page[..start].rfind('<').unwrap();
    let tag = page[tag_start + 1..start]
        .split_whitespace()
        .next()
        .unwrap();
    let close = open + page[open..].find(&format!("</{tag}>")).unwrap();
    let mut text = String::new();
    let mut inside_tag = false;
    for character in page[open..close].chars() {
        match character {
            '<' => inside_tag = true,
            '>' if inside_tag => inside_tag = false,
            _ if !inside_tag => text.push(character),
            _ => {}
        }
    }
    unescape(&text)
}

/// The opening tag of the element that carries this id.
fn tag_of<'a>(page: &'a str, id: &str) -> &'a str {
    let marker = format!("id=\"{id}\"");
    let at = page.find(&marker).unwrap_or_else(|| panic!("no #{id}"));
    let start = page[..at].rfind('<').unwrap();
    let end = at + page[at..].find('>').unwrap();
    &page[start..=end]
}

/// The opening tag of the code block that holds this snippet of the install dialog.
fn snippet_block<'a>(page: &'a str, id: &str) -> &'a str {
    let at = page.find(&format!("id=\"{id}\"")).unwrap();
    let start = page[..at].rfind("<div class=\"code-block\"").unwrap();
    let end = start + page[start..].find('>').unwrap();
    &page[start..=end]
}

fn is_hidden(tag: &str) -> bool {
    tag.ends_with(" hidden>")
}

/// The part of the page a table row or a list row of this token takes.
fn row_of<'a>(page: &'a str, opening: &str, closing: &str, name: &str) -> &'a str {
    page.split(opening)
        .skip(1)
        .map(|rest| rest.split(closing).next().unwrap_or(rest))
        .find(|row| row.contains(name))
        .unwrap_or_else(|| panic!("no row for {name}"))
}

fn table_row<'a>(page: &'a str, name: &str) -> &'a str {
    let body = page.split("<tbody>").nth(1).expect("a table");
    row_of(body, "<tr>", "</tr>", name)
}

fn mobile_row<'a>(page: &'a str, name: &str) -> &'a str {
    let list = page
        .split("<section class=\"card hide-desktop\"")
        .nth(1)
        .expect("a mobile list");
    row_of(list, "<div class=\"list-row\">", "</div></div>", name)
}

// ------------------------------------------------------------ access tokens

#[tokio::test]
async fn creates_a_token_and_flashes_plaintext_only_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .form(&[("name", "Production agent"), ("scopeMode", "all")])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some("/tokens"));
    assert_eq!(flashed_text(&response, "success").as_deref(), Some(CREATED));

    let plaintext = flashed_text(&response, "createdPlaintext").unwrap();
    assert!(is_secret(&plaintext, "mcp_"), "{plaintext}");

    let token = find_by_name(&app, "Production agent").await.unwrap();
    assert_ne!(token.token_hash, plaintext);
    assert_eq!(token.token_hash, access_token::hash(&plaintext));
    assert!(!token.is_revoked());
    assert_eq!(token.created_by, admin.id);

    // The page that follows shows the token, in a banner and in the install
    // dialog, and the browser is told to keep no copy of it.
    let shown = follow(&app, &response).await;
    assert_eq!(shown.status, StatusCode::OK);
    assert_eq!(shown.header("cache-control"), Some("no-store"));
    let page = shown.text();
    assert_eq!(text_of(&page, "created-token"), plaintext);
    assert!(page.contains("<div class=\"banner banner--success\" role=\"status\">"));
    // The sentence is the banner's: it is not said again in a toast.
    assert_eq!(page.matches(CREATED).count(), 1);
    assert_eq!(page.matches(&plaintext).count(), 2 + 6);

    // The page after that one has nothing of it left.
    let later = app.get("/tokens").session(shown.session()).send().await;
    assert_eq!(later.status, StatusCode::OK);
    assert!(!later.text().contains(&plaintext));
    assert!(!later.text().contains(CREATED));
    assert!(!later.text().contains("id=\"created-token\""));
}

#[tokio::test]
async fn answers_the_page_script_with_where_to_go_and_never_with_the_token() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[("name", "Scripted agent"), ("scopeMode", "all")])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.header("x-location"), Some("/tokens"));
    let plaintext = flashed_text(&response, "createdPlaintext").unwrap();
    assert!(!response.text().contains(&plaintext));
    for (_, value) in &response.headers {
        if let Ok(value) = value.to_str() {
            assert!(!value.contains(&plaintext));
        }
    }
    assert_eq!(
        text_of(&follow(&app, &response).await.text(), "created-token"),
        plaintext
    );
}

#[tokio::test]
async fn rejects_selected_mcp_ids_that_do_not_exist() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;

    let response = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Selected agent"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &mcp.id.to_string()),
            ("mcpIds[]", "999999"),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/tokens"));
    assert_eq!(
        flashed_text(&response, "error").as_deref(),
        Some("One or more selected MCPs do not exist")
    );
    assert!(find_by_name(&app, "Selected agent").await.is_none());
}

#[tokio::test]
async fn creates_a_token_limited_to_the_selected_mcps() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let first = create_mcp(&app, admin.id, |_| {}).await;
    let second = create_mcp(&app, admin.id, |_| {}).await;
    create_mcp(&app, admin.id, |_| {}).await;

    let response = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Selected agent"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &first.id.to_string()),
            ("mcpIds[]", &second.id.to_string()),
            ("expiresAt", "2035-09-06T11:13"),
        ])
        .send()
        .await;

    assert_eq!(flashed_text(&response, "success").as_deref(), Some(CREATED));
    let token = find_by_name(&app, "Selected agent").await.unwrap();
    assert_eq!(token.scope_mode, ScopeMode::Selected);
    assert_eq!(mcp_ids(&app, &token).await, vec![first.id, second.id]);
    // Without the page script the field has no zone: it is read as UTC.
    assert_eq!(
        token
            .expires_at
            .map(|expires_at| expires_at.to_iso())
            .as_deref(),
        Some("2035-09-06T11:13:00.000Z")
    );

    // The same MCP twice is one MCP too many.
    let repeated = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Repeating agent"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &first.id.to_string()),
            ("mcpIds[]", &first.id.to_string()),
        ])
        .send()
        .await;
    assert_eq!(
        flashed_text(&repeated, "error").as_deref(),
        Some("One or more selected MCPs do not exist")
    );
}

#[tokio::test]
async fn sends_a_refused_token_form_back_with_its_errors() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| mcp.enabled = false).await;

    // To the page's script: the form again, to put in place of the one it sent.
    let response = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[("name", " "), ("scopeMode", "selected")])
        .send()
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = response.text();
    assert!(form.starts_with("<form method=\"post\" action=\"/tokens\" data-async>"));
    assert!(form.contains("name=\"_csrf\""));
    assert!(tag_of(&form, "create-token-name").contains("aria-invalid=\"true\""));
    assert_eq!(
        text_of(&form, "create-token-name-error"),
        "The name field must be defined"
    );
    assert_eq!(
        text_of(&form, "create-token-mcps-error"),
        "The mcpIds field must be defined"
    );
    // What was chosen is kept, and the list it needs is shown.
    assert!(form.contains("name=\"scopeMode\" value=\"selected\" checked>"));
    assert!(form.contains(
        "<fieldset class=\"field-group\" data-show-when=\"scopeMode=selected\" aria-describedby=\"create-token-mcps-error\">"
    ));
    assert!(form.contains(&format!("{} (disabled)", mcp.slug)));
    assert!(find_by_name(&app, " ").await.is_none());

    // To a browser without the script: the first error, on the page it came from.
    let plain = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .header("host", "localhost:3333")
        .header("referer", "http://localhost:3333/tokens?status=active")
        .form(&[("name", "x"), ("scopeMode", "somewhere")])
        .send()
        .await;
    assert_eq!(plain.status, StatusCode::FOUND);
    assert_eq!(plain.location(), Some("/tokens?status=active"));
    assert_eq!(
        flashed_text(&plain, "error").as_deref(),
        Some("The selected scopeMode is invalid")
    );

    let unreadable_date = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[
            ("name", "Dated agent"),
            ("scopeMode", "all"),
            ("expiresAt", "tomorrow"),
        ])
        .send()
        .await;
    assert_eq!(unreadable_date.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        text_of(&unreadable_date.text(), "create-token-expires-error"),
        "The expiresAt field must be a datetime value"
    );
}

#[tokio::test]
async fn updates_token_settings_without_rotating_its_secret() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let first_mcp = create_mcp(&app, admin.id, |mcp| mcp.name = "First MCP".into()).await;
    let second_mcp = create_mcp(&app, admin.id, |mcp| mcp.name = "Second MCP".into()).await;
    let created = create_token(&app, &admin, "Original token", &[], None).await;
    let expires_at = (Utc::now() + Duration::days(7))
        .duration_trunc(Duration::minutes(1))
        .unwrap();

    let response = app
        .put(&format!("/tokens/{}", created.id))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Updated token"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &first_mcp.id.to_string()),
            ("mcpIds[]", &second_mcp.id.to_string()),
            (
                "expiresAt",
                &expires_at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            ),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/tokens"));
    assert_eq!(
        flashed_text(&response, "success").as_deref(),
        Some("Token updated")
    );

    let updated = find(&app, created.id).await.unwrap();
    assert_eq!(updated.name, "Updated token");
    assert_eq!(updated.scope_mode, ScopeMode::Selected);
    assert_eq!(
        updated
            .expires_at
            .map(|expires_at| expires_at.as_datetime()),
        Some(expires_at)
    );
    assert_eq!(
        mcp_ids(&app, &updated).await,
        vec![first_mcp.id, second_mcp.id]
    );
    assert_eq!(updated.token_hash, created.token_hash);
    assert_eq!(updated.token_prefix, created.token_prefix);
    assert_eq!(updated.created_by, admin.id);
    assert_eq!(updated.revoked_at, None);
}

#[tokio::test]
async fn clears_selected_mcps_and_reactivates_an_expired_token() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;
    let created = create_token(
        &app,
        &admin,
        "Expired token",
        &[mcp.id],
        Some(Utc::now() - Duration::days(1)),
    )
    .await;
    assert!(!created.is_usable());

    // As a browser sends it: a POST that stands for a PUT.
    let response = app
        .post(&format!("/tokens/{}?_method=PUT", created.id))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Reactivated token"),
            ("scopeMode", "all"),
            ("expiresAt", ""),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some("/tokens"));
    assert_eq!(
        flashed_text(&response, "success").as_deref(),
        Some("Token updated")
    );

    let updated = find(&app, created.id).await.unwrap();
    assert_eq!(updated.scope_mode, ScopeMode::All);
    assert_eq!(updated.expires_at, None);
    assert!(mcp_ids(&app, &updated).await.is_empty());
    assert!(updated.is_usable());
}

#[tokio::test]
async fn rejects_invalid_mcp_ids_without_changing_token_settings() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;
    let created = create_token(&app, &admin, "Unchanged token", &[], None).await;

    let response = app
        .put(&format!("/tokens/{}", created.id))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Rejected update"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &mcp.id.to_string()),
            ("mcpIds[]", "999999"),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&response, "error").as_deref(),
        Some("One or more selected MCPs do not exist")
    );

    let unchanged = find(&app, created.id).await.unwrap();
    assert_eq!(unchanged.name, "Unchanged token");
    assert_eq!(unchanged.scope_mode, ScopeMode::All);
}

#[tokio::test]
async fn rejects_updates_to_missing_and_revoked_tokens() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let created = create_revoked_token(&app, &admin, "Revoked token").await;
    let connection = create_oauth_connection(&app, &admin, "Claude Desktop").await;
    let update = |path: String| {
        app.put(&path)
            .login_as(&admin)
            .csrf()
            .form(&[("name", "Rejected update"), ("scopeMode", "all")])
            .send()
    };

    let revoked_response = update(format!("/tokens/{}", created.id)).await;
    assert_eq!(revoked_response.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&revoked_response, "error").as_deref(),
        Some("Revoked tokens cannot be edited")
    );

    let missing_response = update("/tokens/999999".into()).await;
    assert_eq!(missing_response.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&missing_response, "error").as_deref(),
        Some("Token not found")
    );
    assert_eq!(find(&app, created.id).await.unwrap().name, "Revoked token");

    let oauth_response = update(format!("/tokens/{}", connection.id)).await;
    assert_eq!(oauth_response.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&oauth_response, "error").as_deref(),
        Some("OAuth connections cannot be edited — revoke the connection instead")
    );
    assert_eq!(
        find(&app, connection.id).await.unwrap().name,
        "Claude Desktop"
    );

    // The edit dialog says the same, to the script and to a plain request.
    for (id, message) in [
        (created.id, "Revoked tokens cannot be edited"),
        (999999, "Token not found"),
        (
            connection.id,
            "OAuth connections cannot be edited — revoke the connection instead",
        ),
    ] {
        let scripted = app
            .get(&format!("/tokens/{id}/edit"))
            .login_as(&admin)
            .header("x-requested-with", "fetch")
            .send()
            .await;
        assert_eq!(scripted.status, StatusCode::NO_CONTENT);
        assert_eq!(scripted.header("x-location"), Some("/tokens"));
        assert_eq!(flashed_text(&scripted, "error").as_deref(), Some(message));

        let plain = app
            .get(&format!("/tokens/{id}/edit"))
            .login_as(&admin)
            .send()
            .await;
        assert_eq!(plain.status, StatusCode::FOUND);
        assert_eq!(plain.location(), Some("/tokens"));
        assert_eq!(flashed_text(&plain, "error").as_deref(), Some(message));
    }
}

#[tokio::test]
async fn validates_the_token_route_parameter_with_vine() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app
        .put("/tokens/not-a-token-id")
        .login_as(&admin)
        .csrf()
        .form(&[("name", "Invalid token id"), ("scopeMode", "all")])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response.flashed("errors"),
        Some(json!({ "id": "The id field must be a number" }))
    );
    assert_eq!(
        flashed_text(&response, "error").as_deref(),
        Some("The id field must be a number")
    );
}

#[tokio::test]
async fn an_id_that_is_not_a_number_is_answered_like_a_missing_record() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    for path in [
        "/tokens/abc/revoke",
        "/tokens/1.5/revoke",
        "/tokens/0/revoke",
    ] {
        let response = app.post(path).login_as(&admin).csrf().send().await;
        assert_eq!(response.status, StatusCode::FOUND, "{path}");
        assert_eq!(
            flashed_text(&response, "error").as_deref(),
            Some("Token not found"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn revokes_a_token() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let created = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .form(&[("name", "Revocable agent"), ("scopeMode", "all")])
        .send()
        .await;
    let token = find_by_name(&app, "Revocable agent").await.unwrap();

    let response = app
        .post(&format!("/tokens/{}/revoke", token.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_eq!(created.status, StatusCode::FOUND);
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/tokens"));
    assert_eq!(
        flashed_text(&response, "success").as_deref(),
        Some("Token revoked")
    );
    assert!(find(&app, token.id).await.unwrap().is_revoked());

    let again = app
        .post(&format!("/tokens/{}/revoke", token.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flashed_text(&again, "error").as_deref(),
        Some("Token already revoked")
    );
}

#[tokio::test]
async fn refuses_changes_without_a_csrf_token_and_requests_without_a_session() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_token(&app, &admin, "Kept token", &[], None).await;

    let forged = app
        .post(&format!("/tokens/{}/revoke", token.id))
        .login_as(&admin)
        .api()
        .send()
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    assert!(!find(&app, token.id).await.unwrap().is_revoked());

    for path in ["/tokens", "/tokens/new", "/tokens/install"] {
        let guest = app.get(path).send().await;
        assert_eq!(guest.status, StatusCode::FOUND, "{path}");
        assert_eq!(guest.location(), Some("/login"), "{path}");
    }
    let guest = app
        .post("/tokens")
        .csrf()
        .form(&[("name", "Guest token"), ("scopeMode", "all")])
        .send()
        .await;
    assert_eq!(guest.location(), Some("/login"));
    assert!(find_by_name(&app, "Guest token").await.is_none());

    // Tokens are not an administrator's matter only.
    let member = create_member(&app).await;
    let page = app.get("/tokens").login_as(&member).send().await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("Kept token"));
}

// ---------------------------------------------------- access token cleanup

struct CleanupTokens {
    admin: User,
    active: AccessToken,
    expired: AccessToken,
    revoked: AccessToken,
}

async fn create_cleanup_tokens(app: &TestApp) -> CleanupTokens {
    let admin = create_admin(app).await;
    let active = create_token(app, &admin, "Active token", &[], None).await;
    let expired = create_token(
        app,
        &admin,
        "Expired token",
        &[],
        Some(Utc::now() - Duration::minutes(1)),
    )
    .await;
    let revoked = create_revoked_token(app, &admin, "Revoked token").await;
    CleanupTokens {
        admin,
        active,
        expired,
        revoked,
    }
}

#[tokio::test]
async fn marks_only_expired_and_revoked_tokens_as_deletable() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;

    let response = app.get("/tokens").login_as(&tokens.admin).send().await;

    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    for deletable in [&tokens.revoked, &tokens.expired] {
        assert!(page.contains(&format!("<form id=\"delete-token-{}\"", deletable.id)));
        assert!(!page.contains(&format!("<form id=\"revoke-token-{}\"", deletable.id)));
    }
    assert!(!page.contains(&format!("<form id=\"delete-token-{}\"", tokens.active.id)));
    assert!(page.contains(&format!("<form id=\"revoke-token-{}\"", tokens.active.id)));

    // Newest first, each with its status.
    let names: Vec<usize> = ["Revoked token", "Expired token", "Active token"]
        .iter()
        .map(|name| {
            page.find(&format!("<td class=\"cell-strong\">{name}</td>"))
                .unwrap()
        })
        .collect();
    assert!(names.is_sorted());
    assert!(table_row(&page, "Revoked token").contains("<span class=\"badge\">Revoked</span>"));
    assert!(
        table_row(&page, "Expired token")
            .contains("<span class=\"badge badge--warning\">Expired</span>")
    );
    assert!(
        table_row(&page, "Active token")
            .contains("<span class=\"badge badge--success\">Active</span>")
    );

    // "Delete all" names every token that can be deleted, and no other.
    let delete_all = page
        .split("data-confirm-title=\"Delete all expired and revoked tokens?\"")
        .nth(1)
        .unwrap()
        .split("</form>")
        .next()
        .unwrap();
    assert!(delete_all.contains(&format!("name=\"ids[]\" value=\"{}\"", tokens.expired.id)));
    assert!(delete_all.contains(&format!("name=\"ids[]\" value=\"{}\"", tokens.revoked.id)));
    assert!(!delete_all.contains(&format!("name=\"ids[]\" value=\"{}\"", tokens.active.id)));
    assert!(page.contains("<p class=\"toolbar__note push-end\">2 expired or revoked</p>"));
    assert!(page.contains(
        "data-confirm=\"Permanently delete 2 tokens? This cannot be undone. Existing activity logs will keep their token names and identifiers.\""
    ));
}

#[tokio::test]
async fn deletes_selected_expired_and_revoked_tokens_while_preserving_active_tokens() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;

    let response = app
        .delete("/tokens")
        .login_as(&tokens.admin)
        .csrf()
        .json(json!({ "ids": [tokens.expired.id, tokens.revoked.id] }))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/tokens"));
    assert_eq!(
        flashed_text(&response, "success").as_deref(),
        Some("2 tokens deleted")
    );
    assert!(find(&app, tokens.expired.id).await.is_none());
    assert!(find(&app, tokens.revoked.id).await.is_none());
    assert!(find(&app, tokens.active.id).await.is_some());
}

#[tokio::test]
async fn rejects_the_entire_deletion_when_an_active_token_is_selected() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;

    let response = app
        .delete("/tokens")
        .login_as(&tokens.admin)
        .csrf()
        .json(json!({ "ids": [tokens.active.id, tokens.expired.id] }))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&response, "error").as_deref(),
        Some("Active tokens must be revoked before they can be deleted")
    );
    assert!(find(&app, tokens.active.id).await.is_some());
    assert!(find(&app, tokens.expired.id).await.is_some());
}

#[tokio::test]
async fn deletes_what_the_forms_of_the_page_send() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;
    let connection = create_oauth_connection(&app, &tokens.admin, "Old laptop").await;
    let admin = &tokens.admin;
    let delete = |ids: Vec<i64>| {
        let ids: Vec<String> = ids.iter().map(i64::to_string).collect();
        let fields: Vec<(&str, &str)> = ids.iter().map(|id| ("ids[]", id.as_str())).collect();
        app.post("/tokens?_method=DELETE&status=inactive")
            .login_as(admin)
            .csrf()
            .form(&fields)
            .send()
    };

    // One row: back to the view the form was sent from.
    let single = delete(vec![tokens.expired.id]).await;
    assert_eq!(single.status, StatusCode::FOUND);
    assert_eq!(single.location(), Some("/tokens?status=inactive"));
    assert_eq!(
        flashed_text(&single, "success").as_deref(),
        Some("1 token deleted")
    );
    assert!(find(&app, tokens.expired.id).await.is_none());

    // A token that is gone refuses the whole deletion, and so does one checked twice in a row.
    let missing = delete(vec![tokens.revoked.id, tokens.expired.id]).await;
    assert_eq!(
        flashed_text(&missing, "error").as_deref(),
        Some("One or more tokens no longer exist")
    );
    assert!(find(&app, tokens.revoked.id).await.is_some());

    // A live OAuth connection is active, whatever its access token: it stays.
    let live = delete(vec![connection.id]).await;
    assert_eq!(
        flashed_text(&live, "error").as_deref(),
        Some("Active tokens must be revoked before they can be deleted")
    );

    // Once its refresh token has expired it can go, and an id sent twice counts once.
    sqlx::query("update `access_tokens` set `oauth_refresh_expires_at` = ?, `expires_at` = ? where `id` = ?")
        .bind(Timestamp::now() - Duration::days(1))
        .bind(Timestamp::now() - Duration::days(30))
        .bind(connection.id)
        .execute(&*app.core.db)
        .await
        .unwrap();
    let both = delete(vec![connection.id, tokens.revoked.id, connection.id]).await;
    assert_eq!(
        flashed_text(&both, "success").as_deref(),
        Some("2 tokens deleted")
    );
    assert!(find(&app, connection.id).await.is_none());
    assert!(find(&app, tokens.revoked.id).await.is_none());
    assert!(find(&app, tokens.active.id).await.is_some());

    // Nothing selected is a refused form.
    let nothing = delete(vec![]).await;
    assert_eq!(nothing.status, StatusCode::FOUND);
    assert_eq!(
        flashed_text(&nothing, "error").as_deref(),
        Some("The ids field must be defined")
    );
}

// ------------------------------------------------- the page and its dialogs

#[tokio::test]
async fn hides_inactive_bulk_selection_while_every_token_is_active() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    create_token(&app, &admin, "Active token", &[], None).await;

    let page = app.get("/tokens").login_as(&admin).send().await.text();

    assert!(page.contains("Active token"));
    assert!(!page.contains("data-select-all"));
    assert!(!page.contains("data-select-item"));
    assert!(!page.contains("expired or revoked</p>"));
    assert!(!page.contains("Delete all"));
    assert!(page.contains("<a class=\"segment\" href=\"/tokens\" aria-current=\"true\">All 1</a>"));
    assert!(page.contains("<a class=\"segment\" href=\"/tokens?status=active\">Active 1</a>"));
    assert!(page.contains(
        "<a class=\"segment\" href=\"/tokens?status=inactive\">Expired &amp; revoked 0</a>"
    ));

    // The view of what can be deleted is empty, and says so.
    let inactive = app
        .get("/tokens?status=inactive")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(inactive.contains("No expired or revoked tokens"));
    assert!(!inactive.contains("<table"));
}

#[tokio::test]
async fn offers_selection_and_confirmation_where_tokens_can_be_deleted() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;

    let all = app
        .get("/tokens")
        .login_as(&tokens.admin)
        .send()
        .await
        .text();
    // Rows are selected in the view of what can be deleted, not in the others.
    assert!(!all.contains("data-select-all"));
    let expired_row = table_row(&all, "Expired token");
    assert!(expired_row.contains("data-confirm-title=\"Delete tokens?\""));
    assert!(expired_row.contains(
        "data-confirm=\"Permanently delete 1 token? This cannot be undone. Existing activity logs will keep their token names and identifiers.\""
    ));
    assert!(
        expired_row.contains("data-confirm-label=\"Delete tokens\" data-confirm-tone=\"critical\"")
    );
    // Row actions are quiet buttons: the destructive one is in the prompt.
    assert!(expired_row.contains("<button type=\"submit\" class=\"button\">Delete</button>"));
    let active_row = table_row(&all, "Active token");
    assert!(active_row.contains("data-confirm-title=\"Revoke Active token?\""));
    assert!(active_row.contains(
        "data-confirm=\"Agents that use Active token lose access to the gateway immediately.\""
    ));

    let inactive = app
        .get("/tokens?status=inactive")
        .login_as(&tokens.admin)
        .send()
        .await
        .text();
    assert!(inactive.contains(
        "<input type=\"checkbox\" class=\"checkbox\" data-select-all=\"#tokens-table\" aria-label=\"Select all\">"
    ));
    for token in [&tokens.expired, &tokens.revoked] {
        assert!(inactive.contains(&format!(
            "<input type=\"checkbox\" class=\"checkbox\" name=\"ids[]\" value=\"{}\" form=\"delete-selected\" data-select-item aria-label=\"Select {} for deletion\">",
            token.id, token.name
        )));
    }
    assert!(!inactive.contains("Active token"));
    assert!(inactive.contains(
        "<p class=\"toolbar__note push-end\" data-select-empty=\"#tokens-table\">2 expired or revoked</p>"
    ));
    let selected = tag_of(&inactive, "delete-selected");
    assert!(selected.contains("action=\"/tokens?_method=DELETE&amp;status=inactive\""));
    assert!(selected.contains("data-select-bar=\"#tokens-table\""));
    assert!(selected.contains("data-confirm-title=\"Delete tokens?\""));
    assert!(is_hidden(selected));
    assert!(inactive.contains("<span data-select-count=\"#tokens-table\">0</span> selected"));
    assert!(inactive.contains("Delete selected"));
    assert!(inactive.contains("data-confirm-label=\"Delete all\""));
    assert!(inactive.contains(
        "<a class=\"segment\" href=\"/tokens?status=inactive\" aria-current=\"true\">Expired &amp; revoked 2</a>"
    ));

    let active = app
        .get("/tokens?status=active")
        .login_as(&tokens.admin)
        .send()
        .await
        .text();
    assert!(table_row(&active, "Active token").contains("Revoke"));
    assert!(!active.contains("<td class=\"cell-strong\">Expired token</td>"));
    assert!(!active.contains("data-select-all"));
}

#[tokio::test]
async fn keeps_cleanup_and_row_actions_in_the_mobile_list() {
    let app = TestApp::new().await;
    let tokens = create_cleanup_tokens(&app).await;

    let page = app
        .get("/tokens")
        .login_as(&tokens.admin)
        .send()
        .await
        .text();

    let expired = mobile_row(&page, "Expired token");
    assert!(expired.contains("aria-label=\"Actions for Expired token\""));
    assert!(expired.contains("data-dialog-open=\"#edit-token\" data-dialog-fetch"));
    // The menu submits the forms of the table: each request exists once in the page.
    assert!(expired.contains(&format!(
        "<button type=\"submit\" class=\"menu__item menu__item--critical\" role=\"menuitem\" form=\"delete-token-{}\">",
        tokens.expired.id
    )));
    assert!(!expired.contains("Revoke"));
    assert_eq!(
        page.matches(&format!("id=\"delete-token-{}\"", tokens.expired.id))
            .count(),
        1
    );

    let revoked = mobile_row(&page, "Revoked token");
    assert!(!revoked.contains("Edit"));
    assert!(revoked.contains(&format!("form=\"delete-token-{}\"", tokens.revoked.id)));

    let active = mobile_row(&page, "Active token");
    assert!(active.contains(&format!("form=\"revoke-token-{}\"", tokens.active.id)));
    assert!(active.contains("Manual · "));
    assert!(active.contains("… · All MCPs"));
    assert!(active.contains("No expiry"));
    assert!(active.contains("Never used"));
    // No checkboxes under 768px.
    assert!(
        !page
            .split("hide-desktop")
            .nth(1)
            .unwrap()
            .contains("type=\"checkbox\" class=\"checkbox\" name=\"ids[]\"")
    );
}

#[tokio::test]
async fn edits_active_and_expired_tokens_while_keeping_revoked_tokens_immutable() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| mcp.name = "Search MCP".into()).await;
    let expires_at = (Utc::now() + Duration::days(1)).trunc_subsecs(0);
    let editable = create_token(&app, &admin, "Editable token", &[], Some(expires_at)).await;
    let expired = create_token(
        &app,
        &admin,
        "Expired token",
        &[],
        Some(Utc::now() - Duration::days(1)),
    )
    .await;
    create_revoked_token(&app, &admin, "Revoked token").await;

    let page = app.get("/tokens").login_as(&admin).send().await.text();
    let editable_row = table_row(&page, "Editable token");
    assert!(editable_row.contains(&format!(
        "<a class=\"button\" href=\"/tokens/{}/edit\" data-dialog-open=\"#edit-token\" data-dialog-fetch>Edit</a>",
        editable.id
    )));
    let expired_row = table_row(&page, "Expired token");
    assert_eq!(expired_row.matches(">Edit</a>").count(), 1);
    assert!(!expired_row.contains("Revoke"));
    assert!(!table_row(&page, "Revoked token").contains(">Edit</a>"));
    // The dialog is in the page, empty until "Edit" fills it.
    assert!(page.contains(
        "<dialog class=\"dialog dialog--sm\" id=\"edit-token\" aria-labelledby=\"edit-token-title\" data-dialog-return=\"/tokens\"><div data-fragment></div></dialog>"
    ));

    // What "Edit" loads: the form of this token.
    let dialog = app
        .get(&format!("/tokens/{}/edit", editable.id))
        .login_as(&admin)
        .header("x-requested-with", "fetch")
        .send()
        .await;
    assert_eq!(dialog.status, StatusCode::OK);
    let form = dialog.text();
    assert!(form.starts_with(&format!(
        "<form method=\"post\" action=\"/tokens/{}?_method=PUT\" data-async>",
        editable.id
    )));
    assert_eq!(text_of(&form, "edit-token-title"), "Edit Editable token");
    assert!(form.contains(&format!(
        "<p class=\"dialog__subtitle\">{}…</p>",
        editable.token_prefix
    )));
    assert!(tag_of(&form, "edit-token-name").contains("value=\"Editable token\""));
    assert!(form.contains("name=\"scopeMode\" value=\"all\" checked>"));
    let expires = tag_of(&form, "edit-token-expires");
    assert!(expires.contains(&format!(
        "value=\"{}\"",
        expires_at.format("%Y-%m-%dT%H:%M")
    )));
    assert!(expires.contains(&format!(
        "data-utc=\"{}\"",
        expires_at.format("%Y-%m-%dT%H:%M:%S.000Z")
    )));
    assert!(form.contains(&format!(
        "<input type=\"checkbox\" class=\"checkbox\" name=\"mcpIds[]\" value=\"{}\">",
        mcp.id
    )));
    assert!(form.contains("<span class=\"choice__label\">Search MCP</span>"));

    // Without the script the same address is the page with that dialog open.
    let plain = app
        .get(&format!("/tokens/{}/edit", editable.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(plain.contains(
        "<dialog class=\"dialog dialog--sm\" id=\"edit-token\" aria-labelledby=\"edit-token-title\" data-open data-dialog-return=\"/tokens\"><div data-fragment><form"
    ));
    assert!(plain.contains("Edit Editable token"));
    assert!(plain.contains("<h1 class=\"page-header__title\">Access tokens</h1>"));

    // Saving, as the script sends it.
    let saved = app
        .post(&format!("/tokens/{}?_method=PUT", editable.id))
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[
            ("name", "Browser edited token"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &mcp.id.to_string()),
            (
                "expiresAt",
                &expires_at.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
            ),
        ])
        .send()
        .await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT);
    assert_eq!(saved.header("x-location"), Some("/tokens"));
    let after = follow(&app, &saved).await.text();
    let updated_row = table_row(&after, "Browser edited token");
    assert!(updated_row.contains("<td>1 MCP</td>"));
    assert!(after.contains("Token updated"));

    // A refused change comes back in the dialog, which stays open.
    let refused = app
        .post(&format!(
            "/tokens/{}?_method=PUT&status=inactive",
            expired.id
        ))
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[("name", ""), ("scopeMode", "all")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(form.starts_with(&format!(
        "<form method=\"post\" action=\"/tokens/{}?_method=PUT&amp;status=inactive\" data-async>",
        expired.id
    )));
    assert_eq!(text_of(&form, "edit-token-title"), "Edit Expired token");
    assert_eq!(
        text_of(&form, "edit-token-name-error"),
        "The name field must be defined"
    );
    assert_eq!(find(&app, expired.id).await.unwrap().name, "Expired token");
}

#[tokio::test]
async fn opens_the_dialog_its_address_names() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let get = |path: &'static str| {
        let request = app.get(path).login_as(&admin);
        async move { request.send().await.text() }
    };

    let list = get("/tokens").await;
    assert!(!tag_of(&list, "create-token").contains("data-open"));
    assert!(!tag_of(&list, "install").contains("data-open"));
    assert!(list.contains(
        "<a class=\"button button--primary\" href=\"/tokens/new\" data-dialog-open=\"#create-token\">"
    ));
    assert!(list.contains(
        "<a class=\"button button--secondary\" href=\"/tokens/install\" data-dialog-open=\"#install\">"
    ));

    let create = get("/tokens/new").await;
    assert!(tag_of(&create, "create-token").contains(" data-open "));
    assert!(tag_of(&create, "create-token").contains("data-dialog-return=\"/tokens\""));
    assert!(!tag_of(&create, "install").contains("data-open"));
    assert!(create.contains("<form method=\"post\" action=\"/tokens\" data-async>"));
    assert_eq!(text_of(&create, "create-token-title"), "Create token");
    // No MCP yet: the list of the form says so.
    assert!(create.contains("Add an MCP first"));

    let install = get("/tokens/install?status=active").await;
    assert!(tag_of(&install, "install").contains(" data-open "));
    assert!(tag_of(&install, "install").contains("data-dialog-return=\"/tokens?status=active\""));
    assert!(!tag_of(&install, "create-token").contains("data-open"));

    // Without a token yet, the list is an invitation.
    assert!(list.contains("<p class=\"empty-state__title\">No tokens yet</p>"));
    assert!(list.contains("Create a token so agents can call the gateway."));
    assert!(!list.contains("class=\"toolbar\""));
}

/// The twelve snippets of the install dialog.
fn snippets() -> Vec<(McpClient, McpInstallAuthMode, bool, String)> {
    let mut all = Vec::new();
    for client in McpClient::ALL {
        for auth in [McpInstallAuthMode::Oauth, McpInstallAuthMode::Token] {
            for lazy in [false, true] {
                let id = format!(
                    "install-{}-{}{}",
                    client.key(),
                    auth.key(),
                    if lazy { "-lazy" } else { "" }
                );
                all.push((client, auth, lazy, id));
            }
        }
    }
    all
}

#[tokio::test]
async fn builds_copyable_configurations_for_codex_claude_and_cursor() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = app.get("/tokens").login_as(&admin).send().await.text();

    // Every snippet is what the Node app wrote for the same choices.
    for (client, auth, lazy, id) in snippets() {
        let expected = create_mcp_install_config(
            client,
            "http://localhost:3333/mcp",
            "<YOUR_ACCESS_TOKEN>",
            lazy,
            auth,
        );
        assert_eq!(text_of(&page, &id), expected.code, "{id}");
        let block = snippet_block(&page, &id);
        assert!(block.contains(&format!(
            "data-show-when=\"auth={}&amp;lazy={}\"",
            auth.key(),
            if lazy { "on" } else { "off" }
        )));
        // The dialog opens on OAuth, lazy tool mode off.
        assert_eq!(
            is_hidden(block),
            !(auth == McpInstallAuthMode::Oauth && !lazy),
            "{id}"
        );
    }
    assert_eq!(
        text_of(&page, "install-claude-oauth"),
        "claude mcp add --transport http --scope user mymcps 'http://localhost:3333/mcp'\nclaude mcp get mymcps"
    );
    assert!(text_of(&page, "install-codex-token-lazy").contains(
        "http_headers = { Authorization = \"Bearer <YOUR_ACCESS_TOKEN>\", \"X-MyMCPs-Tool-Mode\" = \"lazy\" }"
    ));
    assert!(
        text_of(&page, "install-cursor-token-lazy").contains("\"X-MyMCPs-Tool-Mode\": \"lazy\"")
    );

    // The token typed in the dialog is written into the snippets by the
    // script, escaped for its place.
    assert!(page.contains(
        "Authorization = &quot;Bearer <span data-bind=\"token\" data-bind-format=\"json\" data-bind-empty=\"&lt;YOUR_ACCESS_TOKEN&gt;\">&lt;YOUR_ACCESS_TOKEN&gt;</span>&quot;"
    ));
    assert!(page.contains(
        "--header 'Authorization: Bearer <span data-bind=\"token\" data-bind-format=\"shell\" data-bind-empty=\"&lt;YOUR_ACCESS_TOKEN&gt;\">&lt;YOUR_ACCESS_TOKEN&gt;</span>'"
    ));
    let token = tag_of(&page, "install-token");
    assert!(token.contains("type=\"password\""));
    assert!(token.contains("placeholder=\"Paste your MyMCPs access token\""));
    assert!(token.contains("data-bind-source=\"token\""));
    assert!(!token.contains("value="));
    assert!(page.contains(
        "data-password-toggle=\"#install-token\" aria-pressed=\"false\" aria-label=\"Show access token\" data-label-pressed=\"Hide access token\""
    ));
    assert!(
        page.contains("<input type=\"checkbox\" class=\"switch\" role=\"switch\" name=\"lazy\">")
    );
    // The form of the dialog is never sent.
    assert!(page.contains(
        "<form method=\"dialog\" data-bind-scope><button type=\"submit\" disabled hidden></button>"
    ));

    // Each client has its file name and its two steps.
    for (client, title, restart, verify) in [
        (
            "codex",
            "~/.codex/config.toml",
            "Save the file, then restart Codex or restart the IDE extension.",
            "Open /mcp in Codex and confirm that mymcps is connected.",
        ),
        (
            "claude",
            "Terminal",
            "Restart Claude Code after the command completes.",
            "Run /mcp in Claude Code and confirm that mymcps is connected.",
        ),
        (
            "cursor",
            "~/.cursor/mcp.json",
            "Save the file, then restart Cursor.",
            "Open Cursor MCP settings and confirm that mymcps is enabled.",
        ),
    ] {
        let panel = page
            .split(&format!("id=\"install-panel-{client}\""))
            .nth(1)
            .unwrap()
            .split("</ol>")
            .next()
            .unwrap();
        assert_eq!(
            panel
                .matches(&format!("<span class=\"code-block__title\">{title}</span>"))
                .count(),
            4
        );
        assert!(panel.contains(&format!(
            "<ol class=\"steps\"><li class=\"step\">{restart}</li><li class=\"step\">{verify}</li>"
        )));
    }
}

#[tokio::test]
async fn opens_in_oauth_mode_without_requiring_a_token() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = app
        .get("/tokens/install")
        .login_as(&admin)
        .send()
        .await
        .text();

    assert!(page.contains("name=\"auth\" value=\"oauth\" checked>OAuth (recommended)"));
    assert!(page.contains("name=\"auth\" value=\"token\">Access token"));
    assert!(page.contains("<div class=\"banner\" role=\"status\" data-show-when=\"auth=oauth\">"));
    assert!(page.contains("No token to copy"));
    assert!(page.contains("<div class=\"stack gap-300\" data-show-when=\"auth=token\" hidden>"));
    // What is shown asks for no token and sends no header.
    for id in [
        "install-codex-oauth",
        "install-claude-oauth",
        "install-cursor-oauth",
    ] {
        assert!(!is_hidden(snippet_block(&page, id)));
        let snippet = text_of(&page, id);
        assert!(!snippet.contains("Authorization"));
        assert!(!snippet.contains("<YOUR_ACCESS_TOKEN>"));
        assert!(!snippet.contains("--header"));
        assert!(!snippet.contains("\"headers\""));
    }
    // An OAuth snippet can be copied at once; a token one once a token is there.
    assert!(page.contains(
        "<button type=\"button\" class=\"icon-button\" data-copy-target=\"#install-claude-oauth\" aria-label=\"Copy\">"
    ));
    assert!(page.contains(
        "<button type=\"button\" class=\"icon-button\" data-copy-target=\"#install-claude-token\" data-show-when=\"token!=\" aria-label=\"Copy\" hidden>"
    ));
    assert!(page.contains(
        "<div class=\"banner\" role=\"status\" data-show-when=\"auth=token&amp;token=\" hidden>"
    ));

    // Claude is the client the dialog opens on.
    assert!(tag_of(&page, "install-tab-claude").contains("aria-selected=\"true\">"));
    assert!(tag_of(&page, "install-tab-codex").contains("aria-selected=\"false\" tabindex=\"-1\""));
    assert!(
        tag_of(&page, "install-tab-cursor").contains("aria-selected=\"false\" tabindex=\"-1\"")
    );
    assert!(!is_hidden(tag_of(&page, "install-panel-claude")));
    assert!(is_hidden(tag_of(&page, "install-panel-codex")));
    assert!(is_hidden(tag_of(&page, "install-panel-cursor")));
}

#[tokio::test]
async fn opens_with_the_newly_created_access_token_pre_filled_and_visible() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let created = app
        .post("/tokens")
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .form(&[("name", "Fresh install token"), ("scopeMode", "all")])
        .send()
        .await;
    let plaintext = flashed_text(&created, "createdPlaintext").unwrap();
    let page = follow(&app, &created).await.text();

    assert!(tag_of(&page, "install").contains(" data-open "));
    assert!(page.contains("name=\"auth\" value=\"oauth\">OAuth (recommended)"));
    assert!(page.contains("name=\"auth\" value=\"token\" checked>Access token"));
    let token = tag_of(&page, "install-token");
    assert!(token.contains("type=\"text\""));
    assert!(token.contains(&format!("value=\"{plaintext}\"")));
    assert!(page.contains(
        "data-password-toggle=\"#install-token\" aria-pressed=\"true\" aria-label=\"Hide access token\" data-label=\"Show access token\" data-label-pressed=\"Hide access token\""
    ));

    // The token is in the snippets, and they can be copied.
    for (client, auth, lazy, id) in snippets() {
        let expected =
            create_mcp_install_config(client, "http://localhost:3333/mcp", &plaintext, lazy, auth);
        assert_eq!(text_of(&page, &id), expected.code, "{id}");
        assert_eq!(
            is_hidden(snippet_block(&page, &id)),
            !(auth == McpInstallAuthMode::Token && !lazy),
            "{id}"
        );
    }
    assert!(page.contains(
        "<button type=\"button\" class=\"icon-button\" data-copy-target=\"#install-claude-token\" data-show-when=\"token!=\" aria-label=\"Copy\">"
    ));
    assert!(page.contains(
        "<div class=\"banner\" role=\"status\" data-show-when=\"auth=token&amp;token=\" hidden>"
    ));
    // The new token is the first row of the list.
    assert!(
        page.split("<tbody>")
            .nth(1)
            .unwrap()
            .trim_start_matches("<tr>")
            .starts_with("<td class=\"cell-strong\">Fresh install token</td>")
    );
}

#[tokio::test]
async fn shows_no_gateway_url_and_no_oauth_while_app_url_is_not_set() {
    for app_url in [None, Some("http://mcp.example.com")] {
        let app = TestApp::with_config(|config| config.app_url = app_url.map(str::to_string)).await;
        let admin = create_admin(&app).await;

        let page = app
            .get("/tokens/install")
            .login_as(&admin)
            .send()
            .await
            .text();

        assert!(page.contains(
            "<span class=\"copy-field__value\">Configure APP_URL to reveal the gateway URL.</span>"
        ));
        assert!(page.contains(
            "<button type=\"button\" class=\"icon-button\" disabled title=\"Set APP_URL to enable public links\" aria-label=\"Copy gateway URL\">"
        ));
        assert!(!page.contains("id=\"gateway-url\""));

        // The install dialog is left with tokens, and nothing in it can be copied.
        assert!(page.contains("<input type=\"hidden\" name=\"auth\" value=\"token\">"));
        assert!(!page.contains("OAuth (recommended)"));
        assert!(page.contains("OAuth is unavailable"));
        assert!(page.contains(
            "Set APP_URL to this instance's public HTTPS origin to enable OAuth installation."
        ));
        assert!(!page.contains("data-copy-target=\"#install-"));
        assert_eq!(
            page.matches("Configure APP_URL before copying this configuration.")
                .count(),
            3
        );
        assert!(!page.contains("Paste an access token to enable copying."));
        for (client, auth, lazy, id) in snippets() {
            let expected = create_mcp_install_config(
                client,
                "<YOUR_GATEWAY_URL>",
                "<YOUR_ACCESS_TOKEN>",
                lazy,
                auth,
            );
            assert_eq!(text_of(&page, &id), expected.code, "{id}");
            assert_eq!(
                is_hidden(snippet_block(&page, &id)),
                !(auth == McpInstallAuthMode::Token && !lazy),
                "{id}"
            );
        }
    }
}

#[tokio::test]
async fn renders_dates_day_first_for_the_page_script_to_localize() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let displayed_at = DateTime::parse_from_rfc3339("2035-09-06T11:13:52Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut token = create_token(&app, &admin, "Day-first token", &[], Some(displayed_at)).await;
    token.last_used_at = Some(Timestamp::from(displayed_at));
    token.save(&*app.core.db).await.unwrap();
    let connection = create_oauth_connection(&app, &admin, "Day-first connection").await;

    let page = app.get("/tokens").login_as(&admin).send().await.text();

    // To the minute in the table, with the instant for the script.
    let row = table_row(&page, "Day-first token");
    assert!(row.contains(
        "<td><time datetime=\"2035-09-06T11:13:52.000Z\" data-format=\"minute\">06/09/2035, 11:13</time></td>"
    ));
    assert!(row.contains(
        "<td class=\"cell-secondary\"><time datetime=\"2035-09-06T11:13:52.000Z\" data-format=\"minute\">06/09/2035, 11:13</time></td>"
    ));
    let mobile = mobile_row(&page, "Day-first token");
    assert!(mobile.contains(
        "Expires <time datetime=\"2035-09-06T11:13:52.000Z\" data-format=\"date\">06/09/2035</time>"
    ));
    assert!(mobile.contains(
        "Last used <time datetime=\"2035-09-06T11:13:52.000Z\">06/09/2035, 11:13:52</time>"
    ));

    // A connection shows when its refresh token expires, not its access token.
    let refresh_expires_at = find(&app, connection.id)
        .await
        .unwrap()
        .oauth_refresh_expires_at
        .unwrap();
    let row = table_row(&page, "Day-first connection");
    assert!(row.contains(&format!(
        "<time datetime=\"{}\" data-format=\"minute\">",
        refresh_expires_at.to_iso()
    )));
    assert!(row.contains("<span class=\"badge badge--info badge--no-dot\">OAuth</span>"));
    assert!(row.contains("<td class=\"cell-secondary\">Never</td>"));
    assert!(row.contains(
        "data-confirm=\"Revoking this OAuth connection stops both its access and refresh tokens.\""
    ));
    assert!(!row.contains(">Edit</a>"));
    let manual = table_row(&page, "Day-first token");
    assert!(manual.contains("<span class=\"badge badge--no-dot\">Manual</span>"));
    assert!(manual.contains(&format!(
        "<td class=\"cell-code\">{}…</td>",
        token.token_prefix
    )));
}

#[tokio::test]
async fn pages_through_the_tokens_of_a_view() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for index in 0..27 {
        create_token(&app, &admin, &format!("Agent {index:02}"), &[], None).await;
    }
    for index in 0..3 {
        create_revoked_token(&app, &admin, &format!("Retired {index}")).await;
    }
    let get = |path: &'static str| {
        let request = app.get(path).login_as(&admin);
        async move { request.send().await.text() }
    };

    let first = get("/tokens").await;
    assert!(first.contains("All 30</a>"));
    assert!(first.contains("Active 27</a>"));
    assert!(first.contains("Expired &amp; revoked 3</a>"));
    assert_eq!(
        first
            .split("<tbody>")
            .nth(1)
            .unwrap()
            .split("</tbody>")
            .next()
            .unwrap()
            .matches("<tr>")
            .count(),
        25
    );
    assert!(first.contains("<p class=\"table-footer__note\">25 rows per page</p>"));
    assert!(first.contains("<span class=\"pagination__range\">1–25 of 30</span>"));
    assert!(first.contains(
        "<a class=\"icon-button icon-button--secondary\" aria-disabled=\"true\" aria-label=\"Previous page\">"
    ));
    assert!(first.contains(
        "<a class=\"icon-button icon-button--secondary\" href=\"/tokens?page=2\" aria-label=\"Next page\">"
    ));
    // The newest are first.
    assert!(first.contains("Retired 2"));
    assert!(!first.contains("Agent 00"));

    let second = get("/tokens?page=2").await;
    assert!(second.contains("<span class=\"pagination__range\">26–30 of 30</span>"));
    assert!(second.contains(
        "<a class=\"icon-button icon-button--secondary\" href=\"/tokens\" aria-label=\"Previous page\">"
    ));
    assert!(second.contains("Agent 00"));
    // The forms of a page come back to it.
    assert!(second.contains("/revoke?page=2\""));
    assert!(tag_of(&second, "create-token").contains("data-dialog-return=\"/tokens?page=2\""));

    let active = get("/tokens?status=active&page=2").await;
    assert!(active.contains("<span class=\"pagination__range\">26–27 of 27</span>"));
    assert!(active.contains(
        "<a class=\"icon-button icon-button--secondary\" href=\"/tokens?status=active\" aria-label=\"Previous page\">"
    ));
    assert!(active.contains("/revoke?status=active&amp;page=2\""));

    // A page past the end is the last one, and a view that is not one is the whole list.
    let beyond = get("/tokens?status=inactive&page=9").await;
    assert!(beyond.contains("<span class=\"pagination__range\">1–3 of 3</span>"));
    let unknown = get("/tokens?status=everything&page=first").await;
    assert!(unknown.contains("<span class=\"pagination__range\">1–25 of 30</span>"));
}

// ------------------------------------------------- hardening: browser history

#[tokio::test]
async fn asks_the_browser_to_keep_no_copy_of_the_tokens_page() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_token(&app, &admin, "Listed token", &[], None).await;

    // With or without a token to show: a copy kept for the back button or on
    // disk would outlive the one time the token is shown, and the session.
    for path in [
        "/tokens".to_string(),
        "/tokens/new".to_string(),
        "/tokens/install".to_string(),
        format!("/tokens/{}/edit", token.id),
    ] {
        let response = app.get(&path).login_as(&admin).send().await;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert_eq!(response.header("cache-control"), Some("no-store"), "{path}");
    }
}

#[tokio::test]
async fn shows_nothing_of_the_tokens_page_once_signed_out() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    create_token(&app, &admin, "Listed token", &[], None).await;
    let before = app.get("/tokens").login_as(&admin).send().await;
    assert!(before.text().contains("Listed token"));

    let logout = app
        .post("/logout")
        .session(before.session())
        .csrf()
        .send()
        .await;
    assert_eq!(logout.status, StatusCode::FOUND);
    assert_eq!(logout.redirect_path().as_deref(), Some("/login"));

    // The browser kept no copy, so going back asks the server again: with
    // the session as it was left, and with the one it had before.
    for session in [logout.session(), before.session()] {
        let response = app.get("/tokens").session(session).send().await;
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(response.location(), Some("/login"));
        assert!(!response.text().contains("Listed token"));
    }
}
