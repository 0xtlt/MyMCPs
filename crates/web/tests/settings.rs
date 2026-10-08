//! Port of `tests/functional/settings.spec.ts`,
//! `hardening_auth_password_checks.spec.ts`, the password change case of
//! `hardening_auth_sessions.spec.ts`, the instance settings cases of
//! `logs_analytics.spec.ts` and `mcp_npm_update.spec.ts`, and
//! `tests/browser/settings_instance.spec.ts`.

use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{
    CallOutcome, GatewayToolMode, InstanceSetting, McpCallLog, McpLogLevel, User,
};
use mymcps_web::auth::SESSION_STAMP_KEY;
use mymcps_web::testing::factories::{
    create_admin, create_admin_with, create_member, create_member_with,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::json;

/// Send the request as the page's script sends an async form.
fn from_script(request: TestRequest<'_>) -> TestRequest<'_> {
    request
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
}

async fn change_email(
    app: &TestApp,
    user: &User,
    email: &str,
    current_password: &str,
) -> TestResponse {
    app.patch("/settings/email")
        .login_as(user)
        .csrf()
        .form(&[("email", email), ("currentPassword", current_password)])
        .send()
        .await
}

async fn change_password(app: &TestApp, user: &User, current_password: &str) -> TestResponse {
    app.patch("/settings/password")
        .login_as(user)
        .csrf()
        .form(&[
            ("currentPassword", current_password),
            ("newPassword", "new-password123"),
            ("passwordConfirmation", "new-password123"),
        ])
        .send()
        .await
}

async fn save_instance(app: &TestApp, user: &User, fields: &[(&str, &str)]) -> TestResponse {
    // As a browser sends it: a POST that names the method.
    app.post("/settings/mcp-logging?_method=PATCH")
        .login_as(user)
        .csrf()
        .form(fields)
        .send()
        .await
}

async fn reload(app: &TestApp, user: &User) -> User {
    User::find(&*app.core.db, user.id).await.unwrap().unwrap()
}

async fn remember_tokens(app: &TestApp, user: &User) -> i64 {
    sqlx::query_scalar("select count(*) from `remember_me_tokens` where `tokenable_id` = ?")
        .bind(user.id)
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

fn refused_field(response: &TestResponse, field: &str) -> Option<String> {
    response
        .flashed("errors")?
        .get(field)?
        .as_str()
        .map(str::to_string)
}

// --- settings ---

#[tokio::test]
async fn requires_authentication_for_settings_pages_and_updates() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    let page = app.get("/settings").send().await;
    let email = app
        .patch("/settings/email")
        .csrf()
        .form(&[
            ("email", "new@example.com"),
            ("currentPassword", "password123"),
        ])
        .send()
        .await;
    let password = app
        .patch("/settings/password")
        .csrf()
        .form(&[
            ("currentPassword", "password123"),
            ("newPassword", "new-password123"),
            ("passwordConfirmation", "new-password123"),
        ])
        .send()
        .await;
    let instance = app
        .patch("/settings/mcp-logging")
        .csrf()
        .form(&[("gatewayToolMode", "lazy")])
        .send()
        .await;

    for response in [page, email, password, instance] {
        assert_redirect(&response, "/login");
    }
}

#[tokio::test]
async fn shows_instance_settings_only_to_admins() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let member = create_member_with(&app, "member@example.com").await;

    let admin_page = app.get("/settings").login_as(&admin).send().await;
    let member_page = app.get("/settings").login_as(&member).send().await;

    assert_eq!(admin_page.status, StatusCode::OK);
    let page = admin_page.text();
    assert!(page.contains("<title>Settings · MyMCPs</title>"));
    assert!(page.contains("<h1 class=\"page-header__title\">Settings</h1>"));
    assert!(page.contains("<span class=\"detail-row__value\">Test User</span>"));
    assert!(page.contains("<span class=\"detail-row__value\">admin@example.com</span>"));
    assert!(page.contains("data-dialog-open=\"#change-email\""));
    assert!(page.contains("data-dialog-open=\"#change-password\""));
    assert!(
        page.contains(
            "My Instance <span class=\"badge badge--info badge--no-dot\">Admin only</span>"
        )
    );
    assert!(page.contains(
        "<form class=\"card\" method=\"post\" action=\"/settings/mcp-logging?_method=PATCH\">"
    ));
    // The defaults of a new instance.
    assert!(page.contains("name=\"gatewayToolMode\" value=\"eager\" checked>"));
    assert!(!page.contains("value=\"lazy\" checked"));
    assert!(page.contains("<option value=\"metadata\" selected>Metadata</option>"));
    assert!(page.contains("name=\"mcpLogRetentionDays\" type=\"number\" inputmode=\"numeric\" min=\"1\" max=\"365\" step=\"1\" value=\"14\""));
    assert!(page.contains("name=\"mcpAutoUpdateEnabled\">"));
    assert!(page.contains("name=\"mcpAutoUpdateCron\" type=\"text\" value=\"0 2 * * *\""));
    // Every form of the page carries the CSRF token.
    assert_eq!(
        page.matches("<form ").count(),
        page.matches("name=\"_csrf\"").count()
    );
    assert_eq!(page.matches("<form ").count(), 4);

    assert_eq!(member_page.status, StatusCode::OK);
    let page = member_page.text();
    assert!(page.contains("<span class=\"detail-row__value\">member@example.com</span>"));
    assert!(page.contains("action=\"/settings/email?_method=PATCH\""));
    assert!(page.contains("action=\"/settings/password?_method=PATCH\""));
    for admin_only in [
        "My Instance",
        "Admin only",
        "/settings/mcp-logging",
        "gatewayToolMode",
        "mcpLogLevel",
        "mcpLogRetentionDays",
        "mcpAutoUpdateCron",
    ] {
        assert!(!page.contains(admin_only), "{admin_only}");
    }
    // Reading the page as a member does not create the settings row.
    let rows: i64 = sqlx::query_scalar("select count(*) from `instance_settings`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(rows, 1, "created for the administrator's page only");
}

#[tokio::test]
async fn says_so_when_the_account_has_no_name() {
    let app = TestApp::new().await;
    let member = create_member(&app).await;
    sqlx::query("update `users` set `full_name` = null where `id` = ?")
        .bind(member.id)
        .execute(&*app.core.db)
        .await
        .unwrap();

    let page = app.get("/settings").login_as(&member).send().await.text();
    assert!(page.contains("<span class=\"detail-row__label\">Name</span><span class=\"detail-row__value\">Not set</span>"));
}

#[tokio::test]
async fn updates_the_signed_in_user_email_after_password_confirmation() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;

    let response = change_email(&app, &admin, "updated@example.com", "password123").await;
    assert_redirect(&response, "/settings");
    assert_eq!(response.flashed("success"), Some(json!("Email updated")));

    let admin = reload(&app, &admin).await;
    assert_eq!(admin.email, "updated@example.com");
    assert!(admin.verify_password("password123").await.unwrap());
    // The session of this browser goes on.
    assert_eq!(admin.session_version, 1);

    let page = app
        .get("/settings")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">Email updated</p>"));
    assert!(page.contains("<span class=\"detail-row__value\">updated@example.com</span>"));
}

