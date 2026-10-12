//! Passkeys: adding one to an account and removing it, under Settings, and
//! signing in with one, alone or as the second step after a password.
//!
//! A passkey is asked for by the page's script: the browser has no form
//! for it. The script asks the server what to sign, the browser for the
//! passkey, and posts what the passkey answered as JSON. The server checks
//! the answer against the challenge it kept in the session, which it gives
//! once.

use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::client_ip::rate_limit_client_key;
use mymcps_core::crypto::webauthn::{self, ALGORITHMS, RelyingParty};
use mymcps_core::crypto::{random_base64url, sha256_hex};
use mymcps_core::models::{Passkey, RecoveryCode, User};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::CurrentUser;
use crate::client_ip::ClientIp;
use crate::cookies::Cookies;
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::redirect_to;
use crate::respond::{fragment, navigate};
use crate::routes::FeatureRoutes;
use crate::routes::auth::consume;
use crate::routes::logs::{fragment_target, uncached};
use crate::routes::settings::{
    self, PasswordCheck, REMOVE_PASSKEY_FORM, confirm_current_password, wrong_current_password,
};
use crate::routes::two_factor::{after_protection, finish_sign_in, pending_sign_in};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::record_id;
use crate::validators::two_factor::{
    CONFIRM_PASSWORD_VALIDATOR, NEW_PASSKEY_VALIDATOR, PASSKEY_ASSERTION_VALIDATOR,
    PASSKEY_REGISTRATION_VALIDATOR,
};
use crate::views::shell::PageContext;
use crate::views::two_factor::{
    ISSUER, REMOVE_PASSKEY_DIALOG, add_passkey_form, remove_passkey_form, remove_passkey_href,
};

/// Session key of the challenge a passkey was asked to answer.
pub(crate) const CEREMONY_KEY: &str = "passkeyCeremony";
/// How long the browser and the person have to answer it.
const CEREMONY_MS: i64 = 5 * 60 * 1000;
/// How many passkeys an account keeps.
const MAX_PASSKEYS: usize = 20;
/// What a passkey is called when the person gave it no name.
const DEFAULT_NAME: &str = "Passkey";

const UNREADABLE: &str = "The answer of the browser could not be read. Try again.";
const EXPIRED: &str = "The request expired. Try again.";
const NO_ADDRESS: &str = "Passkeys need APP_URL to be the address of this instance";
const UNKNOWN: &str = "This passkey is not one of this instance";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        guest: Router::new()
            .route("/login/passkey/options", post(sign_in_options))
            .route("/login/passkey", post(sign_in_with_passkey)),
        signed_in: Router::new()
            .route("/settings/passkeys/options", post(options))
            .route("/settings/passkeys", post(store))
            .route("/settings/passkeys/{id}/delete", get(confirm_destroy))
            .route("/settings/passkeys/{id}", delete(destroy)),
        ..Default::default()
    }
}

fn now_ms() -> i64 {
    Timestamp::now().timestamp_millis()
}

