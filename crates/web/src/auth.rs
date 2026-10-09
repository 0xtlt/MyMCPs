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

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::request::Parts;
use mymcps_core::crypto::{constant_time_eq, random_base64url, sha256_hex};
use mymcps_core::models::User;
use mymcps_core::{Core, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cookies::{Cookies, MaxAge};
use crate::error::AppError;
use crate::session::{SESSION_AGE_SECONDS, Session};

/// Session key of the signed-in user's id.
pub const LOGIN_KEY: &str = "auth_web";
pub const SESSION_STAMP_KEY: &str = "auth_stamp";
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
}

/// The outcome of authenticating a request. In the request extensions once
/// [`silent_auth_layer`] has run.
#[derive(Debug, Clone, Default)]
pub struct Auth {
    pub user: Option<User>,
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
        Ok::<_, sqlx::Error>(user)
    }
    .await;

    match resolved {
        Ok(user) => {
            request.extensions_mut().insert(Auth { user });
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
