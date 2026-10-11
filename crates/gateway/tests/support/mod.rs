//! What the tests of the gateway share: a gateway on a fresh database, the
//! factories of `tests/helpers/factories.ts`, and an OAuth client that
//! speaks to the server the way an MCP client does.
#![allow(dead_code)]

pub mod mcp;

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mymcps_core::models::{
    AccessToken, CallOutcome, Mcp, McpCallLog, McpStatus, McpTransport, OauthClient, ScopeMode,
    User, UserRole,
};
use mymcps_core::{Config, Core, Db, TestCore, Timestamp};
use mymcps_gateway::access_token::{self, CreatedAccessToken, NewAccessToken};
use mymcps_gateway::{Error, Gateway, GatewayOauthError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const RESOURCE: &str = "http://localhost:3333/mcp";

/// A gateway on a fresh, migrated database.
pub struct TestGateway {
    pub gateway: Gateway,
    pub core: Arc<Core>,
    _test_core: TestCore,
}

impl TestGateway {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(adjust: impl FnOnce(&mut Config)) -> Self {
        let test_core = TestCore::with_config(adjust).await;
        let core = test_core.core.clone();
        Self {
            gateway: Gateway::new(core.clone()),
            core,
            _test_core: test_core,
        }
    }

    pub fn db(&self) -> &Db {
        &self.core.db
    }
}

impl Deref for TestGateway {
    type Target = Gateway;

    fn deref(&self) -> &Gateway {
        &self.gateway
    }
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_value(prefix: &str) -> String {
    format!("{prefix}-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1)
}

pub async fn create_admin(db: &Db) -> User {
    let mut user = User {
        full_name: Some("Test User".into()),
        email: format!("{}@example.com", next_value("user")),
        // No test here signs in with a password.
        password: "not-a-password-hash".into(),
        role: UserRole::Admin,
        session_version: 1,
        ..Default::default()
    };
    user.insert(&**db).await.unwrap();
    user
}

/// An HTTP MCP that is ready and enabled, after `adjust` had its say. Its
/// slug follows its name unless `adjust` sets one.
pub async fn create_mcp(db: &Db, created_by: i64, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    let mut mcp = Mcp {
        name: next_value("MCP"),
        transport: McpTransport::Http,
        http_url: Some("http://127.0.0.1:9999/mcp".into()),
        status: McpStatus::Ready,
        enabled: true,
        created_by,
        ..Default::default()
    };
    adjust(&mut mcp);
    if mcp.slug.is_empty() {
        mcp.slug = Mcp::slugify(&mcp.name);
    }
    mcp.insert(&**db).await.unwrap();
    mcp
}

pub async fn create_access_token(
    db: &Db,
    created_by: i64,
    scope_mode: ScopeMode,
    mcp_ids: &[i64],
) -> CreatedAccessToken {
    create_named_access_token(
        db,
        created_by,
        &next_value("token"),
        scope_mode,
        mcp_ids,
        None,
    )
    .await
}

pub async fn create_named_access_token(
    db: &Db,
    created_by: i64,
    name: &str,
    scope_mode: ScopeMode,
    mcp_ids: &[i64],
    expires_at: Option<Timestamp>,
) -> CreatedAccessToken {
    access_token::create(
        db,
        NewAccessToken {
            name,
            scope_mode,
            mcp_ids,
            expires_at,
            created_by,
        },
    )
    .await
    .unwrap()
}

pub async fn create_stored_access_token(
    db: &Db,
    created_by: i64,
    adjust: impl FnOnce(&mut AccessToken),
) -> AccessToken {
    let mut token = AccessToken {
        name: next_value("stored-token"),
        token_hash: "stored-token-hash".into(),
        token_prefix: "mcp_stored".into(),
        scope_mode: ScopeMode::All,
        created_by,
        ..Default::default()
    };
    adjust(&mut token);
    token.insert(&**db).await.unwrap();
    token
}

pub async fn create_mcp_call_log(
    db: &Db,
    token: &AccessToken,
    created_at: Option<Timestamp>,
) -> McpCallLog {
    let mut log = McpCallLog {
        access_token_id: Some(token.id),
        access_token_name: token.name.clone(),
        access_token_prefix: token.token_prefix.clone(),
        requested_tool_name: "test__echo".into(),
        tool_name: Some("echo".into()),
        outcome: CallOutcome::Success,
        duration_ms: 25,
        ..Default::default()
    };
    log.insert(&**db).await.unwrap();
    if let Some(created_at) = created_at {
        sqlx::query("update `mcp_call_logs` set `created_at` = ? where `id` = ?")
            .bind(created_at)
            .bind(log.id)
            .execute(&**db)
            .await
            .unwrap();
    }
    log
}

pub async fn call_logs(db: &Db) -> Vec<McpCallLog> {
    sqlx::query_as("select * from `mcp_call_logs` order by `id` asc")
        .fetch_all(&**db)
        .await
        .unwrap()
}

pub async fn find_token(db: &Db, id: i64) -> AccessToken {
    AccessToken::find(&**db, id).await.unwrap().unwrap()
}

pub async fn find_token_by_name(db: &Db, name: &str) -> AccessToken {
    sqlx::query_as("select * from `access_tokens` where `name` = ? limit 1")
        .bind(name)
        .fetch_one(&**db)
        .await
        .unwrap()
}

pub async fn find_client(db: &Db, client_id: &str) -> OauthClient {
    sqlx::query_as("select * from `oauth_clients` where `client_id` = ? limit 1")
        .bind(client_id)
        .fetch_one(&**db)
        .await
        .unwrap()
}

pub async fn count(db: &Db, table: &'static str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select count(*) from `{table}`"
    )))
    .fetch_one(&**db)
    .await
    .unwrap()
}

