//! The browser session, kept whole in one encrypted cookie as the Node app
//! kept it (its `cookie` session store). The server holds nothing, so it can
//! neither expire nor delete a session: see `auth` for how a signed-in
//! session is bounded and revoked all the same.

use std::sync::{Arc, Mutex};

use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{Duration, Utc};
use http::request::Parts;
use mymcps_core::Core;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::cookies::{Cookies, MaxAge};

pub const SESSION_COOKIE: &str = "mymcps_session";

/// A session that is not used for this long is gone (`age: '2h'`).
pub const SESSION_AGE_SECONDS: i64 = 2 * 60 * 60;

/// Binds the cookie's ciphertext to its use.
const SESSION_PURPOSE: &str = "session";

/// Where messages for the next request travel inside the cookie.
const FLASH_KEY: &str = "__flash__";

/// Browsers drop a cookie larger than this, and the session with it.
const MAX_COOKIE_BYTES: usize = 4096;

#[derive(Debug, Default)]
struct State_ {
    values: Map<String, Value>,
    /// Flashed by the previous request, readable during this one.
    flashed: Map<String, Value>,
    /// Flashed by this request for the next one.
    flash: Map<String, Value>,
    had_cookie: bool,
}

/// The session of the current request. Cloning shares it.
#[derive(Debug, Clone, Default)]
pub struct Session {
    state: Arc<Mutex<State_>>,
}

impl Session {
    fn state(&self) -> std::sync::MutexGuard<'_, State_> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A session holding these values, as if a previous request had left them.
    pub fn from_values(mut values: Map<String, Value>) -> Self {
        let flashed = match values.remove(FLASH_KEY) {
            Some(Value::Object(flashed)) => flashed,
            _ => Map::new(),
        };
        Self {
            state: Arc::new(Mutex::new(State_ {
                values,
                flashed,
                flash: Map::new(),
                had_cookie: true,
            })),
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.state().values.get(key).cloned()
    }

    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.get(key)
            .and_then(|value| serde_json::from_value(value).ok())
    }

    pub fn has(&self, key: &str) -> bool {
        self.state().values.contains_key(key)
    }

    pub fn put(&self, key: &str, value: impl Serialize) {
        if let Ok(value) = serde_json::to_value(value) {
            self.state().values.insert(key.to_string(), value);
        }
    }

    /// Read a value and remove it.
    pub fn pull(&self, key: &str) -> Option<Value> {
        self.state().values.shift_remove(key)
    }

    pub fn forget(&self, key: &str) {
        self.state().values.shift_remove(key);
    }

    /// Every key, the one written first at the front. A key keeps its place
    /// when its value is replaced.
    pub fn keys(&self) -> Vec<String> {
        self.state().values.keys().cloned().collect()
    }

    /// Leave a value for the next request only.
    pub fn flash(&self, key: &str, value: impl Serialize) {
        if let Ok(value) = serde_json::to_value(value) {
            self.state().flash.insert(key.to_string(), value);
        }
    }

    /// What the previous request flashed under this key.
    pub fn flashed(&self, key: &str) -> Option<Value> {
        self.state().flashed.get(key).cloned()
    }

    pub fn flashed_text(&self, key: &str) -> Option<String> {
        match self.flashed(key) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        }
    }

    /// Carry what the previous request flashed over to the next one, for a
    /// request that only redirects.
    pub fn reflash(&self) {
        let mut state = self.state();
        let flashed = state.flashed.clone();
        for (key, value) in flashed {
            state.flash.entry(key).or_insert(value);
        }
    }

    /// Every value, and what is flashed for the next request. For tests.
    pub fn snapshot(&self) -> Map<String, Value> {
        let state = self.state();
        let mut values = state.values.clone();
        if !state.flash.is_empty() {
            values.insert(FLASH_KEY.to_string(), Value::Object(state.flash.clone()));
        }
        values
    }

    fn read(core: &Core, cookies: &Cookies) -> Self {
        let stored = cookies.get(SESSION_COOKIE).and_then(|cookie| {
            core.encryption
                .decrypt_value(&cookie, Some(SESSION_PURPOSE))
        });
        match stored {
            Some(Value::Object(values)) => Self::from_values(values),
            _ => {
                let session = Self::default();
                session.state().had_cookie = cookies.get(SESSION_COOKIE).is_some();
                session
            }
        }
    }

    /// The cookie value holding this session, or `None` when it is empty.
    pub fn seal(&self, core: &Core) -> Option<String> {
        Self::seal_values(core, self.snapshot())
    }

    /// The cookie value holding these values, as [`Session::snapshot`]
    /// returns them, or `None` when there are none.
    pub fn seal_values(core: &Core, values: Map<String, Value>) -> Option<String> {
        if values.is_empty() {
            return None;
        }
        let expires_at = Utc::now() + Duration::seconds(SESSION_AGE_SECONDS);
        Some(core.encryption.encrypt_value(
            &Value::Object(values),
            Some(SESSION_PURPOSE),
            Some(expires_at),
        ))
    }

    fn write(&self, core: &Core, cookies: &Cookies) {
        match self.seal(core) {
            Some(sealed) => {
                if sealed.len() > MAX_COOKIE_BYTES {
                    tracing::warn!(
                        bytes = sealed.len(),
                        "The session cookie is larger than browsers keep"
                    );
                }
                // Written on every response, which also pushes its expiry back.
                cookies.set(
                    SESSION_COOKIE,
                    &sealed,
                    MaxAge::Seconds(SESSION_AGE_SECONDS),
                );
            }
            None if self.state().had_cookie => cookies.clear(SESSION_COOKIE),
            None => {}
        }
    }
}

