//! Two-step verification and passkeys: the second step of a sign-in with a
//! password, the sign-in with a passkey alone, and their settings.
//!
//! A password that is right for an account protected by a passkey or an
//! authenticator app opens no session. The session only remembers, for ten
//! minutes, whose password it was; `/login/verify` then asks for a passkey,
//! a code of the app, or a recovery code, and signs the person in.

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use http::{HeaderMap, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::{User, UserPasskey, UserRecoveryCode, UserTotpSecret};
use mymcps_core::two_factor::{
    TotpKey, TwoFactorStatus, forget_recovery_codes_if_unprotected, generate_recovery_codes,
    hash_recovery_code, unix_now,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::auth::{CurrentUser, sign_in};
use crate::cookies::Cookies;
use crate::error::AppError;
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::passkeys::{NewPasskey, PasskeyFailure, Passkeys};
use crate::redirect::{redirect_to, with_query};
use crate::respond::{navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::auth::{consume, finish_sign_in};
use crate::routes::invites::forwarded;
use crate::routes::settings::{
    PasswordCheck, confirm_current_password, refuse, wrong_current_password,
};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::record_id;
use crate::validators::two_factor::{
    CONFIRM_PASSWORD_VALIDATOR, PASSKEY_CREDENTIAL_VALIDATOR, PASSKEY_OPTIONS_VALIDATOR,
    RECOVERY_CODE_VALIDATOR, RENAME_PASSKEY_VALIDATOR, TOTP_CODE_VALIDATOR,
};
use crate::views::auth::login_page;
use crate::views::shell::PageContext;
use crate::views::two_factor::{
    ADD_PASSKEY_DIALOG, DISABLE_TOTP_DIALOG, REGENERATE_CODES_DIALOG, SETUP_TOTP_DIALOG,
    VerifyMethod, VerifyPage, add_passkey_form, disable_totp_form, recovery_codes_page,
    regenerate_codes_form, remove_passkey_dialog_id, remove_passkey_form, rename_passkey_dialog_id,
    rename_passkey_form, setup_totp_form, totp_setup_page, verify_page,
};

/// Session key of a sign-in waiting for its second step.
const PENDING_KEY: &str = "twoFactorPending";
/// How long the second step may wait after the password.
const PENDING_LIFETIME_MS: i64 = 10 * 60 * 1000;
/// Flash key of recovery codes that were just generated, shown once.
const RECOVERY_CODES_KEY: &str = "recoveryCodes";

pub const VERIFY_PATH: &str = "/login/verify";
const TOTP_SETUP_PATH: &str = "/settings/two-factor/totp";
const RECOVERY_CODES_PATH: &str = "/settings/two-factor/recovery-codes";

const WRONG_CODE: &str = "This code is not valid. Check the time of your device and try again.";
const REPLAYED_CODE: &str =
    "This code was already used. Wait for the next one of your authenticator app.";
const WRONG_RECOVERY_CODE: &str = "This recovery code is not valid, or was already used.";
const PASSKEY_REFUSED: &str = "The passkey was not accepted. Try again.";
const UNKNOWN_PASSKEY: &str = "This passkey is not registered on this instance.";
const ALREADY_REGISTERED: &str = "This passkey is already registered";
const NO_CEREMONY: &str = "The passkey request expired. Try again.";
const PASSKEYS_OFF: &str = "Passkeys are not available on this instance.";
const BUSY: &str = "Too many passkey requests are in progress. Try again in a moment.";
const EXPIRED: &str = "Your sign-in expired. Sign in again.";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        guest: Router::new()
            .route("/login/passkey/options", post(passkey_sign_in_options))
            .route("/login/passkey", post(passkey_sign_in))
            .route(VERIFY_PATH, get(show_verify))
            .route("/login/verify/totp", post(verify_totp))
            .route("/login/verify/recovery", post(verify_recovery_code))
            .route(
                "/login/verify/passkey/options",
                post(verify_passkey_options),
            )
            .route("/login/verify/passkey", post(verify_passkey)),
        signed_in: Router::new()
            .route(
                TOTP_SETUP_PATH,
                get(show_totp_setup).post(setup_totp).delete(disable_totp),
            )
            .route("/settings/two-factor/totp/confirm", post(confirm_totp))
            .route(
                RECOVERY_CODES_PATH,
                get(show_recovery_codes).post(regenerate_recovery_codes),
            )
            .route(
                "/settings/passkeys/options",
                post(passkey_registration_options),
            )
            .route("/settings/passkeys", post(add_passkey))
            .route(
                "/settings/passkeys/{id}",
                patch(rename_passkey).delete(remove_passkey),
            ),
        ..Default::default()
    }
}

// ------------------------------------------------------------ pending sign-in

/// A password that was accepted, waiting for its second step.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    user_id: i64,
    /// The session version of the user then: signing out everywhere, or a
    /// new password, also cancels the sign-ins under way.
    version: i64,
    expires_at: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Remember that the password of `user` was right, and nothing more: the
/// session stays signed out until the second step.
pub(crate) fn begin_second_step(session: &Session, user: &User) {
    session.put(
        PENDING_KEY,
        Pending {
            user_id: user.id,
            version: user.session_version,
            expires_at: now_ms() + PENDING_LIFETIME_MS,
        },
    );
}

/// The user whose second step this browser owes, if it still does.
async fn pending_user(state: &AppState, session: &Session) -> Result<Option<User>, AppError> {
    let Some(pending) = session.get_as::<Pending>(PENDING_KEY) else {
        return Ok(None);
    };
    let user = if pending.expires_at > now_ms() {
        User::find(&*state.core.db, pending.user_id)
            .await?
            .filter(|user| user.session_version == pending.version)
    } else {
        None
    };
    if user.is_none() {
        session.forget(PENDING_KEY);
    }
    Ok(user)
}

/// Back to the sign-in page, which says why.
fn sign_in_again(session: &Session) -> Response {
    FormState::with_error("credentials", EXPIRED, &Map::new()).flash(session);
    redirect_to("/login")
}

/// The second step was proven: open the session.
async fn complete(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
    user: &User,
    query: Option<&str>,
) -> Result<Response, AppError> {
    session.forget(PENDING_KEY);
    state
        .limiters
        .two_factor
        .delete(&two_factor_key(user.id))
        .await?;
    finish_sign_in(state, session, cookies, user, query).await
}

fn two_factor_key(user_id: i64) -> String {
    format!("two-factor:{user_id}")
}

/// Back to the verification page, on `method`, with `message` above it.
fn refuse_step(
    session: &Session,
    method: VerifyMethod,
    message: &str,
    input: &Map<String, Value>,
) -> Response {
    FormState::with_error("code", message, input).flash(session);
    redirect_to(&format!("{VERIFY_PATH}?method={}", method.as_str()))
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(
            http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body.to_string(),
    )
        .into_response()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &json!({ "error": message }))
}

