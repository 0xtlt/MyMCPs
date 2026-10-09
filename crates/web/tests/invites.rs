//! Port of `tests/functional/invites.spec.ts`, of the invite and member
//! cases of `vine_route_params.spec.ts`, `hardening_auth_sessions.spec.ts`
//! and `public_url_config.spec.ts`, and of what `tests/browser/` checks of
//! the Team page that the server decides (`public_url_config.spec.ts`,
//! `date_formatting.spec.ts`).

use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{AccessToken, Invite, Mcp, User};
use mymcps_web::auth::{REMEMBER_COOKIE, SESSION_STAMP_KEY};
use mymcps_web::testing::factories::{
    create_admin, create_admin_with, create_invite, create_mcp, create_member, create_member_with,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::json;

const INVALID_INVITE: &str = "This invite is invalid or has expired";

/// Send the request as the page's script sends an async form.
fn from_script(request: TestRequest<'_>) -> TestRequest<'_> {
    request
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
}

async fn invite_by_email(app: &TestApp, email: &str) -> Option<Invite> {
    sqlx::query_as("select * from `invites` where `email` = ?")
        .bind(email)
        .fetch_optional(&*app.core.db)
        .await
        .unwrap()
}

async fn count(app: &TestApp, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

async fn create(app: &TestApp, admin: &User, email: &str) -> TestResponse {
    app.post("/invites")
        .login_as(admin)
        .csrf()
        .form(&[("email", email)])
        .send()
        .await
}

async fn accept(app: &TestApp, token: &str, name: &str, password: &str) -> TestResponse {
    app.post(&format!("/invite/{token}"))
        .csrf()
        .form(&[
            ("fullName", name),
            ("password", password),
            ("passwordConfirmation", password),
        ])
        .send()
        .await
}

fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

// --- invites and member administration ---

#[tokio::test]
async fn restricts_invite_administration_to_admins() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let invite = create_invite(&app, admin.id, |_| {}).await;

    let response = app.get("/invites").login_as(&member).send().await;
    assert_redirect(&response, "/");
    assert_eq!(
        response.flashed("error"),
        Some(json!("Admin access required"))
    );

    // Nor may a member change the team.
    for (method, path) in [
        ("post", "/invites".to_string()),
        ("delete", format!("/invites/{}", invite.id)),
        ("delete", format!("/members/{}", admin.id)),
    ] {
        let request = match method {
            "post" => app.post(&path).form(&[("email", "friend@example.com")]),
            _ => app.delete(&path),
        };
        let response = request.login_as(&member).csrf().send().await;
        assert_redirect(&response, "/");
        assert_eq!(
            response.flashed("error"),
            Some(json!("Admin access required")),
            "{method} {path}"
        );
    }
    assert_eq!(count(&app, "select count(*) from `invites`").await, 1);
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);

    // A visitor is sent to sign in.
    assert_redirect(&app.get("/invites").send().await, "/login");
}

#[tokio::test]
async fn creates_an_invite_and_rejects_duplicate_emails() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let created = create(&app, &admin, "new-member@example.com").await;
    assert_redirect(&created, "/invites");
    assert_eq!(created.flashed("success"), Some(json!("Invite created")));

    let invite = invite_by_email(&app, "new-member@example.com")
        .await
        .unwrap();
    assert_eq!(invite.created_by, admin.id);
    assert!(invite.is_usable());
    assert_eq!(invite.role.as_str(), "member");
    assert_eq!(invite.token.len(), 64);
    let valid_for = invite.expires_at - Timestamp::now();
    assert!((valid_for - chrono::Duration::days(7)).num_seconds().abs() < 5);

    let duplicate = create(&app, &admin, "new-member@example.com").await;
    assert_redirect(&duplicate, "/invites");
    assert_eq!(
        duplicate.flashed("error"),
        Some(json!("A pending invite already exists for that email"))
    );
    assert_eq!(count(&app, "select count(*) from `invites`").await, 1);
}