/// JSON for the page's script, which no browser keeps.
fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [
            (CONTENT_TYPE, "application/json; charset=utf-8"),
            (CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

/// What the script shows the person when a passkey is refused.
fn refuse(message: &str) -> Response {
    json_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        &json!({ "message": message }),
    )
}

/// The instance as a passkey knows it: the host and the origin of `APP_URL`.
/// A passkey made under one address answers under no other.
fn relying_party(state: &AppState) -> Option<RelyingParty> {
    state
        .core
        .config
        .public_app_url()
        .and_then(|url| RelyingParty::from_url(&url))
}

fn decode(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

fn descriptors(passkeys: &[Passkey]) -> Vec<Value> {
    passkeys
        .iter()
        .map(|passkey| json!({ "type": "public-key", "id": passkey.credential_id }))
        .collect()
}

/// What an authenticator files the passkeys of an account under: a value
/// that says nothing of the person, and that only this instance derives.
fn user_handle(state: &AppState, user: &User) -> String {
    let mut hash = Sha256::new();
    hash.update(state.core.encryption.signing_key());
    hash.update(b"passkey-user:");
    hash.update(user.id.to_string().as_bytes());
    URL_SAFE_NO_PAD.encode(hash.finalize())
}

/// A challenge a passkey was asked to answer, and what for.
#[derive(Serialize, Deserialize)]
struct Ceremony {
    challenge: String,
    at: i64,
    /// The account a passkey is being added to, with the name it gets.
    /// `None` when the passkey is asked to sign someone in.
    adding: Option<(i64, String)>,
}

impl Ceremony {
    /// Remember a new challenge in this browser, in place of the last one.
    fn begin(session: &Session, adding: Option<(i64, String)>) -> String {
        let challenge = random_base64url(32);
        session.put(
            CEREMONY_KEY,
            Self {
                challenge: challenge.clone(),
                at: now_ms(),
                adding,
            },
        );
        challenge
    }

    /// The challenge this browser was given, which is answered once:
    /// whatever the answer turns out to be, it is gone. The session forgets
    /// it, and since a browser can send the cookie of before again, the
    /// server remembers that it was answered for as long as it could be.
    async fn take(state: &AppState, session: &Session) -> Result<Option<Self>, sqlx::Error> {
        let ceremony = session
            .pull(CEREMONY_KEY)
            .and_then(|stored| serde_json::from_value::<Self>(stored).ok())
            .filter(|ceremony| (0..CEREMONY_MS).contains(&(now_ms() - ceremony.at)));
        let Some(ceremony) = ceremony else {
            return Ok(None);
        };
        let key = format!("passkey-challenge:{}", sha256_hex(&ceremony.challenge));
        let first = state.limiters.passkey_challenge.attempt(&key).await?;
        Ok(first.then_some(ceremony))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewPasskey {
    name: Option<String>,
    current_password: String,
}

/// `POST /settings/passkeys/options`
///
/// The first request of the "Add passkey" dialog: the name and the password
/// are checked, and the script gets what `navigator.credentials.create()`
/// takes, or the form again with what is wrong.
pub async fn options(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        fragment(
            StatusCode::UNPROCESSABLE_ENTITY,
            add_passkey_form(&context, &form),
        )
    };
    let NewPasskey {
        name,
        current_password,
    } = match NEW_PASSKEY_VALIDATOR.validate_as(&Value::Object(input.clone())) {
        Ok(passkey) => passkey,
        Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
    };
    let Some(rp) = relying_party(&state) else {
        return Ok(refused(FormState::with_error("name", NO_ADDRESS, &input)));
    };
    match confirm_current_password(&state, &user, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }
    let passkeys = Passkey::of_user(&*state.core.db, user.id).await?;
    if passkeys.len() >= MAX_PASSKEYS {
        let message = "An account keeps 20 passkeys at most. Remove one first.";
        return Ok(refused(FormState::with_error("name", message, &input)));
    }

    let name = name.unwrap_or_else(|| DEFAULT_NAME.to_string());
    let challenge = Ceremony::begin(&session, Some((user.id, name)));
    let algorithms: Vec<Value> = ALGORITHMS
        .iter()
        .map(|algorithm| json!({ "type": "public-key", "alg": algorithm }))
        .collect();
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "rp": { "id": rp.id, "name": ISSUER },
            "user": {
                "id": user_handle(&state, &user),
                "name": user.email,
                "displayName": user.display_name(),
            },
            "challenge": challenge,
            "pubKeyCredParams": algorithms,
            "timeout": CEREMONY_MS,
            "attestation": "none",
            // A key the authenticator keeps with the account it is for, and
            // only uses once it has checked a fingerprint, a face or a PIN.
            "authenticatorSelection": {
                "residentKey": "required",
                "requireResidentKey": true,
                "userVerification": "required",
            },
            // An authenticator makes one passkey for an account.
            "excludeCredentials": descriptors(&passkeys),
        }),
    ))
}

#[derive(Deserialize)]
struct Registered {
    id: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "attestationObject")]
    attestation_object: String,
}