#[tokio::test]
async fn rejects_duplicate_emails_and_incorrect_current_passwords() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    create_member_with(&app, "member@example.com").await;

    let duplicate = change_email(&app, &admin, "member@example.com", "password123").await;
    assert_redirect(&duplicate, "/settings");
    assert_eq!(
        refused_field(&duplicate, "email").as_deref(),
        Some("The email has already been taken")
    );

    let incorrect_password =
        change_email(&app, &admin, "updated@example.com", "incorrect-password").await;
    assert_redirect(&incorrect_password, "/settings");
    assert_eq!(
        refused_field(&incorrect_password, "currentPassword").as_deref(),
        Some("The current password is incorrect")
    );

    assert_eq!(reload(&app, &admin).await.email, "admin@example.com");

    // The page opens the dialog again on what was refused, and keeps the
    // address that was typed, never a password.
    let page = app
        .get("/settings")
        .session(incorrect_password.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "id=\"change-email\" aria-labelledby=\"change-email-title\" data-dialog-reset data-open>"
    ));
    assert!(page.contains(
        "id=\"change-password\" aria-labelledby=\"change-password-title\" data-dialog-reset>"
    ));
    assert!(page.contains("name=\"email\" type=\"email\" autocomplete=\"email\" value=\"updated@example.com\" required>"));
    assert!(page.contains("<p class=\"field__error\" id=\"email-current-password-error\">The current password is incorrect</p>"));
    assert!(!page.contains("incorrect-password"));
    assert!(!page.contains("<p class=\"toast__message\">The current password is incorrect</p>"));
}

