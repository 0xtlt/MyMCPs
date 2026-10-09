//! Passkeys, the authenticator app, and recovery codes: their setup in
//! Settings, and the sign-ins they make or complete.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::StatusCode;
use mymcps_core::models::{User, UserPasskey, UserRecoveryCode, UserTotpSecret};
use mymcps_core::two_factor::{TotpKey, TwoFactorStatus, unix_now};
use mymcps_web::auth::LOGIN_KEY;
use mymcps_web::session::SESSION_COOKIE;
use mymcps_web::testing::factories::{PASSWORD, create_admin, create_member};
use mymcps_web::testing::{TestApp, TestResponse};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const ORIGIN: &str = "http://localhost:3333";
const RP_ID: &str = "localhost";

// ------------------------------------------------------------------ browser

/// A browser: it keeps the session cookie from one response to the next.
struct Browser<'a> {
    app: &'a TestApp,
    session: Map<String, Value>,
}

impl<'a> Browser<'a> {
    fn new(app: &'a TestApp) -> Self {
        Self {
            app,
            session: Map::new(),
        }
    }

    fn signed_in(app: &'a TestApp, user: &User) -> Self {
        Self {
            app,
            session: mymcps_web::testing::session_for(user),
        }
    }

    fn keep(&mut self, response: &TestResponse) {
        if response.cookie(SESSION_COOKIE).is_some() {
            self.session = response.session();
        }
    }

    fn user_id(&self) -> Option<i64> {
        self.session.get(LOGIN_KEY).and_then(Value::as_i64)
    }

    async fn get(&mut self, path: &str) -> TestResponse {
        let response = self
            .app
            .get(path)
            .session(self.session.clone())
            .send()
            .await;
        self.keep(&response);
        response
    }

    async fn post(&mut self, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        let response = self
            .app
            .post(path)
            .session(self.session.clone())
            .csrf()
            .form(fields)
            .send()
            .await;
        self.keep(&response);
        response
    }

    /// As the page's script sends a form.
    async fn fetch(&mut self, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        let response = self
            .app
            .post(path)
            .session(self.session.clone())
            .header("x-requested-with", "fetch")
            .csrf()
            .form(fields)
            .send()
            .await;
        self.keep(&response);
        response
    }

    async fn sign_in(&mut self, email: &str) -> TestResponse {
        self.post("/login", &[("email", email), ("password", PASSWORD)])
            .await
    }
}

