//! Sign-in protection: an authenticator app, passkeys, and the recovery
//! codes that stand in for either. An account that has one of them is not
//! signed in by its password alone.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::crypto::totp;
use mymcps_core::crypto::webauthn::RelyingParty;
use mymcps_core::crypto::webauthn::testing::SoftwareAuthenticator;
use mymcps_core::models::{Passkey, RecoveryCode, Team, TeamRole, User};
use mymcps_web::testing::factories::{
    PASSWORD, create_account, create_member_with, create_super_admin, create_team, join_team,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::{Map, Value, json};

/// Where the test instance says it is: what a passkey is made for.
const ORIGIN: &str = "http://localhost:3333";

fn rp() -> RelyingParty {
    RelyingParty::from_url(ORIGIN).unwrap()
}

fn now() -> i64 {
    Timestamp::now().timestamp_millis() / 1000
}

fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

fn flash(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

fn from_script(request: TestRequest<'_>) -> TestRequest<'_> {
    request.header("x-requested-with", "fetch")
}

async fn reload(app: &TestApp, user: &User) -> User {
    User::find(&*app.core.db, user.id).await.unwrap().unwrap()
}

/// An account in a team of its own, as one that signed up.
async fn account(app: &TestApp, email: &str) -> User {
    let user = create_account(app, Some(email)).await;
    let team = create_team(app, "Their Team").await;
    join_team(app, team.id, &user, TeamRole::Owner).await;
    user
}

/// Turn the authenticator app of an account on, as the setup page does,
/// with its first recovery codes. The secret and the codes are returned.
async fn protect(app: &TestApp, user: &User) -> (String, Vec<String>) {
    let secret = totp::generate_secret();
    let mut user = reload(app, user).await;
    let mut transaction = app.core.db.begin().await.unwrap();
    user.enable_authenticator(&mut transaction, &app.core.encryption, &secret, 0)
        .await
        .unwrap();
    let codes = RecoveryCode::replace_all(&mut transaction, user.id)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    (secret, codes)
}

/// The code the app shows now, or the one it shows `steps` steps from now.
fn code(secret: &str, steps: i64) -> String {
    totp::current_code(secret, now() + steps * totp::STEP_SECONDS).unwrap()
}

/// Type the password of an account: the session the browser has after it.
async fn type_password(app: &TestApp, user: &User) -> TestResponse {
    app.post("/login")
        .csrf()
        .form(&[("email", &user.email), ("password", PASSWORD)])
        .send()
        .await
}

async fn answer_second_step(
    app: &TestApp,
    session: Map<String, Value>,
    code: &str,
) -> TestResponse {
    app.post("/login/two-factor")
        .session(session)
        .csrf()
        .form(&[("code", code)])
        .send()
        .await
}

/// Whether the browser that holds this session is signed in.
async fn signed_in(app: &TestApp, session: Map<String, Value>) -> bool {
    app.get("/").session(session).send().await.status == StatusCode::OK
}

// --------------------------------------------------------- authenticator app

#[tokio::test]
async fn an_authenticator_app_is_turned_on_with_a_code_and_the_password() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;

    // Nothing is asked for besides the password until it is turned on.
    let settings = app.get("/settings").login_as(&user).send().await.text();
    assert!(
        settings.contains(
            "<h2 class=\"section__title\" id=\"protection-title\">Sign-in protection</h2>"
        )
    );
    assert!(settings.contains(
        "<a class=\"button button--secondary\" href=\"/settings/two-factor\">Set up</a>"
    ));
    assert!(!settings.contains("Recovery codes"));
    assert!(!settings.contains("authenticator-off"));

    // The setup page shows a secret, as a QR code the server draws and as
    // text, and keeps it in the session until a code proves the app has it.
    let shown = app.get("/settings/two-factor").login_as(&user).send().await;
    assert_eq!(shown.status, StatusCode::OK);
    assert_eq!(shown.header("cache-control"), Some("no-store"));
    let page = shown.text();
    assert!(page.contains("<svg class=\"qr\" viewBox=\"0 0 "));
    assert!(page.contains("<form method=\"post\" action=\"/settings/two-factor\">"));
    let secret: String = page
        .split("id=\"authenticator-secret\">")
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .expect("the key of the app")
        .replace(' ', "");
    assert_eq!(secret.len(), 32);
    assert!(page.contains(&format!("data-copy=\"{secret}\"")));
    assert!(page.contains("id=\"authenticator-code\" name=\"code\" type=\"text\" inputmode=\"numeric\" autocomplete=\"one-time-code\""));
    // The same secret as long as it is being set up.
    let again = app
        .get("/settings/two-factor")
        .session(shown.session())
        .send()
        .await
        .text();
    assert!(again.contains(&format!("data-copy=\"{secret}\"")));
    assert!(!reload(&app, &user).await.has_authenticator());

    let enable = async |code: &str, password: &str| {
        app.post("/settings/two-factor")
            .session(shown.session())
            .csrf()
            .form(&[("code", code), ("currentPassword", password)])
            .send()
            .await
    };
    // The right code is not enough without the password, nor the reverse.
    let refused = enable(&code(&secret, 0), "not-the-password").await;
    assert_redirect(&refused, "/settings/two-factor");
    assert_eq!(
        refused.flashed("errors"),
        Some(json!({ "currentPassword": "The current password is incorrect" }))
    );
    let refused = enable("000000", PASSWORD).await;
    assert_redirect(&refused, "/settings/two-factor");
    assert_eq!(
        refused.flashed("errors"),
        Some(json!({ "code": "This is not the code the app shows now" }))
    );
    let reopened = app
        .get("/settings/two-factor")
        .session(refused.session())
        .send()
        .await
        .text();
    assert!(reopened.contains("<p class=\"field__error\" id=\"authenticator-code-error\">This is not the code the app shows now</p>"));
    assert!(reopened.contains(&format!("data-copy=\"{secret}\"")));
    assert!(!reload(&app, &user).await.has_authenticator());

    let enabled = enable(&code(&secret, 0), PASSWORD).await;
    assert_redirect(&enabled, "/settings/recovery-codes");
    assert_eq!(
        flash(&enabled, "success").as_deref(),
        Some("Authenticator app turned on")
    );
    let stored = reload(&app, &user).await;
    assert!(stored.has_authenticator());
    // The secret is kept encrypted, and the code that proved it is spent.
    let ciphertext = stored.two_factor_secret.clone().unwrap();
    assert!(!ciphertext.contains(&secret));
    assert_eq!(
        app.core.encryption.decrypt(&ciphertext).as_deref(),
        Some(secret.as_str())
    );
    assert!(
        !stored
            .accept_authenticator_code(
                &*app.core.db,
                &app.core.encryption,
                &code(&secret, 0),
                now()
            )
            .await
            .unwrap()
    );

    // The first recovery codes come with it, and are shown once.
    let codes: Vec<String> =
        serde_json::from_value(enabled.flashed("recoveryCodes").unwrap()).unwrap();
    assert_eq!(codes.len(), 8);
    let page = app
        .get("/settings/recovery-codes")
        .session(enabled.session())
        .send()
        .await;
    assert_eq!(page.header("cache-control"), Some("no-store"));
    let page = page.text();
    assert!(page.contains(&format!(
        "<pre class=\"code-block__body\" id=\"recovery-codes\">{}</pre>",
        codes.join("\n")
    )));
    let later = app
        .get("/settings/recovery-codes")
        .login_as(&user)
        .send()
        .await
        .text();
    assert!(!later.contains(&codes[0]));
    assert!(later.contains("8 of 8 codes are left."));
    assert!(later.contains("<form method=\"post\" action=\"/settings/recovery-codes\">"));

    let settings = app.get("/settings").login_as(&user).send().await.text();
    assert!(settings.contains("On since "));
    assert!(settings.contains("data-dialog-open=\"#authenticator-off\">Turn off</button>"));
    assert!(settings.contains("8 of 8 left."));
    assert!(!settings.contains("href=\"/settings/two-factor\""));
    // It is set up once.
    let again = app.get("/settings/two-factor").login_as(&user).send().await;
    assert_redirect(&again, "/settings");
}

#[tokio::test]
async fn the_authenticator_app_is_turned_off_with_the_password() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    protect(&app, &user).await;
    let turn_off = async |request: TestRequest<'_>, password: &str| {
        request
            .login_as(&user)
            .csrf()
            .form(&[("currentPassword", password)])
            .send()
            .await
    };
    let path = "/settings/two-factor?_method=DELETE";

    // A signed-in browser is no proof that it is them.
    let refused = turn_off(app.post(path), "not-the-password").await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("The current password is incorrect")
    );
    let reopened = app
        .get("/settings")
        .session(refused.session())
        .send()
        .await
        .text();
    assert!(reopened.contains("id=\"authenticator-off\" aria-labelledby=\"authenticator-off-title\" data-dialog-reset data-open>"));
    assert!(reopened.contains("The current password is incorrect</p>"));
    let scripted = turn_off(from_script(app.post(path)), "not-the-password").await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(scripted.text().starts_with(
        "<form method=\"post\" action=\"/settings/two-factor?_method=DELETE\" data-async>"
    ));
    assert!(reload(&app, &user).await.has_authenticator());

    let off = turn_off(app.post(path), PASSWORD).await;
    assert_redirect(&off, "/settings");
    assert_eq!(
        flash(&off, "success").as_deref(),
        Some("Authenticator app turned off")
    );
    let stored = reload(&app, &user).await;
    assert!(!stored.has_authenticator() && stored.two_factor_secret.is_none());
    // The recovery codes stood in for it alone: they go with it.
    assert_eq!(
        RecoveryCode::remaining(&*app.core.db, user.id)
            .await
            .unwrap(),
        0
    );
    // The password signs in alone again.
    let session = type_password(&app, &user).await;
    assert_redirect(&session, "/");
    assert!(signed_in(&app, session.session()).await);
    let codes = app
        .get("/settings/recovery-codes")
        .login_as(&user)
        .send()
        .await;
    assert_redirect(&codes, "/settings");
}

