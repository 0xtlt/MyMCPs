//! The authorizations an administrator has started and not finished yet.
//!
//! Each lives in the browser session between the redirect to the provider
//! and its callback, under a key made of its `state`. The session belongs to
//! the web layer: this crate only asks it for the four operations of
//! [`OauthSessionStore`].

use mymcps_core::models::Mcp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::validators::OAUTH_SESSION_VALIDATOR;

const OAUTH_SESSION_PREFIX: &str = "mcp_oauth:";

/// Authorizations one browser session may have in flight. Older starts are dropped.
const MAX_PENDING_OAUTH_STARTS: usize = 5;

/// Most the pending authorizations may weigh in the session. The cookie store
/// keeps the whole session in one cookie, and a browser ignores a cookie over
/// 4096 bytes: the write would be lost without an error. This leaves room for
/// the sign-in, the CSRF secret and a flash message.
pub const MAX_PENDING_OAUTH_START_BYTES: usize = 1536;

/// What the OAuth flow needs from the browser session of the administrator.
///
/// The web layer implements it for its session type. Values are the JSON the
/// session stores, and `keys` lists every key of the session with the one
/// written first at the front: a key keeps its place when its value is
/// replaced.
pub trait OauthSessionStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Value>;
    fn put(&self, key: &str, value: Value);
    fn forget(&self, key: &str);
    fn keys(&self) -> Vec<String>;
}

/// A pending authorization, as it is read back from the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthSession {
    pub mcp_id: i64,
    /// Absent for a built-in MCP, which authenticates with its client secret
    /// instead of PKCE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
    pub state: String,
    pub redirect_uri: String,
    pub authorization_server_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub client_id: String,
}

/// What a flow that starts puts in the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthStart {
    pub redirect_uri: String,
    pub authorization_server_url: String,
    pub resource: Option<String>,
    pub client_id: String,
    pub code_verifier: Option<String>,
    pub state: String,
}

fn oauth_session_key(state: &str) -> String {
    format!("{OAUTH_SESSION_PREFIX}{state}")
}

/// The JSON kept in the session, with its members in the order the Node app
/// wrote them.
fn session_payload(mcp_id: i64, start: &OauthStart) -> Value {
    let mut payload = Map::new();
    payload.insert("mcpId".to_owned(), Value::from(mcp_id));
    payload.insert(
        "redirectUri".to_owned(),
        Value::from(start.redirect_uri.as_str()),
    );
    payload.insert(
        "authorizationServerUrl".to_owned(),
        Value::from(start.authorization_server_url.as_str()),
    );
    if let Some(resource) = &start.resource {
        payload.insert("resource".to_owned(), Value::from(resource.as_str()));
    }
    payload.insert("clientId".to_owned(), Value::from(start.client_id.as_str()));
    if let Some(code_verifier) = &start.code_verifier {
        payload.insert(
            "codeVerifier".to_owned(),
            Value::from(code_verifier.as_str()),
        );
    }
    payload.insert("state".to_owned(), Value::from(start.state.as_str()));
    Value::Object(payload)
}

/// The bytes an entry takes in the session, as its key and its JSON.
fn weight(key: &str, value: Option<&Value>) -> usize {
    key.len() + value.map_or(0, |value| value.to_string().len())
}

/// Keep a starting authorization in the session until its callback.
pub fn start_oauth_session(
    session: &dyn OauthSessionStore,
    mcp: &Mcp,
    start: OauthStart,
) -> OauthSession {
    let payload = session_payload(mcp.id, &start);
    let key = oauth_session_key(&start.state);

    // An abandoned authorization never reaches the callback that clears it.
    // Session keys keep their insertion order, so the first ones are the oldest:
    // they make room until the count and the size fit. The new one always stays.
    let pending: Vec<(String, usize)> = session
        .keys()
        .into_iter()
        .filter(|entry| entry.starts_with(OAUTH_SESSION_PREFIX))
        .map(|entry| {
            let bytes = weight(&entry, session.get(&entry).as_ref());
            (entry, bytes)
        })
        .collect();
    let mut count = pending.len() + 1;
    let mut bytes = pending
        .iter()
        .fold(weight(&key, Some(&payload)), |total, (_, entry)| {
            total + entry
        });
    for (entry, entry_bytes) in &pending {
        if count <= MAX_PENDING_OAUTH_STARTS && bytes <= MAX_PENDING_OAUTH_START_BYTES {
            break;
        }
        count -= 1;
        bytes -= entry_bytes;
        session.forget(entry);
    }

    session.put(&key, payload);
    OauthSession {
        mcp_id: mcp.id,
        code_verifier: start.code_verifier,
        state: start.state,
        redirect_uri: start.redirect_uri,
        authorization_server_url: start.authorization_server_url,
        resource: start.resource,
        client_id: start.client_id,
    }
}

/// The pending authorization a callback names by its `state`, or `None` when
/// this session holds no such authorization.
pub fn read_oauth_session(
    session: &dyn OauthSessionStore,
    state: Option<&str>,
) -> Option<OauthSession> {
    let state = state.filter(|state| !state.is_empty())?;
    let saved = session.get(&oauth_session_key(state));
    OAUTH_SESSION_VALIDATOR.validate_as(saved.as_ref()).ok()
}

pub fn clear_oauth_session(session: &dyn OauthSessionStore, state: Option<&str>) {
    if let Some(state) = state.filter(|state| !state.is_empty()) {
        session.forget(&oauth_session_key(state));
    }
}