fn assert_redirect(response: &TestResponse, location: &str) {
    assert_eq!(response.status, StatusCode::FOUND, "{}", response.text());
    assert_eq!(response.location(), Some(location));
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

// ------------------------------------------------------- authenticator app

async fn totp_key(app: &TestApp, user: &User) -> TotpKey {
    let secret = UserTotpSecret::for_user(&*app.core.db, user.id)
        .await
        .unwrap()
        .expect("a secret");
    let base32 = app.core.encryption.decrypt(&secret.secret).unwrap();
    assert!(!secret.secret.contains(&base32), "stored encrypted");
    TotpKey::from_base32(&base32, &user.email).unwrap()
}

/// Turn the authenticator app on for `user`, and return its key and the
/// recovery codes.
async fn enroll_totp(app: &TestApp, user: &User) -> (TotpKey, Vec<String>) {
    let mut browser = Browser::signed_in(app, user);
    let setup = browser
        .post(
            "/settings/two-factor/totp",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&setup, "/settings/two-factor/totp");
    let key = totp_key(app, user).await;
    let code = key.code_at(unix_now());
    let confirmed = browser
        .post("/settings/two-factor/totp/confirm", &[("code", &code)])
        .await;
    assert_redirect(&confirmed, "/settings/two-factor/recovery-codes");
    let codes: Vec<String> =
        serde_json::from_value(confirmed.flashed("recoveryCodes").unwrap()).unwrap();
    (key, codes)
}

/// A code of the app that was not used yet: the one of the next period,
/// which the server accepts for clocks that run a little ahead.
fn next_code(key: &TotpKey) -> String {
    key.code_at(unix_now() + 30)
}

#[tokio::test]
async fn setting_up_the_authenticator_app_shows_a_qr_code_and_asks_for_a_code() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut browser = Browser::signed_in(&app, &admin);

    let settings = browser.get("/settings").await;
    let page = settings.text();
    assert!(page.contains("Sign-in security"));
    assert!(page.contains("data-dialog-open=\"#setup-totp\""));
    assert!(page.contains("data-dialog-open=\"#add-passkey\""));

    // The password first.
    let wrong = browser
        .post(
            "/settings/two-factor/totp",
            &[("currentPassword", "nope-nope")],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FOUND);
    assert!(
        UserTotpSecret::for_user(&*app.core.db, admin.id)
            .await
            .unwrap()
            .is_none()
    );

    let setup = browser
        .post(
            "/settings/two-factor/totp",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&setup, "/settings/two-factor/totp");
    let page = browser.get("/settings/two-factor/totp").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    let key = totp_key(&app, &admin).await;
    assert!(html.contains("<svg class=\"qr-code\""));
    assert!(html.contains(&format!("data-copy=\"{}\"", key.base32())));
    assert!(!html.contains("style="));

    // Not on until a code of the app proves it holds the key.
    let status = TwoFactorStatus::of(&app.core.db, admin.id).await.unwrap();
    assert!(!status.is_enabled());
    let signed_in = Browser::new(&app).sign_in(&admin.email).await;
    assert_redirect(&signed_in, "/");

    let wrong = browser
        .post("/settings/two-factor/totp/confirm", &[("code", "000000")])
        .await;
    assert_redirect(&wrong, "/settings/two-factor/totp");
    let malformed = browser
        .post("/settings/two-factor/totp/confirm", &[("code", "12345")])
        .await;
    assert_redirect(&malformed, "/settings/two-factor/totp");

    let confirmed = browser
        .post(
            "/settings/two-factor/totp/confirm",
            &[("code", &key.code_at(unix_now()))],
        )
        .await;
    assert_redirect(&confirmed, "/settings/two-factor/recovery-codes");
    let status = TwoFactorStatus::of(&app.core.db, admin.id).await.unwrap();
    assert!(status.totp);
    assert_eq!(status.recovery_codes, 10);

    // The codes are shown once.
    let codes = browser.get("/settings/two-factor/recovery-codes").await;
    assert_eq!(codes.status, StatusCode::OK);
    let flashed: Vec<String> =
        serde_json::from_value(confirmed.flashed("recoveryCodes").unwrap()).unwrap();
    assert_eq!(flashed.len(), 10);
    for code in &flashed {
        assert!(codes.text().contains(code.as_str()));
    }
    let again = browser.get("/settings/two-factor/recovery-codes").await;
    assert_redirect(&again, "/settings");

    // Only their hashes are kept.
    let stored: Vec<String> =
        sqlx::query_scalar("select `code_hash` from `user_recovery_codes` where `user_id` = ?")
            .bind(admin.id)
            .fetch_all(&*app.core.db)
            .await
            .unwrap();
    assert!(stored.iter().all(|hash| !flashed.contains(hash)));

    // The page now offers to turn it off and to make new codes.
    let page = browser.get("/settings").await.text();
    assert!(page.contains("data-dialog-open=\"#disable-totp\""));
    assert!(page.contains("data-dialog-open=\"#regenerate-recovery-codes\""));
}

#[tokio::test]
async fn the_first_factor_signs_out_the_other_browsers() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut elsewhere = Browser::new(&app);
    assert_redirect(&elsewhere.sign_in(&admin.email).await, "/");
    assert_eq!(remember_tokens(&app, &admin).await, 1);
    assert_eq!(elsewhere.get("/settings").await.status, StatusCode::OK);

    let mut browser = Browser::signed_in(&app, &admin);
    browser
        .post(
            "/settings/two-factor/totp",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    let key = totp_key(&app, &admin).await;
    let confirmed = browser
        .post(
            "/settings/two-factor/totp/confirm",
            &[("code", &key.code_at(unix_now()))],
        )
        .await;
    assert_redirect(&confirmed, "/settings/two-factor/recovery-codes");

    // Only the browser that turned it on stays signed in, with a new token.
    assert_eq!(elsewhere.get("/settings").await.status, StatusCode::FOUND);
    assert_eq!(browser.get("/settings").await.status, StatusCode::OK);
    assert!(reload(&app, &admin).await.session_version > admin.session_version);
    assert_eq!(remember_tokens(&app, &admin).await, 1);
}

