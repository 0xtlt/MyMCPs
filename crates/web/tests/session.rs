//! Port of `tests/functional/session.spec.ts`, `onboarding.spec.ts`,
//! `hardening_auth_login_limits.spec.ts` and
//! `hardening_auth_login_redirect.spec.ts`.

use http::StatusCode;
use mymcps_core::models::User;
use mymcps_web::auth::{REMEMBER_COOKIE, SESSION_STAMP_KEY, SessionStamp};
use mymcps_web::testing::factories::{create_admin, create_admin_with, create_member_with};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::{Map, json};

async fn attempt(app: &TestApp, from: &str, email: &str, password: &str) -> TestResponse {
    // The test server trusts its loopback peer, so the forwarded address is
    // the client address the limiter sees.
    app.post("/login")
        .header("x-forwarded-for", from)
        .csrf()
        .form(&[("email", email), ("password", password)])
        .send()
        .await
}

async fn sign_in(app: &TestApp, email: &str) -> TestResponse {
    app.post("/login")
        .csrf()
        .form(&[("email", email), ("password", "password123")])
        .send()
        .await
}

async fn remember_tokens(app: &TestApp, user: &User) -> Vec<i64> {
    sqlx::query_scalar("select `expires_at` from `remember_me_tokens` where `tokenable_id` = ?")
        .bind(user.id)
        .fetch_all(&*app.core.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn renders_the_login_page_for_guests() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let response = app.get("/login").send().await;

    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("MyMCPs"));
    assert!(page.contains("<h1 class=\"auth-card__title\" id=\"sign-in-title\">Sign in</h1>"));
    assert!(page.contains("name=\"_csrf\""));
    assert!(page.contains("nonce=\""));
    let policy = response.header("content-security-policy").unwrap();
    let nonce = page
        .split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(policy.contains(&format!("'nonce-{nonce}'")));
    // Nothing of the instance belongs in a search engine.
    assert!(
        page.contains("<meta name=\"robots\" content=\"noindex, nofollow, noarchive, nosnippet\">")
    );
}

#[tokio::test]
async fn rate_limits_repeated_credential_attempts_by_resolved_client_ip() {
    let app = TestApp::new().await;
    create_admin_with(&app, "limited@example.com").await;
    for _ in 0..5 {
        let response = attempt(
            &app,
            "198.51.100.77",
            "limited@example.com",
            "wrong-password",
        )
        .await;
        assert_ne!(response.status, StatusCode::TOO_MANY_REQUESTS);
    }

    let limited = attempt(
        &app,
        "198.51.100.77",
        "limited@example.com",
        "wrong-password",
    )
    .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        limited
            .header("retry-after")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert!(limited.text().contains("Too many requests"));
}

#[tokio::test]
async fn does_not_charge_successful_logins_against_the_credential_failure_budget() {
    let app = TestApp::new().await;
    create_admin_with(&app, "successful@example.com").await;
    for _ in 0..6 {
        let response = attempt(
            &app,
            "198.51.100.78",
            "successful@example.com",
            "password123",
        )
        .await;
        assert_eq!(response.status, StatusCode::FOUND);
    }
}

#[tokio::test]
async fn redirects_guests_away_from_authenticated_routes() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let response = app.get("/").send().await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/login"));

    // A script is told, not redirected.
    let api = app.get("/").api().send().await;
    assert_eq!(api.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn allows_an_authenticated_user_to_reach_the_dashboard() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let response = app.get("/").login_as(&admin).send().await;

    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    // The dashboard greets the person, and the shell names them.
    assert!(page.contains("Welcome back, Test."));
    assert!(page.contains("Test User"));
    assert!(page.contains("<body class=\"app\">"));
    assert!(page.contains("aria-current=\"page\""));
    // An administrator sees the whole navigation.
    for entry in [
        "/mcps",
        "/tokens",
        "/approvals",
        "/logs",
        "/analytics",
        "/invites",
        "/settings",
    ] {
        assert!(page.contains(&format!("href=\"{entry}\"")), "{entry}");
    }

    // A signed-in person has no business on the login page.
    let login = app.get("/login").login_as(&admin).send().await;
    assert_eq!(login.status, StatusCode::FOUND);
    assert_eq!(login.redirect_path().as_deref(), Some("/"));
}

