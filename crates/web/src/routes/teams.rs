//! Teams: the page of the team someone works in (its name, its members, its
//! invites), the switch from one team to another, and, for the super admins
//! who run the instance, its accounts.
//!
//! What someone may do in a team follows their role there, least first:
//! a member reads it, an administrator changes it and its members up to
//! their own rank, an owner also deletes it and decides who else owns it.

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use http::{HeaderMap, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::{AccountRemovalRefusal, Invite, Team, TeamRole, User};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::auth::{Auth, CurrentTeam, CurrentUser, switch_team};
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::guards::SUPER_ADMIN_REQUIRED;
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to};
use crate::respond::{fragment, invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::invites::{CREATED_INVITE_KEY, forwarded};
use crate::routes::logs::{fragment_target, uncached};
use crate::routes::settings::{PasswordCheck, confirm_current_password, wrong_current_password};
use crate::routes::tailnets;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::record_id;
use crate::validators::user::{
    DELETE_USER_VALIDATOR, MEMBER_ROLE_VALIDATOR, SUPER_ADMIN_VALIDATOR, SWITCH_TEAM_VALIDATOR,
    TEAM_VALIDATOR,
};
use crate::views::shell::PageContext;
use crate::views::teams::{
    OpenUserDialog, TeamPage, UserDialog, UserRow, UsersPage, create_team_form, team_page,
    users_page,
};

/// Session key naming the form of the Team page that the previous request
/// refused, flashed along with its [`FormState`].
pub(crate) const REFUSED_FORM_KEY: &str = "teamForm";
pub(crate) const INVITE_FORM: &str = "invite";
const TEAM_FORM: &str = "team";
const CREATE_FORM: &str = "create";

const MEMBER_NOT_FOUND: &str = "Member not found";
const LAST_SUPER_ADMIN: &str = "The instance needs at least one super admin";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new()
            .route("/team", get(index))
            .route("/teams", post(store))
            .route("/teams/new", get(new))
            .route("/teams/switch", post(switch)),
        admin: Router::new()
            .route("/team", patch(update).delete(destroy))
            .route("/members/{id}", patch(update_member).delete(destroy_member)),
        super_admin: Router::new()
            .route("/users", get(users))
            .route(
                "/users/{id}/super-admin",
                get(confirm_super_admin).patch(update_super_admin),
            )
            .route(
                "/users/{id}/protection",
                get(confirm_reset_protection).delete(reset_protection),
            )
            .route("/users/{id}/delete", get(confirm_destroy_user))
            .route("/users/{id}", delete(destroy_user)),
        ..Default::default()
    }
}

/// Someone in a team: their account, and what they are there.
#[derive(Debug, Clone)]
pub struct MemberRow {
    pub user: User,
    pub role: TeamRole,
    pub joined_at: Timestamp,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for MemberRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            user: User::from_row(row)?,
            role: row.try_get("member_role")?,
            joined_at: row.try_get("joined_at")?,
        })
    }
}

/// The members of a team, in the order they joined it.
async fn members_of(state: &AppState, team_id: i64) -> Result<Vec<MemberRow>, sqlx::Error> {
    sqlx::query_as(
        "select `users`.*, `team_user`.`role` as `member_role`, `team_user`.`created_at` as `joined_at` \
         from `team_user` inner join `users` on `users`.`id` = `team_user`.`user_id` \
         where `team_user`.`team_id` = ? order by `team_user`.`id` asc",
    )
    .bind(team_id)
    .fetch_all(&*state.core.db)
    .await
}