#[tokio::test]
async fn a_password_alone_no_longer_signs_in_once_the_app_is_on() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (key, _) = enroll_totp(&app, &admin).await;

    let mut browser = Browser::new(&app);
    let password = browser.sign_in(&admin.email).await;
    assert_redirect(&password, "/login/verify");
    assert_eq!(browser.user_id(), None, "no session before the second step");
    assert!(password.cookie("remember_web").is_none());
    assert_eq!(browser.get("/settings").await.status, StatusCode::FOUND);

    let page = browser.get("/login/verify").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("action=\"/login/verify/totp\""));
    assert!(html.contains("autocomplete=\"one-time-code\""));
    assert!(html.contains("href=\"/login/verify?method=recovery\""));
    assert!(
        !html.contains("method=passkey"),
        "the account has no passkey"
    );

    let wrong = browser
        .post("/login/verify/totp", &[("code", "000000")])
        .await;
    assert_redirect(&wrong, "/login/verify?method=totp");
    assert_eq!(browser.user_id(), None);
    let page = browser.get("/login/verify?method=totp").await.text();
    assert!(page.contains("This code is not valid"));

    let code = next_code(&key);
    let verified = browser.post("/login/verify/totp", &[("code", &code)]).await;
    assert_redirect(&verified, "/");
    assert_eq!(browser.user_id(), Some(admin.id));
    assert!(!browser.session.contains_key("twoFactorPending"));
    assert_eq!(browser.get("/settings").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_code_of_the_app_is_accepted_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (key, _) = enroll_totp(&app, &admin).await;
    let code = next_code(&key);

    let mut first = Browser::new(&app);
    first.sign_in(&admin.email).await;
    let verified = first.post("/login/verify/totp", &[("code", &code)]).await;
    assert_redirect(&verified, "/");

    // Whoever saw the code cannot use it again, even right away.
    let mut second = Browser::new(&app);
    second.sign_in(&admin.email).await;
    let replayed = second.post("/login/verify/totp", &[("code", &code)]).await;
    assert_redirect(&replayed, "/login/verify?method=totp");
    assert_eq!(second.user_id(), None);
    let page = second.get("/login/verify?method=totp").await.text();
    assert!(page.contains("This code was already used"));

    // Nor an older one.
    let older = second
        .post(
            "/login/verify/totp",
            &[("code", &key.code_at(unix_now() - 30))],
        )
        .await;
    assert_redirect(&older, "/login/verify?method=totp");
    assert_eq!(second.user_id(), None);
}

#[tokio::test]
async fn a_recovery_code_replaces_the_second_step_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (_, codes) = enroll_totp(&app, &admin).await;

    let mut browser = Browser::new(&app);
    browser.sign_in(&admin.email).await;
    let page = browser.get("/login/verify?method=recovery").await.text();
    assert!(page.contains("action=\"/login/verify/recovery\""));

    // Typed loosely: capitals, no dashes.
    let typed = codes[0].replace('-', "").to_uppercase();
    let used = browser
        .post("/login/verify/recovery", &[("code", &typed)])
        .await;
    assert_redirect(&used, "/");
    assert_eq!(browser.user_id(), Some(admin.id));
    assert_eq!(
        used.flashed("success"),
        Some(json!("Recovery code used. 9 codes left."))
    );
    assert_eq!(
        UserRecoveryCode::remaining(&*app.core.db, admin.id)
            .await
            .unwrap(),
        9
    );

    let mut again = Browser::new(&app);
    again.sign_in(&admin.email).await;
    let reused = again
        .post("/login/verify/recovery", &[("code", &codes[0])])
        .await;
    assert_redirect(&reused, "/login/verify?method=recovery");
    assert_eq!(again.user_id(), None);
    let page = again.get("/login/verify?method=recovery").await.text();
    assert!(page.contains("This recovery code is not valid, or was already used."));

    // New codes replace the old ones. Turning the app on signed out the
    // sessions that were open then.
    let mut settings = Browser::signed_in(&app, &reload(&app, &admin).await);
    let regenerated = settings
        .post(
            "/settings/two-factor/recovery-codes",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&regenerated, "/settings/two-factor/recovery-codes");
    let fresh: Vec<String> =
        serde_json::from_value(regenerated.flashed("recoveryCodes").unwrap()).unwrap();
    let old = again
        .post("/login/verify/recovery", &[("code", &codes[1])])
        .await;
    assert_redirect(&old, "/login/verify?method=recovery");
    let new = again
        .post("/login/verify/recovery", &[("code", &fresh[0])])
        .await;
    assert_redirect(&new, "/");
}

