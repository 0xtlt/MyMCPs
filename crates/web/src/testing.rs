//! A client for functional tests: it sends requests straight to the router,
//! and can read and forge the session cookie the way a browser holding a
//! copied cookie could.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use http::header::{ACCEPT, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use http_body_util::BodyExt;
use mymcps_core::models::User;
use mymcps_core::{Core, TestCore};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::{Map, Value};
use tower::ServiceExt;

use crate::auth::{LOGIN_KEY, SESSION_STAMP_KEY, SessionStamp};
use crate::cookies::Cookies;
use crate::csrf::{CSRF_FIELD, CSRF_HEADER, csrf_token};
use crate::session::{SESSION_COOKIE, Session};
use crate::state::AppState;

/// The app on a fresh database in a temporary directory.
pub struct TestApp {
    pub state: AppState,
    pub core: Arc<Core>,
    _test_core: TestCore,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(adjust: impl FnOnce(&mut mymcps_core::Config)) -> Self {
        Self::with_state(adjust, AppState::new).await
    }

    /// The app around an upstream the test configures: where its requests
    /// go, how names resolve, what runs npm MCPs.
    ///
    /// ```ignore
    /// let app = TestApp::with_upstream(|upstream| upstream.fetcher(Fetcher::offline())).await;
    /// ```
    pub async fn with_upstream(
        configure: impl FnOnce(mymcps_upstream::UpstreamBuilder) -> mymcps_upstream::UpstreamBuilder,
    ) -> Self {
        Self::with_state(
            |_| {},
            |core| {
                let builder =
                    mymcps_upstream::Upstream::builder(core.clone(), crate::state::builtins());
                AppState::with_upstream(core, configure(builder).build())
            },
        )
        .await
    }

    /// The app with a configuration and a state of the test's making, for
    /// what the two shorter constructors do not cover (a registry of
    /// built-in MCPs written for the test).
    pub async fn with_state(
        adjust: impl FnOnce(&mut mymcps_core::Config),
        state: impl FnOnce(Arc<Core>) -> AppState,
    ) -> Self {
        let test_core = TestCore::with_config(adjust).await;
        let core = test_core.core.clone();
        Self {
            state: state(core.clone()),
            core,
            _test_core: test_core,
        }
    }

    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest {
            app: self,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            cookies: BTreeMap::new(),
            session: None,
            with_csrf: false,
            body: TestBody::Empty,
            browser: true,
        }
    }

    pub fn get(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PUT, path)
    }

    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PATCH, path)
    }

    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::DELETE, path)
    }
}

enum TestBody {
    Empty,
    Form(Vec<(String, String)>),
    Json(Value),
    Raw(Bytes, String),
    Multipart(Vec<(String, Option<String>, Bytes)>),
}