#[tokio::test]
async fn new_recovery_codes_replace_the_old_ones() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (_, old) = protect(&app, &user).await;
    let db = &*app.core.db;
    assert!(RecoveryCode::consume(db, user.id, &old[0]).await.unwrap());
    let renew = async |password: &str| {
        app.post("/settings/recovery-codes")
            .login_as(&user)
            .csrf()
            .form(&[("currentPassword", password)])
            .send()
            .await
    };

    let refused = renew("not-the-password").await;
    assert_redirect(&refused, "/settings/recovery-codes");
    assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 7);

    let renewed = renew(PASSWORD).await;
    assert_redirect(&renewed, "/settings/recovery-codes");
    let new: Vec<String> =
        serde_json::from_value(renewed.flashed("recoveryCodes").unwrap()).unwrap();
    assert_eq!(new.len(), 8);
    assert!(new.iter().all(|code| !old.contains(code)));
    assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 8);
    assert!(!RecoveryCode::consume(db, user.id, &old[1]).await.unwrap());
    assert!(RecoveryCode::consume(db, user.id, &new[0]).await.unwrap());

    // An account that asks for nothing besides its password has no codes.
    let plain = account(&app, "plain@example.com").await;
    let response = app
        .post("/settings/recovery-codes")
        .login_as(&plain)
        .csrf()
        .form(&[("currentPassword", PASSWORD)])
        .send()
        .await;
    assert_redirect(&response, "/settings");
    assert_eq!(RecoveryCode::remaining(db, plain.id).await.unwrap(), 0);
}