/// `POST /settings/passkeys`
///
/// The second request of the dialog: the passkey the browser made. An
/// account that had no protection yet gets its recovery codes with it.
pub async fn store(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Ok(registered) =
        PASSKEY_REGISTRATION_VALIDATOR.validate_as::<Registered>(&Value::Object(input))
    else {
        return Ok(refuse(UNREADABLE));
    };
    let Some(rp) = relying_party(&state) else {
        return Ok(refuse(NO_ADDRESS));
    };
    // Asked for by this account, in this browser, a moment ago.
    let Some((challenge, name)) = Ceremony::take(&state, &session)
        .await?
        .and_then(|ceremony| {
            let (user_id, name) = ceremony.adding?;
            (user_id == user.id).then_some((ceremony.challenge, name))
        })
    else {
        return Ok(refuse(EXPIRED));
    };
    let (Some(challenge), Some(client_data), Some(attestation)) = (
        decode(&challenge),
        decode(&registered.client_data_json),
        decode(&registered.attestation_object),
    ) else {
        return Ok(refuse(UNREADABLE));
    };
    let registration =
        match webauthn::verify_registration(&rp, &challenge, &client_data, &attestation) {
            Ok(registration) => registration,
            Err(error) => return Ok(refuse(&format!("The passkey was not added: {error}."))),
        };
    let credential_id = URL_SAFE_NO_PAD.encode(&registration.credential_id);
    if credential_id != registered.id {
        return Ok(refuse(UNREADABLE));
    }

    let mut transaction = state.core.db.begin().await?;
    if Passkey::find_by_credential_id(&mut *transaction, &credential_id)
        .await?
        .is_some()
    {
        return Ok(refuse("This passkey is already one of an account"));
    }
    let was_protected = user.is_protected(&mut *transaction).await?;
    let mut passkey = Passkey {
        user_id: user.id,
        name,
        credential_id,
        public_key: URL_SAFE_NO_PAD.encode(&registration.public_key),
        sign_count: i64::from(registration.sign_count),
        backed_up: registration.backed_up,
        ..Default::default()
    };
    passkey.insert(&mut *transaction).await?;
    let codes = if was_protected {
        None
    } else {
        Some(RecoveryCode::replace_all(&mut transaction, user.id).await?)
    };
    transaction.commit().await?;

    session.flash("success", "Passkey added");
    Ok(navigate(&headers, after_protection(&session, codes)))
}

async fn find_passkey(
    state: &AppState,
    user: &User,
    id: &str,
) -> Result<Option<Passkey>, sqlx::Error> {
    let Some(id) = record_id(id) else {
        return Ok(None);
    };
    Ok(Passkey::find(&*state.core.db, id)
        .await?
        .filter(|passkey| passkey.user_id == user.id))
}

/// `GET /settings/passkeys/{id}/delete`: the Settings page with the dialog
/// that removes this passkey open. The page script asks for the content of
/// the dialog alone.
pub async fn confirm_destroy(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(passkey) = find_passkey(&state, &user, &id).await? else {
        session.flash("error", "Passkey not found");
        return Ok(navigate(&headers, "/settings"));
    };
    if is_fetch(&headers) && fragment_target(&headers).as_deref() == Some(REMOVE_PASSKEY_DIALOG) {
        return Ok(uncached(fragment(
            StatusCode::OK,
            remove_passkey_form(&context, &passkey, &FormState::default()),
        )));
    }
    settings::render(&state, &context, &session, &user, Some(&passkey)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Confirmation {
    current_password: String,
}

/// `DELETE /settings/passkeys/{id}`
///
/// Remove a passkey of one's own. The recovery codes go with the last one
/// when the account has no authenticator app either.
pub async fn destroy(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(passkey) = find_passkey(&state, &user, &id).await? else {
        session.flash("error", "Passkey not found");
        return Ok(navigate(&headers, "/settings"));
    };
    // The script gets the form again with what is wrong. Without it, the
    // browser goes to the page that shows the dialog open on it.
    let refused = |form: FormState| {
        if is_fetch(&headers) {
            return fragment(
                StatusCode::UNPROCESSABLE_ENTITY,
                remove_passkey_form(&context, &passkey, &form),
            );
        }
        form.flash(&session);
        session.flash(settings::REFUSED_FORM_KEY, REMOVE_PASSKEY_FORM);
        redirect_to(&remove_passkey_href(&passkey))
    };
    let Confirmation { current_password } =
        match CONFIRM_PASSWORD_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(confirmation) => confirmation,
            Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
        };
    match confirm_current_password(&state, &user, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let mut transaction = state.core.db.begin().await?;
    sqlx::query("delete from `passkeys` where `id` = ? and `user_id` = ?")
        .bind(passkey.id)
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    user.settle_recovery_codes(&mut transaction).await?;
    transaction.commit().await?;

    session.flash("success", "Passkey removed");
    Ok(navigate(&headers, "/settings"))
}

/// The allowance of sign-in attempts of a client address: each passkey
/// answer costs a lookup and a signature check.
fn address_key(ip: &str) -> String {
    format!("login-address:{}", rate_limit_client_key(ip))
}

/// `POST /login/passkey/options`
///
/// What `navigator.credentials.get()` takes. After a password, the
/// passkeys of that account; before one, any passkey the browser has for
/// this instance, which says whose it is.
pub async fn sign_in_options(
    State(state): State<AppState>,
    session: Session,
    ClientIp(ip): ClientIp,
) -> Result<Response, AppError> {
    let Some(rp) = relying_party(&state) else {
        return Ok(refuse(NO_ADDRESS));
    };
    if let Err(refused) = consume(&state.limiters.login_address, &address_key(&ip)).await? {
        return Ok(refused);
    }
    let allowed = match pending_sign_in(&state, &session).await? {
        Some((user, _)) => Passkey::of_user(&*state.core.db, user.id).await?,
        None => Vec::new(),
    };
    let challenge = Ceremony::begin(&session, None);
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "challenge": challenge,
            "rpId": rp.id,
            "timeout": CEREMONY_MS,
            "userVerification": "required",
            "allowCredentials": descriptors(&allowed),
        }),
    ))
}

