//! The Credentials page and the OAuth clients it saves: one Google Cloud
//! client entered with the first Google MCP, chosen by the next, and changed
//! once for all of them.

mod mcps_support;

use http::StatusCode;
use mcps_support::*;
use mymcps_core::Timestamp;
use mymcps_core::models::{Mcp, McpTransport, ProviderCredential, User};
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;

const CLIENT_ID: &str = "1234567890-abc.apps.googleusercontent.com";
const SECRET: &str = "google-client-secret";

fn google_mcp<'a>(name: &'a str, fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("name", name),
        ("transport", "builtin"),
        ("builtinKey", "google-ads"),
        ("authType", "auto"),
        ("enabled", "on"),
    ];
    form.extend_from_slice(fields);
    form
}

async fn send(
    app: &TestApp,
    user: &User,
    method: &str,
    path: &str,
    form: &[(&str, &str)],
) -> TestResponse {
    let request = match method {
        "POST" => app.post(path),
        "PUT" => app.put(path),
        _ => app.delete(path),
    };
    request.login_as(user).csrf().form(form).send().await
}

async fn credentials(app: &TestApp) -> Vec<ProviderCredential> {
    ProviderCredential::of_team(&*app.core.db, 1, None)
        .await
        .unwrap()
}

/// A Google Ads MCP signed in with `credential`, holding a token.
async fn signed_in(app: &TestApp, admin: &User, credential: &ProviderCredential) -> Mcp {
    let token = encrypt(app, "google-access-token");
    let secret = encrypt(app, SECRET);
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("google-ads".into());
        mcp.provider_credential_id = Some(credential.id);
        mcp.oauth_client_id = Some(credential.client_id.clone());
        mcp.oauth_client_secret = secret;
        mcp.oauth_access_token = token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + chrono::Duration::minutes(50));
    })
    .await
}

async fn saved_credential(app: &TestApp, admin: &User) -> ProviderCredential {
    let response = send(
        app,
        admin,
        "POST",
        "/credentials",
        &[
            ("provider", "google"),
            ("name", "Agency"),
            ("clientId", CLIENT_ID),
            ("clientSecret", SECRET),
        ],
    )
    .await;
    assert_redirect(&response, "/credentials");
    credentials(app).await.pop().unwrap()
}

#[tokio::test]
async fn the_client_of_the_first_google_mcp_is_saved_and_the_next_one_chooses_it() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let first = send(
        &app,
        &admin,
        "POST",
        "/mcps",
        &google_mcp(
            "Google Ads",
            &[("oauthClientId", CLIENT_ID), ("oauthClientSecret", SECRET)],
        ),
    )
    .await;
    assert_redirect(&first, "/mcps");
    let [credential] = credentials(&app).await.try_into().unwrap();
    assert_eq!(
        (credential.name.as_str(), credential.client_id.as_str()),
        ("Google Cloud", CLIENT_ID)
    );
    assert_eq!(
        credential.secret(&app.core.encryption).as_deref(),
        Some(SECRET)
    );
    let ads = mcp_by_slug(&app, "google-ads").await.unwrap();
    assert_eq!(ads.provider_credential_id, Some(credential.id));

    // The dialog of the next one offers it, and skips the Cloud console step.
    let page = app
        .get("/mcps/new?template=google-ads")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(page.contains(&format!(
        "<option value=\"{}\" selected>Google Cloud · 1234567890-abc.app…</option>",
        credential.id
    )));
    assert!(page.contains("<option value=\"new\">Add another OAuth client</option>"));

    let id = credential.id.to_string();
    let mut console = google_mcp("Search Console", &[("oauthCredentialId", &id)]);
    console.retain(|(name, _)| *name != "builtinKey");
    console.push(("builtinKey", "google-search-console"));
    let second = send(&app, &admin, "POST", "/mcps", &console).await;
    assert_redirect(&second, "/mcps");
    let copy = mcp_by_slug(&app, "search-console").await.unwrap();
    assert_eq!(copy.builtin_key.as_deref(), Some("google-search-console"));
    assert_eq!(copy.provider_credential_id, Some(credential.id));
    assert_eq!(copy.oauth_client_id.as_deref(), Some(CLIENT_ID));
    assert_eq!(
        decrypt(&app, &copy.oauth_client_secret).as_deref(),
        Some(SECRET)
    );
    assert_eq!(credentials(&app).await.len(), 1);

    // The same client typed again is the saved one, not a second credential;
    // with another secret it is refused, since the page owns the secret.
    let typed = send(
        &app,
        &admin,
        "POST",
        "/mcps",
        &google_mcp(
            "Google Ads 3",
            &[
                ("oauthCredentialId", "new"),
                ("oauthClientId", CLIENT_ID),
                ("oauthClientSecret", SECRET),
            ],
        ),
    )
    .await;
    assert_redirect(&typed, "/mcps");
    assert_eq!(
        mcp_by_slug(&app, "google-ads-3")
            .await
            .unwrap()
            .provider_credential_id,
        Some(credential.id)
    );
    let other_secret = from_script(app.post("/mcps"))
        .login_as(&admin)
        .csrf()
        .form(&google_mcp(
            "Google Ads 4",
            &[
                ("oauthCredentialId", "new"),
                ("oauthClientId", CLIENT_ID),
                ("oauthClientSecret", "other"),
            ],
        ))
        .send()
        .await;
    assert_eq!(other_secret.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(other_secret.text().contains(
        "This Client ID is already saved as &quot;Google Cloud&quot; with another Client Secret"
    ));
    assert_eq!(credentials(&app).await.len(), 1);
}