#[tokio::test]
async fn the_second_step_is_rate_limited_per_account() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (key, _) = enroll_totp(&app, &admin).await;

    let mut browser = Browser::new(&app);
    browser.sign_in(&admin.email).await;
    for _ in 0..5 {
        let wrong = browser
            .post("/login/verify/totp", &[("code", "000000")])
            .await;
        assert_redirect(&wrong, "/login/verify?method=totp");
    }
    // The right code is refused too: guessing six digits takes many tries.
    let limited = browser
        .post("/login/verify/totp", &[("code", &next_code(&key))])
        .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert_eq!(browser.user_id(), None);

    // Another browser with the password gets no new allowance, nor do
    // recovery codes.
    let mut other = Browser::new(&app);
    other.sign_in(&admin.email).await;
    let limited = other
        .post("/login/verify/recovery", &[("code", "aaaa-bbbb-cccc-dddd")])
        .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn the_second_step_waits_ten_minutes_for_the_same_password() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (key, _) = enroll_totp(&app, &admin).await;

    // Without a password first, there is nothing to verify.
    let mut stranger = Browser::new(&app);
    assert_redirect(&stranger.get("/login/verify").await, "/login");
    let guessed = stranger
        .post("/login/verify/totp", &[("code", &next_code(&key))])
        .await;
    assert_redirect(&guessed, "/login");
    assert_eq!(stranger.user_id(), None);

    // Expired.
    let mut browser = Browser::new(&app);
    browser.sign_in(&admin.email).await;
    let pending = browser.session.get_mut("twoFactorPending").unwrap();
    pending["expiresAt"] = json!(chrono::Utc::now().timestamp_millis() - 1);
    let expired = browser
        .post("/login/verify/totp", &[("code", &next_code(&key))])
        .await;
    assert_redirect(&expired, "/login");
    assert_eq!(browser.user_id(), None);
    assert!(!browser.session.contains_key("twoFactorPending"));

    // Signing out everywhere cancels the sign-ins under way.
    let mut browser = Browser::new(&app);
    browser.sign_in(&admin.email).await;
    let mut user = reload(&app, &admin).await;
    user.invalidate_sessions(&*app.core.db).await.unwrap();
    let cancelled = browser
        .post("/login/verify/totp", &[("code", &next_code(&key))])
        .await;
    assert_redirect(&cancelled, "/login");
    assert_eq!(browser.user_id(), None);
}

#[tokio::test]
async fn the_second_step_keeps_where_the_sign_in_was_going() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (key, _) = enroll_totp(&app, &admin).await;

    let mut browser = Browser::new(&app);
    let approval = format!("/approvals/{}", "a".repeat(32));
    browser
        .session
        .insert("approvalReturnTo".into(), json!(approval));
    assert_redirect(&browser.sign_in(&admin.email).await, "/login/verify");
    let verified = browser
        .post("/login/verify/totp", &[("code", &next_code(&key))])
        .await;
    assert_redirect(&verified, &approval);
    assert_eq!(browser.user_id(), Some(admin.id));
}

#[tokio::test]
async fn turning_the_app_off_takes_the_password_and_the_recovery_codes() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    enroll_totp(&app, &admin).await;
    let mut browser = Browser::signed_in(&app, &reload(&app, &admin).await);

    let wrong = browser
        .post(
            "/settings/two-factor/totp?_method=DELETE",
            &[("currentPassword", "not-it-at-all")],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FOUND);
    assert_eq!(wrong.flashed("settingsForm"), Some(json!("disable-totp")));
    assert!(
        TwoFactorStatus::of(&app.core.db, admin.id)
            .await
            .unwrap()
            .totp
    );
    // The dialog opens again on what was refused.
    let page = browser.get("/settings").await.text();
    assert!(page.contains("id=\"disable-totp\""));
    assert!(page.contains("The current password is incorrect"));

    let off = browser
        .post(
            "/settings/two-factor/totp?_method=DELETE",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&off, "/settings");
    let status = TwoFactorStatus::of(&app.core.db, admin.id).await.unwrap();
    assert!(!status.totp);
    assert_eq!(status.recovery_codes, 0, "no factor left to recover");
    assert_redirect(&Browser::new(&app).sign_in(&admin.email).await, "/");
}