#[tokio::test]
async fn keeping_ones_own_email_is_not_a_duplicate() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;

    let response = change_email(&app, &admin, "admin@example.com", "password123").await;
    assert_redirect(&response, "/settings");
    assert_eq!(response.flashed("success"), Some(json!("Email updated")));
}

#[tokio::test]
async fn refuses_an_email_form_that_is_not_filled_in() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;

    let response = change_email(&app, &admin, "not an email", "").await;
    assert_redirect(&response, "/settings");
    assert_eq!(
        response.flashed("errors"),
        Some(json!({
            "email": "The email field must be a valid email address",
            "currentPassword": "The currentPassword field must be defined",
        }))
    );
    assert_eq!(
        response.flashed("error"),
        Some(json!("The email field must be a valid email address"))
    );
    assert_eq!(reload(&app, &admin).await.email, "admin@example.com");
}

#[tokio::test]
async fn updates_the_password_and_invalidates_the_old_credential() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    for secret in ["first browser", "second browser"] {
        sqlx::query(
            "insert into `remember_me_tokens` (`tokenable_id`, `hash`, `created_at`, `updated_at`, `expires_at`) values (?, ?, ?, ?, ?)",
        )
        .bind(admin.id)
        .bind(mymcps_core::crypto::sha256_hex(secret))
        .bind(Timestamp::now().timestamp_millis())
        .bind(Timestamp::now().timestamp_millis())
        .bind((Timestamp::now() + chrono::Duration::days(365)).timestamp_millis())
        .execute(&*app.core.db)
        .await
        .unwrap();
    }
    assert_eq!(remember_tokens(&app, &admin).await, 2);

    let response = change_password(&app, &admin, "password123").await;
    assert_redirect(&response, "/settings");
    assert_eq!(response.flashed("success"), Some(json!("Password updated")));

    let updated = reload(&app, &admin).await;
    assert!(!updated.verify_password("password123").await.unwrap());
    assert!(updated.verify_password("new-password123").await.unwrap());
    assert_eq!(remember_tokens(&app, &updated).await, 0);
}

#[tokio::test]
async fn rejects_mismatched_password_confirmation() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app
        .patch("/settings/password")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("currentPassword", "password123"),
            ("newPassword", "new-password123"),
            ("passwordConfirmation", "different-password"),
        ])
        .send()
        .await;
    assert_redirect(&response, "/settings");
    assert!(refused_field(&response, "passwordConfirmation").is_some());

    let stored = reload(&app, &admin).await;
    assert!(stored.verify_password("password123").await.unwrap());
    assert_eq!(stored.session_version, admin.session_version);

    // The dialog opens again; no password comes back.
    let page = app
        .get("/settings")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "id=\"change-password\" aria-labelledby=\"change-password-title\" data-dialog-reset data-open>"
    ));
    assert!(page.contains("id=\"new-password-confirmation-error\""));
    assert!(!page.contains("new-password123") && !page.contains("different-password"));
}