/// The input of a passkey form without the credential, which is large and
/// never shown again.
fn without_credential(input: &Map<String, Value>) -> Map<String, Value> {
    let mut input = input.clone();
    input.remove("credential");
    input
}

fn credential_of(input: &Map<String, Value>) -> Result<Result<String, String>, AppError> {
    match PASSKEY_CREDENTIAL_VALIDATOR.validate(&Value::Object(input.clone())) {
        Ok(valid) => Ok(Ok(valid["credential"]
            .as_str()
            .unwrap_or_default()
            .to_string())),
        Err(error) => {
            let error = error;
            Ok(Err(error
                .messages
                .first()
                .map(|message| message.message.clone())
                .unwrap_or_default()))
        }
    }
}

fn failure_message(failure: PasskeyFailure) -> &'static str {
    match failure {
        PasskeyFailure::NoCeremony => NO_CEREMONY,
        PasskeyFailure::Rejected => PASSKEY_REFUSED,
        PasskeyFailure::Unknown => UNKNOWN_PASSKEY,
    }
}

// ------------------------------------------------------- sign-in with a passkey

/// `POST /login/passkey/options`
pub async fn passkey_sign_in_options(State(state): State<AppState>, session: Session) -> Response {
    if !state.passkeys.is_available() {
        return json_error(StatusCode::NOT_FOUND, PASSKEYS_OFF);
    }
    match state.passkeys.start_sign_in(&session) {
        Some(options) => json_response(StatusCode::OK, &options),
        None => json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY),
    }
}