#[tokio::test]
async fn the_settings_forms_answer_the_script_with_their_fragment() {
    let app = TestApp::new().await;
    let member = create_member(&app).await;
    let mut browser = Browser::signed_in(&app, &member);

    let refused = browser
        .fetch(
            "/settings/two-factor/totp",
            &[("currentPassword", "not-it-at-all")],
        )
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused
            .text()
            .contains("action=\"/settings/two-factor/totp\"")
    );
    assert!(refused.text().contains("The current password is incorrect"));

    let accepted = browser
        .fetch(
            "/settings/two-factor/totp",
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_eq!(
        accepted.header("x-location"),
        Some("/settings/two-factor/totp")
    );
}

// ----------------------------------------------------------------- passkeys

/// A minimal CBOR writer: the attestation object and the COSE key.
enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(&'static str),
    Map(Vec<(Cbor, Cbor)>),
}

impl Cbor {
    fn head(out: &mut Vec<u8>, major: u8, value: u64) {
        let major = major << 5;
        match value {
            0..=23 => out.push(major | value as u8),
            24..=0xff => out.extend([major | 24, value as u8]),
            0x100..=0xffff => {
                out.push(major | 25);
                out.extend((value as u16).to_be_bytes());
            }
            _ => {
                out.push(major | 26);
                out.extend((value as u32).to_be_bytes());
            }
        }
    }

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Int(value) if *value >= 0 => Self::head(out, 0, *value as u64),
            Self::Int(value) => Self::head(out, 1, (-1 - value) as u64),
            Self::Bytes(bytes) => {
                Self::head(out, 2, bytes.len() as u64);
                out.extend(bytes);
            }
            Self::Text(text) => {
                Self::head(out, 3, text.len() as u64);
                out.extend(text.as_bytes());
            }
            Self::Map(entries) => {
                Self::head(out, 5, entries.len() as u64);
                for (key, value) in entries {
                    key.encode(out);
                    value.encode(out);
                }
            }
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A security key in software: one P-256 credential, user verification
/// included.
struct Authenticator {
    key: SigningKey,
    credential_id: Vec<u8>,
    user_handle: Option<Vec<u8>>,
    counter: u32,
    origin: &'static str,
}

impl Authenticator {
    fn new(seed: u8) -> Self {
        Self {
            key: SigningKey::from_bytes(&[seed; 32].into()).unwrap(),
            credential_id: vec![seed; 32],
            user_handle: None,
            counter: 0,
            origin: ORIGIN,
        }
    }

    fn client_data(&self, kind: &str, challenge: &str) -> Vec<u8> {
        json!({
            "type": kind,
            "challenge": challenge,
            "origin": self.origin,
            "crossOrigin": false,
        })
        .to_string()
        .into_bytes()
    }

    fn rp_id_hash() -> Vec<u8> {
        Sha256::digest(RP_ID.as_bytes()).to_vec()
    }

    /// `navigator.credentials.create`, serialized as the page's script does.
    fn create(&mut self, options: &Value) -> String {
        let public_key = &options["publicKey"];
        assert_eq!(public_key["rp"]["id"], RP_ID);
        let user_id = public_key["user"]["id"].as_str().unwrap();
        self.user_handle = Some(URL_SAFE_NO_PAD.decode(user_id).unwrap());
        let client_data =
            self.client_data("webauthn.create", public_key["challenge"].as_str().unwrap());

        let point = self.key.verifying_key().to_encoded_point(false);
        let cose = Cbor::Map(vec![
            (Cbor::Int(1), Cbor::Int(2)),
            (Cbor::Int(3), Cbor::Int(-7)),
            (Cbor::Int(-1), Cbor::Int(1)),
            (Cbor::Int(-2), Cbor::Bytes(point.x().unwrap().to_vec())),
            (Cbor::Int(-3), Cbor::Bytes(point.y().unwrap().to_vec())),
        ]);
        let mut auth_data = Self::rp_id_hash();
        auth_data.push(0x01 | 0x04 | 0x40); // user present, verified, attested data
        auth_data.extend(self.counter.to_be_bytes());
        auth_data.extend([0u8; 16]); // AAGUID
        auth_data.extend((self.credential_id.len() as u16).to_be_bytes());
        auth_data.extend(&self.credential_id);
        auth_data.extend(cose.to_vec());
        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt"), Cbor::Text("none")),
            (Cbor::Text("attStmt"), Cbor::Map(vec![])),
            (Cbor::Text("authData"), Cbor::Bytes(auth_data)),
        ]);
        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": {
                "attestationObject": b64(&attestation.to_vec()),
                "clientDataJSON": b64(&client_data),
            },
        })
        .to_string()
    }

    /// `navigator.credentials.get`, serialized as the page's script does.
    fn get(&mut self, options: &Value) -> String {
        let public_key = &options["publicKey"];
        let client_data =
            self.client_data("webauthn.get", public_key["challenge"].as_str().unwrap());
        self.counter += 1;
        let mut auth_data = Self::rp_id_hash();
        auth_data.push(0x01 | 0x04);
        auth_data.extend(self.counter.to_be_bytes());
        let mut signed = auth_data.clone();
        signed.extend(Sha256::digest(&client_data));
        let signature: Signature = self.key.sign(&signed);
        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": {
                "authenticatorData": b64(&auth_data),
                "clientDataJSON": b64(&client_data),
                "signature": b64(signature.to_der().as_bytes()),
                "userHandle": self.user_handle.as_deref().map(b64),
            },
        })
        .to_string()
    }
}

