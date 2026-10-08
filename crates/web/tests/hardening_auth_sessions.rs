//! Port of the session lifetime and revocation cases of
//! `tests/functional/hardening_auth_sessions.spec.ts` that do not need a
//! form: what a replayed session cookie and a remember-me cookie are worth.

use http::StatusCode;
use mymcps_core::models::{User, UserRole};
use mymcps_web::auth::{self, REMEMBER_COOKIE, SESSION_STAMP_KEY, SessionStamp};
use mymcps_web::cookies::Cookies;
use mymcps_web::session::Session;
use mymcps_web::testing::{TestApp, TestResponse, session_for};
use serde_json::{Map, Value, json};

const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn create_admin(app: &TestApp) -> User {
    let mut user = User::with_password(
        "admin@example.com",
        Some("Admin"),
        "password123",
        UserRole::Admin,
    )
    .await
    .unwrap();
    user.insert(&*app.core.db).await.unwrap();
    user
}

fn stamped(user: &User, authenticated_at: i64, last_seen_at: i64) -> Map<String, Value> {
    let mut session = session_for(user);
    session.insert(
        SESSION_STAMP_KEY.into(),
        json!({ "version": user.session_version, "authenticatedAt": authenticated_at, "lastSeenAt": last_seen_at }),
    );
    session
}

/// The cookie store keeps the whole session in the cookie, so presenting the
/// contents of a session again is what replaying a copied cookie amounts to.
async fn replay(
    app: &TestApp,
    session: &Map<String, Value>,
    remember: Option<&str>,
) -> TestResponse {
    let request = app.get("/").session(session.clone());
    match remember {
        Some(cookie) => request.cookie(REMEMBER_COOKIE, cookie).send().await,
        None => request.send().await,
    }
}

async fn assert_accepted(
    app: &TestApp,
    session: &Map<String, Value>,
    remember: Option<&str>,
) -> TestResponse {
    let response = replay(app, session, remember).await;
    assert_eq!(response.status, StatusCode::OK);
    response
}

async fn assert_rejected(
    app: &TestApp,
    session: &Map<String, Value>,
    remember: Option<&str>,
) -> TestResponse {
    let response = replay(app, session, remember).await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/login"));
    response
}

/// What signing in leaves in the browser: the session and the remember-me cookie.
async fn sign_in(app: &TestApp, user: &User) -> (Map<String, Value>, String) {
    let session = Session::default();
    let cookies = Cookies::default();
    auth::sign_in(&app.core, &session, &cookies, user)
        .await
        .unwrap();
    let response = app.get("/").session(session.snapshot()).send().await;
    assert_eq!(response.status, StatusCode::OK);

    let remember = cookies
        .pending(REMEMBER_COOKIE)
        .expect("signing in sets a remember-me cookie");
    (session.snapshot(), remember)
}

#[tokio::test]
async fn rejects_a_session_that_carries_no_valid_stamp() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let mut missing = Map::new();
    missing.insert("auth_web".into(), json!(admin.id));
    assert_rejected(&app, &missing, None).await;

    let mut malformed = missing.clone();
    malformed.insert(
        SESSION_STAMP_KEY.into(),
        json!({ "version": admin.session_version, "authenticatedAt": "now" }),
    );
    assert_rejected(&app, &malformed, None).await;

    for not_a_stamp in [
        json!({ "version": 1, "authenticatedAt": now(), "lastSeenAt": now().to_string() }),
        json!({ "version": 1.5, "authenticatedAt": now(), "lastSeenAt": now() }),
        json!({ "version": 0, "authenticatedAt": now(), "lastSeenAt": now() }),
        json!("stamp"),
    ] {
        let mut session = missing.clone();
        session.insert(SESSION_STAMP_KEY.into(), not_a_stamp);
        assert_rejected(&app, &session, None).await;
    }
}

#[tokio::test]
async fn rejects_a_session_stamped_under_an_earlier_session_version() {
    let app = TestApp::new().await;
    let mut admin = create_admin(&app).await;
    let session = session_for(&admin);
    assert_accepted(&app, &session, None).await;

    admin.invalidate_sessions(&*app.core.db).await.unwrap();

    let response = assert_rejected(&app, &session, None).await;
    assert!(!response.session().contains_key("auth_web"));
    assert!(!response.session().contains_key(SESSION_STAMP_KEY));
}

#[tokio::test]
async fn accepts_a_session_until_it_sat_idle_for_the_session_age() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for (idle_for, status) in [
        (2 * HOUR - MINUTE, StatusCode::OK),
        (2 * HOUR + MINUTE, StatusCode::FOUND),
    ] {
        let response = replay(
            &app,
            &stamped(&admin, now() - 3 * HOUR, now() - idle_for),
            None,
        )
        .await;
        assert_eq!(response.status, status);
    }
}

#[tokio::test]
async fn accepts_an_active_session_until_its_absolute_lifetime_ran_out() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for (age, status) in [
        (24 * HOUR - MINUTE, StatusCode::OK),
        (24 * HOUR + MINUTE, StatusCode::FOUND),
    ] {
        let response = replay(&app, &stamped(&admin, now() - age, now()), None).await;
        assert_eq!(response.status, status);
    }
}

#[tokio::test]
async fn refreshes_last_seen_while_a_session_is_in_use() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let authenticated_at = now() - 3 * HOUR;

    let response = assert_accepted(
        &app,
        &stamped(&admin, authenticated_at, now() - 90 * MINUTE),
        None,
    )
    .await;

    let stamp: SessionStamp =
        serde_json::from_value(response.session()[SESSION_STAMP_KEY].clone()).unwrap();
    assert_eq!(stamp.authenticated_at, authenticated_at);
    assert!((stamp.last_seen_at - now()).abs() < 5_000);
}