/// `POST /login/passkey`: a passkey signs in on its own. It verified the
/// person (fingerprint, face, PIN) and holds a key the device keeps: two
/// factors, so no second step follows.
pub async fn passkey_sign_in(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    crate::client_ip::ClientIp(ip): crate::client_ip::ClientIp,
    OriginalUri(uri): OriginalUri,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |message: &str| {
        FormState::with_error("passkey", message, &Map::new()).flash(&session);
        redirect_to("/login")
    };
    let credential = match credential_of(&input)? {
        Ok(credential) => credential,
        Err(message) => return Ok(refused(&message)),
    };

    let address_key = format!(
        "login-address:{}",
        mymcps_core::client_ip::rate_limit_client_key(&ip)
    );
    if let Err(limited) = consume(&state.limiters.login_address, &address_key).await? {
        return Ok(limited);
    }
    let db = state.core.db.clone();
    let used = state
        .passkeys
        .finish_sign_in(&session, &credential, |id| async move {
            UserPasskey::find_by_credential_id(&*db, &id).await
        })
        .await?;
    let mut used = match used {
        Ok(used) => used,
        Err(failure) => return Ok(refused(failure_message(failure))),
    };
    let Some(user) = User::find(&*state.core.db, used.stored.user_id).await? else {
        return Ok(refused(UNKNOWN_PASSKEY));
    };
    used.stored.save(&*state.core.db).await?;
    state.limiters.login_address.decrement(&address_key).await?;
    session.forget(PENDING_KEY);
    finish_sign_in(&state, &session, &cookies, &user, uri.query()).await
}

// -------------------------------------------------------------- second step

/// `GET /login/verify`
pub async fn show_verify(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(user) = pending_user(&state, &session).await? else {
        return Ok(sign_in_again(&session));
    };
    let status = TwoFactorStatus::of(&state.core.db, user.id).await?;
    let passkeys = status.passkeys > 0 && state.passkeys.is_available();
    let available = |method: VerifyMethod| match method {
        VerifyMethod::Passkey => passkeys,
        VerifyMethod::Totp => status.totp,
        VerifyMethod::Recovery => status.recovery_codes > 0,
    };
    let asked = input
        .get("method")
        .and_then(Value::as_str)
        .and_then(VerifyMethod::parse)
        .filter(|method| available(*method));
    let method = asked.unwrap_or(if passkeys {
        VerifyMethod::Passkey
    } else if status.totp {
        VerifyMethod::Totp
    } else {
        VerifyMethod::Recovery
    });
    let others: Vec<VerifyMethod> = [
        VerifyMethod::Passkey,
        VerifyMethod::Totp,
        VerifyMethod::Recovery,
    ]
    .into_iter()
    .filter(|other| *other != method && available(*other))
    .collect();
    Ok(page(verify_page(
        &context,
        &VerifyPage {
            method,
            others: &others,
            form: &FormState::from_session(&session),
        },
    )))
}

/// `POST /login/verify/totp`
pub async fn verify_totp(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(user) = pending_user(&state, &session).await? else {
        return Ok(sign_in_again(&session));
    };
    let method = VerifyMethod::Totp;
    let code: String = match TOTP_CODE_VALIDATOR.validate(&Value::Object(input.clone())) {
        Ok(valid) => valid["code"].as_str().unwrap_or_default().to_string(),
        Err(error) => {
            FormState::new(&error, &input).flash(&session);
            return Ok(redirect_to(&format!("{VERIFY_PATH}?method=totp")));
        }
    };
    if let Err(limited) = consume(&state.limiters.two_factor, &two_factor_key(user.id)).await? {
        return Ok(limited);
    }
    let secret = UserTotpSecret::for_user(&*state.core.db, user.id)
        .await?
        .filter(UserTotpSecret::is_confirmed);
    let Some(mut secret) = secret else {
        return Ok(refuse_step(&session, method, WRONG_CODE, &input));
    };
    let key = state
        .core
        .encryption
        .decrypt(&secret.secret)
        .and_then(|base32| TotpKey::from_base32(&base32, &user.email));
    let Some(step) = key.and_then(|key| key.matching_step(&code, unix_now())) else {
        return Ok(refuse_step(&session, method, WRONG_CODE, &input));
    };
    if !secret.consume_step(&*state.core.db, step).await? {
        return Ok(refuse_step(&session, method, REPLAYED_CODE, &input));
    }
    complete(&state, &session, &cookies, &user, uri.query()).await
}