/// Register `authenticator` for `user` as the Settings page does, and
/// return the response to the passkey itself.
async fn register_passkey(
    browser: &mut Browser<'_>,
    authenticator: &mut Authenticator,
    name: &str,
) -> TestResponse {
    let options = browser
        .fetch(
            "/settings/passkeys/options",
            &[("name", name), ("currentPassword", PASSWORD)],
        )
        .await;
    assert_eq!(options.status, StatusCode::OK, "{}", options.text());
    assert!(
        options
            .header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    let credential = authenticator.create(&options.json());
    browser
        .fetch(
            "/settings/passkeys",
            &[("name", name), ("credential", &credential)],
        )
        .await
}

#[tokio::test]
async fn a_passkey_is_added_with_the_password_and_named() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut browser = Browser::signed_in(&app, &admin);

    // The password and a name come first; the dialog gets its errors back.
    let refused = browser
        .fetch(
            "/settings/passkeys/options",
            &[("name", "Laptop"), ("currentPassword", "not-it-at-all")],
        )
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.text().contains("data-passkey=\"register\""));
    assert!(refused.text().contains("The current password is incorrect"));
    let unnamed = browser
        .fetch(
            "/settings/passkeys/options",
            &[("name", ""), ("currentPassword", PASSWORD)],
        )
        .await;
    assert_eq!(unnamed.status, StatusCode::UNPROCESSABLE_ENTITY);

    // An answer without a ceremony is refused.
    let mut authenticator = Authenticator::new(7);
    let forged = browser
        .fetch("/settings/passkeys", &[("credential", "{\"id\":\"x\"}")])
        .await;
    assert_eq!(forged.status, StatusCode::UNPROCESSABLE_ENTITY);

    let added = register_passkey(&mut browser, &mut authenticator, "Laptop").await;
    assert_eq!(
        added.header("x-location"),
        Some("/settings/two-factor/recovery-codes"),
        "{}",
        added.text()
    );
    let passkeys = UserPasskey::for_user(&*app.core.db, admin.id)
        .await
        .unwrap();
    assert_eq!(passkeys.len(), 1);
    assert_eq!(passkeys[0].name, "Laptop");
    assert_eq!(passkeys[0].credential_id, b64(&authenticator.credential_id));
    let status = TwoFactorStatus::of(&app.core.db, admin.id).await.unwrap();
    assert_eq!(status.passkeys, 1);
    assert_eq!(status.recovery_codes, 10);

    // The same key cannot be added twice; the server excludes it already.
    let options = browser
        .fetch(
            "/settings/passkeys/options",
            &[("name", "Again"), ("currentPassword", PASSWORD)],
        )
        .await
        .json();
    let excluded = options["publicKey"]["excludeCredentials"]
        .as_array()
        .unwrap();
    assert_eq!(excluded[0]["id"], b64(&authenticator.credential_id));
    let mut copy = Authenticator::new(7);
    let credential = copy.create(&options);
    let twice = browser
        .fetch("/settings/passkeys", &[("credential", &credential)])
        .await;
    assert_eq!(twice.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(twice.text().contains("This passkey is already registered"));

    // A second key: no new recovery codes.
    let mut second = Authenticator::new(9);
    let added = register_passkey(&mut browser, &mut second, "YubiKey").await;
    assert_eq!(added.header("x-location"), Some("/settings"));
    let page = browser.get("/settings").await.text();
    assert!(page.contains("Laptop"));
    assert!(page.contains("YubiKey"));
}