// ------------------------------------------------------------- second step

#[tokio::test]
async fn a_protected_account_signs_in_in_two_steps() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (secret, codes) = protect(&app, &user).await;

    // The password is half of it: nobody is signed in yet, and no cookie
    // remembers them.
    let typed = type_password(&app, &user).await;
    assert_redirect(&typed, "/login/two-factor");
    assert!(typed.cookie("remember_web").is_none());
    assert!(!signed_in(&app, typed.session()).await);
    let page = app
        .get("/login/two-factor")
        .session(typed.session())
        .send()
        .await;
    assert_eq!(page.status, StatusCode::OK);
    let page = page.text();
    assert!(page.contains(
        "The password of ada@example.com is right. Type a code of your authenticator app."
    ));
    assert!(page.contains("<form class=\"form\" method=\"post\" action=\"/login/two-factor\">"));
    assert!(page.contains(
        "id=\"second-step-code\" name=\"code\" type=\"text\" autocomplete=\"one-time-code\""
    ));
    // No passkey, no passkey button.
    assert!(!page.contains("data-passkey"));

    let wrong = answer_second_step(&app, typed.session(), "000000").await;
    assert_redirect(&wrong, "/login/two-factor");
    assert_eq!(
        wrong.flashed("errors"),
        Some(json!({ "code": "This code is not valid" }))
    );
    assert!(!signed_in(&app, wrong.session()).await);
    let empty = answer_second_step(&app, typed.session(), "").await;
    assert_redirect(&empty, "/login/two-factor");

    let answered = answer_second_step(&app, typed.session(), &code(&secret, 0)).await;
    assert_redirect(&answered, "/");
    assert!(answered.cookie("remember_web").is_some());
    assert!(signed_in(&app, answered.session()).await);
    // The pending sign-in is over.
    assert!(!answered.session().contains_key("pendingSignIn"));

    // A code works once: whoever read it over a shoulder gets nothing.
    let typed = type_password(&app, &user).await;
    let replayed = answer_second_step(&app, typed.session(), &code(&secret, 0)).await;
    assert_redirect(&replayed, "/login/two-factor");
    assert!(!signed_in(&app, replayed.session()).await);
    // The next one does.
    let next = answer_second_step(&app, typed.session(), &code(&secret, 1)).await;
    assert_redirect(&next, "/");

    // A recovery code stands in for the app, once, however it is typed.
    let typed = type_password(&app, &user).await;
    let recovered = answer_second_step(
        &app,
        typed.session(),
        &codes[0].to_uppercase().replace('-', " "),
    )
    .await;
    assert_redirect(&recovered, "/");
    assert_eq!(
        flash(&recovered, "success").as_deref(),
        Some("Signed in with a recovery code: 7 left. Make new ones under Settings.")
    );
    assert!(signed_in(&app, recovered.session()).await);
    let typed = type_password(&app, &user).await;
    let reused = answer_second_step(&app, typed.session(), &codes[0]).await;
    assert_redirect(&reused, "/login/two-factor");
    assert!(!signed_in(&app, reused.session()).await);
}