#[tokio::test]
async fn a_member_does_not_see_the_administration_entries() {
    let app = TestApp::new().await;
    let member = create_member_with(&app, "member@example.com").await;
    let page = app.get("/").login_as(&member).send().await.text();

    for entry in ["/mcps", "/tokens", "/approvals", "/settings"] {
        assert!(page.contains(&format!("href=\"{entry}\"")), "{entry}");
    }
    for entry in ["/logs", "/analytics", "/invites"] {
        assert!(!page.contains(&format!("href=\"{entry}\"")), "{entry}");
    }
    assert!(page.contains("<span class=\"nav-user__role\">member</span>"));
}

#[tokio::test]
async fn always_remembers_credential_logins_for_one_year() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let requested_at = chrono::Utc::now().timestamp_millis();

    let response = sign_in(&app, "admin@example.com").await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/"));
    assert!(
        response
            .cookie(REMEMBER_COOKIE)
            .unwrap()
            .starts_with("e:gcm.v1:")
    );
    let tokens = remember_tokens(&app, &admin).await;
    assert_eq!(tokens.len(), 1);
    let one_year_ms = (365.25 * 24.0 * 3600.0 * 1000.0) as i64;
    assert!((tokens[0] - (requested_at + one_year_ms)).abs() < 2_000);

    // The session of a credential login is stamped.
    let stamp: SessionStamp =
        serde_json::from_value(response.session()[SESSION_STAMP_KEY].clone()).unwrap();
    assert_eq!(stamp.version, admin.session_version);
    assert!((stamp.authenticated_at - requested_at).abs() < 5_000);
    assert_eq!(stamp.last_seen_at, stamp.authenticated_at);
    assert_eq!(
        app.get("/").session(response.session()).send().await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn revokes_the_remember_token_on_logout() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let login = sign_in(&app, "admin@example.com").await;
    let remember = login.cookie(REMEMBER_COOKIE).unwrap();

    let response = app
        .post("/logout")
        .session(login.session())
        .cookie(REMEMBER_COOKIE, &remember)
        .csrf()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/login"));
    assert!(remember_tokens(&app, &admin).await.is_empty());
    assert!(response.clears_cookie(REMEMBER_COOKIE));
    assert!(!response.session().contains_key("auth_web"));

    // Signing out retires the session captured beforehand.
    let replayed = app.get("/").session(login.session()).send().await;
    assert_eq!(replayed.redirect_path().as_deref(), Some("/login"));
    let reloaded = User::find(&*app.core.db, admin.id).await.unwrap().unwrap();
    assert_eq!(reloaded.session_version, admin.session_version + 1);
}

#[tokio::test]
async fn logs_an_authenticated_user_out() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let response = app.post("/logout").login_as(&admin).csrf().send().await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/login"));

    // Without a CSRF token nothing happens.
    let forged = app.post("/logout").login_as(&admin).send().await;
    assert_eq!(forged.status, StatusCode::FOUND);
    assert_eq!(
        forged.flashed("error"),
        Some(json!("Invalid or expired CSRF token"))
    );
    let forged_api = app.post("/logout").login_as(&admin).api().send().await;
    assert_eq!(forged_api.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn shows_why_a_sign_in_was_refused() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;

    let wrong = attempt(&app, "198.51.100.1", "admin@example.com", "wrong-password").await;
    assert_eq!(wrong.status, StatusCode::FOUND);
    assert_eq!(wrong.redirect_path().as_deref(), Some("/login"));
    assert!(wrong.cookie(REMEMBER_COOKIE).is_none());
    let page = app
        .get("/login")
        .session(wrong.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"banner__title\">Invalid user credentials</p>"));
    assert!(
        page.contains("value=\"admin@example.com\""),
        "the email is kept"
    );
    assert!(!page.contains("wrong-password"), "the password is not");

    let invalid = attempt(&app, "198.51.100.1", "not an email", "").await;
    assert_eq!(invalid.status, StatusCode::FOUND);
    let page = app
        .get("/login")
        .session(invalid.session())
        .send()
        .await
        .text();
    assert!(page.contains("The email field must be a valid email address"));
    assert!(page.contains("The password field must be defined"));
    assert!(page.contains("aria-invalid=\"true\""));
}

#[tokio::test]
async fn returns_to_the_pending_authorization_request_without_the_login_query_string() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let return_to = "/authorize?client_id=abc&state=xyz";
    let mut session = Map::new();
    session.insert("oauthReturnTo".into(), json!(return_to));

    let response = app
        .post("/login?state=forged&extra=1")
        .csrf()
        .session(session)
        .form(&[("email", "admin@example.com"), ("password", "password123")])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some(return_to));
    assert!(!response.session().contains_key("oauthReturnTo"));
}