/// The Team page, with the dialog that creates a team open or not.
async fn render(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    user: &User,
    team: &CurrentTeam,
    open_create: bool,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let members = members_of(state, team.id()).await?;
    // Who was invited, and the links that let them in, are for those who
    // may invite, up to the role they may give: the link of an invite is
    // all it takes to join with its role, so an administrator who read the
    // link of an owner's invite could make themselves an owner.
    let invites: Vec<Invite> = if team.is_admin() {
        let invites: Vec<Invite> = sqlx::query_as(
            "select * from `invites` where `team_id` = ? order by `created_at` desc, `id` desc",
        )
        .bind(team.id())
        .fetch_all(&**db)
        .await?;
        invites
            .into_iter()
            .filter(|invite| team.role().may_grant(invite.role))
            .collect()
    } else {
        Vec::new()
    };
    let mcps: i64 = sqlx::query_scalar("select count(*) from `mcps` where `team_id` = ?")
        .bind(team.id())
        .fetch_one(&**db)
        .await?;

    let refused = session.flashed_text(REFUSED_FORM_KEY);
    let form = |name: &str| {
        if refused.as_deref() == Some(name) {
            FormState::from_session(session)
        } else {
            FormState::default()
        }
    };
    let create_form = form(CREATE_FORM);
    let created = session
        .flashed(CREATED_INVITE_KEY)
        .and_then(|id| id.as_i64())
        .and_then(|id| invites.iter().find(|invite| invite.id == id));
    Ok(page(team_page(
        context,
        &TeamPage {
            team: &team.0,
            members: &members,
            invites: &invites,
            current_user_id: user.id,
            mcps,
            team_form: &form(TEAM_FORM),
            invite_form: &form(INVITE_FORM),
            open_create: open_create || create_form.has_errors(),
            create_form: &create_form,
            created,
        },
    )))
}

/// `GET /team`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    team: CurrentTeam,
) -> Result<Response, AppError> {
    render(&state, &context, &session, &user, &team, false).await
}

/// `GET /teams/new`: the Team page with the dialog that creates a team
/// open, for a browser that follows the link of the team switcher itself.
pub async fn new(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    team: CurrentTeam,
) -> Result<Response, AppError> {
    render(&state, &context, &session, &user, &team, true).await
}

#[derive(Deserialize)]
struct TeamDetails {
    name: String,
    description: Option<String>,
}

impl TeamDetails {
    fn description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .filter(|description| !description.is_empty())
    }
}