#[tokio::test]
async fn refuses_an_invite_for_someone_who_already_has_an_account() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    create_member_with(&app, "member@example.com").await;

    let response = create(&app, &admin, "member@example.com").await;
    assert_redirect(&response, "/invites");
    assert_eq!(
        response.flashed("error"),
        Some(json!("A user with that email already exists"))
    );
    assert_eq!(count(&app, "select count(*) from `invites`").await, 0);
}

#[tokio::test]
async fn invites_again_once_the_earlier_invite_expired_or_was_accepted() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    create_invite(&app, admin.id, |invite| {
        invite.email = "late@example.com".into();
        invite.expires_at = Timestamp::now() - chrono::Duration::minutes(1);
    })
    .await;
    create_invite(&app, admin.id, |invite| {
        invite.email = "gone@example.com".into();
        invite.accepted_at = Some(Timestamp::now());
    })
    .await;

    for email in ["late@example.com", "gone@example.com"] {
        let response = create(&app, &admin, email).await;
        assert_eq!(
            response.flashed("success"),
            Some(json!("Invite created")),
            "{email}"
        );
    }
    assert_eq!(count(&app, "select count(*) from `invites`").await, 4);
}

#[tokio::test]
async fn shows_the_link_of_a_new_invite_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let created = create(&app, &admin, "new-member@example.com").await;
    let invite = invite_by_email(&app, "new-member@example.com")
        .await
        .unwrap();
    let link = format!("http://localhost:3333/invite/{}", invite.token);

    let page = app
        .get("/invites")
        .session(created.session())
        .send()
        .await
        .text();
    assert!(page.contains("<dialog class=\"dialog dialog--sm\" id=\"invite-created\" aria-labelledby=\"invite-created-title\" data-open>"));
    assert!(page.contains("Send this one-time join link to new-member@example.com"));
    assert!(page.contains(&format!(
        "<span class=\"copy-field__value\" id=\"invite-link\">{link}</span>"
    )));
    // The dialog says it: no toast besides.
    assert!(!page.contains("<p class=\"toast__message\">Invite created</p>"));

    let later = app.get("/invites").login_as(&admin).send().await.text();
    assert!(!later.contains("id=\"invite-created\""));
    assert!(
        later.contains(&format!("data-copy=\"{link}\"")),
        "the link stays in the list"
    );
}

#[tokio::test]
async fn answers_the_create_invite_dialog_of_the_page_script() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let refused = from_script(app.post("/invites"))
        .login_as(&admin)
        .csrf()
        .form(&[("email", "not an email")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(form.starts_with("<form method=\"post\" action=\"/invites\" data-async>"));
    assert!(form.contains("name=\"_csrf\""));
    assert!(form.contains("value=\"not an email\""));
    assert!(form.contains("aria-invalid=\"true\" aria-describedby=\"invite-email-error\""));
    assert!(form.contains(
        "<p class=\"field__error\" id=\"invite-email-error\">The email field must be a valid email address</p>"
    ));
    assert!(
        !form.contains("autofocus"),
        "the invalid field takes the focus"
    );
    // Nothing is left for the next page: the script put the form in place.
    assert!(refused.flashed("errors").is_none() && refused.flashed("error").is_none());

    let created = from_script(app.post("/invites"))
        .login_as(&admin)
        .csrf()
        .form(&[("email", "new-member@example.com")])
        .send()
        .await;
    assert_eq!(created.status, StatusCode::NO_CONTENT);
    assert_eq!(created.header("x-location"), Some("/invites"));
    assert_eq!(created.flashed("success"), Some(json!("Invite created")));
    assert!(
        invite_by_email(&app, "new-member@example.com")
            .await
            .is_some()
    );
}

#[tokio::test]
async fn reopens_the_create_invite_dialog_after_a_refused_plain_post() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let refused = app
        .post("/invites")
        .login_as(&admin)
        .header("host", "localhost:3333")
        .header("referer", "http://localhost:3333/invites")
        .csrf()
        .form(&[("email", "")])
        .send()
        .await;
    assert_redirect(&refused, "/invites");
    assert_eq!(
        refused.flashed("errors"),
        Some(json!({ "email": "The email field must be defined" }))
    );
    assert_eq!(count(&app, "select count(*) from `invites`").await, 0);

    let page = app
        .get("/invites")
        .session(refused.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "id=\"create-invite\" aria-labelledby=\"create-invite-title\" data-dialog-reset data-open>"
    ));
    assert!(page.contains(
        "<p class=\"field__error\" id=\"invite-email-error\">The email field must be defined</p>"
    ));
    // Said once, under the field.
    assert!(!page.contains("<p class=\"toast__message\">The email field must be defined</p>"));

    let untouched = app.get("/invites").login_as(&admin).send().await.text();
    assert!(untouched.contains(
        "id=\"create-invite\" aria-labelledby=\"create-invite-title\" data-dialog-reset>"
    ));
    assert!(!untouched.contains("field__error"));
}