#[tokio::test]
async fn refuses_a_new_password_that_is_too_short_or_too_long() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let too_long = "p".repeat(33);
    for (new_password, message) in [
        (
            "short",
            "The newPassword field must have at least 8 characters",
        ),
        (
            too_long.as_str(),
            "The newPassword field must not be greater than 32 characters",
        ),
    ] {
        let response = app
            .patch("/settings/password")
            .login_as(&admin)
            .csrf()
            .form(&[
                ("currentPassword", "password123"),
                ("newPassword", new_password),
                ("passwordConfirmation", new_password),
            ])
            .send()
            .await;
        assert_redirect(&response, "/settings");
        assert_eq!(
            refused_field(&response, "newPassword").as_deref(),
            Some(message)
        );
    }
    assert!(
        reload(&app, &admin)
            .await
            .verify_password("password123")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn answers_the_account_dialogs_of_the_page_script() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;

    let refused = from_script(app.post("/settings/password?_method=PATCH"))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("currentPassword", "wrong-password"),
            ("newPassword", "new-password123"),
            ("passwordConfirmation", "new-password123"),
        ])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(form.starts_with(
        "<form method=\"post\" action=\"/settings/password?_method=PATCH\" data-async>"
    ));
    assert!(form.contains("name=\"_csrf\""));
    assert!(form.contains(
        "id=\"current-password\" name=\"currentPassword\" type=\"password\" autocomplete=\"current-password\" required aria-invalid=\"true\" aria-describedby=\"current-password-error\">"
    ));
    assert!(form.contains(
        "<p class=\"field__error\" id=\"current-password-error\">The current password is incorrect</p>"
    ));
    assert!(
        !form.contains("autofocus"),
        "the invalid field takes the focus"
    );
    assert!(!form.contains("wrong-password") && !form.contains("new-password123"));
    // Nothing is left for the next page: the script put the form in place.
    assert!(refused.flashed("errors").is_none() && refused.flashed("error").is_none());

    let refused = from_script(app.post("/settings/email?_method=PATCH"))
        .login_as(&admin)
        .csrf()
        .form(&[("email", "nope"), ("currentPassword", "password123")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(
        form.starts_with(
            "<form method=\"post\" action=\"/settings/email?_method=PATCH\" data-async>"
        )
    );
    assert!(form.contains("value=\"nope\""));
    assert!(form.contains(
        "<p class=\"field__error\" id=\"new-email-error\">The email field must be a valid email address</p>"
    ));

    let saved = from_script(app.post("/settings/email?_method=PATCH"))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("email", "updated@example.com"),
            ("currentPassword", "password123"),
        ])
        .send()
        .await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT);
    assert_eq!(saved.header("x-location"), Some("/settings"));
    assert_eq!(saved.flashed("success"), Some(json!("Email updated")));
    assert_eq!(reload(&app, &admin).await.email, "updated@example.com");
}

#[tokio::test]
async fn a_fresh_dialog_starts_on_its_first_field() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let page = app.get("/settings").login_as(&admin).send().await.text();

    assert!(page.contains("name=\"email\" type=\"email\" autocomplete=\"email\" value=\"admin@example.com\" required autofocus>"));
    assert!(page.contains("id=\"current-password\" name=\"currentPassword\" type=\"password\" autocomplete=\"current-password\" required autofocus>"));
    assert!(!page.contains("data-open"));
    assert!(!page.contains("aria-invalid"));
    // A password is never rendered.
    assert!(!page.contains("type=\"password\" value"));
}

// --- hardening: current-password checks ---

#[tokio::test]
async fn limits_wrong_current_passwords_across_the_email_and_password_forms() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;

    for guess in 0..5 {
        let wrong_password = format!("wrong-password-{guess}");
        let wrong = if guess % 2 == 0 {
            change_email(&app, &admin, "updated@example.com", &wrong_password).await
        } else {
            change_password(&app, &admin, &wrong_password).await
        };
        assert_eq!(wrong.status, StatusCode::FOUND);
        assert!(refused_field(&wrong, "currentPassword").is_some());
    }

    let email = change_email(&app, &admin, "updated@example.com", "password123").await;
    assert_eq!(email.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(email.header("retry-after").is_some());
    assert!(email.text().contains("Too many requests"));

    let password = change_password(&app, &admin, "password123").await;
    assert_eq!(password.status, StatusCode::TOO_MANY_REQUESTS);

    let admin = reload(&app, &admin).await;
    assert_eq!(admin.email, "admin@example.com");
    assert!(admin.verify_password("password123").await.unwrap());
}

