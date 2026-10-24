//! An OAuth step that is in flight when an admin changes the MCP: the
//! exchange of the code of a callback and the refresh of an access token both
//! ask the provider and then save what it answered. What the provider issued
//! belongs to the destination and the sign-in the step started from, so once
//! the MCP's URL or network changed (or changed and changed back), nothing of
//! it may be saved, and nothing may be sent with it to the new destination.
//!
//! The provider's token endpoint is held until the test lets it answer, so
//! the order of events is the test's and no test waits for a delay.
//!
//! The same goes for the sign-in itself. An authorization carries the
//! generation of the sign-in it began on and is refused once the MCP was moved
//! or signed out since, even when it is back where it started, with the same
//! client. And an edit that moves the MCP writes the sign-in it clears, so
//! tokens that a code exchange saved after the edit read the row do not
//! survive it.

mod mcps_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Duration as Age;
use http::StatusCode;
use mcps_support::*;
use mymcps_core::Timestamp;
use mymcps_core::gate::Gate;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, Tailnet, Team, User};
use mymcps_gateway::mcp_management::{ManagementActor, McpFields, McpRegistryManager};
use mymcps_net::{AddressGuard, CannedResponse, Resolver};
use mymcps_web::mcp_management::WebMcpRegistry;
use mymcps_web::routes::mcps::assign_mcp_from_payload;
use mymcps_web::testing::factories::{create_admin, create_mcp};
use mymcps_web::testing::{TestApp, TestResponse};
use mymcps_web::validators::mcp::{McpPayload, UPDATE_MCP_VALIDATOR};
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
                gate.hold(&call.path());
                json_response(registered_client("client-1"))
            }
            "https://auth.example/token" => {
                gate.hold(&call.path());
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

/// Every name is public, and the lookup of one of them can be held: the
/// registry tool looks up the new address of an MCP after it read the row and
/// before it writes it.
struct HeldName {
    name: &'static str,
    armed: AtomicBool,
    arrived: Notify,
    released: Notify,
}

impl HeldName {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            armed: AtomicBool::new(false),
            arrived: Notify::new(),
            released: Notify::new(),
        })
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn reached(&self) {
        tokio::time::timeout(WAIT, self.arrived.notified())
            .await
            .expect("the name was looked up");
    }

    fn release(&self) {
        self.released.notify_one();
    }
}

#[async_trait::async_trait]
impl Resolver for HeldName {
    async fn lookup(&self, hostname: &str) -> std::io::Result<Vec<std::net::IpAddr>> {
        if hostname == self.name && self.armed.swap(false, Ordering::SeqCst) {
            self.arrived.notify_one();
            self.released.notified().await;
        }
        Ok(vec!["203.0.113.10".parse().unwrap()])
    }
}

/// An MCP at `OLD_URL` that wants its authorization.
async fn lab() -> Lab {
    lab_resolving(public_names()).await
}