#[tokio::test]
async fn returns_to_an_approval_link_but_nowhere_else() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let approval = format!("/approvals/{}", "a".repeat(32));
    for (stored, expected) in [
        (approval.as_str(), approval.as_str()),
        ("/approvals/short", "/"),
        (
            "https://evil.example/approvals/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "/",
        ),
    ] {
        let mut session = Map::new();
        session.insert("approvalReturnTo".into(), json!(stored));
        let response = app
            .post("/login")
            .csrf()
            .session(session)
            .form(&[("email", "admin@example.com"), ("password", "password123")])
            .send()
            .await;
        assert_eq!(response.location(), Some(expected), "{stored}");
    }

    // Anything that is not a path on the authorization endpoint is ignored too.
    let mut session = Map::new();
    session.insert(
        "oauthReturnTo".into(),
        json!("https://evil.example/authorize?x=1"),
    );
    let response = app
        .post("/login")
        .csrf()
        .session(session)
        .form(&[("email", "admin@example.com"), ("password", "password123")])
        .send()
        .await;
    assert_eq!(response.location(), Some("/"));
}

// --- onboarding ---

#[tokio::test]
async fn redirects_the_first_visit_to_onboarding() {
    let app = TestApp::new().await;
    for path in ["/", "/login"] {
        let response = app.get(path).send().await;
        assert_eq!(response.status, StatusCode::FOUND, "{path}");
        assert_eq!(
            response.redirect_path().as_deref(),
            Some("/onboarding"),
            "{path}"
        );
    }
    let page = app.get("/onboarding").send().await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("Set up MyMCPs"));
}