#[tokio::test]
async fn the_second_step_is_only_for_who_just_typed_the_password() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (secret, _) = protect(&app, &user).await;

    // Nobody typed a password in this browser.
    assert_redirect(&app.get("/login/two-factor").send().await, "/login");
    let answered = answer_second_step(&app, Map::new(), &code(&secret, 0)).await;
    assert_redirect(&answered, "/login");
    assert_eq!(
        flash(&answered, "error").as_deref(),
        Some("Sign in again: the second step took too long")
    );

    // The step is answered within ten minutes.
    let typed = type_password(&app, &user).await;
    let mut late = typed.session();
    let pending = late.get_mut("pendingSignIn").unwrap();
    pending["at"] = json!(Timestamp::now().timestamp_millis() - 11 * 60 * 1000);
    assert_redirect(
        &app.get("/login/two-factor")
            .session(late.clone())
            .send()
            .await,
        "/login",
    );
    let answered = answer_second_step(&app, late, &code(&secret, 0)).await;
    assert_redirect(&answered, "/login");
    assert!(!signed_in(&app, answered.session()).await);

    // Another password typed in the same browser ends it too, right or not.
    let typed = type_password(&app, &user).await;
    assert_redirect(&typed, "/login/two-factor");
    let retried = app
        .post("/login")
        .session(typed.session())
        .csrf()
        .form(&[
            ("email", "ada@example.com"),
            ("password", "not-the-password"),
        ])
        .send()
        .await;
    assert_redirect(&retried, "/login");
    assert!(!retried.session().contains_key("pendingSignIn"));
    assert_redirect(
        &app.get("/login/two-factor")
            .session(retried.session())
            .send()
            .await,
        "/login",
    );

    // A password changed meanwhile ends the sign-in the old one opened.
    let typed = type_password(&app, &user).await;
    reload(&app, &user)
        .await
        .change_password(&app.core.db, "another-password")
        .await
        .unwrap();
    let answered = answer_second_step(&app, typed.session(), &code(&secret, 0)).await;
    assert_redirect(&answered, "/login");
    assert!(!signed_in(&app, answered.session()).await);

    // Someone who is signed in has no second step to answer.
    let other = account(&app, "other@example.com").await;
    let page = app.get("/login/two-factor").login_as(&other).send().await;
    assert_redirect(&page, "/");
}

#[tokio::test]
async fn limits_the_codes_one_account_is_guessed_with() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (secret, _) = protect(&app, &user).await;
    let typed = type_password(&app, &user).await;

    for _ in 0..5 {
        let wrong = answer_second_step(&app, typed.session(), "000000").await;
        assert_redirect(&wrong, "/login/two-factor");
    }
    // The right code comes too late: the budget is counted before it is read.
    let limited = answer_second_step(&app, typed.session(), &code(&secret, 0)).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert!(!signed_in(&app, typed.session()).await);
    // Another account is not held back by it.
    let other = account(&app, "other@example.com").await;
    let (secret, _) = protect(&app, &other).await;
    let typed = type_password(&app, &other).await;
    assert_redirect(
        &answer_second_step(&app, typed.session(), &code(&secret, 0)).await,
        "/",
    );
}

#[tokio::test]
async fn what_sent_someone_to_sign_in_gets_them_after_the_second_step() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (secret, _) = protect(&app, &user).await;
    let mut session = Map::new();
    let approval = format!("/approvals/{}", "a".repeat(32));
    session.insert("approvalReturnTo".into(), json!(approval));
    let typed = app
        .post("/login")
        .session(session)
        .csrf()
        .form(&[("email", &user.email), ("password", PASSWORD)])
        .send()
        .await;
    assert_redirect(&typed, "/login/two-factor");
    let answered = answer_second_step(&app, typed.session(), &code(&secret, 0)).await;
    assert_redirect(&answered, &approval);
}

// ------------------------------------------------------------------ passkeys

fn encoded(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Ask to add a passkey, as the dialog does first.
async fn ask_to_add(app: &TestApp, user: &User, name: &str, password: &str) -> TestResponse {
    from_script(app.post("/settings/passkeys/options"))
        .login_as(user)
        .csrf()
        .form(&[("name", name), ("currentPassword", password)])
        .send()
        .await
}

fn challenge_of(options: &Value) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(options["challenge"].as_str().unwrap())
        .unwrap()
}

/// Add the passkey of this authenticator to an account, through both
/// requests of the dialog. The answer to the second one is returned.
async fn add_passkey(
    app: &TestApp,
    user: &User,
    authenticator: &SoftwareAuthenticator,
    name: &str,
) -> TestResponse {
    let asked = ask_to_add(app, user, name, PASSWORD).await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text());
    let created = authenticator.create(&rp(), ORIGIN, &challenge_of(&asked.json()));
    from_script(app.post("/settings/passkeys"))
        .session(asked.session())
        .csrf()
        .json(json!({
            "id": authenticator.id(),
            "clientDataJSON": encoded(&created.client_data_json),
            "attestationObject": encoded(&created.attestation_object),
        }))
        .send()
        .await
}

/// Ask what a passkey must sign to sign in, in the browser with this session.
async fn ask_to_sign_in(app: &TestApp, session: Map<String, Value>) -> TestResponse {
    from_script(app.post("/login/passkey/options"))
        .session(session)
        .csrf()
        .send()
        .await
}