/// `POST /teams`
///
/// Anyone creates a team, which they own, and work in from then on.
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(mut user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let details: TeamDetails = match TEAM_VALIDATOR.validate_as(&Value::Object(input.clone())) {
        Ok(details) => details,
        Err(error) => {
            let form = FormState::new(&refusal(error)?, &input);
            // Without the script the page is drawn again, with the dialog
            // open on what was refused.
            if !is_fetch(&headers) {
                form.flash(&session);
                session.flash(REFUSED_FORM_KEY, CREATE_FORM);
                return Ok(redirect_to("/teams/new"));
            }
            return Ok(invalid_form(
                &headers,
                &session,
                create_team_form(&context, &form),
                form.first_error().unwrap_or_default(),
                "/teams/new",
            ));
        }
    };

    let mut transaction = state.core.db.begin().await?;
    let team = Team::create_owned(
        &mut transaction,
        &details.name,
        details.description(),
        &user,
    )
    .await?;
    transaction.commit().await?;
    switch_team(&state.core, &session, &mut user, team.id).await?;

    session.flash("success", "Team created");
    Ok(navigate(&headers, "/team"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamChoice {
    team_id: i64,
}

/// `POST /teams/switch`
///
/// Work in another team. The person must be in it: the id of a team they
/// are not in is answered like the id of no team.
pub async fn switch(
    State(state): State<AppState>,
    session: Session,
    auth: Auth,
    CurrentUser(mut user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let chosen = SWITCH_TEAM_VALIDATOR
        .validate_as::<TeamChoice>(&Value::Object(input))
        .ok()
        .and_then(|choice| {
            auth.teams
                .iter()
                .find(|membership| membership.team.id == choice.team_id)
        });
    let Some(membership) = chosen else {
        session.flash("error", "Team not found");
        return Ok(navigate(&headers, "/"));
    };
    switch_team(&state.core, &session, &mut user, membership.team.id).await?;
    // Home, not the page they were on: it may show a row of the team they left.
    Ok(navigate(&headers, "/"))
}

/// `PATCH /team`
pub async fn update(
    State(state): State<AppState>,
    session: Session,
    team: CurrentTeam,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let details: TeamDetails = match TEAM_VALIDATOR.validate_as(&Value::Object(input.clone())) {
        Ok(details) => details,
        Err(error) => {
            FormState::new(&refusal(error)?, &input).flash(&session);
            session.flash(REFUSED_FORM_KEY, TEAM_FORM);
            return Ok(redirect_back(&headers, "/team"));
        }
    };

    let mut saved = team.0.team.clone();
    saved.description = details.description().map(str::to_string);
    saved.name = details.name;
    saved.save(&*state.core.db).await?;

    session.flash("success", "Team updated");
    Ok(navigate(&headers, "/team"))
}

/// `DELETE /team`
///
/// An owner deletes the team they work in, with its access tokens, its call
/// logs and its invites. Never the Root Team nor a personal team, and not
/// while it still has MCPs: those hold credentials someone set up.
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    team: CurrentTeam,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, "/team")
    };
    if !team.role().is_owner() {
        return Ok(refuse("Only an owner can delete a team"));
    }
    if team.team.is_root() {
        return Ok(refuse("The Root Team cannot be deleted"));
    }
    if team.team.personal_team {
        return Ok(refuse("A personal team cannot be deleted"));
    }

    let mut transaction = state.core.db.begin().await?;
    // Read with the write lock held: a tailnet connected before it is here,
    // and none can be connected until the team is gone.
    let before = tailnets::all_control_urls(&mut *transaction).await?;
    let mcps: i64 = sqlx::query_scalar("select count(*) from `mcps` where `team_id` = ?")
        .bind(team.id())
        .fetch_one(&mut *transaction)
        .await?;
    if mcps > 0 {
        return Ok(refuse("Delete the MCPs of this team first"));
    }
    team.team.delete(&mut *transaction).await?;
    transaction.commit().await?;
    // Its tailnets went with it: their daemons leave the tailnets too.
    let left = tailnets::discard_removed(&state, &before).await;

    // The next page is drawn in another team of theirs.
    session.forget(crate::auth::TEAM_KEY);
    session.flash("success", tailnets::with_logout_note("Team deleted", left));
    Ok(navigate(&headers, "/"))
}

/// The account a route names, or `None` when its `{id}` is not an id or
/// matches none.
async fn find_account(state: &AppState, id: &str) -> Result<Option<User>, sqlx::Error> {
    match record_id(id) {
        Some(id) => User::find(&*state.core.db, id).await,
        None => Ok(None),
    }
}

/// Why someone may not change or remove a member, if they may not. Read
/// under the write lock of the transaction that makes the change: of two
/// owners who demote each other at once, the second is no longer one.
async fn member_refusal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    team_id: i64,
    actor: &User,
    member: &User,
    own: &'static str,
    above: &'static str,
) -> Result<Option<&'static str>, sqlx::Error> {
    let Some(member_role) = Team::role_of(&mut **transaction, team_id, member.id).await? else {
        return Ok(Some(MEMBER_NOT_FOUND));
    };
    if member.id == actor.id {
        return Ok(Some(own));
    }
    let actor_role = Team::role_of(&mut **transaction, team_id, actor.id).await?;
    if !actor_role.is_some_and(|role| role.may_manage(member_role)) {
        return Ok(Some(above));
    }
    Ok(None)
}

#[derive(Deserialize)]
struct RoleChange {
    role: TeamRole,
}