#[tokio::test]
async fn caps_a_parallel_burst_of_wrong_current_passwords() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let guesses: Vec<String> = (0..12).map(|guess| format!("wrong-{guess}")).collect();

    let responses = futures::future::join_all(
        guesses
            .iter()
            .map(|guess| change_email(&app, &admin, "updated@example.com", guess)),
    )
    .await;

    assert_eq!(
        responses
            .iter()
            .filter(|response| response.status != StatusCode::TOO_MANY_REQUESTS)
            .count(),
        5
    );
}

#[tokio::test]
async fn counts_each_user_separately() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;

    for _ in 0..5 {
        change_email(&app, &admin, "updated@example.com", "wrong-password").await;
    }

    let response = change_email(&app, &member, "updated@example.com", "password123").await;
    assert_redirect(&response, "/settings");
    assert_eq!(response.flashed("success"), Some(json!("Email updated")));
}

#[tokio::test]
async fn a_correct_current_password_clears_the_count() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    for _ in 0..2 {
        for _ in 0..4 {
            change_email(&app, &admin, "updated@example.com", "wrong-password").await;
        }
        let response = change_email(&app, &admin, "updated@example.com", "password123").await;
        assert_redirect(&response, "/settings");
    }
}

#[tokio::test]
async fn a_form_that_fails_validation_does_not_use_up_the_budget() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    for _ in 0..8 {
        let invalid = change_email(&app, &admin, "not an email", "wrong-password").await;
        assert_eq!(invalid.status, StatusCode::FOUND);
    }
    let response = change_email(&app, &admin, "updated@example.com", "password123").await;
    assert_redirect(&response, "/settings");
    assert_eq!(response.flashed("success"), Some(json!("Email updated")));
}

// --- hardening: session lifetime and revocation ---

#[tokio::test]
async fn changing_the_password_retires_captured_sessions_but_not_the_browser_that_did_it() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let signed_in = app
        .post("/login")
        .csrf()
        .form(&[("email", "admin@example.com"), ("password", "password123")])
        .send()
        .await;
    let captured = signed_in.session();
    assert!(captured.contains_key(SESSION_STAMP_KEY));

    let changed = app
        .patch("/settings/password")
        .session(captured.clone())
        .csrf()
        .form(&[
            ("currentPassword", "password123"),
            ("newPassword", "new-password123"),
            ("passwordConfirmation", "new-password123"),
        ])
        .send()
        .await;
    assert_redirect(&changed, "/settings");

    // The session copied before the change is rejected, with or without the
    // remember-me cookie of the sign-in, which the change revoked.
    let rejected = app.get("/").session(captured.clone()).send().await;
    assert_redirect(&rejected, "/login");
    let remember = signed_in.cookie("remember_web").unwrap();
    let rejected = app
        .get("/")
        .session(captured)
        .cookie("remember_web", &remember)
        .send()
        .await;
    assert_redirect(&rejected, "/login");

    let accepted = app.get("/").session(changed.session()).send().await;
    assert_eq!(accepted.status, StatusCode::OK);
}

// --- instance settings ---

#[tokio::test]
async fn changes_the_default_mcp_tool_mode_from_the_settings_page() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let page = app.get("/settings").login_as(&admin).send().await.text();
    assert!(page.contains("name=\"gatewayToolMode\" value=\"eager\" checked>"));

    // What the form of the page sends once "Lazy" is chosen.
    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "lazy"),
            ("mcpLogLevel", "metadata"),
            ("mcpLogRetentionDays", "14"),
            ("mcpAutoUpdateCron", "0 2 * * *"),
        ],
    )
    .await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some("/settings"));
    assert_eq!(
        response.flashed("success"),
        Some(json!("Instance settings updated"))
    );

    let page = app
        .get("/settings")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">Instance settings updated</p>"));
    assert!(page.contains("name=\"gatewayToolMode\" value=\"lazy\" checked>"));
    assert!(!page.contains("value=\"eager\" checked"));
    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Lazy);
}

