//! The Team page: the members of the instance and the invites that let new
//! ones in, and the page behind an invite link. (`invites_controller.ts`)

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{delete, get};
use http::{HeaderMap, Uri};
use mymcps_core::Timestamp;
use mymcps_core::models::{Invite, User, UserRole};
use serde::Deserialize;
use sqlx::sqlite::SqliteExecutor;

use crate::auth::{CurrentUser, sign_in};
use crate::cookies::Cookies;
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::{invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::{invite_token, record_id};
use crate::validators::user::{ACCEPT_INVITE_VALIDATOR, CREATE_INVITE_VALIDATOR};
use crate::views::invites::{TeamPage, accept_page, create_invite_form, team_page};
use crate::views::shell::PageContext;

/// Session key of the invite the previous request created, whose link the
/// Team page shows once.
const CREATED_INVITE_KEY: &str = "inviteCreated";

const INVALID_INVITE: &str = "This invite is invalid or has expired";
const USER_EXISTS: &str = "A user with that email already exists";

/// How long an invite link works.
const INVITE_DAYS: i64 = 7;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        guest: Router::new().route("/invite/{token}", get(show).post(accept)),
        admin: Router::new()
            .route("/invites", get(index).post(store))
            .route("/invites/{id}", delete(destroy))
            .route("/members/{id}", delete(destroy_member)),
        ..Default::default()
    }
}

/// `path`, followed by the query string of the request being answered, as
/// the Node app's redirects carried it over.
pub(crate) fn forwarded(path: &str, uri: &Uri) -> String {
    with_query(path, uri.query())
}

async fn find_by_token<'e, E>(db: E, token: &str) -> Result<Option<Invite>, sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query_as("select * from `invites` where `token` = ?")
        .bind(token)
        .fetch_optional(db)
        .await
}

/// `GET /invites`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let invites: Vec<Invite> =
        sqlx::query_as("select * from `invites` order by `created_at` desc, `id` desc")
            .fetch_all(&**db)
            .await?;
    let members: Vec<User> =
        sqlx::query_as("select * from `users` order by `created_at` asc, `id` asc")
            .fetch_all(&**db)
            .await?;

    let created = session
        .flashed(CREATED_INVITE_KEY)
        .and_then(|id| id.as_i64())
        .and_then(|id| invites.iter().find(|invite| invite.id == id));
    Ok(page(team_page(
        &context,
        &TeamPage {
            members: &members,
            invites: &invites,
            current_user_id: user.id,
            form: &FormState::from_session(&session),
            created,
        },
    )))
}

#[derive(Deserialize)]
struct NewInvite {
    email: String,
}

/// `POST /invites`
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let team = forwarded("/invites", &uri);
    let NewInvite { email } =
        match CREATE_INVITE_VALIDATOR.validate_as(&serde_json::Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                let form = FormState::new(&refusal(error)?, &input);
                // Without the script the page is drawn again, with the dialog
                // open on what was refused.
                if !is_fetch(&headers) {
                    form.flash(&session);
                }
                return Ok(invalid_form(
                    &headers,
                    &session,
                    create_invite_form(&context, &form),
                    form.first_error().unwrap_or_default(),
                    "/invites",
                ));
            }
        };

    if User::find_by_email(&**db, &email).await?.is_some() {
        session.flash("error", USER_EXISTS);
        return Ok(navigate(&headers, &team));
    }

    let pending: Option<i64> = sqlx::query_scalar(
        "select `id` from `invites` where `email` = ? and `accepted_at` is null and `expires_at` > ? limit 1",
    )
    .bind(&email)
    .bind(Timestamp::now())
    .fetch_optional(&**db)
    .await?;
    if pending.is_some() {
        session.flash("error", "A pending invite already exists for that email");
        return Ok(navigate(&headers, &team));
    }

    let mut invite = Invite {
        email,
        role: UserRole::Member,
        token: Invite::generate_token(),
        created_by: user.id,
        expires_at: Timestamp::now() + chrono::Duration::days(INVITE_DAYS),
        ..Default::default()
    };
    invite.insert(&**db).await?;
    session.flash("success", "Invite created");
    session.flash(CREATED_INVITE_KEY, invite.id);
    Ok(navigate(&headers, &team))
}

/// `DELETE /invites/{id}`
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let team = forwarded("/invites", &uri);
    let invite = match record_id(&id) {
        Some(id) => Invite::find(&**db, id).await?,
        None => None,
    };
    let Some(invite) = invite else {
        session.flash("error", "Invite not found");
        return Ok(navigate(&headers, &team));
    };

    invite.delete(&**db).await?;
    session.flash("success", "Invite removed");
    Ok(navigate(&headers, &team))
}