/// `PATCH /members/{id}`
///
/// Change what a member is in the team. Nobody changes their own role, a
/// member ranked above them, or gives a role above their own.
pub async fn update_member(
    State(state): State<AppState>,
    session: Session,
    auth: Auth,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let (actor, team) = auth.signed_in()?;
    let page = forwarded("/team", &uri);
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, &page)
    };

    let Some(member) = find_account(&state, &id).await? else {
        return Ok(refuse(MEMBER_NOT_FOUND));
    };
    let Ok(RoleChange { role }) = MEMBER_ROLE_VALIDATOR.validate_as(&Value::Object(input)) else {
        return Ok(refuse("Choose a role: member, admin or owner"));
    };

    let mut transaction = state.core.db.begin().await?;
    let refusal = member_refusal(
        &mut transaction,
        team.id(),
        &actor,
        &member,
        "You cannot change your own role",
        "Only an owner can change an owner",
    )
    .await?;
    if let Some(message) = refusal {
        return Ok(refuse(message));
    }
    let actor_role = Team::role_of(&mut *transaction, team.id(), actor.id).await?;
    if !actor_role.is_some_and(|actor_role| actor_role.may_grant(role)) {
        return Ok(refuse("Only an owner can make someone an owner"));
    }
    sqlx::query(
        "update `team_user` set `role` = ?, `updated_at` = ? where `team_id` = ? and `user_id` = ?",
    )
    .bind(role)
    .bind(Timestamp::now())
    .bind(team.id())
    .bind(member.id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    session.flash(
        "success",
        format!("{} is now {}", member.display_name(), role_phrase(role)),
    );
    Ok(navigate(&headers, &page))
}

fn role_phrase(role: TeamRole) -> &'static str {
    match role {
        TeamRole::Member => "a member",
        TeamRole::Admin => "an admin",
        TeamRole::Owner => "an owner",
    }
}

/// `DELETE /members/{id}`
///
/// Take a member out of the team. Their account stays, with the other teams
/// it is in; the access tokens they made here stop working.
pub async fn destroy_member(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(actor): CurrentUser,
    team: CurrentTeam,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let page = forwarded("/team", &uri);
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, &page)
    };

    let Some(member) = find_account(&state, &id).await? else {
        return Ok(refuse(MEMBER_NOT_FOUND));
    };
    let mut transaction = state.core.db.begin().await?;
    let refusal = member_refusal(
        &mut transaction,
        team.id(),
        &actor,
        &member,
        "You cannot remove yourself from a team",
        "Only an owner can remove an owner",
    )
    .await?;
    if let Some(message) = refusal {
        return Ok(refuse(message));
    }
    Team::detach(&mut transaction, team.id(), member.id).await?;
    transaction.commit().await?;

    session.flash("success", "Member removed");
    Ok(navigate(&headers, &page))
}

/// What someone is told who tries a dialog of the Users page on their own
/// account: another super admin decides for them.
fn own_account_refusal(dialog: UserDialog) -> &'static str {
    match dialog {
        UserDialog::SuperAdmin => "You cannot change your own super admin access",
        UserDialog::Protection => "Turn your own sign-in protection off under Settings",
        UserDialog::Delete => "You cannot delete your own account",
    }
}

/// Whether this person still runs the instance. Read under the write lock
/// of the transaction that acts for them: of two super admins who remove
/// each other at once, the second is no longer one.
async fn still_super_admin(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &User,
) -> Result<bool, sqlx::Error> {
    let found: Option<bool> =
        sqlx::query_scalar("select `is_super_admin` from `users` where `id` = ?")
            .bind(actor.id)
            .fetch_optional(&mut **transaction)
            .await?;
    Ok(found.unwrap_or(false))
}

/// The Users page, with one of its dialogs open on an account or not.
async fn users_page_for(
    state: &AppState,
    context: &PageContext,
    user: &User,
    open: Option<OpenUserDialog<'_>>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let accounts: Vec<User> =
        sqlx::query_as("select * from `users` order by `created_at` asc, `id` asc")
            .fetch_all(&**db)
            .await?;
    let memberships: Vec<(i64, String, TeamRole)> = sqlx::query_as(
        "select `team_user`.`user_id`, `teams`.`name`, `team_user`.`role` from `team_user` \
         inner join `teams` on `teams`.`id` = `team_user`.`team_id` order by `team_user`.`id` asc",
    )
    .fetch_all(&**db)
    .await?;
    let with_passkeys: Vec<i64> = sqlx::query_scalar("select distinct `user_id` from `passkeys`")
        .fetch_all(&**db)
        .await?;

    let rows: Vec<UserRow> = accounts
        .into_iter()
        .map(|account| {
            let teams: Vec<(String, TeamRole)> = memberships
                .iter()
                .filter(|(user_id, ..)| *user_id == account.id)
                .map(|(_, name, role)| (name.clone(), *role))
                .collect();
            UserRow {
                protected: account.has_authenticator() || with_passkeys.contains(&account.id),
                user: account,
                teams,
            }
        })
        .collect();
    Ok(page(users_page(
        context,
        &UsersPage {
            users: &rows,
            current_user_id: user.id,
            open,
        },
    )))
}