#[tokio::test]
async fn an_mcp_cannot_choose_the_credential_of_another_team() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut foreign = ProviderCredential {
        team_id: 999,
        provider: "google".into(),
        name: "Theirs".into(),
        client_id: CLIENT_ID.into(),
        client_secret: encrypt(&app, SECRET).unwrap(),
        created_by: admin.id,
        ..Default::default()
    };
    sqlx::query("insert into `teams` (`id`, `name`, `created_at`) values (999, 'Other', '2026-01-01 00:00:00')")
        .execute(&*app.core.db)
        .await
        .unwrap();
    foreign.insert(&*app.core.db).await.unwrap();

    let id = foreign.id.to_string();
    let response = from_script(app.post("/mcps"))
        .login_as(&admin)
        .csrf()
        .form(&google_mcp("Google Ads", &[("oauthCredentialId", &id)]))
        .send()
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = response.text();
    assert!(
        text.contains("Choose an OAuth client saved for this team, or add one"),
        "{text}"
    );
    assert_eq!(count_mcps(&app).await, 0);
}

#[tokio::test]
async fn a_new_secret_reaches_every_mcp_and_a_new_client_id_signs_them_out() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let credential = saved_credential(&app, &admin).await;
    let mcp = signed_in(&app, &admin, &credential).await;
    let path = format!("/credentials/{}", credential.id);

    let rotated = send(
        &app,
        &admin,
        "PUT",
        &path,
        &[
            ("name", "Agency"),
            ("clientId", CLIENT_ID),
            ("clientSecret", "rotated"),
        ],
    )
    .await;
    assert_redirect(&rotated, "/credentials");
    assert_eq!(
        rotated.flashed("success"),
        Some(json!("OAuth client saved"))
    );
    let kept = find_mcp(&app, mcp.id).await;
    assert_eq!(
        decrypt(&app, &kept.oauth_client_secret).as_deref(),
        Some("rotated")
    );
    assert_eq!(
        decrypt(&app, &kept.oauth_access_token).as_deref(),
        Some("google-access-token")
    );
    assert_eq!(kept.oauth_generation, mcp.oauth_generation);

    // A blank secret keeps the saved one.
    let renamed = send(
        &app,
        &admin,
        "PUT",
        &path,
        &[
            ("name", "Agency"),
            ("clientId", "999-other.apps.googleusercontent.com"),
        ],
    )
    .await;
    assert_eq!(
        renamed.flashed("success"),
        Some(json!(
            "OAuth client saved. Its MCP has to be connected again: the Client ID changed"
        ))
    );
    let moved = find_mcp(&app, mcp.id).await;
    assert_eq!(
        moved.oauth_client_id.as_deref(),
        Some("999-other.apps.googleusercontent.com")
    );
    assert_eq!(
        decrypt(&app, &moved.oauth_client_secret).as_deref(),
        Some("rotated")
    );
    assert_eq!(moved.oauth_access_token, None);
    assert!(moved.health().awaits_sign_in());
    assert_ne!(moved.oauth_generation, mcp.oauth_generation);
}

#[tokio::test]
async fn a_credential_in_use_is_not_deleted_and_the_page_never_shows_a_secret() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let credential = saved_credential(&app, &admin).await;
    let mcp = signed_in(&app, &admin, &credential).await;
    let path = format!("/credentials/{}", credential.id);

    let page = app.get("/credentials").login_as(&admin).send().await.text();
    assert!(page.contains("Agency") && page.contains(CLIENT_ID) && page.contains(">1 MCP<"));
    assert!(!page.contains(SECRET));
    let edit = app
        .get(&format!("{path}/edit"))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(edit.contains("Used by 1 MCP") && !edit.contains(SECRET));

    let refused = send(&app, &admin, "DELETE", &path, &[]).await;
    assert_eq!(
        refused.flashed("error"),
        Some(json!(
            "Agency is used by Google Ads. Choose another OAuth client for it first."
        ))
    );
    assert_eq!(credentials(&app).await.len(), 1);

    mcp.delete(&*app.core.db).await.unwrap();
    let deleted = send(&app, &admin, "DELETE", &path, &[]).await;
    assert_eq!(
        deleted.flashed("success"),
        Some(json!("OAuth client deleted"))
    );
    assert!(credentials(&app).await.is_empty());
}

#[tokio::test]
async fn the_page_refuses_a_duplicate_and_is_only_for_admins() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    saved_credential(&app, &admin).await;

    let duplicate = from_script(app.post("/credentials"))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("provider", "google"),
            ("name", "Other"),
            ("clientId", CLIENT_ID),
            ("clientSecret", SECRET),
        ])
        .send()
        .await;
    assert_eq!(duplicate.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        duplicate
            .text()
            .contains("This Client ID is already saved for this team")
    );
    assert!(!duplicate.text().contains(SECRET));

    let member = create_member(&app).await;
    assert_redirect(&app.get("/credentials").login_as(&member).send().await, "/");
    assert!(
        !app.get("/mcps")
            .login_as(&member)
            .send()
            .await
            .text()
            .contains("href=\"/credentials\"")
    );
    assert!(
        app.get("/mcps")
            .login_as(&admin)
            .send()
            .await
            .text()
            .contains("href=\"/credentials\"")
    );
}
