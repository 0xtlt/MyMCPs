//! Sign-in protection: the authenticator app of an account and its recovery
//! codes, and the second step of a sign-in, which asks for one of them or
//! for a passkey once the password was right. `passkeys.rs` has the passkeys.
//!
//! Turning the protection on, off, or making new codes asks for the
//! password again: a signed-in browser is no proof that it is the person.

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use http::HeaderMap;
use mymcps_core::Timestamp;
use mymcps_core::crypto::totp;
use mymcps_core::models::{Passkey, RecoveryCode, User};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auth::{CurrentUser, sign_in};
use crate::cookies::Cookies;
use crate::error::AppError;
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::redirect_to;
use crate::respond::{navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::auth::{consume, signed_in_destination};
use crate::routes::logs::uncached;
use crate::routes::passkeys::CEREMONY_KEY;
use crate::routes::settings::{
    AUTHENTICATOR_FORM, PasswordCheck, confirm_current_password, refuse, wrong_current_password,
};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::two_factor::{
    CONFIRM_PASSWORD_VALIDATOR, ENABLE_AUTHENTICATOR_VALIDATOR, SECOND_STEP_VALIDATOR,
};
use crate::views::shell::PageContext;
use crate::views::two_factor::{
    SecondStep, authenticator_off_form, authenticator_setup_page, recovery_codes_page,
    second_step_page,
};

/// Session key of the secret an authenticator app is being given.
const SETUP_KEY: &str = "authenticatorSetup";
/// How long a secret that was shown may still be confirmed.
const SETUP_MS: i64 = 15 * 60 * 1000;
/// Session key of a sign-in whose password was right and whose second step
/// is awaited.
const PENDING_KEY: &str = "pendingSignIn";
/// How long the second step may take.
const PENDING_MS: i64 = 10 * 60 * 1000;
/// Flashed: the recovery codes that were just made, for the page that
/// shows them once.
const RECOVERY_CODES_KEY: &str = "recoveryCodes";

const SETUP_PATH: &str = "/settings/two-factor";
const RECOVERY_CODES_PATH: &str = "/settings/recovery-codes";
const SECOND_STEP_PATH: &str = "/login/two-factor";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        guest: Router::new().route(SECOND_STEP_PATH, get(show_second_step).post(second_step)),
        signed_in: Router::new()
            .route(SETUP_PATH, get(setup).post(enable).delete(disable))
            .route(
                RECOVERY_CODES_PATH,
                get(recovery_codes).post(renew_recovery_codes),
            ),
        ..Default::default()
    }
}

fn now_ms() -> i64 {
    Timestamp::now().timestamp_millis()
}

/// Whether something that happened at `at` did less than `within`
/// milliseconds ago. A time in the future did not.
fn is_recent(at: i64, within: i64) -> bool {
    (0..within).contains(&(now_ms() - at))
}

#[derive(Serialize, Deserialize)]
struct Setup {
    secret: String,
    at: i64,
}

impl Setup {
    /// The secret this browser was shown, while it may still be confirmed.
    fn current(session: &Session) -> Option<Self> {
        session
            .get_as::<Self>(SETUP_KEY)
            .filter(|setup| is_recent(setup.at, SETUP_MS))
    }
}

/// `GET /settings/two-factor`
///
/// The secret for the authenticator app. It stays in the session until a
/// code proves the app has it: nothing is asked for at sign-in before that.
pub async fn setup(
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    if user.has_authenticator() {
        return Ok(redirect_to("/settings"));
    }
    let secret = match Setup::current(&session) {
        Some(setup) => setup.secret,
        None => {
            let secret = totp::generate_secret();
            session.put(
                SETUP_KEY,
                Setup {
                    secret: secret.clone(),
                    at: now_ms(),
                },
            );
            secret
        }
    };
    Ok(uncached(page(authenticator_setup_page(
        &context,
        &user.email,
        &secret,
        &FormState::from_session(&session),
    ))))
}

