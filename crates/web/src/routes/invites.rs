//! Invites: the links that let someone into a team, and the page behind
//! one. An invite names an email address, a team and a role. Whoever
//! follows the link joins with the account of that address, or makes that
//! account first: an invite works whether or not the instance lets anyone
//! register. (`invites_controller.ts`)

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{delete, get};
use http::{HeaderMap, Uri};
use mymcps_core::Timestamp;
use mymcps_core::models::{Invite, Team, TeamRole, User};
use serde::Deserialize;
use sqlx::sqlite::SqliteExecutor;

use crate::auth::{Auth, CurrentTeam, CurrentUser, sign_in, switch_team};
use crate::cookies::Cookies;
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::{invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::auth::INVITE_RETURN_TO_KEY;
use crate::routes::teams::{INVITE_FORM, REFUSED_FORM_KEY};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::{invite_token, record_id};
use crate::validators::user::{ACCEPT_INVITE_VALIDATOR, CREATE_INVITE_VALIDATOR};
use crate::views::invites::{accept_page, create_invite_form, join_page};
use crate::views::shell::PageContext;

/// Session key of the invite the previous request created, whose link the
/// Team page shows once.
pub(crate) const CREATED_INVITE_KEY: &str = "inviteCreated";

const INVALID_INVITE: &str = "This invite is invalid or has expired";

/// How long an invite link works.
const INVITE_DAYS: i64 = 7;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // The page behind an invite link is for a visitor, who makes an
        // account there or is sent to sign in, and for someone signed in
        // with the invited address: it looks at who is signed in itself.
        open: Router::new().route("/invite/{token}", get(show).post(accept)),
        signed_in: Router::new().route("/invites", get(index)),
        admin: Router::new()
            .route("/invites", axum::routing::post(store))
            .route("/invites/{id}", delete(destroy)),
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

/// Whether two addresses are the same account's: letter case aside.
fn same_address(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// `GET /invites`: the invites are a part of the Team page.
pub async fn index(OriginalUri(uri): OriginalUri) -> Response {
    redirect_to(&forwarded("/team", &uri))
}

#[derive(Deserialize)]
struct NewInvite {
    email: String,
    role: Option<TeamRole>,
}

/// `POST /invites`
///
/// Invite an address into the team the person works in. They give no role
/// above their own.
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let db = &state.core.db;
    let team_page = forwarded("/team", &uri);
    let refused = |form: FormState| {
        // Without the script the page is drawn again, with the dialog open
        // on what was refused.
        if !is_fetch(&headers) {
            form.flash(&session);
            session.flash(REFUSED_FORM_KEY, INVITE_FORM);
        }
        invalid_form(
            &headers,
            &session,
            create_invite_form(&context, &form, team.role()),
            form.first_error().unwrap_or_default(),
            "/team",
        )
    };
    let NewInvite { email, role } =
        match CREATE_INVITE_VALIDATOR.validate_as(&serde_json::Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
        };
    let role = role.unwrap_or_default();
    if !team.role().may_grant(role) {
        return Ok(refused(FormState::with_error(
            "role",
            "Only an owner can invite an owner",
            &input,
        )));
    }

    // An address that has an account may join: it is refused only when it
    // is in this team already.
    let member: Option<i64> = sqlx::query_scalar(
        "select `users`.`id` from `users` \
         inner join `team_user` on `team_user`.`user_id` = `users`.`id` \
         where `team_user`.`team_id` = ? and lower(`users`.`email`) = lower(?) limit 1",
    )
    .bind(team.id())
    .bind(&email)
    .fetch_optional(&**db)
    .await?;
    if member.is_some() {
        session.flash("error", "That person is already a member of this team");
        return Ok(navigate(&headers, &team_page));
    }

    let pending: Option<i64> = sqlx::query_scalar(
        "select `id` from `invites` where `team_id` = ? and lower(`email`) = lower(?) \
         and `accepted_at` is null and `expires_at` > ? limit 1",
    )
    .bind(team.id())
    .bind(&email)
    .bind(Timestamp::now())
    .fetch_optional(&**db)
    .await?;
    if pending.is_some() {
        session.flash("error", "A pending invite already exists for that email");
        return Ok(navigate(&headers, &team_page));
    }

    let mut invite = Invite {
        email,
        role,
        team_id: team.id(),
        token: Invite::generate_token(),
        created_by: user.id,
        expires_at: Timestamp::now() + chrono::Duration::days(INVITE_DAYS),
        ..Default::default()
    };
    invite.insert(&**db).await?;
    session.flash("success", "Invite created");
    session.flash(CREATED_INVITE_KEY, invite.id);
    Ok(navigate(&headers, &team_page))
}

/// `DELETE /invites/{id}`
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    team: CurrentTeam,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let team_page = forwarded("/team", &uri);
    let invite = match record_id(&id) {
        Some(id) => Invite::find(&**db, id).await?,
        None => None,
    };
    // The invite of another team is answered like one that does not exist.
    let Some(invite) = invite.filter(|invite| invite.team_id == team.id()) else {
        session.flash("error", "Invite not found");
        return Ok(navigate(&headers, &team_page));
    };

    invite.delete(&**db).await?;
    session.flash("success", "Invite removed");
    Ok(navigate(&headers, &team_page))
}

/// The invite a link names while it can still be used, with its team.
async fn usable_invite<'e, E>(db: E, token: &str) -> Result<Option<(Invite, Team)>, sqlx::Error>
where
    E: SqliteExecutor<'e> + Copy,
{
    let Some(token) = invite_token(token) else {
        return Ok(None);
    };
    let Some(invite) = find_by_token(db, &token).await?.filter(Invite::is_usable) else {
        return Ok(None);
    };
    Ok(Team::find(db, invite.team_id)
        .await?
        .map(|team| (invite, team)))
}