/// A `multipart/form-data` body and its content type, as a browser sends a
/// form that carries a file: `(name, file name, content)` for each part, in
/// this order, with a file name for the parts that are files.
pub fn multipart_form(parts: &[(&str, Option<&str>, &[u8])]) -> (Bytes, String) {
    const BOUNDARY: &str = "----MyMCPsTestFormBoundaryZ4hKx7q2LmP9";
    let mut body = Vec::new();
    for (name, file_name, content) in parts {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(file_name) = file_name {
            body.extend_from_slice(
                format!("; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (
        Bytes::from(body),
        format!("multipart/form-data; boundary={BOUNDARY}"),
    )
}

/// One request. Nothing is followed and nothing is kept between requests:
/// pass what a response set on to the next request yourself.
pub struct TestRequest<'app> {
    app: &'app TestApp,
    method: Method,
    path: String,
    headers: HeaderMap,
    cookies: BTreeMap<String, String>,
    session: Option<Map<String, Value>>,
    with_csrf: bool,
    body: TestBody,
    browser: bool,
}

/// The session values of a user signed in just now.
pub fn session_for(user: &User) -> Map<String, Value> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut values = Map::new();
    values.insert(LOGIN_KEY.into(), Value::from(user.id));
    values.insert(
        SESSION_STAMP_KEY.into(),
        serde_json::to_value(SessionStamp::new(user, now)).unwrap_or(Value::Null),
    );
    values
}

impl TestRequest<'_> {
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::from_str(value)) {
            self.headers.insert(name, value);
        }
        self
    }

    /// Send a cookie, with the value as the browser would hold it (decoded).
    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.insert(name.to_string(), value.to_string());
        self
    }

    /// Present a session holding these values, as replaying a copied session
    /// cookie would.
    pub fn session(mut self, values: Map<String, Value>) -> Self {
        self.session.get_or_insert_with(Map::new).extend(values);
        self
    }

    /// Sign the request in as this user, with a session stamped now.
    pub fn login_as(self, user: &User) -> Self {
        self.session(session_for(user))
    }

    /// Send a valid CSRF token with the request.
    pub fn csrf(mut self) -> Self {
        self.with_csrf = true;
        self
    }

    pub fn form(mut self, fields: &[(&str, &str)]) -> Self {
        self.body = TestBody::Form(
            fields
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        );
        self
    }

    pub fn json(mut self, value: Value) -> Self {
        self.body = TestBody::Json(value);
        self
    }

    pub fn raw_body(mut self, body: impl Into<Bytes>, content_type: &str) -> Self {
        self.body = TestBody::Raw(body.into(), content_type.to_string());
        self
    }

    /// Send a form that carries a file: see [`multipart_form`]. With
    /// [`Self::csrf`], the token is its first part, where the pages put it.
    pub fn multipart(mut self, parts: &[(&str, Option<&str>, &[u8])]) -> Self {
        self.body = TestBody::Multipart(
            parts
                .iter()
                .map(|(name, file_name, content)| {
                    (
                        name.to_string(),
                        file_name.map(str::to_string),
                        Bytes::copy_from_slice(content),
                    )
                })
                .collect(),
        );
        self
    }

    /// Send the request as a script or an API client would: without asking for HTML.
    pub fn api(mut self) -> Self {
        self.browser = false;
        self
    }

    pub async fn send(mut self) -> TestResponse {
        let core = &self.app.core;
        let mut csrf = None;
        if self.session.is_some() || self.with_csrf {
            let mut values = self.session.take().unwrap_or_default();
            if self.with_csrf {
                // The token needs the session's secret, which is created on demand.
                let session = Session::from_values(values.clone());
                csrf = Some(csrf_token(&session));
                let flashed = values.shift_remove("__flash__");
                values = session.snapshot();
                values.extend(flashed.map(|flashed| ("__flash__".to_string(), flashed)));
            }
            if let Some(sealed) = Session::seal_values(core, values) {
                self.cookies.insert(SESSION_COOKIE.to_string(), sealed);
            }
        }

        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(&self.path);
        if self.browser && !self.headers.contains_key(ACCEPT) {
            builder = builder.header(ACCEPT, "text/html,application/xhtml+xml");
        }
        if !self.cookies.is_empty() {
            let header: Vec<String> = self
                .cookies
                .iter()
                .map(|(name, value)| {
                    format!("{name}={}", utf8_percent_encode(value, NON_ALPHANUMERIC))
                })
                .collect();
            builder = builder.header(COOKIE, header.join("; "));
        }

        let body = match self.body {
            TestBody::Empty => {
                if let Some(token) = &csrf {
                    builder = builder.header(CSRF_HEADER, token);
                }
                Body::empty()
            }
            TestBody::Form(mut fields) => {
                if let Some(token) = &csrf {
                    fields.push((CSRF_FIELD.to_string(), token.clone()));
                }
                let encoded: Vec<String> = fields
                    .iter()
                    .map(|(name, value)| {
                        format!(
                            "{}={}",
                            utf8_percent_encode(name, NON_ALPHANUMERIC),
                            utf8_percent_encode(value, NON_ALPHANUMERIC)
                        )
                    })
                    .collect();
                builder = builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(encoded.join("&"))
            }
            TestBody::Json(value) => {
                if let Some(token) = &csrf {
                    builder = builder.header(CSRF_HEADER, token);
                }
                builder = builder.header(CONTENT_TYPE, "application/json");
                Body::from(value.to_string())
            }
            TestBody::Raw(bytes, content_type) => {
                if let Some(token) = &csrf {
                    builder = builder.header(CSRF_HEADER, token);
                }
                builder = builder.header(CONTENT_TYPE, content_type);
                Body::from(bytes)
            }
            TestBody::Multipart(parts) => {
                let token = csrf
                    .as_ref()
                    .map(|token| (CSRF_FIELD, None, token.as_bytes()));
                let parts: Vec<(&str, Option<&str>, &[u8])> = token
                    .into_iter()
                    .chain(parts.iter().map(|(name, file_name, content)| {
                        (name.as_str(), file_name.as_deref(), &content[..])
                    }))
                    .collect();
                let (body, content_type) = multipart_form(&parts);
                builder = builder.header(CONTENT_TYPE, content_type);
                Body::from(body)
            }
        };

        let mut request = builder.body(body).expect("a valid test request");
        for (name, value) in &self.headers {
            request.headers_mut().insert(name, value.clone());
        }
        // Tests reach the router without a socket: say where they come from.
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                49152,
            ))));

        let response = crate::app::service(self.app.state.clone())
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        let (parts, body) = response.into_parts();
        let body = body
            .collect()
            .await
            .map(|collected| collected.to_bytes())
            .unwrap_or_default();
        TestResponse {
            status: parts.status,
            headers: parts.headers,
            body,
            core: core.clone(),
        }
    }
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    core: Arc<Core>,
}