/// `POST /login/verify/recovery`
pub async fn verify_recovery_code(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(user) = pending_user(&state, &session).await? else {
        return Ok(sign_in_again(&session));
    };
    let method = VerifyMethod::Recovery;
    let code: String = match RECOVERY_CODE_VALIDATOR.validate(&Value::Object(input.clone())) {
        Ok(valid) => valid["code"].as_str().unwrap_or_default().to_string(),
        Err(error) => {
            FormState::new(&error, &input).flash(&session);
            return Ok(redirect_to(&format!("{VERIFY_PATH}?method=recovery")));
        }
    };
    if let Err(limited) = consume(&state.limiters.two_factor, &two_factor_key(user.id)).await? {
        return Ok(limited);
    }
    if !UserRecoveryCode::redeem(&*state.core.db, user.id, &hash_recovery_code(&code)).await? {
        return Ok(refuse_step(&session, method, WRONG_RECOVERY_CODE, &input));
    }
    let left = UserRecoveryCode::remaining(&*state.core.db, user.id).await?;
    let response = complete(&state, &session, &cookies, &user, uri.query()).await?;
    session.flash(
        "success",
        match left {
            0 => "You used your last recovery code. Generate new ones in Settings.".to_string(),
            1 => "Recovery code used. 1 code left.".to_string(),
            left => format!("Recovery code used. {left} codes left."),
        },
    );
    Ok(response)
}

/// `POST /login/verify/passkey/options`
pub async fn verify_passkey_options(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, AppError> {
    let Some(user) = pending_user(&state, &session).await? else {
        return Ok(json_error(StatusCode::UNAUTHORIZED, EXPIRED));
    };
    if !state.passkeys.is_available() {
        return Ok(json_error(StatusCode::NOT_FOUND, PASSKEYS_OFF));
    }
    let passkeys = UserPasskey::for_user(&*state.core.db, user.id).await?;
    if passkeys.is_empty() {
        return Ok(json_error(StatusCode::NOT_FOUND, UNKNOWN_PASSKEY));
    }
    Ok(
        match state
            .passkeys
            .start_second_step(&session, user.id, &passkeys)
        {
            Some(options) => json_response(StatusCode::OK, &options),
            None => json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY),
        },
    )
}

/// `POST /login/verify/passkey`
pub async fn verify_passkey(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(user) = pending_user(&state, &session).await? else {
        return Ok(sign_in_again(&session));
    };
    let method = VerifyMethod::Passkey;
    let credential = match credential_of(&input)? {
        Ok(credential) => credential,
        Err(message) => return Ok(refuse_step(&session, method, &message, &Map::new())),
    };
    if let Err(limited) = consume(&state.limiters.two_factor, &two_factor_key(user.id)).await? {
        return Ok(limited);
    }
    let passkeys = UserPasskey::for_user(&*state.core.db, user.id).await?;
    let mut used = match state
        .passkeys
        .finish_second_step(&session, user.id, &credential, passkeys)
    {
        Ok(used) => used,
        Err(failure) => {
            return Ok(refuse_step(
                &session,
                method,
                failure_message(failure),
                &Map::new(),
            ));
        }
    };
    used.stored.save(&*state.core.db).await?;
    complete(&state, &session, &cookies, &user, uri.query()).await
}

// ----------------------------------------------------------------- settings

/// Retire every other session of the user: whoever held one signed in with
/// the password alone, which no longer suffices. This browser stays signed in.
async fn sign_out_other_browsers(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
    user: &mut User,
) -> Result<(), AppError> {
    let mut transaction = state.core.db.begin().await?;
    user.invalidate_sessions(&mut *transaction).await?;
    sqlx::query("delete from `remember_me_tokens` where `tokenable_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    sign_in(&state.core, session, cookies, user).await?;
    Ok(())
}

/// Replace the recovery codes of the user, and keep the new ones for the
/// next page, the only one that shows them.
async fn issue_recovery_codes(
    state: &AppState,
    session: &Session,
    user_id: i64,
) -> Result<(), AppError> {
    let codes = generate_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|code| hash_recovery_code(code)).collect();
    let mut transaction = state.core.db.begin().await?;
    UserRecoveryCode::replace_all(&mut transaction, user_id, &hashes).await?;
    transaction.commit().await?;
    session.flash(RECOVERY_CODES_KEY, &codes);
    Ok(())
}