#[tokio::test]
async fn saves_every_instance_setting() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "lazy"),
            ("mcpLogLevel", "responses"),
            ("mcpLogRetentionDays", "30"),
            ("mcpAutoUpdateEnabled", "on"),
            ("mcpAutoUpdateCron", " 30 4 * * 1 "),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");

    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Lazy);
    assert_eq!(settings.mcp_log_level, McpLogLevel::Responses);
    assert_eq!(settings.mcp_log_retention_days, 30);
    assert!(settings.mcp_auto_update_enabled);
    assert_eq!(settings.mcp_auto_update_cron, "30 4 * * 1");
    assert_eq!(settings.updated_by, Some(admin.id));

    let page = app.get("/settings").login_as(&admin).send().await.text();
    assert!(page.contains(
        "<option value=\"responses\" selected>Metadata + arguments + responses</option>"
    ));
    assert!(page.contains("step=\"1\" value=\"30\""));
    assert!(page.contains("name=\"mcpAutoUpdateEnabled\" checked>"));
    assert!(page.contains("name=\"mcpAutoUpdateCron\" type=\"text\" value=\"30 4 * * 1\""));

    // An unchecked switch is not sent, and an empty schedule keeps the one in place.
    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "eager"),
            ("mcpLogLevel", "off"),
            ("mcpLogRetentionDays", "365"),
            ("mcpAutoUpdateCron", ""),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");
    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Eager);
    assert_eq!(settings.mcp_log_level, McpLogLevel::Off);
    assert_eq!(settings.mcp_log_retention_days, 365);
    assert!(!settings.mcp_auto_update_enabled);
    assert_eq!(settings.mcp_auto_update_cron, "30 4 * * 1");
}

#[tokio::test]
async fn sends_the_instance_form_back_with_what_is_wrong() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "sometimes"),
            ("mcpLogLevel", "arguments"),
            ("mcpLogRetentionDays", "0"),
            ("mcpAutoUpdateEnabled", "on"),
            ("mcpAutoUpdateCron", "every day"),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");
    assert_eq!(
        response.flashed("errors"),
        Some(json!({
            "gatewayToolMode": "The selected gatewayToolMode is invalid",
            "mcpLogRetentionDays": "The mcpLogRetentionDays field must be at least 1",
            "mcpAutoUpdateCron": "The mcpAutoUpdateCron field must be a valid 5-field cron expression",
        }))
    );
    assert!(response.flashed("success").is_none());

    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Eager);
    assert_eq!(settings.mcp_log_retention_days, 14);
    assert_eq!(settings.updated_by, None);

    let page = app
        .get("/settings")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"banner__title\">The instance settings were not saved</p><p>Check the 3 fields marked below, then save again.</p>"));
    assert!(page.contains(
        "<p class=\"field__error\" id=\"tool-mode-error\">The selected gatewayToolMode is invalid</p>"
    ));
    assert!(page.contains("<p class=\"field__error\" id=\"log-retention-error\">The mcpLogRetentionDays field must be at least 1</p>"));
    assert!(page.contains("<p class=\"field__error\" id=\"auto-update-cron-error\">The mcpAutoUpdateCron field must be a valid 5-field cron expression</p>"));
    // What was typed is kept.
    assert!(page.contains("<option value=\"arguments\" selected>"));
    assert!(page.contains("step=\"1\" value=\"0\" aria-invalid=\"true\" aria-describedby=\"log-retention-error log-retention-help\">"));
    assert!(page.contains("name=\"mcpAutoUpdateEnabled\" checked>"));
    assert!(page.contains("name=\"mcpAutoUpdateCron\" type=\"text\" value=\"every day\""));
    // The first field to correct takes the focus.
    assert!(page.contains("name=\"gatewayToolMode\" value=\"eager\" autofocus>"));
    // The account dialogs stay closed.
    assert!(!page.contains("data-open"));
}