/// `DELETE /members/{id}`
pub async fn destroy_member(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(actor): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let team = forwarded("/invites", &uri);
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, &team)
    };

    let member = match record_id(&id) {
        Some(id) => User::find(&**db, id).await?,
        None => None,
    };
    let Some(member) = member else {
        return Ok(refuse("Member not found"));
    };
    if member.id == actor.id {
        return Ok(refuse("You cannot remove your own account"));
    }
    if member.is_admin() {
        let admins: i64 = sqlx::query_scalar("select count(*) from `users` where `role` = ?")
            .bind(UserRole::Admin)
            .fetch_one(&**db)
            .await?;
        if admins <= 1 {
            return Ok(refuse("Cannot remove the last admin"));
        }
    }

    // What the member owned goes to the administrator who removes them, and
    // their access tokens stop working.
    let mut transaction = db.begin().await?;
    sqlx::query("update `mcps` set `created_by` = ? where `created_by` = ?")
        .bind(actor.id)
        .bind(member.id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "update `access_tokens` set `revoked_at` = ? where `created_by` = ? and `revoked_at` is null",
    )
    .bind(Timestamp::now())
    .bind(member.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("update `access_tokens` set `created_by` = ? where `created_by` = ?")
        .bind(actor.id)
        .bind(member.id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("update `invites` set `created_by` = ? where `created_by` = ?")
        .bind(actor.id)
        .bind(member.id)
        .execute(&mut *transaction)
        .await?;
    member.delete(&mut *transaction).await?;
    transaction.commit().await?;

    session.flash("success", "Member removed");
    Ok(navigate(&headers, &team))
}

/// `GET /invite/{token}`
pub async fn show(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let invite = match invite_token(&token) {
        Some(token) => find_by_token(&*state.core.db, &token).await?,
        None => None,
    };
    let Some(invite) = invite.filter(Invite::is_usable) else {
        session.flash("error", INVALID_INVITE);
        return Ok(redirect_to(&forwarded("/", &uri)));
    };

    Ok(page(accept_page(
        &context,
        &invite,
        &FormState::from_session(&session),
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewMember {
    full_name: String,
    password: String,
}

/// `POST /invite/{token}`
pub async fn accept(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(token): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let payload: NewMember =
        match ACCEPT_INVITE_VALIDATOR.validate_as(&serde_json::Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                FormState::new(&refusal(error)?, &input).flash(&session);
                return Ok(redirect_back(&headers, uri.path()));
            }
        };
    let invalid = || {
        session.flash("error", INVALID_INVITE);
        redirect_to(&forwarded("/", &uri))
    };
    let Some(token) = invite_token(&token) else {
        return Ok(invalid());
    };

    // Hashing a password takes tens of milliseconds: it is done before the
    // transaction, which holds the write lock of the database, and only for
    // a link that can still be used.
    let Some(invite) = find_by_token(&**db, &token)
        .await?
        .filter(Invite::is_usable)
    else {
        return Ok(invalid());
    };
    let mut user = User::with_password(
        &invite.email,
        Some(&payload.full_name),
        &payload.password,
        invite.role,
    )
    .await?;

    // Two people accepting the same link at once must not both get an
    // account. The transaction takes the write lock when it begins, so the
    // second one reads the invite as the first one left it: accepted.
    let mut transaction = db.begin().await?;
    let Some(mut invite) = find_by_token(&mut *transaction, &token)
        .await?
        .filter(Invite::is_usable)
    else {
        transaction.rollback().await?;
        return Ok(invalid());
    };
    let existing = User::find_by_email(&mut *transaction, &invite.email).await?;
    if existing.is_none() {
        user.email = invite.email.clone();
        user.role = invite.role;
        user.insert(&mut *transaction).await?;
    }
    invite.accepted_at = Some(Timestamp::now());
    invite.save(&mut *transaction).await?;
    transaction.commit().await?;

    if existing.is_some() {
        session.flash("error", USER_EXISTS);
        return Ok(redirect_to(&forwarded("/login", &uri)));
    }

    sign_in(&state.core, &session, &cookies, &user).await?;
    session.flash("success", "Welcome to MyMCPs");
    Ok(redirect_to(&forwarded("/", &uri)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_query_string_over_without_the_method_override() {
        let forward = |path: &str, uri: &str| forwarded(path, &uri.parse().unwrap());
        assert_eq!(forward("/invites", "/invites/3?_method=DELETE"), "/invites");
        assert_eq!(
            forward("/invites", "/members/3?tab=members&_method=DELETE&page=2"),
            "/invites?tab=members&page=2"
        );
        assert_eq!(
            forward("/settings", "/settings/email?_method=PATCH&_methodical=1"),
            "/settings?_methodical=1"
        );
        assert_eq!(forward("/", "/invite/abc"), "/");
        assert_eq!(forward("/login", "/invite/abc?"), "/login");
    }
}