#[tokio::test]
async fn lists_members_and_invites() {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "admin@example.com").await;
    let member = create_member_with(&app, "member@example.com").await;
    sqlx::query("update `users` set `full_name` = null where `id` = ?")
        .bind(member.id)
        .execute(&*app.core.db)
        .await
        .unwrap();
    let pending = create_invite(&app, admin.id, |invite| {
        invite.email = "pending@example.com".into()
    })
    .await;
    let accepted = create_invite(&app, admin.id, |invite| {
        invite.email = "accepted@example.com".into();
        invite.accepted_at = Some(Timestamp::now());
    })
    .await;
    let expired = create_invite(&app, admin.id, |invite| {
        invite.email = "expired@example.com".into();
        invite.expires_at = Timestamp::now() - chrono::Duration::minutes(1);
    })
    .await;

    let response = app.get("/invites").login_as(&admin).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("<title>Team · MyMCPs</title>"));
    assert!(page.contains("<h1 class=\"page-header__title\">Team</h1>"));
    assert!(page.contains("<a class=\"nav-item\" href=\"/invites\" aria-current=\"page\">"));
    assert!(page.contains("2 people can sign in to this instance"));

    // The administrator, with their name; they cannot remove their own account.
    assert!(page.contains("<span class=\"cell-title\">Test User</span><span class=\"cell-sub cell-sub--secondary\">admin@example.com</span>"));
    assert!(page.contains("<span class=\"badge badge--info\">admin</span>"));
    assert!(page.contains("<td class=\"cell-end cell-tertiary\">You</td>"));
    assert!(!page.contains(&format!("action=\"/members/{}?_method=DELETE\"", admin.id)));
    // A member without a name goes by their email.
    assert!(page.contains("<span class=\"cell-title\">member@example.com</span></div>"));
    assert!(page.contains("<span class=\"badge\">member</span>"));
    assert!(page.contains(&format!(
        "action=\"/members/{}?_method=DELETE\" data-confirm=\"member@example.com will no longer be able to sign in.",
        member.id
    )));
    assert!(page.contains("data-confirm-title=\"Remove member@example.com?\""));
    assert!(page.contains(&format!(
        "<time datetime=\"{}\" data-format=\"minute\">",
        member.created_at.to_iso()
    )));

    // Each invite with its status; only a pending one has a link to copy.
    for (email, badge) in [
        (
            "pending@example.com",
            "<span class=\"badge badge--info\">Pending</span>",
        ),
        (
            "accepted@example.com",
            "<span class=\"badge badge--success\">Accepted</span>",
        ),
        (
            "expired@example.com",
            "<span class=\"badge\">Expired</span>",
        ),
    ] {
        assert!(
            page.contains(&format!("<td class=\"cell-strong\">{email}</td>")),
            "{email}"
        );
        assert!(page.contains(badge), "{badge}");
    }
    assert!(page.contains(&format!(
        "data-copy=\"http://localhost:3333/invite/{}\"",
        pending.token
    )));
    for other in [&accepted, &expired] {
        assert!(!page.contains(&other.token));
    }
    for invite in [&pending, &accepted, &expired] {
        assert!(page.contains(&format!("action=\"/invites/{}?_method=DELETE\"", invite.id)));
    }
    assert!(
        page.contains("data-confirm=\"The join link sent to pending@example.com stops working.\"")
    );

    // Every form of the page carries the CSRF token.
    assert_eq!(
        page.matches("<form ").count(),
        page.matches("name=\"_csrf\"").count()
    );
}