#[tokio::test]
async fn rejects_invalid_instance_settings() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for (field, value) in [
        ("gatewayToolMode", "sometimes"),
        ("mcpLogLevel", "everything"),
        ("mcpLogRetentionDays", "0"),
        ("mcpLogRetentionDays", "366"),
        ("mcpLogRetentionDays", "1.5"),
        ("mcpLogRetentionDays", "many"),
        ("mcpAutoUpdateCron", "0 2 * * * *"),
        ("mcpAutoUpdateEnabled", "perhaps"),
    ] {
        let mut fields = vec![
            ("gatewayToolMode", "lazy"),
            ("mcpLogLevel", "responses"),
            ("mcpLogRetentionDays", "30"),
        ];
        fields.retain(|(name, _)| *name != field);
        fields.push((field, value));
        let response = save_instance(&app, &admin, &fields).await;
        assert_redirect(&response, "/settings");
        assert!(refused_field(&response, field).is_some(), "{field}={value}");
    }
    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Eager);
    assert_eq!(settings.mcp_log_level, McpLogLevel::Metadata);
    assert_eq!(settings.mcp_log_retention_days, 14);
}

#[tokio::test]
async fn persists_auto_update_enablement_and_cron_for_admins() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "eager"),
            ("mcpLogLevel", "metadata"),
            ("mcpLogRetentionDays", "14"),
            ("mcpAutoUpdateEnabled", "on"),
            ("mcpAutoUpdateCron", "15 3 * * *"),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");
    assert_eq!(
        response.flashed("success"),
        Some(json!("Instance settings updated"))
    );

    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert!(settings.mcp_auto_update_enabled);
    assert_eq!(settings.mcp_auto_update_cron, "15 3 * * *");
}

#[tokio::test]
async fn rejects_an_invalid_auto_update_cron_expression() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "eager"),
            ("mcpLogLevel", "metadata"),
            ("mcpLogRetentionDays", "14"),
            ("mcpAutoUpdateEnabled", "on"),
            ("mcpAutoUpdateCron", "not a cron"),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");
    assert_eq!(
        refused_field(&response, "mcpAutoUpdateCron").as_deref(),
        Some("The mcpAutoUpdateCron field must be a valid 5-field cron expression")
    );

    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert!(!settings.mcp_auto_update_enabled);
    assert_eq!(settings.mcp_auto_update_cron, "0 2 * * *");
}

#[tokio::test]
async fn restricts_instance_settings_updates_to_admins() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    // The settings exist, as the administrator's page left them.
    app.get("/settings").login_as(&admin).send().await;

    let response = save_instance(
        &app,
        &member,
        &[
            ("gatewayToolMode", "lazy"),
            ("mcpLogLevel", "responses"),
            ("mcpLogRetentionDays", "1"),
        ],
    )
    .await;
    assert_redirect(&response, "/");
    assert_eq!(
        response.flashed("error"),
        Some(json!("Admin access required"))
    );

    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.gateway_tool_mode, GatewayToolMode::Eager);
    assert_eq!(settings.mcp_log_level, McpLogLevel::Metadata);
    assert_eq!(settings.mcp_log_retention_days, 14);
    assert_eq!(settings.updated_by, None);
}

#[tokio::test]
async fn a_shorter_retention_deletes_the_older_call_logs_at_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let db = &*app.core.db;
    for (tool, days_ago) in [("recent", 2), ("older", 10), ("oldest", 40)] {
        let mut log = McpCallLog {
            access_token_name: "Token".into(),
            access_token_prefix: "mmcp_test".into(),
            requested_tool_name: tool.into(),
            outcome: CallOutcome::Success,
            ..Default::default()
        };
        log.insert(db).await.unwrap();
        sqlx::query("update `mcp_call_logs` set `created_at` = ? where `id` = ?")
            .bind(Timestamp::now() - chrono::Duration::days(days_ago))
            .bind(log.id)
            .execute(db)
            .await
            .unwrap();
    }

    let response = save_instance(
        &app,
        &admin,
        &[
            ("gatewayToolMode", "eager"),
            ("mcpLogLevel", "metadata"),
            ("mcpLogRetentionDays", "7"),
        ],
    )
    .await;
    assert_redirect(&response, "/settings");

    let kept: Vec<String> = sqlx::query_scalar(
        "select `requested_tool_name` from `mcp_call_logs` order by `created_at` desc",
    )
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(kept, ["recent"]);
}