#[derive(Deserialize)]
struct Answered {
    id: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
}

/// `POST /login/passkey`
///
/// Sign in with what a passkey answered. A passkey is something the person
/// has that checked something they are or know: it stands for the password
/// and the second step at once.
pub async fn sign_in_with_passkey(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &*state.core.db;
    let Ok(answered) = PASSKEY_ASSERTION_VALIDATOR.validate_as::<Answered>(&Value::Object(input))
    else {
        return Ok(refuse(UNREADABLE));
    };
    let Some(rp) = relying_party(&state) else {
        return Ok(refuse(NO_ADDRESS));
    };
    let Some(ceremony) = Ceremony::take(&state, &session)
        .await?
        .filter(|ceremony| ceremony.adding.is_none())
    else {
        return Ok(refuse(EXPIRED));
    };
    let pending = pending_sign_in(&state, &session).await?;

    let Some(passkey) = Passkey::find_by_credential_id(db, &answered.id).await? else {
        return Ok(refuse(UNKNOWN));
    };
    // After a password, only a passkey of the account it was the password of.
    if pending
        .as_ref()
        .is_some_and(|(user, _)| user.id != passkey.user_id)
    {
        return Ok(refuse("This passkey is the one of another account"));
    }
    let (Some(challenge), Some(public_key), Some(client_data), Some(data), Some(signature)) = (
        decode(&ceremony.challenge),
        decode(&passkey.public_key),
        decode(&answered.client_data_json),
        decode(&answered.authenticator_data),
        decode(&answered.signature),
    ) else {
        return Ok(refuse(UNREADABLE));
    };
    let assertion = match webauthn::verify_assertion(
        &rp,
        &challenge,
        &public_key,
        &client_data,
        &data,
        &signature,
    ) {
        Ok(assertion) => assertion,
        Err(error) => return Ok(refuse(&format!("The passkey was refused: {error}."))),
    };
    if !passkey.record_use(db, assertion).await? {
        tracing::warn!(
            passkey = passkey.id,
            "A passkey answered with a counter that went back"
        );
        return Ok(refuse(
            "The passkey was refused: a copy of it was used since. Remove it under Settings and add it again.",
        ));
    }
    let Some(user) = User::find(db, passkey.user_id).await? else {
        return Ok(refuse(UNKNOWN));
    };

    // The address gets back the attempt this request was charged.
    state
        .limiters
        .login_address
        .decrement(&address_key(&ip))
        .await?;
    let query = pending.and_then(|(_, query)| query);
    let destination = finish_sign_in(&state, &session, &cookies, &user, query.as_deref()).await?;
    Ok(navigate(&headers, &destination))
}