#[tokio::test]
async fn renders_dates_day_first() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let displayed_at = Timestamp::parse_sql("2035-09-06 11:13:52").unwrap();
    let member = create_member(&app).await;
    sqlx::query("update `users` set `full_name` = 'Date Member', `created_at` = ? where `id` = ?")
        .bind(displayed_at)
        .bind(member.id)
        .execute(&*app.core.db)
        .await
        .unwrap();
    create_invite(&app, admin.id, |invite| {
        invite.email = "day-first@example.com".into();
        invite.expires_at = displayed_at;
    })
    .await;

    let page = app.get("/invites").login_as(&admin).send().await.text();
    // To the minute in the tables, the day alone in the lists of a phone.
    // The script rewrites both in the viewer's time zone.
    let to_the_minute = "<td class=\"cell-secondary\"><time datetime=\"2035-09-06T11:13:52.000Z\" data-format=\"minute\">06/09/2035, 11:13</time></td>";
    assert_eq!(page.matches(to_the_minute).count(), 2);
    assert!(page.contains(
        "Expires <time datetime=\"2035-09-06T11:13:52.000Z\" data-format=\"date\">06/09/2035</time>"
    ));
}

#[tokio::test]
async fn says_so_when_there_is_no_invite() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let page = app.get("/invites").login_as(&admin).send().await.text();

    assert!(page.contains("1 person can sign in to this instance"));
    assert!(page.contains("<p class=\"empty-state__title\">No invites yet</p>"));
    assert!(page.contains("Create an invite link to add a teammate."));
    assert!(page.contains("data-dialog-open=\"#create-invite\""));
}

#[tokio::test]
async fn removes_an_invite() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |_| {}).await;

    // As a browser sends it: a POST that names the method.
    let response = app
        .post(&format!("/invites/{}?_method=DELETE", invite.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some("/invites"));
    assert_eq!(response.flashed("success"), Some(json!("Invite removed")));
    assert!(
        Invite::find(&*app.core.db, invite.id)
            .await
            .unwrap()
            .is_none()
    );

    let again = app
        .delete(&format!("/invites/{}", invite.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&again, "/invites");
    assert_eq!(again.flashed("error"), Some(json!("Invite not found")));
}

#[tokio::test]
async fn accepts_a_usable_invite_and_signs_in_the_new_member() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "invited@example.com".into()
    })
    .await;

    let response = accept(&app, &invite.token, " Invited Member ", "password123").await;
    assert_redirect(&response, "/");
    assert_eq!(
        response.flashed("success"),
        Some(json!("Welcome to MyMCPs"))
    );

    let member = User::find_by_email(&*app.core.db, "invited@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member.full_name.as_deref(), Some("Invited Member"));
    assert_eq!(member.role.as_str(), "member");
    assert!(member.verify_password("password123").await.unwrap());
    assert!(response.cookie(REMEMBER_COOKIE).is_some());
    let remembered: i64 =
        sqlx::query_scalar("select count(*) from `remember_me_tokens` where `tokenable_id` = ?")
            .bind(member.id)
            .fetch_one(&*app.core.db)
            .await
            .unwrap();
    assert_eq!(remembered, 1);
    let accepted = Invite::find(&*app.core.db, invite.id)
        .await
        .unwrap()
        .unwrap();
    assert!(accepted.is_accepted());

    // The link worked once.
    let again = accept(&app, &invite.token, "Someone Else", "password123").await;
    assert_redirect(&again, "/");
    assert_eq!(again.flashed("error"), Some(json!(INVALID_INVITE)));
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
}