/// Answer with this authenticator, on a page of `origin`.
async fn answer_with(
    app: &TestApp,
    asked: &TestResponse,
    authenticator: &mut SoftwareAuthenticator,
    origin: &str,
) -> TestResponse {
    let answered = authenticator.get(&rp(), origin, &challenge_of(&asked.json()));
    from_script(app.post("/login/passkey"))
        .session(asked.session())
        .csrf()
        .json(json!({
            "id": authenticator.id(),
            "clientDataJSON": encoded(&answered.client_data_json),
            "authenticatorData": encoded(&answered.authenticator_data),
            "signature": encoded(&answered.signature),
        }))
        .send()
        .await
}

fn refusal(response: &TestResponse) -> String {
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    response.json()["message"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_passkey_is_added_with_the_password_and_what_the_browser_made() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let authenticator = SoftwareAuthenticator::new(b"credential-of-ada");
    let db = &*app.core.db;

    let settings = app.get("/settings").login_as(&user).send().await.text();
    assert!(settings.contains("<form method=\"post\" action=\"/settings/passkeys\" data-passkey=\"create\" data-passkey-options=\"/settings/passkeys/options\">"));
    assert!(settings.contains("data-dialog-open=\"#add-passkey\">Add passkey</button>"));

    // The password first: the form comes back with what is wrong.
    let refused = ask_to_add(&app, &user, "MacBook", "not-the-password").await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(form.starts_with(
        "<form method=\"post\" action=\"/settings/passkeys\" data-passkey=\"create\""
    ));
    assert!(form.contains("The current password is incorrect"));
    assert!(form.contains("value=\"MacBook\""));
    assert!(!refused.session().contains_key("passkeyCeremony"));

    // Then what the browser is asked: a key for this instance and this
    // account, that the authenticator keeps and only uses once it has
    // verified the person.
    let asked = ask_to_add(&app, &user, "MacBook", PASSWORD).await;
    assert_eq!(asked.status, StatusCode::OK);
    assert_eq!(asked.header("cache-control"), Some("no-store"));
    let options = asked.json();
    assert_eq!(
        options["rp"],
        json!({ "id": "localhost", "name": "MyMCPs" })
    );
    assert_eq!(options["user"]["name"], "ada@example.com");
    assert_eq!(options["user"]["displayName"], "Test User");
    assert_eq!(challenge_of(&options).len(), 32);
    assert_eq!(options["attestation"], "none");
    assert_eq!(
        options["authenticatorSelection"],
        json!({ "residentKey": "required", "requireResidentKey": true, "userVerification": "required" })
    );
    assert_eq!(
        options["pubKeyCredParams"],
        json!([
            { "type": "public-key", "alg": -7 },
            { "type": "public-key", "alg": -8 },
            { "type": "public-key", "alg": -257 },
        ])
    );
    assert_eq!(options["excludeCredentials"], json!([]));
    // The handle says nothing of the person, and is theirs alone.
    let handle = options["user"]["id"].as_str().unwrap();
    assert_eq!(URL_SAFE_NO_PAD.decode(handle).unwrap().len(), 32);
    let other = account(&app, "other@example.com").await;
    let theirs = ask_to_add(&app, &other, "", PASSWORD).await.json();
    assert_ne!(theirs["user"]["id"], options["user"]["id"]);

    let send = async |session: Map<String, Value>, body: Value| {
        from_script(app.post("/settings/passkeys"))
            .session(session)
            .csrf()
            .json(body)
            .send()
            .await
    };
    let answer = |origin: &str, challenge: &[u8]| {
        let created = authenticator.create(&rp(), origin, challenge);
        json!({
            "id": authenticator.id(),
            "clientDataJSON": encoded(&created.client_data_json),
            "attestationObject": encoded(&created.attestation_object),
        })
    };
    let challenge = challenge_of(&options);

    // Made on a page of another site: a passkey of that site, not of this one.
    let phished = send(
        asked.session(),
        answer("https://evil.example.com", &challenge),
    )
    .await;
    assert_eq!(
        refusal(&phished),
        "The passkey was not added: the answer was made for another address than the one of this instance."
    );
    // The challenge was given once: the right answer comes too late.
    let late = send(phished.session(), answer(ORIGIN, &challenge)).await;
    assert_eq!(refusal(&late), "The request expired. Try again.");
    assert_eq!(Passkey::count_of_user(db, user.id).await.unwrap(), 0);
    // Not the answer to this challenge, nor one that can be read.
    let asked = ask_to_add(&app, &user, "MacBook", PASSWORD).await;
    let other_challenge = send(asked.session(), answer(ORIGIN, b"another challenge")).await;
    assert_eq!(
        refusal(&other_challenge),
        "The passkey was not added: the answer is not one to this request."
    );
    let asked = ask_to_add(&app, &user, "MacBook", PASSWORD).await;
    let unreadable = send(asked.session(), json!({ "id": "x" })).await;
    assert_eq!(
        refusal(&unreadable),
        "The answer of the browser could not be read. Try again."
    );
    // Asked for by one account, answered in the session of another.
    let asked = ask_to_add(&app, &user, "MacBook", PASSWORD).await;
    let mut swapped = asked.session();
    swapped.extend(mymcps_web::testing::session_for(&other));
    let crossed = send(swapped, answer(ORIGIN, &challenge_of(&asked.json()))).await;
    assert_eq!(refusal(&crossed), "The request expired. Try again.");
    assert_eq!(Passkey::count_of_user(db, other.id).await.unwrap(), 0);

    // The first protection of the account: its recovery codes come with it.
    let added = add_passkey(&app, &user, &authenticator, "MacBook").await;
    assert_eq!(added.status, StatusCode::NO_CONTENT);
    assert_eq!(added.header("x-location"), Some("/settings/recovery-codes"));
    assert_eq!(flash(&added, "success").as_deref(), Some("Passkey added"));
    assert!(added.flashed("recoveryCodes").is_some());
    let passkeys = Passkey::of_user(db, user.id).await.unwrap();
    assert_eq!(passkeys.len(), 1);
    assert_eq!(passkeys[0].name, "MacBook");
    assert_eq!(passkeys[0].credential_id, authenticator.id());
    assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 8);
    assert!(reload(&app, &user).await.is_protected(db).await.unwrap());

    let settings = app.get("/settings").login_as(&user).send().await.text();
    assert!(settings.contains("<span class=\"detail-row__label\">MacBook</span>"));
    assert!(settings.contains(&format!(
        "href=\"/settings/passkeys/{}/delete\" data-dialog-open=\"#remove-passkey\" data-dialog-fetch>Remove</a>",
        passkeys[0].id
    )));
    assert!(settings.contains("8 of 8 left."));

    // The authenticator is not asked for a second passkey of the account,
    // and a passkey is the one of a single account.
    let asked = ask_to_add(&app, &user, "", PASSWORD).await.json();
    assert_eq!(
        asked["excludeCredentials"],
        json!([{ "type": "public-key", "id": authenticator.id() }])
    );
    let again = add_passkey(&app, &other, &authenticator, "Copy").await;
    assert_eq!(refusal(&again), "This passkey is already one of an account");
    // A second one keeps the codes the account has, and gets a name.
    let second = SoftwareAuthenticator::new(b"security-key-of-ada");
    let added = add_passkey(&app, &user, &second, "").await;
    assert_eq!(added.header("x-location"), Some("/settings"));
    let names: Vec<String> = Passkey::of_user(db, user.id)
        .await
        .unwrap()
        .into_iter()
        .map(|passkey| passkey.name)
        .collect();
    assert_eq!(names, ["MacBook", "Passkey"]);
}