impl TestResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The path a redirect points to, without its query string.
    pub fn redirect_path(&self) -> Option<String> {
        let location = self.headers.get(LOCATION)?.to_str().ok()?;
        Some(
            location
                .split(['?', '#'])
                .next()
                .unwrap_or(location)
                .to_string(),
        )
    }

    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The value a `Set-Cookie` of this response gives the cookie, decoded.
    /// `None` when the response does not set it, or deletes it.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie| {
                let (pair, attributes) = cookie.split_once(';').unwrap_or((cookie, ""));
                let (cookie_name, value) = pair.split_once('=')?;
                if cookie_name != name || attributes.contains("Max-Age=0") {
                    return None;
                }
                Some(percent_decode_str(value).decode_utf8_lossy().into_owned())
            })
    }

    /// Whether the response tells the browser to delete the cookie.
    pub fn clears_cookie(&self, name: &str) -> bool {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|cookie| cookie.starts_with(&format!("{name}=;")) && cookie.contains("Max-Age=0"))
    }

    /// The session the response leaves in the browser, with what it flashed.
    pub fn session(&self) -> Map<String, Value> {
        let Some(sealed) = self.cookie(SESSION_COOKIE) else {
            return Map::new();
        };
        let header = format!(
            "{SESSION_COOKIE}={}",
            utf8_percent_encode(&sealed, NON_ALPHANUMERIC)
        );
        let cookies = Cookies::parse(Some(&header), false);
        cookies
            .get(SESSION_COOKIE)
            .and_then(|cookie| self.core.encryption.decrypt_value(&cookie, Some("session")))
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    /// What the response flashed for the next request under this key.
    pub fn flashed(&self, key: &str) -> Option<Value> {
        self.session().get("__flash__")?.get(key).cloned()
    }
}

/// Records for tests, as `tests/helpers/factories.ts` made them. Every user
/// has the password `password123`.
pub mod factories {
    use std::sync::atomic::{AtomicU64, Ordering};

    use mymcps_core::Timestamp;
    use mymcps_core::models::{Invite, Mcp, McpStatus, McpTransport, User, UserRole};

    use super::TestApp;

    pub const PASSWORD: &str = "password123";

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    pub fn next_value(prefix: &str) -> String {
        format!("{prefix}-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1)
    }

    pub async fn create_user(app: &TestApp, email: Option<&str>, role: UserRole) -> User {
        let email = email
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}@example.com", next_value("user")));
        let mut user = User::with_password(&email, Some("Test User"), PASSWORD, role)
            .await
            .expect("a hashed password");
        user.insert(&*app.core.db).await.expect("a new user");
        user
    }

    pub async fn create_admin(app: &TestApp) -> User {
        create_user(app, None, UserRole::Admin).await
    }

    pub async fn create_admin_with(app: &TestApp, email: &str) -> User {
        create_user(app, Some(email), UserRole::Admin).await
    }

    pub async fn create_member(app: &TestApp) -> User {
        create_user(app, None, UserRole::Member).await
    }

    pub async fn create_member_with(app: &TestApp, email: &str) -> User {
        create_user(app, Some(email), UserRole::Member).await
    }

    /// An invite for a member, valid for a week. Change it with `adjust`.
    pub async fn create_invite(
        app: &TestApp,
        created_by: i64,
        adjust: impl FnOnce(&mut Invite),
    ) -> Invite {
        let mut invite = Invite {
            email: format!("{}@example.com", next_value("invite")),
            role: UserRole::Member,
            token: Invite::generate_token(),
            created_by,
            expires_at: Timestamp::now() + chrono::Duration::days(7),
            ..Default::default()
        };
        adjust(&mut invite);
        invite.insert(&*app.core.db).await.expect("a new invite");
        invite
    }

    /// A ready, enabled HTTP MCP. Change it with `adjust`.
    pub async fn create_mcp(app: &TestApp, created_by: i64, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
        let name = next_value("MCP");
        let mut mcp = Mcp {
            slug: Mcp::slugify(&name),
            name,
            transport: McpTransport::Http,
            status: McpStatus::Ready,
            enabled: true,
            created_by,
            ..Default::default()
        };
        adjust(&mut mcp);
        match mcp.transport {
            McpTransport::Http if mcp.http_url.is_none() => {
                mcp.http_url = Some("http://127.0.0.1:9999/mcp".into())
            }
            McpTransport::Npm if mcp.npm_package.is_none() => {
                mcp.npm_package = Some("@example/mcp".into())
            }
            _ => {}
        }
        mcp.insert(&*app.core.db).await.expect("a new MCP");
        mcp
    }
}