#[tokio::test]
async fn stamps_the_session_created_by_accepting_an_invite() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "invited@example.com".into()
    })
    .await;

    let response = accept(&app, &invite.token, "Invited Member", "password123").await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert!(response.session().contains_key(SESSION_STAMP_KEY));

    let home = app.get("/").session(response.session()).send().await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(home.text().contains("Welcome to MyMCPs"));
}

#[tokio::test]
async fn rejects_expired_invites() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.expires_at = Timestamp::now() - chrono::Duration::minutes(1);
    })
    .await;

    let response = app.get(&format!("/invite/{}", invite.token)).send().await;
    assert_redirect(&response, "/");
    assert_eq!(response.flashed("error"), Some(json!(INVALID_INVITE)));

    let posted = accept(&app, &invite.token, "Late Comer", "password123").await;
    assert_redirect(&posted, "/");
    assert_eq!(posted.flashed("error"), Some(json!(INVALID_INVITE)));
    assert_eq!(count(&app, "select count(*) from `users`").await, 1);
}

#[tokio::test]
async fn shows_the_form_behind_an_invite_link() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "invited@example.com".into()
    })
    .await;
    let path = format!("/invite/{}", invite.token);

    let response = app.get(&path).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("<title>Join MyMCPs · MyMCPs</title>"));
    assert!(page.contains("<h1 class=\"auth-card__title\" id=\"invite-title\">Join MyMCPs</h1>"));
    assert!(page.contains("You were invited as invited@example.com"));
    assert!(page.contains(&format!(
        "<form class=\"form\" method=\"post\" action=\"{path}\">"
    )));
    assert!(page.contains("name=\"_csrf\""));
    for field in ["fullName", "password", "passwordConfirmation"] {
        assert!(page.contains(&format!("name=\"{field}\"")), "{field}");
    }
    // The email comes from the invite.
    assert!(!page.contains("name=\"email\""));

    // Someone who is signed in has no use for an invite.
    assert_redirect(&app.get(&path).login_as(&admin).send().await, "/");
}

#[tokio::test]
async fn sends_an_invite_form_back_with_what_is_wrong() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "invited@example.com".into()
    })
    .await;
    let path = format!("/invite/{}", invite.token);

    let response = app
        .post(&path)
        .csrf()
        .form(&[
            ("fullName", "Invited Member"),
            ("password", "short"),
            ("passwordConfirmation", "different"),
        ])
        .send()
        .await;
    assert_redirect(&response, &path);
    assert!(response.cookie(REMEMBER_COOKIE).is_none());

    let page = app
        .get(&path)
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "<p class=\"banner__title\">The password field must have at least 8 characters</p>"
    ));
    assert!(page.contains(
        "<p class=\"field__error\" id=\"password-error\">The password field must have at least 8 characters</p>"
    ));
    assert!(page.contains("value=\"Invited Member\""));
    assert!(!page.contains("short") && !page.contains("different"));
    // The field to correct takes the focus.
    assert!(page.contains(
        "id=\"password\" name=\"password\" type=\"password\" autocomplete=\"new-password\" autofocus aria-invalid=\"true\""
    ));
    assert_eq!(page.matches(" autofocus").count(), 1);
    assert_eq!(count(&app, "select count(*) from `users`").await, 1);
    let untouched = Invite::find(&*app.core.db, invite.id)
        .await
        .unwrap()
        .unwrap();
    assert!(untouched.is_usable());
}