/// What follows a new passkey or authenticator app. When it is the first
/// factor of the account, the person gets recovery codes, and the browsers
/// signed in with the password alone are signed out.
async fn factor_added(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
    headers: &HeaderMap,
    user: &mut User,
    was_enabled: bool,
    message: &str,
) -> Result<Response, AppError> {
    session.flash("success", message);
    if was_enabled {
        return Ok(navigate(headers, "/settings"));
    }
    issue_recovery_codes(state, session, user.id).await?;
    sign_out_other_browsers(state, session, cookies, user).await?;
    Ok(navigate(headers, RECOVERY_CODES_PATH))
}

async fn check_password(
    state: &AppState,
    user: &User,
    input: &Map<String, Value>,
) -> Result<Result<(), Option<Response>>, AppError> {
    let password = input
        .get("currentPassword")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(
        match confirm_current_password(state, user, password).await? {
            PasswordCheck::Confirmed => Ok(()),
            PasswordCheck::Wrong => Err(None),
            PasswordCheck::Limited(response) => Err(Some(response)),
        },
    )
}

/// `POST /settings/two-factor/totp`: a new secret, to confirm with a code.
pub async fn setup_totp(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            SETUP_TOTP_DIALOG,
            &form,
            setup_totp_form(&context, &form),
        )
    };
    if let Err(error) = CONFIRM_PASSWORD_VALIDATOR.validate(&Value::Object(input.clone())) {
        return Ok(refused(FormState::new(&error, &input)));
    }
    match check_password(&state, &user, &input).await? {
        Ok(()) => {}
        Err(None) => return Ok(refused(wrong_current_password(&input))),
        Err(Some(limited)) => return Ok(limited),
    }

    let db = &state.core.db;
    let existing = UserTotpSecret::for_user(&**db, user.id).await?;
    if existing.as_ref().is_some_and(UserTotpSecret::is_confirmed) {
        session.flash("error", "The authenticator app is already on");
        return Ok(navigate(&headers, "/settings"));
    }
    let key = TotpKey::generate(&user.email);
    let mut secret = existing.unwrap_or(UserTotpSecret {
        user_id: user.id,
        ..Default::default()
    });
    secret.secret = state.core.encryption.encrypt(&key.base32());
    secret.last_used_step = None;
    secret.save(&**db).await?;
    Ok(navigate(&headers, TOTP_SETUP_PATH))
}

/// The secret of the user that waits for its first code.
async fn unconfirmed_key(state: &AppState, user: &User) -> Result<Option<TotpKey>, AppError> {
    let secret = UserTotpSecret::for_user(&*state.core.db, user.id)
        .await?
        .filter(|secret| !secret.is_confirmed());
    Ok(secret
        .and_then(|secret| state.core.encryption.decrypt(&secret.secret))
        .and_then(|base32| TotpKey::from_base32(&base32, &user.email)))
}

/// `GET /settings/two-factor/totp`
pub async fn show_totp_setup(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    let Some(key) = unconfirmed_key(&state, &user).await? else {
        return Ok(redirect_to("/settings"));
    };
    Ok(page(totp_setup_page(
        &context,
        &key,
        &FormState::from_session(&session),
    )))
}

/// `POST /settings/two-factor/totp/confirm`
pub async fn confirm_totp(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    CurrentUser(mut user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        form.flash(&session);
        redirect_to(TOTP_SETUP_PATH)
    };
    let code: String = match TOTP_CODE_VALIDATOR.validate(&Value::Object(input.clone())) {
        Ok(valid) => valid["code"].as_str().unwrap_or_default().to_string(),
        Err(error) => return Ok(refused(FormState::new(&error, &input))),
    };
    let key = format!("two-factor-setup:{}", user.id);
    if let Err(limited) = consume(&state.limiters.two_factor, &key).await? {
        return Ok(limited);
    }
    let db = &state.core.db;
    let secret = UserTotpSecret::for_user(&**db, user.id)
        .await?
        .filter(|secret| !secret.is_confirmed());
    let Some(mut secret) = secret else {
        return Ok(redirect_to("/settings"));
    };
    let step = state
        .core
        .encryption
        .decrypt(&secret.secret)
        .and_then(|base32| TotpKey::from_base32(&base32, &user.email))
        .and_then(|totp| totp.matching_step(&code, unix_now()));
    let Some(step) = step else {
        return Ok(refused(FormState::with_error("code", WRONG_CODE, &input)));
    };
    let was_enabled = TwoFactorStatus::of(db, user.id).await?.is_enabled();
    if !secret.consume_step(&**db, step).await? {
        return Ok(refused(FormState::with_error(
            "code",
            REPLAYED_CODE,
            &input,
        )));
    }
    secret.confirmed_at = Some(Timestamp::now());
    secret.save(&**db).await?;
    state.limiters.two_factor.delete(&key).await?;
    factor_added(
        &state,
        &session,
        &cookies,
        &headers,
        &mut user,
        was_enabled,
        "Authenticator app turned on",
    )
    .await
}