/// `GET /users`
///
/// Every account of the instance, for those who run it.
pub async fn users(
    State(state): State<AppState>,
    context: PageContext,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    users_page_for(&state, &context, &user, None).await
}

/// The Users page with this dialog open on an account. The page script asks
/// for the content of the dialog alone.
async fn user_dialog(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    actor: &User,
    headers: &HeaderMap,
    id: &str,
    dialog: UserDialog,
) -> Result<Response, AppError> {
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(headers, "/users")
    };
    let Some(account) = find_account(state, id).await? else {
        return Ok(refuse("User not found"));
    };
    if account.id == actor.id {
        return Ok(refuse(own_account_refusal(dialog)));
    }

    if is_fetch(headers) && fragment_target(headers).as_deref() == Some(dialog.id()) {
        return Ok(uncached(fragment(
            StatusCode::OK,
            dialog.form(context, &account, &FormState::default()),
        )));
    }
    let form = if session.flashed_text(REFUSED_FORM_KEY).as_deref() == Some(dialog.id()) {
        FormState::from_session(session)
    } else {
        FormState::default()
    };
    let open = OpenUserDialog {
        dialog,
        account: &account,
        form: &form,
    };
    users_page_for(state, context, actor, Some(open)).await
}

/// What a dialog of the Users page answers when its form is refused. The
/// script gets the form again with what is wrong. Without it, the browser
/// goes to the page that shows the dialog open on it.
fn refused_user_form(
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    dialog: UserDialog,
    account: &User,
    form: FormState,
) -> Response {
    if is_fetch(headers) {
        return fragment(
            StatusCode::UNPROCESSABLE_ENTITY,
            dialog.form(context, account, &form),
        );
    }
    form.flash(session);
    session.flash(REFUSED_FORM_KEY, dialog.id());
    redirect_to(&dialog.href(account))
}

/// `GET /users/{id}/super-admin`: the Users page with the dialog that makes
/// this account a super admin, or takes that from it, open.
pub async fn confirm_super_admin(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let dialog = UserDialog::SuperAdmin;
    user_dialog(&state, &context, &session, &actor, &headers, &id, dialog).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuperAdminChange {
    super_admin: bool,
    current_password: String,
}

/// `PATCH /users/{id}/super-admin`
///
/// Make an account a super admin, or an account like any other. Never one's
/// own: a super admin does not leave the instance without one by mistake.
/// It hands over the instance, so the person types their own password
/// again: a signed-in browser is no proof that it is them.
pub async fn update_super_admin(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let dialog = UserDialog::SuperAdmin;
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, "/users")
    };

    let Some(account) = find_account(&state, &id).await? else {
        return Ok(refuse("User not found"));
    };
    if account.id == actor.id {
        return Ok(refuse(own_account_refusal(dialog)));
    }

    let refused =
        |form: FormState| refused_user_form(&context, &session, &headers, dialog, &account, form);
    let SuperAdminChange {
        super_admin,
        current_password,
    } = match SUPER_ADMIN_VALIDATOR.validate_as(&Value::Object(input.clone())) {
        Ok(change) => change,
        Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
    };
    match confirm_current_password(&state, &actor, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let mut transaction = state.core.db.begin().await?;
    if !still_super_admin(&mut transaction, &actor).await? {
        return Ok(refuse(SUPER_ADMIN_REQUIRED));
    }
    if !User::set_super_admin(&mut transaction, account.id, super_admin).await? {
        return Ok(refuse(LAST_SUPER_ADMIN));
    }
    transaction.commit().await?;

    let name = account.display_name();
    session.flash(
        "success",
        if super_admin {
            format!("{name} is now a super admin")
        } else {
            format!("{name} is no longer a super admin")
        },
    );
    Ok(navigate(&headers, "/users"))
}