#[tokio::test]
async fn an_invite_for_an_email_that_got_an_account_meanwhile_is_used_up() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "taken@example.com".into()
    })
    .await;
    let existing = create_member_with(&app, "taken@example.com").await;

    let response = accept(&app, &invite.token, "Second Account", "another-password").await;
    assert_redirect(&response, "/login");
    assert_eq!(
        response.flashed("error"),
        Some(json!("A user with that email already exists"))
    );
    assert!(response.cookie(REMEMBER_COOKIE).is_none());
    assert!(!response.session().contains_key("auth_web"));

    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
    let stored = User::find(&*app.core.db, existing.id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.verify_password("password123").await.unwrap());
    let used = Invite::find(&*app.core.db, invite.id)
        .await
        .unwrap()
        .unwrap();
    assert!(used.is_accepted());

    // The sign-in page says why.
    let page = app
        .get("/login")
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">A user with that email already exists</p>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_accepting_the_same_link_at_once_do_not_both_get_an_account() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |invite| {
        invite.email = "invited@example.com".into()
    })
    .await;

    let responses = futures::future::join_all(
        (0..6).map(|_| accept(&app, &invite.token, "Invited Member", "password123")),
    )
    .await;

    let welcomed = responses
        .iter()
        .filter(|response| response.flashed("success") == Some(json!("Welcome to MyMCPs")))
        .count();
    let refused = responses
        .iter()
        .filter(|response| response.flashed("error") == Some(json!(INVALID_INVITE)))
        .count();
    assert_eq!((welcomed, refused), (1, 5));
    assert!(
        responses
            .iter()
            .all(|response| response.status == StatusCode::FOUND)
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.cookie(REMEMBER_COOKIE).is_some())
            .count(),
        1
    );
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
}