/// Loads the session from its cookie and writes it back after the handler.
/// Runs inside [`crate::cookies::cookies_layer`].
pub async fn session_layer(
    State(core): State<Arc<Core>>,
    mut request: Request,
    next: Next,
) -> Response {
    let cookies = request
        .extensions()
        .get::<Cookies>()
        .cloned()
        .unwrap_or_default();
    let session = Session::read(&core, &cookies);
    request.extensions_mut().insert(session.clone());

    let response = next.run(request).await;
    session.write(&core, &cookies);
    response
}

impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Session>()
            .cloned()
            .unwrap_or_default())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Cookies {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Cookies>()
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use mymcps_core::{Config, Db};
    use serde_json::json;

    use super::*;

    async fn core() -> (Arc<Core>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db.sqlite3")).await.unwrap();
        (Core::new(Config::for_tests(dir.path()), db), dir)
    }

    #[tokio::test]
    async fn round_trips_values_and_flashes_once() {
        let (core, _dir) = core().await;
        let first = Session::default();
        assert_eq!(first.seal(&core), None, "an empty session sets no cookie");

        first.put("auth_web", 7);
        first.put("oauthReturnTo", "/authorize?client_id=x");
        first.flash("success", "Saved");
        let cookie = first.seal(&core).unwrap();

        let cookies = Cookies::parse(Some(&format!("{SESSION_COOKIE}={cookie}")), false);
        let second = Session::read(&core, &cookies);
        assert_eq!(second.get_as::<i64>("auth_web"), Some(7));
        assert!(second.has("oauthReturnTo"));
        assert_eq!(second.flashed_text("success").as_deref(), Some("Saved"));
        assert_eq!(
            second.pull("oauthReturnTo"),
            Some(json!("/authorize?client_id=x"))
        );
        assert!(!second.has("oauthReturnTo"));

        // The flash is not carried further unless asked.
        let third = Session::read(
            &core,
            &Cookies::parse(
                Some(&format!("{SESSION_COOKIE}={}", second.seal(&core).unwrap())),
                false,
            ),
        );
        assert_eq!(third.flashed("success"), None);
        assert_eq!(third.get_as::<i64>("auth_web"), Some(7));

        second.reflash();
        let kept = Session::read(
            &core,
            &Cookies::parse(
                Some(&format!("{SESSION_COOKIE}={}", second.seal(&core).unwrap())),
                false,
            ),
        );
        assert_eq!(kept.flashed_text("success").as_deref(), Some("Saved"));
    }

    #[tokio::test]
    async fn ignores_cookies_it_did_not_write() {
        let (core, _dir) = core().await;
        for forged in [
            "",
            "garbage",
            "gcm.v1:AAAA.AAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAA",
        ] {
            let cookies = Cookies::parse(Some(&format!("{SESSION_COOKIE}={forged}")), false);
            let session = Session::read(&core, &cookies);
            assert!(session.snapshot().is_empty());
        }

        // A value sealed for another purpose with the same key is not a session.
        let other =
            core.encryption
                .encrypt_value(&json!({"auth_web": 1}), Some("remember_web"), None);
        let session = Session::read(
            &core,
            &Cookies::parse(Some(&format!("{SESSION_COOKIE}={other}")), false),
        );
        assert!(!session.has("auth_web"));
    }
}