#[tokio::test]
async fn a_passkey_from_another_site_is_refused() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut browser = Browser::signed_in(&app, &admin);
    let mut phished = Authenticator::new(11);
    phished.origin = "http://localhost.evil.example";
    let refused = register_passkey(&mut browser, &mut phished, "Laptop").await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        UserPasskey::for_user(&*app.core.db, admin.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_passkey_signs_in_on_its_own() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut authenticator = Authenticator::new(7);
    register_passkey(
        &mut Browser::signed_in(&app, &admin),
        &mut authenticator,
        "Laptop",
    )
    .await;

    let mut browser = Browser::new(&app);
    let login = browser.get("/login").await.text();
    assert!(login.contains("data-passkey=\"authenticate\""));
    assert!(login.contains("data-passkey-options=\"/login/passkey/options\""));

    let options = browser.fetch("/login/passkey/options", &[]).await;
    assert_eq!(options.status, StatusCode::OK);
    let options = options.json();
    assert!(
        options["publicKey"].get("allowCredentials").is_none()
            || options["publicKey"]["allowCredentials"]
                .as_array()
                .unwrap()
                .is_empty(),
        "the passkey says whose it is"
    );
    let credential = authenticator.get(&options);
    let signed_in = browser
        .post("/login/passkey", &[("credential", &credential)])
        .await;
    assert_redirect(&signed_in, "/");
    assert_eq!(browser.user_id(), Some(admin.id));
    assert!(signed_in.cookie("remember_web").is_some());
    let stored = &UserPasskey::for_user(&*app.core.db, admin.id)
        .await
        .unwrap()[0];
    assert!(stored.last_used_at.is_some());

    // The same answer again: its challenge is spent.
    let mut thief = Browser::new(&app);
    let replayed = thief
        .post("/login/passkey", &[("credential", &credential)])
        .await;
    assert_redirect(&replayed, "/login");
    assert_eq!(thief.user_id(), None);
    thief.fetch("/login/passkey/options", &[]).await;
    let replayed = thief
        .post("/login/passkey", &[("credential", &credential)])
        .await;
    assert_redirect(&replayed, "/login");
    assert_eq!(thief.user_id(), None);
}

#[tokio::test]
async fn an_unknown_or_cloned_passkey_does_not_sign_in() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut authenticator = Authenticator::new(7);
    register_passkey(
        &mut Browser::signed_in(&app, &admin),
        &mut authenticator,
        "Laptop",
    )
    .await;

    // A key the instance never saw.
    let mut stranger = Authenticator::new(13);
    stranger.user_handle = authenticator.user_handle.clone();
    let mut browser = Browser::new(&app);
    let options = browser.fetch("/login/passkey/options", &[]).await.json();
    let refused = browser
        .post("/login/passkey", &[("credential", &stranger.get(&options))])
        .await;
    assert_redirect(&refused, "/login");
    assert_eq!(browser.user_id(), None);
    let page = browser.get("/login").await.text();
    assert!(page.contains("This passkey is not registered on this instance."));

    // A copy of the key whose counter went back.
    let options = browser.fetch("/login/passkey/options", &[]).await.json();
    authenticator.counter = 5;
    let used = browser
        .post(
            "/login/passkey",
            &[("credential", &authenticator.get(&options))],
        )
        .await;
    assert_redirect(&used, "/");
    let mut clone = Browser::new(&app);
    let options = clone.fetch("/login/passkey/options", &[]).await.json();
    authenticator.counter = 2;
    let cloned = clone
        .post(
            "/login/passkey",
            &[("credential", &authenticator.get(&options))],
        )
        .await;
    assert_redirect(&cloned, "/login");
    assert_eq!(clone.user_id(), None);
}

