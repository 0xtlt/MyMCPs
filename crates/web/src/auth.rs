//! Who is signed in.
//!
//! The session cookie holds the login (`auth_web`, the user id) and a stamp.
//! The cookie store keeps a session entirely in the browser, where the
//! server can neither expire nor delete it: a copied session cookie would
//! stay valid forever. Every authenticated session therefore carries a stamp,
//! checked on each request. It records the user's session version at
//! sign-in, which the user bumps to retire their sessions, and the times that
//! bound how long the session lives.
//!
//! When the session cannot authenticate the request, the remember-me cookie
//! can: its token lives in the database, where it can be revoked. Cookie and
//! token are in the format of the AdonisJS session guard, so browsers signed
//! in to the Node app stay signed in.
//!
//! Someone who is signed in works in one team at a time: [`Auth::team`].
//! The session says which, then the account (the team it last worked in),
//! then the order the teams were joined in. What the person may do there is
//! read from the database on every request, never from the session.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::request::Parts;
use mymcps_core::crypto::{constant_time_eq, random_base64url, sha256_hex};
use mymcps_core::models::{Membership, Team, TeamRole, User};
use mymcps_core::{Core, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cookies::{Cookies, MaxAge};
use crate::error::AppError;
use crate::session::{SESSION_AGE_SECONDS, Session};

/// Session key of the signed-in user's id.
pub const LOGIN_KEY: &str = "auth_web";
pub const SESSION_STAMP_KEY: &str = "auth_stamp";
/// Session key of the id of the team this browser works in.
pub const TEAM_KEY: &str = "currentTeam";
pub const REMEMBER_COOKIE: &str = "remember_web";

/// However active, a session has to be re-established this long after sign-in.
const ABSOLUTE_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;
/// One year, as the Node app counted it: 365.25 days.
const REMEMBER_SECONDS: i64 = 31_557_600;
/// Length of a remember-me secret, as the Node guard generated it.
const REMEMBER_SECRET_CHARS: usize = 40;

/// The stamp of an authenticated session: the user's session version at
/// sign-in and two millisecond timestamps. Strict positive integers, because
/// the server wrote them as numbers and anything else is not a stamp it issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStamp {
    pub version: i64,
    pub authenticated_at: i64,
    pub last_seen_at: i64,
}

impl SessionStamp {
    pub fn new(user: &User, now: i64) -> Self {
        Self {
            version: user.session_version,
            authenticated_at: now,
            last_seen_at: now,
        }
    }

    /// The stamp of the session, or `None` when it carries none or one the
    /// app did not write.
    pub fn read(session: &Session) -> Option<Self> {
        let Value::Object(fields) = session.get(SESSION_STAMP_KEY)? else {
            return None;
        };
        let strict = |name: &str| fields.get(name)?.as_i64().filter(|value| *value > 0);
        Some(Self {
            version: strict("version")?,
            authenticated_at: strict("authenticatedAt")?,
            last_seen_at: strict("lastSeenAt")?,
        })
    }