/// `GET /users/{id}/protection`: the Users page with the dialog that
/// removes the sign-in protection of this account open.
pub async fn confirm_reset_protection(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let dialog = UserDialog::Protection;
    user_dialog(&state, &context, &session, &actor, &headers, &id, dialog).await
}

/// `DELETE /users/{id}/protection`
///
/// Take away the authenticator app, the passkeys and the recovery codes of
/// an account whose owner lost them all: its password signs in alone
/// again. Never one's own, which Settings turns off. It lowers the guard
/// of someone else's account, so the person types their own password.
pub async fn reset_protection(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let dialog = UserDialog::Protection;
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, "/users")
    };

    let Some(mut account) = find_account(&state, &id).await? else {
        return Ok(refuse("User not found"));
    };
    if account.id == actor.id {
        return Ok(refuse(own_account_refusal(dialog)));
    }

    let name = account.display_name().to_string();
    let Deletion { current_password } =
        match DELETE_USER_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(confirmation) => confirmation,
            Err(error) => {
                let form = FormState::new(&refusal(error)?, &input);
                return Ok(refused_user_form(
                    &context, &session, &headers, dialog, &account, form,
                ));
            }
        };
    match confirm_current_password(&state, &actor, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => {
            let form = wrong_current_password(&input);
            return Ok(refused_user_form(
                &context, &session, &headers, dialog, &account, form,
            ));
        }
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let mut transaction = state.core.db.begin().await?;
    if !still_super_admin(&mut transaction, &actor).await? {
        return Ok(refuse(SUPER_ADMIN_REQUIRED));
    }
    account.remove_protection(&mut transaction).await?;
    transaction.commit().await?;

    session.flash(
        "success",
        format!("{name} signs in with their password alone"),
    );
    Ok(navigate(&headers, "/users"))
}

/// `GET /users/{id}/delete`: the Users page with the dialog that deletes
/// this account open.
pub async fn confirm_destroy_user(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let dialog = UserDialog::Delete;
    user_dialog(&state, &context, &session, &actor, &headers, &id, dialog).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deletion {
    current_password: String,
}

/// `DELETE /users/{id}`
///
/// Delete an account, a super admin or not. Never one's own. The teams it
/// was alone in go with it; the others keep an owner. It cannot be undone,
/// so the person types their own password again: a signed-in browser is no
/// proof that it is them.
pub async fn destroy_user(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(actor): CurrentUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let dialog = UserDialog::Delete;
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, "/users")
    };

    let Some(account) = find_account(&state, &id).await? else {
        return Ok(refuse("User not found"));
    };
    if account.id == actor.id {
        return Ok(refuse(own_account_refusal(dialog)));
    }

    let refused =
        |form: FormState| refused_user_form(&context, &session, &headers, dialog, &account, form);
    let Deletion { current_password } =
        match DELETE_USER_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(deletion) => deletion,
            Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
        };
    match confirm_current_password(&state, &actor, &current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let mut transaction = state.core.db.begin().await?;
    // Read with the write lock held, see `destroy`.
    let before = tailnets::all_control_urls(&mut *transaction).await?;
    if !still_super_admin(&mut transaction, &actor).await? {
        return Ok(refuse(SUPER_ADMIN_REQUIRED));
    }
    let left = match Team::delete_account(&mut transaction, account.id).await? {
        Ok(()) => {
            transaction.commit().await?;
            // The teams the account was alone in went with it, tailnets included.
            tailnets::discard_removed(&state, &before).await
        }
        Err(AccountRemovalRefusal::LastSuperAdmin) => return Ok(refuse(LAST_SUPER_ADMIN)),
    };

    session.flash("success", tailnets::with_logout_note("User deleted", left));
    Ok(navigate(&headers, "/users"))
}