/// `DELETE /settings/two-factor/totp`
pub async fn disable_totp(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            DISABLE_TOTP_DIALOG,
            &form,
            disable_totp_form(&context, &form),
        )
    };
    if let Err(error) = CONFIRM_PASSWORD_VALIDATOR.validate(&Value::Object(input.clone())) {
        return Ok(refused(FormState::new(&error, &input)));
    }
    match check_password(&state, &user, &input).await? {
        Ok(()) => {}
        Err(None) => return Ok(refused(wrong_current_password(&input))),
        Err(Some(limited)) => return Ok(limited),
    }
    let mut transaction = state.core.db.begin().await?;
    sqlx::query("delete from `user_totp_secrets` where `user_id` = ?")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    forget_recovery_codes_if_unprotected(&mut transaction, user.id).await?;
    transaction.commit().await?;
    session.flash("success", "Authenticator app turned off");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}

/// `POST /settings/two-factor/recovery-codes`
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            REGENERATE_CODES_DIALOG,
            &form,
            regenerate_codes_form(&context, &form),
        )
    };
    if let Err(error) = CONFIRM_PASSWORD_VALIDATOR.validate(&Value::Object(input.clone())) {
        return Ok(refused(FormState::new(&error, &input)));
    }
    match check_password(&state, &user, &input).await? {
        Ok(()) => {}
        Err(None) => return Ok(refused(wrong_current_password(&input))),
        Err(Some(limited)) => return Ok(limited),
    }
    if !TwoFactorStatus::of(&state.core.db, user.id)
        .await?
        .is_enabled()
    {
        session.flash(
            "error",
            "Turn on a passkey or an authenticator app before recovery codes",
        );
        return Ok(navigate(&headers, "/settings"));
    }
    issue_recovery_codes(&state, &session, user.id).await?;
    session.flash("success", "New recovery codes generated");
    Ok(navigate(&headers, RECOVERY_CODES_PATH))
}

/// `GET /settings/two-factor/recovery-codes`: the codes that were just
/// generated, once.
pub async fn show_recovery_codes(context: PageContext, session: Session) -> Response {
    let codes: Vec<String> = session
        .flashed(RECOVERY_CODES_KEY)
        .map_or_else(Vec::new, |codes| {
            serde_json::from_value(codes).unwrap_or_default()
        });
    if codes.is_empty() {
        return redirect_to("/settings");
    }
    page(recovery_codes_page(&context, &codes))
}

/// `POST /settings/passkeys/options`: the name and the password are
/// checked, then the browser gets what it needs to create the passkey.
pub async fn passkey_registration_options(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    if !state.passkeys.is_available() {
        return Ok(json_error(StatusCode::NOT_FOUND, PASSKEYS_OFF));
    }
    let input = without_credential(&input);
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            ADD_PASSKEY_DIALOG,
            &form,
            add_passkey_form(&context, &form),
        )
    };
    let name = match PASSKEY_OPTIONS_VALIDATOR.validate(&Value::Object(input.clone())) {
        Ok(valid) => valid["name"].as_str().unwrap_or_default().to_string(),
        Err(error) => return Ok(refused(FormState::new(&error, &input))),
    };
    match check_password(&state, &user, &input).await? {
        Ok(()) => {}
        Err(None) => return Ok(refused(wrong_current_password(&input))),
        Err(Some(limited)) => return Ok(limited),
    }
    let existing = UserPasskey::for_user(&*state.core.db, user.id).await?;
    Ok(
        match state
            .passkeys
            .start_registration(&session, &user, &name, &existing)
        {
            Some(options) => json_response(StatusCode::OK, &options),
            None => json_error(StatusCode::SERVICE_UNAVAILABLE, BUSY),
        },
    )
}