    /// Whether the session sat idle for longer than the session age, or has
    /// outlived the absolute lifetime.
    pub fn is_expired(&self, now: i64) -> bool {
        now - self.last_seen_at > SESSION_AGE_SECONDS * 1000
            || now - self.authenticated_at > ABSOLUTE_LIFETIME_MS
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Mark the session as authenticated under the user's current session version.
pub fn stamp_session(session: &Session, user: &User) {
    session.put(SESSION_STAMP_KEY, SessionStamp::new(user, now_ms()));
}

/// Drop the login a session carries, keeping the rest of its data.
fn forget_session_login(session: &Session) {
    session.forget(LOGIN_KEY);
    session.forget(SESSION_STAMP_KEY);
    // The next person to sign in on this browser starts in a team of theirs.
    session.forget(TEAM_KEY);
}

/// The outcome of authenticating a request. In the request extensions once
/// [`silent_auth_layer`] has run.
#[derive(Debug, Clone, Default)]
pub struct Auth {
    pub user: Option<User>,
    /// The teams of that person, in the order they joined them. Empty for a
    /// visitor.
    pub teams: Vec<Membership>,
    /// The one they work in. Always there for someone who is signed in.
    pub team: Option<Membership>,
}

impl Auth {
    /// Whether the person runs the instance: an owner or an administrator
    /// of the Root Team, whichever team they work in.
    pub fn is_instance_admin(&self) -> bool {
        self.teams
            .iter()
            .any(|membership| membership.team.is_root() && membership.role.is_admin())
    }

    /// The person and the team they work in, for a handler behind
    /// [`crate::guards::require_auth`] that needs both.
    pub fn signed_in(self) -> Result<(User, CurrentTeam), AppError> {
        match (self.user, self.team) {
            (Some(user), Some(team)) => Ok((user, CurrentTeam(team))),
            _ => Err(AppError::Unauthorized),
        }
    }

    /// Whether the person is an owner or an administrator of the team they
    /// work in.
    pub fn is_team_admin(&self) -> bool {
        self.team
            .as_ref()
            .is_some_and(|membership| membership.role.is_admin())
    }
}

/// The teams of someone who is signed in, and the one they work in.
///
/// An account always has a team. One that has none (its last team was
/// deleted under it) gets its personal team back here.
async fn resolve_teams(
    core: &Core,
    session: &Session,
    user: &User,
) -> Result<(Vec<Membership>, Membership), sqlx::Error> {
    let mut teams = Team::memberships(&*core.db, user.id).await?;
    if teams.is_empty() {
        let mut transaction = core.db.begin().await?;
        let team = Team::create_personal(&mut transaction, user).await?;
        transaction.commit().await?;
        teams.push(Membership {
            team,
            role: TeamRole::Owner,
        });
    }

    let wanted = [
        session.get(TEAM_KEY).and_then(|id| id.as_i64()),
        user.current_team_id,
    ];
    let current = wanted
        .into_iter()
        .flatten()
        .find_map(|id| teams.iter().find(|membership| membership.team.id == id))
        .unwrap_or(&teams[0])
        .clone();
    if session.get(TEAM_KEY).and_then(|id| id.as_i64()) != Some(current.team.id) {
        session.put(TEAM_KEY, current.team.id);
    }
    Ok((teams, current))
}

/// Make a team the one this browser works in, and the one the account
/// comes back to. The caller has checked that the person is in it.
pub async fn switch_team(
    core: &Core,
    session: &Session,
    user: &mut User,
    team_id: i64,
) -> Result<(), sqlx::Error> {
    session.put(TEAM_KEY, team_id);
    user.remember_team(&*core.db, team_id).await
}

async fn create_remember_token(
    core: &Core,
    cookies: &Cookies,
    user: &User,
) -> Result<(), sqlx::Error> {
    let secret = random_base64url(REMEMBER_SECRET_CHARS)[..REMEMBER_SECRET_CHARS].to_string();
    let now = Timestamp::now();
    // The Node app bound JavaScript dates here, which knex stores as epoch milliseconds.
    let id: i64 = sqlx::query_scalar(
        "insert into `remember_me_tokens` (`tokenable_id`, `hash`, `created_at`, `updated_at`, `expires_at`) values (?, ?, ?, ?, ?) returning `id`",
    )
    .bind(user.id)
    .bind(sha256_hex(&secret))
    .bind(now.timestamp_millis())
    .bind(now.timestamp_millis())
    .bind(now.timestamp_millis() + REMEMBER_SECONDS * 1000)
    .fetch_one(&*core.db)
    .await?;

    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(id.to_string()),
        URL_SAFE_NO_PAD.encode(&secret)
    );
    let sealed = core
        .encryption
        .encrypt_value(&Value::String(token), Some(REMEMBER_COOKIE), None);
    cookies.set(
        REMEMBER_COOKIE,
        &format!("e:{sealed}"),
        MaxAge::Seconds(REMEMBER_SECONDS),
    );
    Ok(())
}

/// The id and secret inside a remember-me cookie.
fn decode_remember_cookie(core: &Core, cookies: &Cookies) -> Option<(i64, String)> {
    let cookie = cookies.get(REMEMBER_COOKIE)?;
    let sealed = cookie.strip_prefix("e:")?;
    let Value::String(token) = core
        .encryption
        .decrypt_value(sealed, Some(REMEMBER_COOKIE))?
    else {
        return None;
    };
    let (identifier, secret) = token.split_once('.')?;
    let identifier = String::from_utf8(URL_SAFE_NO_PAD.decode(identifier).ok()?).ok()?;
    let secret = String::from_utf8(URL_SAFE_NO_PAD.decode(secret).ok()?).ok()?;
    Some((identifier.parse().ok()?, secret))
}

/// Sign in with the remember-me cookie. The token is used once: a new one
/// replaces it.
async fn authenticate_via_remember_cookie(
    core: &Core,
    session: &Session,
    cookies: &Cookies,
) -> Result<Option<User>, sqlx::Error> {
    let Some((id, secret)) = decode_remember_cookie(core, cookies) else {
        return Ok(None);
    };
    let row: Option<(i64, String, Timestamp)> = sqlx::query_as(
        "select `tokenable_id`, `hash`, `expires_at` from `remember_me_tokens` where `id` = ?",
    )
    .bind(id)
    .fetch_optional(&*core.db)
    .await?;
    let Some((user_id, hash, expires_at)) = row else {
        return Ok(None);
    };
    if !constant_time_eq(sha256_hex(&secret).as_bytes(), hash.as_bytes()) || expires_at.is_past() {
        return Ok(None);
    }
    let Some(user) = User::find(&*core.db, user_id).await? else {
        return Ok(None);
    };

    sqlx::query("delete from `remember_me_tokens` where `id` = ?")
        .bind(id)
        .execute(&*core.db)
        .await?;
    create_remember_token(core, cookies, &user).await?;
    session.put(LOGIN_KEY, user.id);
    Ok(Some(user))
}

/// Sign the user in on this browser, remembered, with a stamped session.
pub async fn sign_in(
    core: &Core,
    session: &Session,
    cookies: &Cookies,
    user: &User,
) -> Result<(), sqlx::Error> {
    create_remember_token(core, cookies, user).await?;
    session.put(LOGIN_KEY, user.id);
    stamp_session(session, user);
    crate::csrf::forget_csrf_secret(session);
    Ok(())
}

/// Sign out of this browser: forget the login and revoke its remember-me token.
pub async fn sign_out(
    core: &Core,
    session: &Session,
    cookies: &Cookies,
) -> Result<(), sqlx::Error> {
    if let Some((id, _)) = decode_remember_cookie(core, cookies) {
        sqlx::query("delete from `remember_me_tokens` where `id` = ?")
            .bind(id)
            .execute(&*core.db)
            .await?;
    }
    forget_session_login(session);
    crate::csrf::forget_csrf_secret(session);
    cookies.clear(REMEMBER_COOKIE);
    Ok(())
}

async fn authenticate(
    core: &Core,
    session: &Session,
    cookies: &Cookies,
) -> Result<(Option<User>, bool), sqlx::Error> {
    // A session that names a user never falls back to the remember-me cookie,
    // even when that user is gone.
    if let Some(login) = session.get(LOGIN_KEY) {
        let user = match login.as_i64() {
            Some(id) => User::find(&*core.db, id).await?,
            None => None,
        };
        return Ok((user, false));
    }
    let user = authenticate_via_remember_cookie(core, session, cookies).await?;
    let via_remember = user.is_some();
    Ok((user, via_remember))
}

/// Finds out who is signed in, on every request, without refusing anyone.
///
/// A session only counts as logged-in while its stamp is valid. Otherwise its
/// login is dropped, which leaves the remember-me cookie, revocable on the
/// server, as the only way to carry on without signing in again.
pub async fn silent_auth_layer(
    State(core): State<Arc<Core>>,
    mut request: Request,
    next: Next,
) -> Response {
    let session = request
        .extensions()
        .get::<Session>()
        .cloned()
        .unwrap_or_default();
    let cookies = request
        .extensions()
        .get::<Cookies>()
        .cloned()
        .unwrap_or_default();

    let resolved = async {
        // Sessions from before stamps existed, idle sessions, and sessions
        // past their absolute lifetime.
        let has_login = session.has(LOGIN_KEY);
        let mut stamp = if has_login {
            SessionStamp::read(&session)
        } else {
            None
        };
        if has_login && stamp.is_none_or(|stamp| stamp.is_expired(now_ms())) {
            forget_session_login(&session);
            stamp = None;
        }

        let (mut user, mut via_remember) = authenticate(&core, &session, &cookies).await?;

        // The user signed out or changed their password after this session
        // was stamped: try the remember-me cookie instead.
        if let (Some(stamped), Some(found)) = (stamp, &user)
            && stamped.version != found.session_version
        {
            forget_session_login(&session);
            stamp = None;
            (user, via_remember) = authenticate(&core, &session, &cookies).await?;
        }

        match (&user, stamp) {
            (Some(user), _) if via_remember => stamp_session(&session, user),
            (Some(_), Some(stamp)) => {
                session.put(
                    SESSION_STAMP_KEY,
                    SessionStamp {
                        last_seen_at: now_ms(),
                        ..stamp
                    },
                );
            }
            _ => {}
        }
        let (teams, team) = match &user {
            Some(user) => {
                let (teams, team) = resolve_teams(&core, &session, user).await?;
                (teams, Some(team))
            }
            None => (Vec::new(), None),
        };
        Ok::<_, sqlx::Error>(Auth { user, teams, team })
    }
    .await;

    match resolved {
        Ok(auth) => {
            request.extensions_mut().insert(auth);
            next.run(request).await
        }
        Err(error) => AppError::from(error).into_response(),
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Auth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Auth>().cloned().unwrap_or_default())
    }
}

/// The signed-in user, for handlers behind [`crate::guards::require_auth`].
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Auth>()
            .and_then(|auth| auth.user.clone())
            .map(CurrentUser)
            .ok_or(AppError::Unauthorized)
    }
}

impl std::ops::Deref for CurrentUser {
    type Target = User;

    fn deref(&self) -> &User {
        &self.0
    }
}

/// The team the signed-in person works in and what they are in it, for
/// handlers behind [`crate::guards::require_auth`]. Everything a handler
/// reads or writes of a team goes through this id: a row of another team is
/// answered like a row that does not exist.
#[derive(Debug, Clone)]
pub struct CurrentTeam(pub Membership);

impl CurrentTeam {
    pub fn id(&self) -> i64 {
        self.0.team.id
    }

    pub fn role(&self) -> TeamRole {
        self.0.role
    }

    /// An owner or an administrator of the team.
    pub fn is_admin(&self) -> bool {
        self.0.role.is_admin()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentTeam {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Auth>()
            .and_then(|auth| auth.team.clone())
            .map(CurrentTeam)
            .ok_or(AppError::Unauthorized)
    }
}

impl std::ops::Deref for CurrentTeam {
    type Target = Membership;

    fn deref(&self) -> &Membership {
        &self.0
    }
}