async fn lab_resolving(guard: AddressGuard) -> Lab {
    let gate = Arc::new(Gate::default());
    let (fetcher, calls) = mock_fetch(provider(gate.clone()));
    let app =
        TestApp::with_upstream(|upstream| upstream.fetcher(fetcher).address_guard(guard)).await;
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
        self.tailnet_named("pro").await
    }

    async fn tailnet_named(&self, name: &str) -> Tailnet {
        let db = &*self.app.core.db;
        Tailnet::create_for_tests(db, Team::ROOT_ID, self.admin.id, name).await
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

// A sign-in is refused once the MCP it began on is not the MCP anymore.

/// The row as a second sign-in leaves it: the provider already knows the
/// client, so it registers nothing new and keeps the one the first sign-in
/// saved, whatever the MCP was moved to in between.
async fn sign_in_again_like(lab: &Lab, first: &Mcp) {
    let mut row = find_mcp(&lab.app, lab.mcp.id).await;
    row.oauth_client_id = first.oauth_client_id.clone();
    row.oauth_client_secret = first.oauth_client_secret.clone();
    row.oauth_client_auth_method = first.oauth_client_auth_method.clone();
    row.oauth_issuer = first.oauth_issuer.clone();
    row.oauth_resource = first.oauth_resource.clone();
    row.oauth_redirect_uri = first.oauth_redirect_uri.clone();
    row.oauth_authorize_url = first.oauth_authorize_url.clone();
    row.oauth_token_url = first.oauth_token_url.clone();
    row.oauth_required = true;
    row.save(&*lab.app.core.db).await.unwrap();
}

/// Sign-in A starts, the MCP is moved along `route` (`None` is the internet)
/// with the same URL, a sign-in B with the same client is started, and A is
/// finished. B did not clear anything, so nothing but A's own generation
/// tells that the MCP is not where A began, not even when the route ends
/// there.
async fn finishing_a_sign_in_started_before_the_mcp_moved(route: &[Option<&str>]) {
    let lab = lab().await;
    let pro = lab.tailnet_named("pro").await;
    let perso = lab.tailnet_named("perso").await;
    let (start, state) = lab.start().await;
    let first = find_mcp(&lab.app, lab.mcp.id).await;
    for hop in route {
        let tailnet = match hop {
            Some("pro") => Some(pro.id),
            Some("perso") => Some(perso.id),
            _ => None,
        };
        lab.save(OLD_URL, tailnet).await;
    }
    sign_in_again_like(&lab, &first).await;
    lab.calls.clear();

    let callback = lab.callback(&start, &state).await;

    assert_eq!(callback.status, StatusCode::FOUND, "{route:?}");
    assert_eq!(callback.flashed("success"), None, "{route:?}");
    let error = callback.flashed("error").expect("the callback says why");
    assert!(
        error.as_str().unwrap().contains("changed since"),
        "{route:?}: {error}"
    );
    // The code was not even sent to the provider.
    assert!(lab.calls.with_path(TOKEN).is_empty(), "{route:?}");
    let saved = find_mcp(&lab.app, lab.mcp.id).await;
    assert_eq!(saved.oauth_access_token, None, "{route:?}");
    assert_eq!(saved.oauth_refresh_token, None, "{route:?}");
    assert_ne!(saved.oauth_generation, first.oauth_generation, "{route:?}");
}

cases! {
    refuses_a_sign_in_finished_after_the_mcp_moved_to_another_network: finishing_a_sign_in_started_before_the_mcp_moved(&[Some("perso")]);
    refuses_a_sign_in_finished_after_the_mcp_went_through_two_networks: finishing_a_sign_in_started_before_the_mcp_moved(&[Some("pro"), Some("perso")]);
    refuses_a_sign_in_finished_after_the_mcp_moved_and_came_back: finishing_a_sign_in_started_before_the_mcp_moved(&[Some("pro"), Some("perso"), None]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finishes_a_sign_in_when_the_mcp_was_saved_without_moving() {
    let lab = lab().await;
    let (start, state) = lab.start().await;
    // Saved as it is: neither moved nor signed out.
    lab.save(OLD_URL, None).await;
    let first = find_mcp(&lab.app, lab.mcp.id).await;
    assert!(first.oauth_client_id.is_some());

    let callback = lab.callback(&start, &state).await;

    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
}

// An edit that moves the MCP clears its sign-in, and that is written, not
// inferred from what the edit read.

fn assert_signed_out_and_moved(lab: &Lab, before: &Mcp, after: &Mcp) {
    for (column, value) in [
        ("oauth_access_token", &after.oauth_access_token),
        ("oauth_refresh_token", &after.oauth_refresh_token),
        ("oauth_client_id", &after.oauth_client_id),
        ("oauth_client_secret", &after.oauth_client_secret),
        ("oauth_issuer", &after.oauth_issuer),
        ("oauth_token_url", &after.oauth_token_url),
        ("oauth_redirect_uri", &after.oauth_redirect_uri),
    ] {
        assert_eq!(value, &None, "{column} outlived the move");
    }
    assert_eq!(after.oauth_token_expires_at, None);
    assert_eq!(after.oauth_token_type, None);
    assert!(!after.oauth_required);
    assert_eq!(after.http_url.as_deref(), Some(NEW_URL));
    assert_ne!(after.oauth_generation, before.oauth_generation);
    // The old address had them while it was the MCP's; the new one never.
    for call in lab
        .calls
        .all()
        .iter()
        .filter(|call| call.url.starts_with(NEW_URL))
    {
        let authorization = call.header("authorization").unwrap_or_default();
        assert!(!authorization.contains(CODE_ACCESS_TOKEN), "{}", call.url);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_form_that_moves_an_mcp_clears_tokens_saved_after_it_read_the_row() {
    let lab = lab().await;
    let upstream = &lab.app.state.upstream;
    // The edit reads the row: no sign-in yet, every token is empty.
    let read = find_mcp(&lab.app, lab.mcp.id).await;
    let payload: McpPayload = UPDATE_MCP_VALIDATOR
        .validate_as(&json!({
            "name": "Moving",
            "transport": "http",
            "httpUrl": NEW_URL,
            "authType": "auto",
            "enabled": true,
        }))
        .unwrap();
    let mut edited = read.clone();
    assign_mcp_from_payload(upstream, &mut edited, &payload, Some(read.id))
        .await
        .map_err(|_| "refused")
        .unwrap();

    // A code is exchanged, and its tokens saved, before the edit writes.
    let (start, state) = lab.start().await;
    let callback = lab.callback(&start, &state).await;
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
    let signed_in = find_mcp(&lab.app, lab.mcp.id).await;
    assert!(signed_in.oauth_access_token.is_some());

    edited.save(&*lab.app.core.db).await.unwrap();

    assert_signed_out_and_moved(&lab, &read, &find_mcp(&lab.app, lab.mcp.id).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_registry_tool_that_moves_an_mcp_clears_tokens_saved_after_it_read_the_row() {
    let held = HeldName::new("new.example");
    let lab = lab_resolving(AddressGuard::new(held.clone())).await;
    let read = find_mcp(&lab.app, lab.mcp.id).await;
    let registry = WebMcpRegistry::new(lab.app.state.upstream.clone());
    let actor = ManagementActor {
        user_id: lab.admin.id,
        team_id: Team::ROOT_ID,
        access_token_id: 1,
    };

    // The tool has read the row, and looks up the new address before it writes.
    held.arm();
    let updating = tokio::spawn(async move {
        let fields = McpFields {
            url: Some(NEW_URL.into()),
            ..Default::default()
        };
        registry.update(&actor, "moving", fields).await
    });
    held.reached().await;

    let (start, state) = lab.start().await;
    let callback = lab.callback(&start, &state).await;
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));

    held.release();
    updating.await.unwrap().expect("the tool saved the move");

    assert_signed_out_and_moved(&lab, &read, &find_mcp(&lab.app, lab.mcp.id).await);
}