/// Where the browser goes once an account got an authenticator app or a
/// passkey: to its recovery codes when these are its first, which the page
/// shows once.
pub(crate) fn after_protection(session: &Session, codes: Option<Vec<String>>) -> &'static str {
    match codes {
        Some(codes) => {
            session.flash(RECOVERY_CODES_KEY, codes);
            RECOVERY_CODES_PATH
        }
        None => "/settings",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enabling {
    code: String,
    current_password: String,
}

/// `POST /settings/two-factor`
///
/// Turn the authenticator app on, once a code shows the app has the secret.
pub async fn enable(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(mut user): CurrentUser,
    Input(input): Input,
) -> Result<Response, AppError> {
    if user.has_authenticator() {
        return Ok(redirect_to("/settings"));
    }
    let back = |form: FormState| {
        form.flash(&session);
        redirect_to(SETUP_PATH)
    };
    let Enabling {
        code,
        current_password,
    } = match ENABLE_AUTHENTICATOR_VALIDATOR.validate_as(&Value::Object(input.clone())) {
        Ok(enabling) => enabling,
        Err(error) => return Ok(back(FormState::new(&refusal(error)?, &input))),
    };
    let Some(setup) = Setup::current(&session) else {
        // The page gives the app a new secret.
        session.flash("error", "The key expired. Scan the new one.");
        return Ok(redirect_to(SETUP_PATH));
    };
    match confirm_current_password(&state, &user, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(back(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }
    let now = now_ms() / 1000;
    let Some(step) = totp::verify(&setup.secret, &code, now, None) else {
        let message = "This is not the code the app shows now";
        return Ok(back(FormState::with_error("code", message, &Map::new())));
    };

    let mut transaction = state.core.db.begin().await?;
    let was_protected = user.is_protected(&mut *transaction).await?;
    user.enable_authenticator(
        &mut transaction,
        &state.core.encryption,
        &setup.secret,
        step,
    )
    .await?;
    let codes = if was_protected {
        None
    } else {
        Some(RecoveryCode::replace_all(&mut transaction, user.id).await?)
    };
    transaction.commit().await?;

    session.forget(SETUP_KEY);
    session.flash("success", "Authenticator app turned on");
    Ok(redirect_to(after_protection(&session, codes)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Confirmation {
    current_password: String,
}

/// `DELETE /settings/two-factor`
///
/// Turn the authenticator app off. The recovery codes go with it when the
/// account has no passkey either: they would stand in for nothing.
pub async fn disable(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(mut user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            AUTHENTICATOR_FORM,
            &form,
            authenticator_off_form(&context, &form),
        )
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
    user.disable_authenticator(&mut transaction).await?;
    user.settle_recovery_codes(&mut transaction).await?;
    transaction.commit().await?;

    session.flash("success", "Authenticator app turned off");
    Ok(navigate(&headers, "/settings"))
}

/// `GET /settings/recovery-codes`
///
/// The codes that were just made, this once, or the form that makes new
/// ones.
pub async fn recovery_codes(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    let db = &*state.core.db;
    let codes: Option<Vec<String>> = session
        .flashed(RECOVERY_CODES_KEY)
        .and_then(|codes| serde_json::from_value(codes).ok());
    if codes.is_none() && !user.is_protected(db).await? {
        return Ok(redirect_to("/settings"));
    }
    let left = RecoveryCode::remaining(db, user.id).await?;
    Ok(uncached(page(recovery_codes_page(
        &context,
        codes.as_deref(),
        left,
        &FormState::from_session(&session),
    ))))
}

/// `POST /settings/recovery-codes`
///
/// New recovery codes, in place of the ones the account had.
pub async fn renew_recovery_codes(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(user): CurrentUser,
    Input(input): Input,
) -> Result<Response, AppError> {
    let back = |form: FormState| {
        form.flash(&session);
        redirect_to(RECOVERY_CODES_PATH)
    };
    let Confirmation { current_password } =
        match CONFIRM_PASSWORD_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(confirmation) => confirmation,
            Err(error) => return Ok(back(FormState::new(&refusal(error)?, &input))),
        };
    match confirm_current_password(&state, &user, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(back(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let mut transaction = state.core.db.begin().await?;
    if !user.is_protected(&mut *transaction).await? {
        return Ok(redirect_to("/settings"));
    }
    let codes = RecoveryCode::replace_all(&mut transaction, user.id).await?;
    transaction.commit().await?;

    session.flash(RECOVERY_CODES_KEY, codes);
    Ok(redirect_to(RECOVERY_CODES_PATH))
}

#[derive(Serialize, Deserialize)]
struct PendingSignIn {
    user_id: i64,
    /// The sessions of the account when its password was typed: a password
    /// changed meanwhile ends the sign-in it had opened.
    session_version: i64,
    at: i64,
    /// The query string of the sign-in page, which the page after it gets.
    query: Option<String>,
}

/// Someone types a password in this browser: the sign-in another password
/// had opened here is over, whoever it was for.
pub(crate) fn forget_pending_sign_in(session: &Session) {
    session.forget(PENDING_KEY);
    session.forget(CEREMONY_KEY);
}

/// The password of a protected account was right: remember who typed it
/// in this browser, and ask for the rest.
pub(crate) fn begin_second_step(session: &Session, user: &User, query: Option<&str>) -> Response {
    session.put(
        PENDING_KEY,
        PendingSignIn {
            user_id: user.id,
            session_version: user.session_version,
            at: now_ms(),
            query: query.map(str::to_string),
        },
    );
    redirect_to(SECOND_STEP_PATH)
}

/// Who typed their password in this browser and has not finished signing
/// in, with the query string of the page they did it on.
pub(crate) async fn pending_sign_in(
    state: &AppState,
    session: &Session,
) -> Result<Option<(User, Option<String>)>, sqlx::Error> {
    let Some(pending) = session.get_as::<PendingSignIn>(PENDING_KEY) else {
        return Ok(None);
    };
    let user = if is_recent(pending.at, PENDING_MS) {
        User::find(&*state.core.db, pending.user_id)
            .await?
            .filter(|user| user.session_version == pending.session_version)
    } else {
        None
    };
    if user.is_none() {
        session.forget(PENDING_KEY);
    }
    Ok(user.map(|user| (user, pending.query)))
}

/// Sign someone in whose sign-in is proven, by a code or by a passkey, and
/// say where their browser goes.
pub(crate) async fn finish_sign_in(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
    user: &User,
    query: Option<&str>,
) -> Result<String, AppError> {
    forget_pending_sign_in(session);
    sign_in(&state.core, session, cookies, user).await?;
    Ok(signed_in_destination(session, query))
}

/// `GET /login/two-factor`
pub async fn show_second_step(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
) -> Result<Response, AppError> {
    let Some((user, _)) = pending_sign_in(&state, &session).await? else {
        return Ok(redirect_to("/login"));
    };
    let step = SecondStep {
        email: &user.email,
        authenticator: user.has_authenticator(),
        passkeys: Passkey::count_of_user(&*state.core.db, user.id).await? > 0,
    };
    Ok(uncached(page(second_step_page(
        &context,
        step,
        &FormState::from_session(&session),
    ))))
}

#[derive(Deserialize)]
struct Answer {
    code: String,
}

/// `POST /login/two-factor`
///
/// The second step, answered with a code: one of the authenticator app,
/// which works once, or a recovery code, which is used up.
pub async fn second_step(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &*state.core.db;
    let Some((user, query)) = pending_sign_in(&state, &session).await? else {
        session.flash("error", "Sign in again: the second step took too long");
        return Ok(redirect_to("/login"));
    };
    // What was typed is not sent back: a code is of no use the next time.
    let back = |form: FormState| {
        form.flash(&session);
        redirect_to(SECOND_STEP_PATH)
    };
    let Answer { code } = match SECOND_STEP_VALIDATOR.validate_as(&Value::Object(input)) {
        Ok(answer) => answer,
        Err(error) => return Ok(back(FormState::new(&refusal(error)?, &Map::new()))),
    };

    // Counted before the code is checked, as the password is: six digits
    // are few enough to guess without a budget.
    let key = format!("second-step:{}", user.id);
    if let Err(refused) = consume(&state.limiters.login, &key).await? {
        return Ok(refused);
    }
    let by_app = user
        .accept_authenticator_code(db, &state.core.encryption, &code, now_ms() / 1000)
        .await?;
    let by_recovery_code = !by_app && RecoveryCode::consume(db, user.id, &code).await?;
    if !by_app && !by_recovery_code {
        let message = "This code is not valid";
        return Ok(back(FormState::with_error("code", message, &Map::new())));
    }
    state.limiters.login.delete(&key).await?;

    let destination = finish_sign_in(&state, &session, &cookies, &user, query.as_deref()).await?;
    if by_recovery_code {
        let left = RecoveryCode::remaining(db, user.id).await?;
        session.flash(
            "success",
            format!("Signed in with a recovery code: {left} left. Make new ones under Settings."),
        );
    }
    Ok(redirect_to(&destination))
}