/// `POST /settings/passkeys`: the passkey the browser created.
pub async fn add_passkey(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    cookies: Cookies,
    CurrentUser(mut user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let shown = without_credential(&input);
    let refused = |message: &str| {
        let form = FormState::with_error("credential", message, &shown);
        refuse(
            &headers,
            &session,
            ADD_PASSKEY_DIALOG,
            &form,
            add_passkey_form(&context, &form),
        )
    };
    let credential = match credential_of(&input)? {
        Ok(credential) => credential,
        Err(message) => return Ok(refused(&message)),
    };
    let db = &state.core.db;
    // Checked first: webauthn-rs refuses a credential the options excluded,
    // under the name of another error.
    if let Some(id) = Passkeys::registered_id(&credential)
        && UserPasskey::find_by_credential_id(&**db, &id)
            .await?
            .is_some()
    {
        return Ok(refused(ALREADY_REGISTERED));
    }
    let new = match state
        .passkeys
        .finish_registration(&session, &user, &credential)
    {
        Ok(new) => new,
        Err(PasskeyFailure::NoCeremony) => return Ok(refused(NO_CEREMONY)),
        Err(_) => return Ok(refused("The passkey could not be added. Try again.")),
    };
    if UserPasskey::find_by_credential_id(&**db, &new.credential_id)
        .await?
        .is_some()
    {
        return Ok(refused(ALREADY_REGISTERED));
    }
    let was_enabled = TwoFactorStatus::of(db, user.id).await?.is_enabled();
    let NewPasskey {
        user_id,
        name,
        credential_id,
        user_handle,
        passkey,
    } = new;
    UserPasskey {
        user_id,
        name,
        credential_id,
        user_handle,
        passkey,
        ..Default::default()
    }
    .insert(&**db)
    .await?;
    factor_added(
        &state,
        &session,
        &cookies,
        &headers,
        &mut user,
        was_enabled,
        "Passkey added",
    )
    .await
}

async fn passkey_of(
    state: &AppState,
    user: &User,
    id: &str,
) -> Result<Option<UserPasskey>, AppError> {
    Ok(match record_id(id) {
        Some(id) => UserPasskey::find_for_user(&*state.core.db, user.id, id).await?,
        None => None,
    })
}

#[derive(Deserialize)]
struct Rename {
    name: String,
}

/// `PATCH /settings/passkeys/{id}`
#[allow(clippy::too_many_arguments)]
pub async fn rename_passkey(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut passkey) = passkey_of(&state, &user, &id).await? else {
        session.flash("error", "Passkey not found");
        return Ok(navigate(&headers, "/settings"));
    };
    let payload: Rename = match RENAME_PASSKEY_VALIDATOR.validate_as(&Value::Object(input.clone()))
    {
        Ok(payload) => payload,
        Err(error) => {
            let form = FormState::new(&refusal(error)?, &input);
            return Ok(refuse(
                &headers,
                &session,
                &rename_passkey_dialog_id(passkey.id),
                &form,
                rename_passkey_form(&context, &passkey, &form),
            ));
        }
    };
    passkey.name = payload.name;
    passkey.save(&*state.core.db).await?;
    session.flash("success", "Passkey renamed");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}

/// `DELETE /settings/passkeys/{id}`
#[allow(clippy::too_many_arguments)]
pub async fn remove_passkey(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(passkey) = passkey_of(&state, &user, &id).await? else {
        session.flash("error", "Passkey not found");
        return Ok(navigate(&headers, "/settings"));
    };
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            &remove_passkey_dialog_id(passkey.id),
            &form,
            remove_passkey_form(&context, &passkey, &form),
        )
    };
    if let Err(error) = CONFIRM_PASSWORD_VALIDATOR.validate(&Value::Object(input.clone())) {
        return Ok(refused(FormState::new(&error, &input)));
    }
    match check_password(&state, &user, &input).await? {
        Ok(()) => {}
        Err(None) => return Ok(refused(wrong_current_password(&input))),
        Err(Some(limited)) => return Ok(limited),
    }
    let mut transaction = state.core.db.begin().await?;
    passkey.delete(&mut *transaction).await?;
    forget_recovery_codes_if_unprotected(&mut transaction, user.id).await?;
    transaction.commit().await?;
    session.flash("success", "Passkey removed");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}

/// The sign-in page with the passkey button when passkeys work here.
pub fn login_view(state: &AppState, context: &PageContext, session: &Session) -> Response {
    page(login_page(
        context,
        &FormState::from_session(session),
        state.passkeys.is_available(),
    ))
}

/// `with_query` for the verification page, kept here with its path.
pub fn verify_path(query: Option<&str>) -> String {
    with_query(VERIFY_PATH, query)
}