#[tokio::test]
async fn a_passkey_signs_in_alone_or_as_the_second_step() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let other = account(&app, "other@example.com").await;
    let mut authenticator = SoftwareAuthenticator::new(b"credential-of-ada");
    let mut theirs = SoftwareAuthenticator::new(b"credential-of-other");
    add_passkey(&app, &user, &authenticator, "MacBook").await;
    add_passkey(&app, &other, &theirs, "Phone").await;
    let db = &*app.core.db;

    // The sign-in page offers it, hidden until the script finds passkeys.
    let login = app.get("/login").send().await.text();
    assert!(login.contains("<form class=\"form\" method=\"post\" action=\"/login/passkey\" data-passkey=\"get\" data-passkey-options=\"/login/passkey/options\" hidden>"));

    // Alone: any passkey the browser has for this instance, which says
    // whose it is.
    let asked = ask_to_sign_in(&app, Map::new()).await;
    assert_eq!(asked.status, StatusCode::OK);
    let options = asked.json();
    assert_eq!(options["rpId"], "localhost");
    assert_eq!(options["userVerification"], "required");
    assert_eq!(options["allowCredentials"], json!([]));
    let answered = answer_with(&app, &asked, &mut authenticator, ORIGIN).await;
    assert_eq!(answered.status, StatusCode::NO_CONTENT);
    assert_eq!(answered.header("x-location"), Some("/"));
    assert!(answered.cookie("remember_web").is_some());
    let home = app.get("/").session(answered.session()).send().await.text();
    assert!(home.contains("ada@example.com") || home.contains("Test User"));
    let used = Passkey::of_user(db, user.id).await.unwrap().remove(0);
    assert!(used.last_used_at.is_some());

    // An answer is to one challenge, given once, on a page of this site.
    let replayed = answer_with(&app, &asked, &mut authenticator, ORIGIN).await;
    assert_eq!(refusal(&replayed), "The request expired. Try again.");
    let asked = ask_to_sign_in(&app, Map::new()).await;
    let phished = answer_with(&app, &asked, &mut authenticator, "https://evil.example.com").await;
    assert_eq!(
        refusal(&phished),
        "The passkey was refused: the answer was made for another address than the one of this instance."
    );
    assert!(!signed_in(&app, phished.session()).await);
    // A passkey nobody added here.
    let asked = ask_to_sign_in(&app, Map::new()).await;
    let mut unknown = SoftwareAuthenticator::new(b"nobody");
    let refused = answer_with(&app, &asked, &mut unknown, ORIGIN).await;
    assert_eq!(
        refusal(&refused),
        "This passkey is not one of this instance"
    );
    // The id of one passkey with the signature of another.
    let asked = ask_to_sign_in(&app, Map::new()).await;
    let mut forged = SoftwareAuthenticator::new(b"credential-of-ada");
    let refused = answer_with(&app, &asked, &mut forged, ORIGIN).await;
    assert_eq!(
        refusal(&refused),
        "The passkey was refused: the signature does not match the passkey."
    );
    assert!(!signed_in(&app, refused.session()).await);

    // As the second step: after the password, only the passkeys of that
    // account are asked for, and only one of them is taken.
    let typed = type_password(&app, &user).await;
    assert_redirect(&typed, "/login/two-factor");
    let page = app
        .get("/login/two-factor")
        .session(typed.session())
        .send()
        .await
        .text();
    assert!(page.contains("Use your passkey to sign in."));
    assert!(
        page.contains(
            "data-passkey=\"get\" data-passkey-options=\"/login/passkey/options\" hidden>"
        )
    );
    assert!(
        page.contains(
            "<label class=\"field__label\" for=\"second-step-code\">Recovery code</label>"
        )
    );
    let asked = ask_to_sign_in(&app, typed.session()).await;
    assert_eq!(
        asked.json()["allowCredentials"],
        json!([{ "type": "public-key", "id": authenticator.id() }])
    );
    let crossed = answer_with(&app, &asked, &mut theirs, ORIGIN).await;
    assert_eq!(
        refusal(&crossed),
        "This passkey is the one of another account"
    );
    assert!(!signed_in(&app, crossed.session()).await);
    let asked = ask_to_sign_in(&app, typed.session()).await;
    let answered = answer_with(&app, &asked, &mut authenticator, ORIGIN).await;
    assert_eq!(answered.header("x-location"), Some("/"));
    assert!(signed_in(&app, answered.session()).await);
    assert!(!answered.session().contains_key("pendingSignIn"));
}