#[tokio::test]
async fn reassigns_owned_records_and_revokes_tokens_before_removing_a_member() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let mcp = create_mcp(&app, member.id, |_| {}).await;
    let mut token = AccessToken {
        name: "Member token".into(),
        token_prefix: "mmcp_test".into(),
        token_hash: mymcps_core::crypto::sha256_hex("member token"),
        created_by: member.id,
        ..Default::default()
    };
    token.insert(&*app.core.db).await.unwrap();
    let mut revoked_earlier = AccessToken {
        name: "Revoked earlier".into(),
        token_prefix: "mmcp_old".into(),
        token_hash: mymcps_core::crypto::sha256_hex("revoked earlier"),
        created_by: member.id,
        revoked_at: Some(Timestamp::now() - chrono::Duration::days(3)),
        ..Default::default()
    };
    revoked_earlier.insert(&*app.core.db).await.unwrap();
    let invite = create_invite(&app, member.id, |_| {}).await;

    let response = app
        .delete(&format!("/members/{}", member.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&response, "/invites");
    assert_eq!(response.flashed("success"), Some(json!("Member removed")));

    let db = &*app.core.db;
    assert!(User::find(db, member.id).await.unwrap().is_none());
    let reassigned_mcp = Mcp::find(db, mcp.id).await.unwrap().unwrap();
    let reassigned_token = AccessToken::find(db, token.id).await.unwrap().unwrap();
    let reassigned_invite = Invite::find(db, invite.id).await.unwrap().unwrap();
    assert_eq!(reassigned_mcp.created_by, admin.id);
    assert_eq!(reassigned_token.created_by, admin.id);
    assert!(reassigned_token.is_revoked());
    assert_eq!(reassigned_invite.created_by, admin.id);
    // A token revoked before keeps the date it was revoked on.
    let earlier = AccessToken::find(db, revoked_earlier.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(earlier.created_by, admin.id);
    assert_eq!(earlier.revoked_at, revoked_earlier.revoked_at);
}

#[tokio::test]
async fn prevents_an_admin_from_removing_their_own_account() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app
        .delete(&format!("/members/{}", admin.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&response, "/invites");
    assert_eq!(
        response.flashed("error"),
        Some(json!("You cannot remove your own account"))
    );
    assert!(User::find(&*app.core.db, admin.id).await.unwrap().is_some());
}

#[tokio::test]
async fn an_admin_can_remove_another_admin() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let other = create_admin(&app).await;

    let response = app
        .post(&format!("/members/{}?_method=DELETE", other.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(response.location(), Some("/invites"));
    assert_eq!(response.flashed("success"), Some(json!("Member removed")));
    assert!(User::find(&*app.core.db, other.id).await.unwrap().is_none());

    // The session of the removed account is worth nothing.
    assert_redirect(&app.get("/").login_as(&other).send().await, "/login");
}

// --- route params: lookups ---

#[tokio::test]
async fn an_id_that_is_not_a_number_is_answered_like_a_missing_record() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for (path, message) in [
        ("/invites/abc", "Invite not found"),
        ("/invites/1.5", "Invite not found"),
        ("/invites/999999", "Invite not found"),
        ("/members/abc", "Member not found"),
        ("/members/-3", "Member not found"),
        ("/members/999999", "Member not found"),
    ] {
        let response = app.delete(path).login_as(&admin).csrf().send().await;
        assert_redirect(&response, "/invites");
        assert_eq!(response.flashed("error"), Some(json!(message)), "{path}");
    }
}

#[tokio::test]
async fn an_invite_link_with_a_malformed_token_is_refused_like_an_unknown_one() {
    let app = TestApp::new().await;
    create_admin(&app).await;
    let unknown = Invite::generate_token();
    for token in ["not-a-token", unknown.as_str()] {
        let response = app.get(&format!("/invite/{token}")).send().await;
        assert_redirect(&response, "/");
        assert_eq!(
            response.flashed("error"),
            Some(json!(INVALID_INVITE)),
            "{token}"
        );

        let posted = accept(&app, token, "Nobody", "password123").await;
        assert_redirect(&posted, "/");
        assert_eq!(posted.flashed("error"), Some(json!(INVALID_INVITE)));
    }
    assert_eq!(count(&app, "select count(*) from `users`").await, 1);
}

#[tokio::test]
async fn an_invite_link_with_its_token_still_opens() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |_| {}).await;

    let response = app.get(&format!("/invite/{}", invite.token)).send().await;
    assert_eq!(response.status, StatusCode::OK);
}

// --- missing public app URL ---

#[tokio::test]
async fn shows_the_warning_and_omits_invite_links_without_a_public_url() {
    let app = TestApp::with_config(|config| config.app_url = None).await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |_| {}).await;

    let response = app.get("/invites").login_as(&admin).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("<p class=\"banner__title\">Set APP_URL to enable public links</p>"));
    assert!(page.contains(
        "<button type=\"button\" class=\"button\" disabled title=\"Set APP_URL to enable public links\">Copy link</button>"
    ));
    assert!(!page.contains("data-copy="));
    assert!(!page.contains(&invite.token));

    // A new invite has no link to show either: the toast says it exists.
    let created = create(&app, &admin, "new-member@example.com").await;
    assert_eq!(created.flashed("success"), Some(json!("Invite created")));
    let page = app
        .get("/invites")
        .session(created.session())
        .send()
        .await
        .text();
    assert!(!page.contains("id=\"invite-created\""));
    assert!(page.contains("<p class=\"toast__message\">Invite created</p>"));
}

#[tokio::test]
async fn builds_invite_links_from_the_public_url_without_its_trailing_slash() {
    let app =
        TestApp::with_config(|config| config.app_url = Some("https://mcp.example.com/".into()))
            .await;
    let admin = create_admin(&app).await;
    let invite = create_invite(&app, admin.id, |_| {}).await;

    let page = app.get("/invites").login_as(&admin).send().await.text();
    assert!(page.contains(&format!(
        "data-copy=\"https://mcp.example.com/invite/{}\"",
        invite.token
    )));
}
