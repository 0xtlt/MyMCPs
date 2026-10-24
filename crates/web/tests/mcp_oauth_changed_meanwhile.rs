//! An OAuth step that is in flight when an admin changes the MCP: the
//! exchange of the code of a callback and the refresh of an access token both
//! ask the provider and then save what it answered. What the provider issued
//! belongs to the destination and the sign-in the step started from, so once
//! the MCP's URL or network changed (or changed and changed back), nothing of
//! it may be saved, and nothing may be sent with it to the new destination.
//!
//! The provider's token endpoint is held until the test lets it answer, so
//! the order of events is the test's and no test waits for a delay.

mod mcps_support;

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use chrono::Duration as Age;
use http::StatusCode;
use mcps_support::*;
use mymcps_core::Timestamp;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, Tailnet, Team, User};
use mymcps_net::CannedResponse;
use mymcps_web::testing::factories::{create_admin, create_mcp};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;
use tokio::sync::Notify;
use url::Url;

const OLD_URL: &str = "https://old.example/mcp";
const NEW_URL: &str = "https://new.example/mcp";
/// What the provider issues for the code, and for the refresh token. Both are
/// secrets of the old destination: no request to anywhere may carry them
/// after the MCP moved.
const CODE_ACCESS_TOKEN: &str = "access-issued-for-the-code";
const REFRESHED_ACCESS_TOKEN: &str = "access-issued-for-the-refresh";
const TOKEN: &str = "/token";
const REGISTER: &str = "/register";
const WAIT: Duration = Duration::from_secs(20);

/// Holds the answer of one request of the provider while the gate is armed
/// for its path. The answer is computed on a thread of the runtime, which
/// waits: the tests that use it run on more than one.
#[derive(Default)]
struct Gate {
    armed: Mutex<Option<&'static str>>,
    arrived: Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    /// Hold the next request to `path`.
    fn arm(&self, path: &'static str) {
        *self.released.lock().unwrap() = false;
        *self.armed.lock().unwrap() = Some(path);
    }

    /// Called by the provider on each request: when it is the one the gate
    /// waits for, tells the test it is there, then holds the answer until
    /// [`release`](Self::release).
    fn hold(&self, call: &Call) {
        {
            let mut armed = self.armed.lock().unwrap();
            if *armed != Some(call.path().as_str()) {
                return;
            }
            *armed = None;
        }
        self.arrived.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }

    /// The request of the step is at the provider, which has not answered.
    async fn reached(&self) {
        tokio::time::timeout(WAIT, self.arrived.notified())
            .await
            .expect("the token request reached the provider");
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

/// A provider that serves the two origins of the test, and issues a token of
/// its own for each grant.
fn provider(gate: Arc<Gate>) -> impl Fn(&Call) -> CannedResponse + Send + Sync + 'static {
    move |call| {
        let url = call.url.as_str();
        if url.contains("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": format!("{}/mcp", call.origin()),
                "authorization_servers": ["https://auth.example"],
            }));
        }
        match url {
            "https://auth.example/.well-known/oauth-authorization-server" => {
                json_response(authorization_server("https://auth.example"))
            }
            "https://auth.example/register" => {
                gate.hold(call);
                json_response(registered_client("client-1"))
            }
            "https://auth.example/token" => {
                gate.hold(call);
                let refreshing = call.form("grant_type").as_deref() == Some("refresh_token");
                json_response(json!({
                    "access_token": if refreshing { REFRESHED_ACCESS_TOKEN } else { CODE_ACCESS_TOKEN },
                    "token_type": "bearer",
                    "expires_in": 3600,
                    "refresh_token": if refreshing { "refresh-2" } else { "refresh-1" },
                }))
            }
            _ if url.starts_with(OLD_URL) || url.starts_with(NEW_URL) => {
                mcp_answer(call, &json!([]))
            }
            _ => not_found(),
        }
    }
}

struct Lab {
    app: Arc<TestApp>,
    calls: Calls,
    gate: Arc<Gate>,
    admin: User,
    mcp: Mcp,
}