#[tokio::test]
async fn creates_the_first_user_as_an_admin() {
    let app = TestApp::new().await;
    let response = app
        .post("/onboarding")
        .csrf()
        .form(&[
            ("fullName", " First Admin "),
            ("email", "admin@example.com"),
            ("password", "password123"),
            ("passwordConfirmation", "password123"),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/"));
    let user = User::find_by_email(&*app.core.db, "admin@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.full_name.as_deref(), Some("First Admin"));
    assert!(user.is_admin());
    assert!(user.verify_password("password123").await.unwrap());
    assert!(response.cookie(REMEMBER_COOKIE).is_some());
    assert_eq!(remember_tokens(&app, &user).await.len(), 1);
    assert!(response.session().contains_key(SESSION_STAMP_KEY));
    assert_eq!(
        app.get("/").session(response.session()).send().await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn does_not_allow_onboarding_after_setup_is_complete() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "existing@example.com").await;

    let response = app.get("/onboarding").send().await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/login"));
    let signed_in = app.get("/onboarding").login_as(&admin).send().await;
    assert_eq!(signed_in.redirect_path().as_deref(), Some("/"));

    let posted = app
        .post("/onboarding")
        .csrf()
        .form(&[
            ("fullName", "Second"),
            ("email", "second@example.com"),
            ("password", "password123"),
            ("passwordConfirmation", "password123"),
        ])
        .send()
        .await;
    assert_eq!(posted.status, StatusCode::FOUND);
    let total: i64 = sqlx::query_scalar("select count(*) from `users`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(total, 1);
}

#[tokio::test]
async fn sends_an_onboarding_form_back_with_what_is_wrong() {
    let app = TestApp::new().await;
    let response = app
        .post("/onboarding")
        .csrf()
        .form(&[
            ("fullName", ""),
            ("email", "admin@example.com"),
            ("password", "short"),
            ("passwordConfirmation", "different"),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/onboarding"));
    let page = app
        .get("/onboarding")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains("The fullName field must be defined"));
    assert!(page.contains("The password field must have at least 8 characters"));
    assert!(page.contains("value=\"admin@example.com\""));
    assert!(!page.contains("short") && !page.contains("different"));
    let total: i64 = sqlx::query_scalar("select count(*) from `users`")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

// --- hardening: login rate limits ---

#[tokio::test]
async fn a_successful_login_on_another_account_does_not_reset_the_guesses_on_one() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    create_member_with(&app, "member@example.com").await;

    for _ in 0..5 {
        let wrong = attempt(&app, "198.51.100.10", "admin@example.com", "wrong-password").await;
        assert_eq!(wrong.status, StatusCode::FOUND);
        let own = attempt(&app, "198.51.100.10", "member@example.com", "password123").await;
        assert_eq!(own.status, StatusCode::FOUND);
    }

    let limited = attempt(&app, "198.51.100.10", "admin@example.com", "password123").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert!(limited.text().contains("Too many requests"));
}

#[tokio::test]
async fn a_successful_login_clears_the_guesses_on_its_own_account() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    for _ in 0..2 {
        for _ in 0..4 {
            attempt(&app, "198.51.100.11", "admin@example.com", "wrong-password").await;
        }
        let own = attempt(&app, "198.51.100.11", "admin@example.com", "password123").await;
        assert_eq!(own.status, StatusCode::FOUND);
        assert!(own.cookie(REMEMBER_COOKIE).is_some());
    }
}

#[tokio::test]
async fn counts_an_account_under_any_letter_case_of_its_email() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let spellings = [
        "admin@example.com",
        "ADMIN@example.com",
        "Admin@Example.COM",
    ];
    for guess in 0..5 {
        let wrong = attempt(
            &app,
            "198.51.100.12",
            spellings[guess % 3],
            "wrong-password",
        )
        .await;
        assert_eq!(wrong.status, StatusCode::FOUND);
    }
    let limited = attempt(&app, "198.51.100.12", "aDmin@example.com", "wrong-password").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn interleaved_successful_logins_do_not_reset_the_address_wide_budget() {
    let app = TestApp::new().await;
    create_member_with(&app, "member@example.com").await;

    for guess in 0..30 {
        let wrong = attempt(
            &app,
            "198.51.100.13",
            &format!("target-{guess}@example.com"),
            "wrong-password",
        )
        .await;
        assert_ne!(wrong.status, StatusCode::TOO_MANY_REQUESTS);
        if guess % 3 == 0 {
            let own = attempt(&app, "198.51.100.13", "member@example.com", "password123").await;
            assert_ne!(own.status, StatusCode::TOO_MANY_REQUESTS);
        }
    }

    let limited = attempt(&app, "198.51.100.13", "fresh@example.com", "wrong-password").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    let elsewhere = attempt(&app, "198.51.100.14", "fresh@example.com", "wrong-password").await;
    assert_ne!(elsewhere.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn caps_a_parallel_burst_of_wrong_passwords_on_one_account() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let responses = futures::future::join_all(
        (0..20).map(|_| attempt(&app, "198.51.100.15", "admin@example.com", "wrong-password")),
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
async fn caps_a_parallel_burst_of_wrong_passwords_across_accounts() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let emails: Vec<String> = (0..40)
        .map(|guess| format!("target-{guess}@example.com"))
        .collect();
    let responses = futures::future::join_all(
        emails
            .iter()
            .map(|email| attempt(&app, "198.51.100.16", email, "wrong-password")),
    )
    .await;
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.status != StatusCode::TOO_MANY_REQUESTS)
            .count(),
        30
    );
}

#[tokio::test]
async fn counts_every_address_of_an_ipv6_64_as_one_client() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    for guess in 0..5 {
        let wrong = attempt(
            &app,
            &format!("2001:db8:12:34:{}::1", guess + 1),
            "admin@example.com",
            "wrong-password",
        )
        .await;
        assert_ne!(wrong.status, StatusCode::TOO_MANY_REQUESTS);
    }
    let same_network = attempt(
        &app,
        "2001:db8:12:34:ffff::9",
        "admin@example.com",
        "wrong-password",
    )
    .await;
    assert_eq!(same_network.status, StatusCode::TOO_MANY_REQUESTS);
    let other_network = attempt(
        &app,
        "2001:db8:12:35::1",
        "admin@example.com",
        "wrong-password",
    )
    .await;
    assert_ne!(other_network.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn counts_by_socket_address_when_the_forwarded_value_is_not_an_address() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    for guess in 0..5 {
        let wrong = attempt(
            &app,
            &format!("spoofed-{guess}"),
            "admin@example.com",
            "wrong-password",
        )
        .await;
        assert_ne!(wrong.status, StatusCode::TOO_MANY_REQUESTS);
    }
    let limited = attempt(&app, "spoofed-again", "admin@example.com", "wrong-password").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

// --- error pages ---

#[tokio::test]
async fn answers_unknown_addresses_with_a_page_or_with_json() {
    let app = TestApp::new().await;
    let page = app.get("/no/such/page").send().await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(
        page.text()
            .contains("<h1 class=\"auth-card__title\" id=\"error-title\">Page not found</h1>")
    );
    assert!(page.header("content-security-policy").is_some());

    let api = app.get("/no/such/page").api().send().await;
    assert_eq!(api.status, StatusCode::NOT_FOUND);
    assert_eq!(api.json(), json!({ "message": "Cannot GET:/no/such/page" }));
}