/// The PKCE challenge of a code verifier.
pub fn code_challenge(code_verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
}

/// The refusal a request to the OAuth server was answered with.
pub fn oauth_error<T: std::fmt::Debug>(result: Result<T, Error>) -> GatewayOauthError {
    match result {
        Err(Error::Oauth(error)) => error,
        other => panic!("expected an OAuth error, got {other:?}"),
    }
}

/// Assert that a request was refused with this status, error and description.
#[track_caller]
pub fn assert_oauth_error<T: std::fmt::Debug>(
    result: Result<T, Error>,
    status: u16,
    error: &str,
    description: &str,
) {
    let refusal = oauth_error(result);
    assert_eq!(
        (refusal.status, refusal.body()),
        (
            status,
            json!({ "error": error, "error_description": description })
        )
    );
}

/// `{ ...base, ...changes }`, where a change to `None` leaves the parameter out.
pub fn with(base: &Value, changes: &[(&str, Option<Value>)]) -> Value {
    let mut object = base.clone();
    let parameters = object.as_object_mut().unwrap();
    for (name, value) in changes {
        match value {
            Some(value) => parameters.insert((*name).to_owned(), value.clone()),
            None => parameters.remove(*name),
        };
    }
    object
}

/// The metadata the test clients register with.
pub fn registration(client_name: &str, redirect_uri: &str) -> Value {
    json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp:tools",
    })
}

/// Register a client, as `POST /register` does, and return the answer.
pub async fn register(gateway: &Gateway, metadata: &Value) -> Value {
    gateway.oauth.register_client(metadata).await.unwrap()
}

/// The parameters of an authorization request for the gateway.
pub fn authorization(client_id: &str, redirect_uri: &str, code_challenge: &str) -> Value {
    json!({
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "response_type": "code",
        "code_challenge": code_challenge,
        "code_challenge_method": "S256",
        "scope": "mcp:tools",
        "resource": RESOURCE,
        "state": "state-from-client",
    })
}

/// Approve an authorization request as `user_id`, as the consent form does,
/// and return the code its client is sent.
pub async fn approve(gateway: &Gateway, authorization: &Value, user_id: i64) -> String {
    let request = gateway
        .oauth
        .parse_authorization_request(authorization)
        .await
        .unwrap();
    gateway
        .oauth
        .create_authorization_code(&request, user_id)
        .await
        .unwrap()
}

/// What was logged while it is in scope, as text, on the thread that made it.
///
/// One subscriber takes the logs of the whole test binary and keeps them by
/// thread. A subscriber of the test's own thread would miss an event that
/// another test, on its own thread, was first to reach: `tracing` decides
/// once who listens to an event.
pub struct CapturedLogs {
    thread: std::thread::ThreadId,
    start: usize,
}

type LogsByThread = std::sync::Mutex<std::collections::HashMap<std::thread::ThreadId, Vec<u8>>>;

static LOGS: std::sync::LazyLock<LogsByThread> = std::sync::LazyLock::new(|| {
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(|| LogWriter(std::thread::current().id()))
        .finish();
    // Nothing else sets a subscriber in a test binary.
    let _ = tracing::subscriber::set_global_default(subscriber);
    LogsByThread::default()
});

struct LogWriter(std::thread::ThreadId);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        LOGS.lock()
            .unwrap()
            .entry(self.0)
            .or_default()
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedLogs {
    pub fn start() -> Self {
        let thread = std::thread::current().id();
        let start = LOGS.lock().unwrap().get(&thread).map_or(0, Vec::len);
        Self { thread, start }
    }

    pub fn text(&self) -> String {
        let logs = LOGS.lock().unwrap();
        let logged = logs
            .get(&self.thread)
            .map_or(&[][..], |logged| &logged[self.start..]);
        String::from_utf8_lossy(logged).into_owned()
    }
}