#[tokio::test]
async fn the_remember_me_cookie_replaces_a_session_that_cannot_authenticate() {
    for state in [
        "absent",
        "unstamped",
        "idle",
        "past its lifetime",
        "retired",
    ] {
        let app = TestApp::new().await;
        let mut admin = create_admin(&app).await;
        let (_, remember) = sign_in(&app, &admin).await;

        let session = match state {
            "absent" => Map::new(),
            "unstamped" => {
                let mut session = Map::new();
                session.insert("auth_web".into(), json!(admin.id));
                session
            }
            "idle" => stamped(&admin, now() - 4 * HOUR, now() - 3 * HOUR),
            "past its lifetime" => stamped(&admin, now() - 25 * HOUR, now() - 25 * HOUR),
            _ => session_for(&admin),
        };
        if state == "retired" {
            admin.invalidate_sessions(&*app.core.db).await.unwrap();
        }

        let response = assert_accepted(&app, &session, Some(&remember)).await;

        assert_eq!(response.session()["auth_web"], json!(admin.id), "{state}");
        let stamp: SessionStamp =
            serde_json::from_value(response.session()[SESSION_STAMP_KEY].clone()).unwrap();
        assert_eq!(stamp.version, admin.session_version, "{state}");
        assert!((stamp.authenticated_at - now()).abs() < 5_000, "{state}");
        assert_accepted(&app, &response.session(), None).await;

        // The token was used once: the browser got another, and the old one is gone.
        let replaced = response
            .cookie(REMEMBER_COOKIE)
            .expect("a new remember-me cookie");
        assert_ne!(replaced, remember);
        assert_rejected(&app, &Map::new(), Some(&remember)).await;
        assert_accepted(&app, &Map::new(), Some(&replaced)).await;
    }
}

#[tokio::test]
async fn other_browsers_of_the_user_carry_on_after_one_of_them_signs_out() {
    let app = TestApp::new().await;
    let mut admin = create_admin(&app).await;
    let (first_session, first_remember) = sign_in(&app, &admin).await;
    let (second_session, second_remember) = sign_in(&app, &admin).await;

    // What `POST /logout` does.
    let session = Session::from_values(first_session.clone());
    let header = format!(
        "{REMEMBER_COOKIE}={}",
        percent_encoding::utf8_percent_encode(&first_remember, percent_encoding::NON_ALPHANUMERIC)
    );
    let cookies = Cookies::parse(Some(&header), false);
    admin.invalidate_sessions(&*app.core.db).await.unwrap();
    auth::sign_out(&app.core, &session, &cookies).await.unwrap();
    assert!(!session.has("auth_web") && !session.has(SESSION_STAMP_KEY));

    assert_rejected(&app, &first_session, Some(&first_remember)).await;
    assert_rejected(&app, &second_session, None).await;
    assert_accepted(&app, &second_session, Some(&second_remember)).await;
}

#[tokio::test]
async fn a_remember_me_cookie_revoked_by_a_password_change_does_not_rescue_a_retired_session() {
    let app = TestApp::new().await;
    let mut admin = create_admin(&app).await;
    let (session, remember) = sign_in(&app, &admin).await;

    admin
        .change_password(&app.core.db, "new-password123")
        .await
        .unwrap();

    assert_rejected(&app, &session, Some(&remember)).await;
}

#[tokio::test]
async fn a_remember_me_cookie_of_the_node_app_still_signs_in() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    // What the AdonisJS session guard stored and set: the hash of a random
    // secret, dates as epoch milliseconds, and `e:` followed by the token
    // sealed for the cookie's name.
    let secret = "s3cr3t-of-forty-characters-from-node-app";
    let id: i64 = sqlx::query_scalar(
        "insert into `remember_me_tokens` (`tokenable_id`, `hash`, `created_at`, `updated_at`, `expires_at`) values (?, ?, ?, ?, ?) returning `id`",
    )
    .bind(admin.id)
    .bind(mymcps_core::crypto::sha256_hex(secret))
    .bind(now())
    .bind(now())
    .bind(now() + 365 * 24 * HOUR)
    .fetch_one(&*app.core.db)
    .await
    .unwrap();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let token = format!("{}.{}", b64.encode(id.to_string()), b64.encode(secret));
    let cookie = format!(
        "e:{}",
        app.core
            .encryption
            .encrypt_value(&json!(token), Some(REMEMBER_COOKIE), None)
    );

    let response = assert_accepted(&app, &Map::new(), Some(&cookie)).await;
    assert_eq!(response.session()["auth_web"], json!(admin.id));

    // An expired token, a wrong secret and a cookie sealed for something else do not.
    let wrong = format!(
        "{}.{}",
        b64.encode(id.to_string()),
        b64.encode("another-secret")
    );
    for refused in [
        format!(
            "e:{}",
            app.core
                .encryption
                .encrypt_value(&json!(wrong), Some(REMEMBER_COOKIE), None)
        ),
        format!(
            "e:{}",
            app.core
                .encryption
                .encrypt_value(&json!(token), Some("session"), None)
        ),
        token.clone(),
        "e:".to_string(),
    ] {
        assert_rejected(&app, &Map::new(), Some(&refused)).await;
    }
}

#[tokio::test]
async fn sends_everyone_to_onboarding_until_the_first_admin_exists() {
    let app = TestApp::new().await;
    let response = app.get("/").send().await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some("/onboarding"));

    let health = app.get("/health").send().await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.json(), json!({ "status": "ok" }));
    assert_eq!(health.header("x-frame-options"), Some("DENY"));
    assert_eq!(health.header("x-content-type-options"), Some("nosniff"));
    assert!(
        health
            .header("content-security-policy")
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert!(
        health.cookie("mymcps_session").is_none(),
        "no session for a health check"
    );
}