/// Send a visitor to sign in with the invited address, and back to the
/// invite afterwards.
fn sign_in_first(session: &Session, invite: &Invite) -> Response {
    session.put(INVITE_RETURN_TO_KEY, format!("/invite/{}", invite.token));
    session.flash(
        "success",
        format!("Sign in as {} to join the team", invite.email),
    );
    redirect_to("/login")
}

/// What to tell someone signed in with another address than the invited one.
fn other_account(session: &Session, invite: &Invite) -> Response {
    session.flash(
        "error",
        format!(
            "This invite is for {}. Sign in with that address to accept it.",
            invite.email
        ),
    );
    redirect_to("/")
}

/// `GET /invite/{token}`
pub async fn show(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let Some((invite, team)) = usable_invite(&**db, &token).await? else {
        session.flash("error", INVALID_INVITE);
        return Ok(redirect_to(&forwarded("/", &uri)));
    };

    if let Some(user) = &context.user {
        if !same_address(&user.email, &invite.email) {
            return Ok(other_account(&session, &invite));
        }
        return Ok(page(join_page(&context, &invite, &team)));
    }
    if User::find_by_email(&**db, &invite.email).await?.is_some() {
        return Ok(sign_in_first(&session, &invite));
    }
    Ok(page(accept_page(
        &context,
        &invite,
        &team,
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
///
/// Join the team: with the account that is signed in when it has the
/// invited address, or with a new account for an address that has none.
#[allow(clippy::too_many_arguments)]
pub async fn accept(
    State(state): State<AppState>,
    session: Session,
    cookies: Cookies,
    Auth { user, .. }: Auth,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(token): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let invalid = || {
        session.flash("error", INVALID_INVITE);
        redirect_to(&forwarded("/", &uri))
    };
    let Some((invite, _)) = usable_invite(&**db, &token).await? else {
        return Ok(invalid());
    };

    if let Some(mut user) = user {
        if !same_address(&user.email, &invite.email) {
            return Ok(other_account(&session, &invite));
        }
        // Two requests accepting the same link at once must not both use
        // it. The transaction takes the write lock when it begins, so the
        // second one reads the invite as the first one left it: accepted.
        let mut transaction = db.begin().await?;
        let Some(mut invite) = find_by_token(&mut *transaction, &invite.token)
            .await?
            .filter(Invite::is_usable)
        else {
            transaction.rollback().await?;
            return Ok(invalid());
        };
        let already = Team::role_of(&mut *transaction, invite.team_id, user.id).await?;
        if already.is_none() {
            Team::attach(&mut *transaction, invite.team_id, user.id, invite.role).await?;
        }
        invite.accepted_at = Some(Timestamp::now());
        invite.save(&mut *transaction).await?;
        transaction.commit().await?;

        switch_team(&state.core, &session, &mut user, invite.team_id).await?;
        session.flash(
            "success",
            if already.is_some() {
                "You are already a member of this team"
            } else {
                "You joined the team"
            },
        );
        return Ok(redirect_to("/"));
    }

    if User::find_by_email(&**db, &invite.email).await?.is_some() {
        return Ok(sign_in_first(&session, &invite));
    }
    let payload: NewMember =
        match ACCEPT_INVITE_VALIDATOR.validate_as(&serde_json::Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                FormState::new(&refusal(error)?, &input).flash(&session);
                return Ok(redirect_back(&headers, uri.path()));
            }
        };

    // Hashing a password takes tens of milliseconds: it is done before the
    // transaction, which holds the write lock of the database, and only for
    // a link that can still be used.
    let mut user =
        User::with_password(&invite.email, Some(&payload.full_name), &payload.password).await?;

    let mut transaction = db.begin().await?;
    let Some(mut invite) = find_by_token(&mut *transaction, &invite.token)
        .await?
        .filter(Invite::is_usable)
    else {
        transaction.rollback().await?;
        return Ok(invalid());
    };
    if User::find_by_email(&mut *transaction, &invite.email)
        .await?
        .is_some()
    {
        transaction.rollback().await?;
        return Ok(sign_in_first(&session, &invite));
    }
    // The account comes with its personal team, and joins the team it was
    // invited to, which is where it starts.
    user.email = invite.email.clone();
    user.create(&mut transaction).await?;
    Team::attach(&mut *transaction, invite.team_id, user.id, invite.role).await?;
    user.remember_team(&mut *transaction, invite.team_id)
        .await?;
    invite.accepted_at = Some(Timestamp::now());
    invite.save(&mut *transaction).await?;
    transaction.commit().await?;

    sign_in(&state.core, &session, &cookies, &user).await?;
    session.put(crate::auth::TEAM_KEY, invite.team_id);
    session.flash("success", "Welcome to MyMCPs");
    Ok(redirect_to(&forwarded("/", &uri)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_query_string_over_without_the_method_override() {
        let forward = |path: &str, uri: &str| forwarded(path, &uri.parse().unwrap());
        assert_eq!(forward("/team", "/invites/3?_method=DELETE"), "/team");
        assert_eq!(
            forward("/team", "/members/3?tab=members&_method=DELETE&page=2"),
            "/team?tab=members&page=2"
        );
        assert_eq!(
            forward("/settings", "/settings/email?_method=PATCH&_methodical=1"),
            "/settings?_methodical=1"
        );
        assert_eq!(forward("/", "/invite/abc"), "/");
        assert_eq!(forward("/login", "/invite/abc?"), "/login");
    }
}