/// An MCP at `OLD_URL` that wants its authorization.
async fn lab() -> Lab {
    let gate = Arc::new(Gate::default());
    let (app, calls) = app_answering(provider(gate.clone())).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Moving".into();
        mcp.slug = "moving".into();
        mcp.auth_type = McpAuthType::Auto;
        mcp.oauth_required = true;
        mcp.http_url = Some(OLD_URL.into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    Lab {
        app: Arc::new(app),
        calls,
        gate,
        admin,
        mcp,
    }
}

impl Lab {
    /// The authorization of the MCP up to the provider, and what the browser
    /// then sends back: the callback the session of the start answers.
    async fn start(&self) -> (TestResponse, String) {
        let response = self
            .app
            .get(&format!("/mcps/{}/oauth/start", self.mcp.id))
            .login_as(&self.admin)
            .send()
            .await;
        assert_eq!(response.status, StatusCode::FOUND);
        let state = query(&Url::parse(response.location().unwrap()).unwrap(), "state");
        (response, state.unwrap())
    }

    fn callback(
        &self,
        start: &TestResponse,
        state: &str,
    ) -> impl Future<Output = TestResponse> + Send + 'static {
        let app = self.app.clone();
        let session = start.session();
        let path = format!("/mcps/oauth/callback?code=the-code&state={state}");
        async move { app.get(&path).session(session).send().await }
    }

    /// The MCP signed in, with an access token that has expired.
    async fn signed_in_with_an_expired_token(&self) {
        let (start, state) = self.start().await;
        let callback = self.callback(&start, &state).await;
        assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
        let mut saved = find_mcp(&self.app, self.mcp.id).await;
        assert_eq!(
            decrypt(&self.app, &saved.oauth_access_token).as_deref(),
            Some(CODE_ACCESS_TOKEN)
        );
        saved.oauth_token_expires_at = Some(Timestamp::now() - Age::minutes(1));
        saved.save(&*self.app.core.db).await.unwrap();
        self.calls.clear();
    }

    /// A tailnet of the team: an MCP is moved to it by name of its id.
    async fn tailnet(&self) -> Tailnet {
        let mut tailnet = Tailnet {
            team_id: Team::ROOT_ID,
            name: "pro".into(),
            hostname: "mymcps-pro".into(),
            created_by: self.admin.id,
            ..Default::default()
        };
        tailnet.insert(&*self.app.core.db).await.unwrap();
        tailnet
    }

    /// Save the MCP with this URL and network, through the route of the edit
    /// dialog.
    async fn save(&self, http_url: &str, tailnet_id: Option<i64>) {
        let tailnet = tailnet_id.map(|id| id.to_string()).unwrap_or_default();
        let response = self
            .app
            .put(&format!("/mcps/{}", self.mcp.id))
            .login_as(&self.admin)
            .csrf()
            .form(&[
                ("name", "Moving"),
                ("description", ""),
                ("transport", "http"),
                ("httpUrl", http_url),
                ("authType", "auto"),
                ("tailnetId", &tailnet),
                ("enabled", "on"),
            ])
            .send()
            .await;
        assert_redirect(&response, "/mcps");
    }

    /// Change where the MCP goes, as the admin does in the dialog.
    async fn move_it(&self, change: Change) {
        match change {
            Change::Url => self.save(NEW_URL, None).await,
            Change::Network => {
                let tailnet = self.tailnet().await;
                self.save(OLD_URL, Some(tailnet.id)).await;
            }
            Change::NetworkAndBack => {
                let tailnet = self.tailnet().await;
                self.save(OLD_URL, Some(tailnet.id)).await;
                self.save(OLD_URL, None).await;
            }
        }
    }

    /// What the row holds of the sign-in: nothing may have been saved by the
    /// step that was in flight.
    async fn assert_nothing_was_saved(&self) {
        let saved = find_mcp(&self.app, self.mcp.id).await;
        assert_eq!(saved.oauth_access_token, None);
        assert_eq!(saved.oauth_refresh_token, None);
        assert_eq!(saved.oauth_token_expires_at, None);
        assert_eq!(saved.oauth_client_id, None);
        assert_eq!(saved.oauth_issuer, None);
    }

    /// No request, to the old destination or the new, carried what the
    /// provider issued to the step.
    fn assert_no_request_used_the_issued_tokens(&self) {
        for call in self.calls.all() {
            let authorization = call.header("authorization").unwrap_or_default();
            assert!(
                !authorization.contains(CODE_ACCESS_TOKEN)
                    && !authorization.contains(REFRESHED_ACCESS_TOKEN),
                "{} {} carried an issued token",
                call.method,
                call.url
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Url,
    Network,
    /// The network switched and switched back: the destination is the same
    /// again, and the sign-in is not.
    NetworkAndBack,
}

async fn code_exchanged_after(change: Change) {
    let lab = lab().await;
    let (start, state) = lab.start().await;

    lab.gate.arm(TOKEN);
    let exchange = tokio::spawn(lab.callback(&start, &state));
    lab.gate.reached().await;
    lab.move_it(change).await;
    lab.gate.release();
    let callback = exchange.await.unwrap();

    assert_eq!(callback.status, StatusCode::FOUND, "{change:?}");
    assert_eq!(callback.flashed("success"), None, "{change:?}");
    let error = callback.flashed("error").expect("the callback says why");
    assert!(
        error.as_str().unwrap().contains("changed while"),
        "{change:?}: {error}"
    );
    lab.assert_nothing_was_saved().await;
    lab.assert_no_request_used_the_issued_tokens();
}

async fn refresh_answered_after(change: Change) {
    let lab = lab().await;
    lab.signed_in_with_an_expired_token().await;

    // A request of the gateway renews the token before it connects.
    let upstream = lab.app.state.upstream.clone();
    let mut mcp = find_mcp(&lab.app, lab.mcp.id).await;
    lab.gate.arm(TOKEN);
    let connecting = tokio::spawn(async move {
        upstream
            .connect_http_upstream(&mut mcp)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    lab.gate.reached().await;
    lab.move_it(change).await;
    lab.gate.release();
    let outcome = connecting.await.unwrap();

    let error = outcome.expect_err("the connection fails: it has no token to use");
    assert!(error.contains("changed while"), "{change:?}: {error}");
    lab.assert_nothing_was_saved().await;
    lab.assert_no_request_used_the_issued_tokens();
}

/// The configuration a sign-in discovers and registers is the old
/// destination's as well. (A switch and a switch back leaves a row that never
/// had a sign-in as it was: what was registered is right again, so there is
/// no case for it.)
async fn registration_answered_after(change: Change) {
    let lab = lab().await;

    lab.gate.arm(REGISTER);
    let starting = {
        let app = lab.app.clone();
        let admin = lab.admin.clone();
        let path = format!("/mcps/{}/oauth/start", lab.mcp.id);
        tokio::spawn(async move { app.get(&path).login_as(&admin).send().await })
    };
    lab.gate.reached().await;
    lab.move_it(change).await;
    lab.gate.release();
    let start = starting.await.unwrap();

    assert_eq!(start.status, StatusCode::FOUND, "{change:?}");
    // The browser is not sent to the provider: there is no authorization to come back from.
    assert!(
        !start.location().unwrap_or_default().contains("authorize"),
        "{change:?}: {:?}",
        start.location()
    );
    assert!(
        !start
            .session()
            .keys()
            .any(|key| key.starts_with("mcp_oauth:")),
        "{change:?}"
    );
    lab.assert_nothing_was_saved().await;
}

macro_rules! cases {
    ($($name:ident: $case:ident($change:expr);)+) => {$(
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            $case($change).await;
        }
    )+};
}

cases! {
    drops_the_tokens_of_a_code_exchanged_after_the_url_changed: code_exchanged_after(Change::Url);
    drops_the_tokens_of_a_code_exchanged_after_the_network_changed: code_exchanged_after(Change::Network);
    drops_the_tokens_of_a_code_exchanged_after_the_network_changed_and_changed_back: code_exchanged_after(Change::NetworkAndBack);
    drops_the_tokens_of_a_refresh_answered_after_the_url_changed: refresh_answered_after(Change::Url);
    drops_the_tokens_of_a_refresh_answered_after_the_network_changed: refresh_answered_after(Change::Network);
    drops_the_tokens_of_a_refresh_answered_after_the_network_changed_and_changed_back: refresh_answered_after(Change::NetworkAndBack);
    drops_the_registration_of_a_sign_in_started_before_the_url_changed: registration_answered_after(Change::Url);
    drops_the_registration_of_a_sign_in_started_before_the_network_changed: registration_answered_after(Change::Network);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn still_saves_the_tokens_when_only_something_else_of_the_mcp_changed() {
    let lab = lab().await;
    let (start, state) = lab.start().await;

    lab.gate.arm(TOKEN);
    let exchange = tokio::spawn(lab.callback(&start, &state));
    lab.gate.reached().await;
    // Its name, its description and its status are not where it goes.
    let mut renamed = find_mcp(&lab.app, lab.mcp.id).await;
    renamed.name = "Renamed meanwhile".into();
    renamed.description = Some("edited while signing in".into());
    renamed.save(&*lab.app.core.db).await.unwrap();
    lab.gate.release();
    let callback = exchange.await.unwrap();

    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
    let saved = find_mcp(&lab.app, lab.mcp.id).await;
    assert_eq!(
        decrypt(&lab.app, &saved.oauth_access_token).as_deref(),
        Some(CODE_ACCESS_TOKEN)
    );
    assert_eq!(saved.name, "Renamed meanwhile");
}