#[tokio::test]
async fn a_passkey_whose_counter_goes_back_is_refused() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let mut authenticator = SoftwareAuthenticator::new(b"credential-of-ada");
    authenticator.counts = true;
    add_passkey(&app, &user, &authenticator, "Security key").await;
    let sign_in = async |authenticator: &mut SoftwareAuthenticator| {
        let asked = ask_to_sign_in(&app, Map::new()).await;
        answer_with(&app, &asked, authenticator, ORIGIN).await
    };

    assert_eq!(
        sign_in(&mut authenticator).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        sign_in(&mut authenticator).await.status,
        StatusCode::NO_CONTENT
    );
    let stored = Passkey::of_user(&*app.core.db, user.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(stored.sign_count, 2);

    // A copy of the key that signed less often than the server has seen.
    authenticator.sign_count = 0;
    let refused = sign_in(&mut authenticator).await;
    assert_eq!(
        refusal(&refused),
        "The passkey was refused: a copy of it was used since. Remove it under Settings and add it again."
    );
    assert!(!signed_in(&app, refused.session()).await);
    let stored = Passkey::of_user(&*app.core.db, user.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(stored.sign_count, 2);
}

#[tokio::test]
async fn a_passkey_is_removed_with_the_password() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let other = account(&app, "other@example.com").await;
    let authenticator = SoftwareAuthenticator::new(b"credential-of-ada");
    add_passkey(&app, &user, &authenticator, "MacBook").await;
    let db = &*app.core.db;
    let passkey = Passkey::of_user(db, user.id).await.unwrap().remove(0);
    let confirm_path = format!("/settings/passkeys/{}/delete", passkey.id);
    let action = format!("/settings/passkeys/{}?_method=DELETE", passkey.id);
    let form = format!("<form method=\"post\" action=\"{action}\" data-async>");

    // The dialog of a passkey: the page with it open, or its form alone.
    let page = app.get(&confirm_path).login_as(&user).send().await.text();
    assert!(page.contains("id=\"remove-passkey\" aria-labelledby=\"remove-passkey-title\" data-dialog-reset data-dialog-return=\"/settings\" data-open>"));
    assert!(page.contains(&form));
    assert!(
        page.contains(
            "<h2 class=\"dialog__title\" id=\"remove-passkey-title\">Remove MacBook?</h2>"
        )
    );
    let fragment = from_script(app.get(&confirm_path))
        .header("x-fragment", "remove-passkey")
        .login_as(&user)
        .send()
        .await;
    assert_eq!(fragment.header("cache-control"), Some("no-store"));
    assert!(fragment.text().starts_with(&form));

    // Nobody sees or removes the passkey of someone else.
    let theirs = app.get(&confirm_path).login_as(&other).send().await;
    assert_redirect(&theirs, "/settings");
    assert_eq!(
        flash(&theirs, "error").as_deref(),
        Some("Passkey not found")
    );
    let remove = async |actor: &User, password: &str| {
        app.post(&action)
            .login_as(actor)
            .csrf()
            .form(&[("currentPassword", password)])
            .send()
            .await
    };
    let refused = remove(&other, PASSWORD).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Passkey not found")
    );
    let refused = remove(&user, "not-the-password").await;
    assert_redirect(&refused, &confirm_path);
    let reopened = app
        .get(&confirm_path)
        .session(refused.session())
        .send()
        .await
        .text();
    assert!(reopened.contains("The current password is incorrect</p>"));
    assert_eq!(Passkey::count_of_user(db, user.id).await.unwrap(), 1);

    let removed = remove(&user, PASSWORD).await;
    assert_redirect(&removed, "/settings");
    assert_eq!(
        flash(&removed, "success").as_deref(),
        Some("Passkey removed")
    );
    assert_eq!(Passkey::count_of_user(db, user.id).await.unwrap(), 0);
    // It was the last protection of the account: its codes go with it, and
    // the password signs in alone.
    assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 0);
    assert_redirect(&type_password(&app, &user).await, "/");
}

