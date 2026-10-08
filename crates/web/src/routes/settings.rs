//! The Settings page: the email and the password of the signed-in person,
//! and the settings of the instance for administrators.
//! (`settings_controller.ts`)

use axum::Router;
use axum::extract::{OriginalUri, State};
use axum::response::Response;
use axum::routing::{get, patch};
use http::HeaderMap;
use maud::Markup;
use mymcps_core::models::{GatewayToolMode, InstanceSetting, McpLogLevel, User};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::auth::{CurrentUser, stamp_session};
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::redirect_back;
use crate::respond::{invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::auth::consume;
use crate::routes::invites::forwarded;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::user::{
    UPDATE_EMAIL_VALIDATOR, UPDATE_MCP_LOGGING_VALIDATOR, UPDATE_PASSWORD_VALIDATOR,
};
use crate::views::settings::{SettingsPage, email_form, password_form, settings_page};
use crate::views::shell::PageContext;

/// Session key naming the form of the page that the previous request
/// refused, flashed along with its [`FormState`].
const REFUSED_FORM_KEY: &str = "settingsForm";
const EMAIL_FORM: &str = "email";
const PASSWORD_FORM: &str = "password";
const INSTANCE_FORM: &str = "instance";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new()
            .route("/settings", get(index))
            .route("/settings/email", patch(update_email))
            .route("/settings/password", patch(update_password)),
        admin: Router::new().route("/settings/mcp-logging", patch(update_mcp_logging)),
        ..Default::default()
    }
}

/// `GET /settings`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    let instance = if user.is_admin() {
        Some(InstanceSetting::current(&*state.core.db).await?)
    } else {
        None
    };

    let refused = session.flashed_text(REFUSED_FORM_KEY);
    let form = |name: &str| {
        if refused.as_deref() == Some(name) {
            FormState::from_session(&session)
        } else {
            FormState::default()
        }
    };
    Ok(page(settings_page(
        &context,
        &SettingsPage {
            user: &user,
            instance: instance.as_ref(),
            email_form: &form(EMAIL_FORM),
            password_form: &form(PASSWORD_FORM),
            instance_form: &form(INSTANCE_FORM),
        },
    )))
}

/// Answer a dialog form that was refused. The page's script gets the form
/// again with its errors. Without the script, the browser goes back to the
/// page, which opens the dialog on what was refused.
fn refuse(
    headers: &HeaderMap,
    session: &Session,
    name: &str,
    form: &FormState,
    fragment: Markup,
) -> Response {
    if !is_fetch(headers) {
        form.flash(session);
        session.flash(REFUSED_FORM_KEY, name);
    }
    invalid_form(
        headers,
        session,
        fragment,
        form.first_error().unwrap_or_default(),
        "/settings",
    )
}

enum PasswordCheck {
    Confirmed,
    Wrong,
    /// Too many attempts: the answer to give.
    Limited(Response),
}

/// A signed-in browser is no proof of knowing the password: without a
/// budget, whoever holds a hijacked session could guess it here at will.
async fn confirm_current_password(
    state: &AppState,
    user: &User,
    current_password: &str,
) -> Result<PasswordCheck, AppError> {
    let key = format!("current-password:{}", user.id);

    if let Err(refused) = consume(&state.limiters.current_password, &key).await? {
        return Ok(PasswordCheck::Limited(refused));
    }
    if !user.verify_password(current_password).await? {
        return Ok(PasswordCheck::Wrong);
    }
    state.limiters.current_password.delete(&key).await?;
    Ok(PasswordCheck::Confirmed)
}

fn wrong_current_password(input: &Map<String, Value>) -> FormState {
    FormState::with_error(
        "currentPassword",
        "The current password is incorrect",
        input,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailChange {
    email: String,
    current_password: String,
}

/// `PATCH /settings/email`
pub async fn update_email(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(mut user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            EMAIL_FORM,
            &form,
            email_form(&context, &user, &form),
        )
    };

    let mut run = UPDATE_EMAIL_VALIDATOR.start(&Value::Object(input.clone()));
    for check in run.take_checks() {
        // The one check that needs the database: the email does not belong
        // to another user. The user's own address is not a duplicate.
        let taken = match check.value.as_str() {
            Some(email) => sqlx::query_scalar::<_, i64>(
                "select `id` from `users` where `email` = ? and `id` != ? limit 1",
            )
            .bind(email)
            .bind(user.id)
            .fetch_optional(&**db)
            .await?
            .is_some(),
            None => false,
        };
        if taken {
            run.reject(&check);
        }
    }
    let payload: EmailChange = match run.finish_as() {
        Ok(payload) => payload,
        Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
    };

    match confirm_current_password(&state, &user, &payload.current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }
    user.email = payload.email;
    user.save(&**db).await?;

    session.flash("success", "Email updated");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordChange {
    current_password: String,
    new_password: String,
}

/// `PATCH /settings/password`
pub async fn update_password(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(mut user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            PASSWORD_FORM,
            &form,
            password_form(&context, &form),
        )
    };

    let payload: PasswordChange =
        match UPDATE_PASSWORD_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
        };

    match confirm_current_password(&state, &user, &payload.current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }
    user.change_password(&state.core.db, &payload.new_password)
        .await?;

    // The change retired every session of the account: keep this browser signed in.
    stamp_session(&session, &user);

    session.flash("success", "Password updated");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceChange {
    gateway_tool_mode: GatewayToolMode,
    mcp_log_level: McpLogLevel,
    mcp_log_retention_days: i64,
    mcp_auto_update_enabled: Option<bool>,
    mcp_auto_update_cron: Option<String>,
}

/// `PATCH /settings/mcp-logging`
pub async fn update_mcp_logging(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let payload: InstanceChange =
        match UPDATE_MCP_LOGGING_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                FormState::new(&refusal(error)?, &input).flash(&session);
                session.flash(REFUSED_FORM_KEY, INSTANCE_FORM);
                return Ok(redirect_back(&headers, "/settings"));
            }
        };

    let mut settings = InstanceSetting::current(&**db).await?;
    settings.gateway_tool_mode = payload.gateway_tool_mode;
    settings.mcp_log_level = payload.mcp_log_level;
    settings.mcp_log_retention_days = payload.mcp_log_retention_days;
    settings.mcp_auto_update_enabled = payload.mcp_auto_update_enabled.unwrap_or(false);
    if let Some(cron) = payload.mcp_auto_update_cron.filter(|cron| !cron.is_empty()) {
        settings.mcp_auto_update_cron = cron;
    }
    settings.updated_by = Some(user.id);
    settings.save(&**db).await?;
    // A shorter retention applies at once.
    state.gateway.call_log.prune_expired(true).await;
    state.instance_settings_changed().await?;

    session.flash("success", "Instance settings updated");
    Ok(navigate(&headers, &forwarded("/settings", &uri)))
}