#[tokio::test]
async fn a_passkey_completes_a_sign_in_with_the_password() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut authenticator = Authenticator::new(7);
    register_passkey(
        &mut Browser::signed_in(&app, &admin),
        &mut authenticator,
        "Laptop",
    )
    .await;

    let mut browser = Browser::new(&app);
    assert_redirect(&browser.sign_in(&admin.email).await, "/login/verify");
    let page = browser.get("/login/verify").await.text();
    assert!(page.contains("data-passkey-options=\"/login/verify/passkey/options\""));
    assert!(page.contains("href=\"/login/verify?method=recovery\""));
    assert!(!page.contains("method=totp"));

    let options = browser.fetch("/login/verify/passkey/options", &[]).await;
    assert_eq!(options.status, StatusCode::OK);
    let options = options.json();
    let allowed = options["publicKey"]["allowCredentials"].as_array().unwrap();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0]["id"], b64(&authenticator.credential_id));
    let credential = authenticator.get(&options);
    let verified = browser
        .post("/login/verify/passkey", &[("credential", &credential)])
        .await;
    assert_redirect(&verified, "/");
    assert_eq!(browser.user_id(), Some(admin.id));

    // The passkey of another account does not complete this one.
    let member = create_member(&app).await;
    let mut other = Authenticator::new(21);
    register_passkey(&mut Browser::signed_in(&app, &member), &mut other, "Phone").await;
    let mut browser = Browser::new(&app);
    browser.sign_in(&admin.email).await;
    let options = browser
        .fetch("/login/verify/passkey/options", &[])
        .await
        .json();
    let refused = browser
        .post(
            "/login/verify/passkey",
            &[("credential", &other.get(&options))],
        )
        .await;
    assert_redirect(&refused, "/login/verify?method=passkey");
    assert_eq!(browser.user_id(), None);

    // Without a password first, no options.
    let stranger = Browser::new(&app)
        .fetch("/login/verify/passkey/options", &[])
        .await;
    assert_eq!(stranger.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_passkey_is_renamed_and_removed_with_the_password() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut browser = Browser::signed_in(&app, &admin);
    let mut authenticator = Authenticator::new(7);
    register_passkey(&mut browser, &mut authenticator, "Laptop").await;
    let passkey = UserPasskey::for_user(&*app.core.db, admin.id)
        .await
        .unwrap()[0]
        .clone();

    let renamed = browser
        .post(
            &format!("/settings/passkeys/{}?_method=PATCH", passkey.id),
            &[("name", "Work laptop")],
        )
        .await;
    assert_redirect(&renamed, "/settings");
    let stored = &UserPasskey::for_user(&*app.core.db, admin.id)
        .await
        .unwrap()[0];
    assert_eq!(stored.name, "Work laptop");

    // Not someone else's.
    let member = create_member(&app).await;
    let foreign = Browser::signed_in(&app, &member)
        .post(
            &format!("/settings/passkeys/{}?_method=DELETE", passkey.id),
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&foreign, "/settings");
    assert_eq!(
        UserPasskey::for_user(&*app.core.db, admin.id)
            .await
            .unwrap()
            .len(),
        1
    );

    let wrong = browser
        .post(
            &format!("/settings/passkeys/{}?_method=DELETE", passkey.id),
            &[("currentPassword", "not-it-at-all")],
        )
        .await;
    assert_eq!(
        wrong.flashed("settingsForm"),
        Some(json!(format!("remove-passkey-{}", passkey.id)))
    );
    let removed = browser
        .post(
            &format!("/settings/passkeys/{}?_method=DELETE", passkey.id),
            &[("currentPassword", PASSWORD)],
        )
        .await;
    assert_redirect(&removed, "/settings");
    let status = TwoFactorStatus::of(&app.core.db, admin.id).await.unwrap();
    assert_eq!(status.passkeys, 0);
    assert_eq!(status.recovery_codes, 0);
    assert_redirect(&Browser::new(&app).sign_in(&admin.email).await, "/");
}

#[tokio::test]
async fn without_a_public_app_url_there_are_no_passkeys() {
    let app = TestApp::with_config(|config| config.app_url = None).await;
    let admin = create_admin(&app).await;
    let mut browser = Browser::signed_in(&app, &admin);
    let page = browser.get("/settings").await.text();
    assert!(page.contains("Passkeys need APP_URL"));
    assert!(!page.contains("data-dialog-open=\"#add-passkey\""));
    let options = browser
        .fetch(
            "/settings/passkeys/options",
            &[("name", "Laptop"), ("currentPassword", PASSWORD)],
        )
        .await;
    assert_eq!(options.status, StatusCode::NOT_FOUND);
    let login = Browser::new(&app).get("/login").await.text();
    assert!(!login.contains("data-passkey"));
}