#[tokio::test]
async fn the_codes_stay_while_one_protection_is_left() {
    let app = TestApp::new().await;
    let user = account(&app, "ada@example.com").await;
    let (_, codes) = protect(&app, &user).await;
    let authenticator = SoftwareAuthenticator::new(b"credential-of-ada");
    // A second protection keeps the codes of the first.
    let added = add_passkey(&app, &user, &authenticator, "MacBook").await;
    assert_eq!(added.header("x-location"), Some("/settings"));
    let db = &*app.core.db;

    let off = app
        .post("/settings/two-factor?_method=DELETE")
        .login_as(&user)
        .csrf()
        .form(&[("currentPassword", PASSWORD)])
        .send()
        .await;
    assert_redirect(&off, "/settings");
    assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 8);
    // The passkey still guards the account, and a code of before stands in
    // for it.
    let typed = type_password(&app, &user).await;
    assert_redirect(&typed, "/login/two-factor");
    assert_redirect(
        &answer_second_step(&app, typed.session(), &codes[0]).await,
        "/",
    );
}

// --------------------------------------------------- those who run the instance

#[tokio::test]
async fn a_super_admin_removes_the_protection_of_someone_who_lost_it() {
    let app = TestApp::new().await;
    let admin = create_super_admin(&app).await;
    let member = create_member_with(&app, "member@example.com").await;
    let plain = create_member_with(&app, "plain@example.com").await;
    protect(&app, &member).await;
    let authenticator = SoftwareAuthenticator::new(b"credential-of-member");
    add_passkey(&app, &member, &authenticator, "Phone").await;
    let db = &*app.core.db;
    let path = format!("/users/{}/protection", member.id);

    // Offered for an account that has a protection, and for no other.
    let users = app.get("/users").login_as(&admin).send().await.text();
    assert_eq!(
        users
            .matches(&format!(
                "href=\"{path}\" data-dialog-open=\"#reset-protection\" data-dialog-fetch"
            ))
            .count(),
        2
    );
    assert!(!users.contains(&format!("href=\"/users/{}/protection\"", plain.id)));
    let page = app.get(&path).login_as(&admin).send().await.text();
    assert!(page.contains("id=\"reset-protection\" aria-labelledby=\"reset-protection-title\" data-dialog-reset data-dialog-return=\"/users\" data-open>"));
    assert!(page.contains("Remove the sign-in protection of Test User?"));

    let remove = async |actor: &User, account: &User, password: &str| {
        app.post(&format!("/users/{}/protection?_method=DELETE", account.id))
            .login_as(actor)
            .in_team(Team::ROOT_ID)
            .csrf()
            .form(&[("currentPassword", password)])
            .send()
            .await
    };
    // Not by someone who does not run the instance, nor without the password.
    let refused = remove(&plain, &member, PASSWORD).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Super admin access required")
    );
    let refused = remove(&admin, &member, "not-the-password").await;
    assert_redirect(&refused, &path);
    let refused = remove(&admin, &admin, PASSWORD).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Turn your own sign-in protection off under Settings")
    );
    assert!(reload(&app, &member).await.is_protected(db).await.unwrap());

    let removed = remove(&admin, &member, PASSWORD).await;
    assert_redirect(&removed, "/users");
    assert_eq!(
        flash(&removed, "success").as_deref(),
        Some("Test User signs in with their password alone")
    );
    let stored = reload(&app, &member).await;
    assert!(!stored.is_protected(db).await.unwrap());
    assert_eq!(Passkey::count_of_user(db, member.id).await.unwrap(), 0);
    assert_eq!(RecoveryCode::remaining(db, member.id).await.unwrap(), 0);
    assert_redirect(&type_password(&app, &member).await, "/");
}
