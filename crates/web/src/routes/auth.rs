//! Signing in and out, and the first-run onboarding.
//! (`session_controller.ts`, `onboarding_controller.ts`)

use axum::Router;
use axum::extract::{OriginalUri, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::header::RETRY_AFTER;
use http::{HeaderMap, HeaderValue, StatusCode};
use mymcps_core::client_ip::rate_limit_client_key;
use mymcps_core::crypto::sha256_hex;
use mymcps_core::limiter::{Limiter, LimiterError, LimiterResponse};
use mymcps_core::models::{User, UserRole};
use mymcps_core::two_factor::TwoFactorStatus;
use serde::Deserialize;

use crate::auth::{CurrentUser, sign_in, sign_out};
use crate::client_ip::ClientIp;
use crate::cookies::Cookies;
use crate::error::AppError;
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::page;
use crate::routes::FeatureRoutes;
use crate::routes::two_factor::{begin_second_step, login_view, verify_path};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::session::{APPROVAL_RETURN_TO_VALIDATOR, OAUTH_RETURN_TO_VALIDATOR};
use crate::validators::user::{LOGIN_VALIDATOR, ONBOARDING_VALIDATOR};
use crate::views::auth::onboarding_page;
use crate::views::shell::PageContext;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        guest: Router::new().route("/login", get(show_login).post(login)),
        signed_in: Router::new().route("/logout", post(logout)),
        first_run: Router::new().route("/onboarding", get(show_onboarding).post(onboarding)),
        ..Default::default()
    }
}

/// What a client that used up an allowance is told.
pub fn too_many_requests(refused: &LimiterResponse) -> Response {
    let mut response = (StatusCode::TOO_MANY_REQUESTS, "Too many requests").into_response();
    let headers = response.headers_mut();
    headers.insert(RETRY_AFTER, HeaderValue::from(refused.available_in));
    headers.insert("x-ratelimit-limit", HeaderValue::from(refused.limit));
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from(refused.remaining),
    );
    response
}

/// Count one request against an allowance. `Err` is the answer to give
/// when it is used up.
pub async fn consume(limiter: &Limiter, key: &str) -> Result<Result<(), Response>, AppError> {
    match limiter.consume(key).await {
        Ok(_) => Ok(Ok(())),
        Err(LimiterError::TooManyRequests(refused)) => Ok(Err(too_many_requests(&refused))),
        Err(LimiterError::Store(error)) => Err(error.into()),
    }
}

/// `GET /login`
pub async fn show_login(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
) -> Response {
    login_view(&state, &context, &session)
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

/// `POST /login`
pub async fn login(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    ClientIp(ip): ClientIp,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let credentials: Credentials =
        match LOGIN_VALIDATOR.validate_as(&serde_json::Value::Object(input.clone())) {
            Ok(credentials) => credentials,
            Err(error) => {
                let error = refusal(error)?;
                FormState::new(&error, &input).flash(&session);
                return Ok(redirect_back(&headers, "/login"));
            }
        };

    let client = rate_limit_client_key(&ip);
    // Hashed to keep limiter keys short and free of email addresses.
    let account = sha256_hex(&credentials.email.to_lowercase());
    let address_key = format!("login-address:{client}");
    let account_key = format!("login-account:{client}:{account}");

    // Attempts are counted before the password is checked, so a burst of
    // parallel guesses cannot all slip in ahead of the first recorded failure.
    if let Err(refused) = consume(&state.limiters.login_address, &address_key).await? {
        return Ok(refused);
    }
    if let Err(refused) = consume(&state.limiters.login, &account_key).await? {
        return Ok(refused);
    }

    let Some(user) =
        User::verify_credentials(&state.core.db, &credentials.email, &credentials.password).await?
    else {
        FormState::with_error("credentials", "Invalid user credentials", &input).flash(&session);
        return Ok(redirect_back(&headers, "/login"));
    };

    // A sign-in clears the budget of its own account only. The address-wide
    // budget just gets back the attempt this request was charged, so signing
    // in to one account never buys more guesses against another.
    state.limiters.login.delete(&account_key).await?;
    state.limiters.login_address.decrement(&address_key).await?;

    // An account with a passkey or an authenticator app is not signed in by
    // its password alone.
    if TwoFactorStatus::of(&state.core.db, user.id)
        .await?
        .is_enabled()
    {
        begin_second_step(&session, &user);
        return Ok(redirect_to(&verify_path(uri.query())));
    }
    finish_sign_in(&state, &session, &cookies, &user, uri.query()).await
}

/// Open the session of `user`, whose sign-in is complete, and go where the
/// sign-in was asked from: an OAuth authorization, a tool call to approve,
/// or home.
pub(crate) async fn finish_sign_in(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
    user: &User,
    query: Option<&str>,
) -> Result<Response, AppError> {
    sign_in(&state.core, session, cookies, user).await?;

    // Both paths are complete: the query string of this request is not
    // appended after them.
    let return_to = |validator: &mymcps_vine::Validator, key: &str| {
        validator
            .validate_as::<String>(session.pull(key).as_ref())
            .ok()
    };
    if let Some(oauth_return_to) = return_to(&OAUTH_RETURN_TO_VALIDATOR, "oauthReturnTo") {
        return Ok(redirect_to(&oauth_return_to));
    }
    if let Some(approval_return_to) = return_to(&APPROVAL_RETURN_TO_VALIDATOR, "approvalReturnTo") {
        return Ok(redirect_to(&approval_return_to));
    }
    Ok(redirect_to(&with_query("/", query)))
}

/// `POST /logout`
pub async fn logout(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    CurrentUser(mut user): CurrentUser,
) -> Result<Response, AppError> {
    // A cookie-store session cannot be deleted on the server, so signing out
    // retires every session issued to the user so far, a copy of this one
    // included. Their other browsers carry on with their remember-me cookie.
    user.invalidate_sessions(&*state.core.db).await?;
    sign_out(&state.core, &session, &cookies).await?;
    Ok(redirect_to(&with_query("/login", uri.query())))
}

/// `GET /onboarding`
pub async fn show_onboarding(context: PageContext, session: Session) -> Response {
    page(onboarding_page(
        &context,
        &FormState::from_session(&session),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewAdmin {
    full_name: String,
    email: String,
    password: String,
}

/// `POST /onboarding`
pub async fn onboarding(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let mut run = ONBOARDING_VALIDATOR.start(&serde_json::Value::Object(input.clone()));
    for check in run.take_checks() {
        // The one check that needs the database: the email is not taken.
        let taken = match check.value.as_str() {
            Some(email) => User::find_by_email(&**db, email).await?.is_some(),
            None => false,
        };
        if taken {
            run.reject(&check);
        }
    }
    let payload: NewAdmin = match run.finish_as() {
        Ok(payload) => payload,
        Err(error) => {
            let error = refusal(error)?;
            FormState::new(&error, &input).flash(&session);
            return Ok(redirect_back(&headers, "/onboarding"));
        }
    };

    let home = with_query("/", uri.query());
    let mut user = User::with_password(
        &payload.email,
        Some(&payload.full_name),
        &payload.password,
        UserRole::Admin,
    )
    .await?;
    // The first user only: two visitors finishing the form at once must not
    // both become administrators.
    let mut transaction = db.begin().await?;
    if User::setup_complete(&mut *transaction).await? {
        return Ok(redirect_to(&home));
    }
    user.insert(&mut *transaction).await?;
    transaction.commit().await?;

    sign_in(&state.core, &session, &cookies, &user).await?;
    Ok(redirect_to(&home))
}
