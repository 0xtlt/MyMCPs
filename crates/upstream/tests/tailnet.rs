//! HTTP MCPs that reach their server through a tailnet, against the fake
//! `tailscaled` of `mymcps-tailnet`: its LocalAPI answers the status of a
//! tailnet that is up, and bridges every dial to a server on this machine, so
//! the requests of the gateway only reach it through the tailnet.

mod support;

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use mymcps_builtin::BuiltinRegistry;
use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, Tailnet, Team};
use mymcps_net::{CannedResponse, Fetcher};
use mymcps_tailnet::test_support::{FakeLocalApi, fake_tailscaled, running_status, status_in};
use mymcps_tailnet::{TailnetState, Tailnets, TailnetsConfig};
use mymcps_upstream::{
    TailnetStart, Upstream, UpstreamError, clear_oauth_session, read_oauth_session,
};
use serde_json::{Value, json};
use support::*;
use url::Url;

/// The device of the tailnet that serves the MCPs of these tests.
const NAS: &str = "nas.tail1234.ts.net";

/// The tailnets of a test, run by a fake `tailscaled`.
struct Fixture {
    core: TestCore,
    tailnets: Tailnets,
}

impl Fixture {
    async fn new() -> Self {
        let core = TestCore::new().await;
        let binary = fake_tailscaled(&core.dir.path().join("bin"), 0);
        let tailnets = Tailnets::new(TailnetsConfig {
            binary,
            data_dir: core.dir.path().to_owned(),
        });
        Self { core, tailnets }
    }

    /// An upstream that runs these tailnets and sends what they do not carry
    /// over the network.
    fn upstream(&self) -> Arc<Upstream> {
        self.upstream_with(|builder| builder)
    }

    /// [`upstream`](Self::upstream) whose fallback network is `fetcher`: what
    /// reaches it did not go through a tailnet.
    fn upstream_falling_back_to(&self, fetcher: Fetcher) -> Arc<Upstream> {
        self.upstream_with(|builder| builder.fetcher(fetcher))
    }

    fn upstream_with(
        &self,
        adjust: impl FnOnce(mymcps_upstream::UpstreamBuilder) -> mymcps_upstream::UpstreamBuilder,
    ) -> Arc<Upstream> {
        let builder = Upstream::builder(self.core.core.clone(), BuiltinRegistry::default());
        adjust(builder.tailnets(Some(self.tailnets.clone()))).build()
    }

    async fn tailnet_of(&self, team_id: i64, name: &str) -> Tailnet {
        let created_by = create_admin(&self.core).await;
        Tailnet::create_for_tests(&*self.core.db, team_id, created_by, name).await
    }

    async fn tailnet(&self, name: &str) -> Tailnet {
        self.tailnet_of(Team::ROOT_ID, name).await
    }

    /// A team that is not the one of the MCPs.
    async fn other_team(&self) -> Team {
        let mut team = Team {
            name: "Other".to_owned(),
            ..Default::default()
        };
        team.insert(&*self.core.db).await.unwrap();
        team
    }

    /// Runs the daemon of `tailnet`, with a LocalAPI that says `status` and
    /// bridges the dials to `server`.
    async fn join(
        &self,
        tailnet: &Tailnet,
        status: Value,
        server: Option<&LocalServer>,
    ) -> FakeLocalApi {
        self.tailnets.ensure_running(tailnet.id).await.unwrap();
        let api = FakeLocalApi::bind(&self.socket(tailnet), status);
        if let Some(server) = server {
            api.bridge_dials_to(address_of(server));
        }
        api
    }

    fn socket(&self, tailnet: &Tailnet) -> std::path::PathBuf {
        self.core
            .dir
            .path()
            .join("tailnets")
            .join(tailnet.id.to_string())
            .join("ts.sock")
    }

    async fn mcp(&self, tailnet: &Tailnet, http_url: &str) -> Mcp {
        create_mcp(&self.core, |mcp| {
            mcp.http_url = Some(http_url.to_owned());
            mcp.tailnet_id = Some(tailnet.id);
        })
        .await
    }

    /// The row as the database holds it now.
    async fn saved(&self, mcp: &Mcp) -> Mcp {
        find_mcp(&self.core, mcp.id).await
    }
}

/// A tailnet `Office` that is up with a device `nas`, whose server (an MCP
/// server and the provider next to it) answers on this machine.
struct Office {
    fixture: Fixture,
    server: LocalServer,
    tailnet: Tailnet,
    api: FakeLocalApi,
    port: u16,
}

impl Deref for Office {
    type Target = Fixture;

    fn deref(&self) -> &Fixture {
        &self.fixture
    }
}

impl Office {
    async fn new() -> Self {
        Self::on(running_status(), None).await
    }

    /// An office whose daemon says `status`, and whose server names its
    /// OAuth provider by `issuer_host` (its own host when `None`).
    async fn on(status: Value, issuer_host: Option<&'static str>) -> Self {
        let fixture = Fixture::new().await;
        let server = mcp_server(issuer_host).await;
        let tailnet = fixture.tailnet("Office").await;
        let api = fixture.join(&tailnet, status, Some(&server)).await;
        let port = port_of(&server);
        Self {
            fixture,
            server,
            tailnet,
            api,
            port,
        }
    }

    /// An MCP on the tailnet, at `host` and the port of the server.
    async fn mcp_at(&self, host: &str) -> Mcp {
        let url = format!("http://{host}:{}/mcp", self.port);
        self.fixture.mcp(&self.tailnet, &url).await
    }

    async fn mcp(&self) -> Mcp {
        self.mcp_at(NAS).await
    }

    /// An MCP of the device that wants its authorization.
    async fn oauth_mcp(&self) -> Mcp {
        let mut mcp = self.mcp().await;
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
        mcp
    }

    /// What a dial of the daemon is for: `host` on the port of the server.
    fn dial(&self, host: &str) -> (String, String) {
        (host.to_owned(), self.port.to_string())
    }

    fn dials(&self) -> Vec<(String, String)> {
        dials(&self.api)
    }

    /// The requests the server received, as `METHOD /path`.
    fn received(&self) -> Vec<String> {
        self.server
            .requests()
            .iter()
            .map(|request| format!("{} {}", request.method, request.path()))
            .collect()
    }

    /// Everything the server received came over the tailnet, for the host of
    /// the MCP.
    fn assert_only_the_host_of_the_mcp_was_asked(&self, what: &str) {
        for request in self.server.requests() {
            assert_eq!(
                request.header("host"),
                Some(&format!("{NAS}:{}", self.port)[..]),
                "{what}: {}",
                request.path()
            );
        }
    }

    /// Whether an exchange or refresh reached the token endpoint of the server.
    fn token_requests(&self) -> usize {
        self.received()
            .iter()
            .filter(|request| *request == "POST /token")
            .count()
    }
}

fn port_of(server: &LocalServer) -> u16 {
    Url::parse(&server.origin()).unwrap().port().unwrap()
}

fn address_of(server: &LocalServer) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port_of(server)))
}

/// The MCP server, and the provider next to it, as seen by their `Host`: the
/// same documents at every host the dial is bridged for.
async fn mcp_server(issuer_host: Option<&'static str>) -> LocalServer {
    local_server(move |request| async move {
        let host = request.header("host").unwrap_or_default().to_owned();
        let port = host.rsplit(':').next().unwrap_or_default().to_owned();
        let issuer = match issuer_host {
            Some(issuer_host) => format!("http://{issuer_host}:{port}"),
            None => format!("http://{host}"),
        };
        match (request.method.as_str(), request.path()) {
            ("POST", "/mcp") => {
                let message = request.json();
                let answer = |result: Value| {
                    respond_json(
                        200,
                        json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
                    )
                };
                match message["method"].as_str() {
                    Some("initialize") => answer(json!({
                        "protocolVersion": message["params"]["protocolVersion"],
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "on the tailnet", "version": "1.0" },
                    })),
                    Some("tools/list") => answer(json!({ "tools": [
                        { "name": "ping", "inputSchema": { "type": "object" } },
                    ] })),
                    _ => respond(202, &[], ""),
                }
            }
            ("GET", "/.well-known/oauth-protected-resource/mcp") => respond_json(
                200,
                json!({ "resource": format!("http://{host}/mcp"), "authorization_servers": [issuer] }),
            ),
            ("GET", "/.well-known/oauth-authorization-server") => {
                respond_json(200, authorization_server(&format!("http://{host}")))
            }
            ("POST", "/register") => respond_json(200, registered_client("tailnet-client")),
            _ => respond(404, &[], "not found"),
        }
    })
    .await
}

/// The provider `auth.example` of the MCP, which answers on the public
/// network only.
fn public_provider(port: u16) -> impl Fn(&Call) -> CannedResponse + Send + Sync + 'static {
    let issuer = format!("http://auth.example:{port}");
    move |call| match (call.method.as_str(), call.path().as_str()) {
        ("GET", "/.well-known/oauth-authorization-server") => {
            json_response(authorization_server(&issuer))
        }
        ("POST", "/register") => json_response(registered_client("public-client")),
        ("POST", "/token") => json_response(json!({
            "access_token": format!("access-{}", call.form("grant_type").unwrap_or_default()),
            "refresh_token": "refresh-new",
            "token_type": "Bearer",
            "expires_in": 600,
        })),
        _ => not_found(),
    }
}

fn dials(api: &FakeLocalApi) -> Vec<(String, String)> {
    api.requests_to("/localapi/v0/dial")
        .iter()
        .map(|dial| {
            (
                dial.header("dial-host").unwrap_or_default().to_owned(),
                dial.header("dial-port").unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn refused(error: &UpstreamError) -> String {
    assert!(error.is_tailnet_error(), "not a tailnet error: {error}");
    error.to_string()
}

/// The authorization of `mcp` up to the provider.
async fn start_oauth(upstream: &Upstream, mcp: &mut Mcp) -> Result<String, UpstreamError> {
    upstream.start_oauth_flow(&MemorySession::new(), mcp).await
}

/// Lets the status that the daemon's client kept expire.
async fn status_expires() {
    tokio::time::sleep(Duration::from_millis(2300)).await;
}

/// The access token of the row is `expected`, as saved by a step of the flow.
fn assert_access_token(fixture: &Fixture, mcp: &Mcp, expected: &str) {
    assert_eq!(
        decrypt(&fixture.core, &mcp.oauth_access_token).as_deref(),
        Some(expected)
    );
}

fn expire_token(mcp: &mut Mcp) {
    mcp.oauth_token_expires_at = Some(mymcps_core::Timestamp::now() - chrono::Duration::minutes(1));
}

// The MCP transport

#[tokio::test]
async fn lists_the_tools_of_an_mcp_through_the_tailnet_and_its_subnet_routes() {
    let office = Office::new().await;
    let mut mcp = office.mcp().await;

    let tools = office.upstream().probe(&mut mcp).await.unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "ping");
    // The connection went to the tailscale address of the device, and the
    // server was still addressed by the host of the MCP.
    let dialed = office.dials();
    assert!(!dialed.is_empty());
    assert!(
        dialed.iter().all(|dial| *dial == office.dial("100.64.0.2")),
        "{dialed:?}"
    );
    assert!(!office.server.requests().is_empty());
    office.assert_only_the_host_of_the_mcp_was_asked("listing");

    // A device behind a subnet route: 192.168.1.0/24 is routed by the `router`
    // peer of the fake status.
    let mut behind = office.mcp_at("192.168.1.20").await;
    let tools = office.upstream().probe(&mut behind).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(office.dials().last(), Some(&office.dial("192.168.1.20")));
}

#[tokio::test]
async fn pools_the_connections_to_a_tailnet_across_requests() {
    let office = Office::new().await;
    let mut mcp = office.mcp().await;
    let upstream = office.upstream();

    upstream.probe(&mut mcp).await.unwrap();
    let first = office.dials().len();
    for _ in 0..3 {
        upstream.probe(&mut mcp).await.unwrap();
    }

    // Without the shared network each of the four would open its own.
    assert!(
        office.dials().len() < first * 4,
        "{first} connections for the first listing, {} for four",
        office.dials().len()
    );
}

#[tokio::test]
async fn never_sends_the_credentials_elsewhere_when_the_route_disappears_mid_session() {
    let office = Office::new().await;
    // Whatever the tailnet does not carry would reach this: nothing may.
    let (fallback, reached) = mock_fetch(|_| not_found());
    let upstream = office.upstream_falling_back_to(fallback);
    let mut mcp = office.mcp().await;
    mcp.auth_type = McpAuthType::Bearer;
    mcp.auth_bearer = encrypt(&office.core, "s3cret");

    let connected = upstream.connect_http_upstream(&mut mcp).await.unwrap();
    assert_eq!(connected.client.list_tools().await.unwrap().tools.len(), 1);
    // The client also opens a stream with a GET in the background, whose
    // moment is not ours: the POSTs are what the test counts.
    let posts = || {
        let requests = office.server.requests();
        requests
            .iter()
            .filter(|request| request.method == "POST")
            .count()
    };
    let sent = posts();
    assert_eq!(sent, 3, "initialize, initialized, tools/list");
    assert!(
        office
            .server
            .requests()
            .iter()
            .any(|request| request.header("authorization") == Some("Bearer s3cret"))
    );

    // The subnet route (here, the device) is withdrawn while the tailnet stays
    // up, and the daemon's status that was kept expires.
    let mut status = running_status();
    status["Peer"] = json!({});
    office.api.set_status(status);
    status_expires().await;

    let error = connected.client.list_tools().await.unwrap_err();
    assert!(
        error.to_string().contains(NAS),
        "the error says which host the tailnet stopped carrying: {error}"
    );
    // Nothing was sent over the public network, nor over the tailnet.
    assert!(reached.all().is_empty(), "{:?}", reached.all());
    assert_eq!(posts(), sent);
    connected.close().await;
}

// The OAuth flow

#[tokio::test]
async fn discovers_and_registers_with_the_provider_through_the_tailnet() {
    let office = Office::new().await;
    let mut mcp = office.oauth_mcp().await;

    let redirect = start_oauth(&office.upstream(), &mut mcp).await.unwrap();

    let authorization_url = Url::parse(&redirect).unwrap();
    assert_eq!(
        origin(&authorization_url),
        format!("http://{NAS}:{}", office.port)
    );
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("tailnet-client"));
    // Discovery and registration all went over the connection of the tailnet.
    let paths = office.received();
    assert!(
        paths.contains(&"GET /.well-known/oauth-protected-resource/mcp".to_owned()),
        "{paths:?}"
    );
    assert!(paths.contains(&"POST /register".to_owned()), "{paths:?}");
    assert_eq!(
        office.dials(),
        [office.dial("100.64.0.2")],
        "one pooled connection carried every request"
    );
}

#[tokio::test]
async fn sends_the_requests_to_another_origin_of_the_oauth_flow_to_the_public_network() {
    // The MCP names a provider that is not a device of the tailnet, which
    // answers on the public network only.
    let office = Office::on(running_status(), Some("auth.example")).await;
    let (fallback, reached) = mock_fetch(public_provider(office.port));
    let mut mcp = office.oauth_mcp().await;

    let redirect = start_oauth(&office.upstream_falling_back_to(fallback), &mut mcp)
        .await
        .unwrap();

    assert_eq!(
        origin(&Url::parse(&redirect).unwrap()),
        format!("http://auth.example:{}", office.port)
    );
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("public-client"));
    // The MCP's own origin stayed on the tailnet, the provider's did not.
    assert!(!reached.all().is_empty());
    assert!(
        reached
            .all()
            .iter()
            .all(|call| call.hostname() == "auth.example"),
        "{:?}",
        reached.hosts()
    );
    assert!(
        office
            .received()
            .contains(&"GET /.well-known/oauth-protected-resource/mcp".to_owned())
    );
    assert!(!office.dials().is_empty());
}

#[tokio::test]
async fn lets_the_provider_be_another_device_of_the_tailnet() {
    // The MCP names a provider at an address inside the subnet the router
    // routes. For an MCP that is not on a tailnet that is a private address
    // it may not name.
    let office = Office::on(running_status(), Some("192.168.1.10")).await;
    let mut mcp = office.oauth_mcp().await;

    let redirect = start_oauth(&office.upstream(), &mut mcp).await.unwrap();

    assert_eq!(
        origin(&Url::parse(&redirect).unwrap()),
        format!("http://192.168.1.10:{}", office.port)
    );
    assert!(
        office.dials().contains(&office.dial("192.168.1.10")),
        "{:?}",
        office.dials()
    );
}

#[tokio::test]
async fn an_oauth_flow_that_the_tailnet_cannot_carry_fails_as_a_network_error() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    // The daemon does not bridge: it asks the client to dial by itself.
    let api = fixture.join(&tailnet, running_status(), None).await;
    let url = format!("http://{NAS}:{}/mcp", port_of(&server));
    let mut mcp = fixture.mcp(&tailnet, &url).await;
    mcp.status = McpStatus::Draft;
    mcp.oauth_required = true;

    let error = start_oauth(&fixture.upstream(), &mut mcp)
        .await
        .unwrap_err();

    // Discovery found nothing, and said that instead of a refusal.
    assert!(!error.is_restricted_endpoint(), "{error}");
    assert!(error.to_string().contains("OAuth"), "{error}");
    assert!(server.requests().is_empty());
    assert!(!dials(&api).is_empty());
}

/// The status of a tailnet with, next to `nas` and the `router` of
/// 192.168.1.0/24, a `files` device that carries a public IPv4 address behind
/// an advertised /32.
fn status_with_providers() -> Value {
    let mut status = running_status();
    status["Peer"]["nodekey:cccc"] = json!({
        "DNSName": "files.tail1234.ts.net.",
        "TailscaleIPs": ["100.64.0.4"],
        "PrimaryRoutes": ["203.0.113.7/32"],
    });
    status
}

/// The same tailnet once `files` left and the router withdrew its route:
/// `nas`, the server of the MCP, is the only device left.
fn status_without_providers() -> Value {
    let mut status = running_status();
    status["Peer"]["nodekey:bbbb"]["PrimaryRoutes"] = json!([]);
    status
}

/// The provider of an OAuth flow that the tailnet routes (`provider`) loses
/// its route in the middle of the flow, while the server of the MCP keeps its
/// own. Neither the exchange of the code nor the refresh of the token may
/// then go to the network that the tailnet is not: the provider is an address
/// or a name of the tailnet, which no other network may answer for.
async fn provider_route_withdrawn_mid_flow(provider: &'static str) {
    let office = Office::on(status_with_providers(), Some(provider)).await;
    // Whatever the tailnet does not carry would reach this: nothing may.
    let (fallback, reached) = mock_fetch(|_| not_found());
    let upstream = office.upstream_falling_back_to(fallback);
    let mut mcp = office.oauth_mcp().await;
    let session = MemorySession::new();
    let redirect =
        Url::parse(&upstream.start_oauth_flow(&session, &mut mcp).await.unwrap()).unwrap();
    assert_eq!(
        origin(&redirect),
        format!("http://{provider}:{}", office.port)
    );
    let state = query(&redirect, "state").unwrap();
    let oauth = read_oauth_session(&session, Some(&state)).unwrap();
    clear_oauth_session(&session, Some(&state));

    let mut saved = office.saved(&mcp).await;
    saved.oauth_refresh_token = encrypt(&office.core, "refresh-secret");
    expire_token(&mut saved);
    saved.save(&*office.core.db).await.unwrap();

    // Exchanges the code and refreshes the token, as the gateway does.
    let steps = async || {
        let mut exchanging = office.saved(&mcp).await;
        let exchange = upstream
            .exchange_authorization_code(&mut exchanging, &oauth, "code-secret", None)
            .await
            .unwrap_err();
        let mut refreshing = office.saved(&mcp).await;
        upstream
            .refresh_oauth_access_token(&mut refreshing)
            .await
            .unwrap_err();
        exchange
    };

    // While the route is there, both reach the token endpoint of the provider,
    // over the tailnet.
    assert_eq!(office.token_requests(), 0);
    steps().await;
    assert_eq!(office.token_requests(), 2, "{provider}");
    assert!(reached.all().is_empty());

    // The provider's route disappears while the tailnet stays up, and the
    // status that was kept expires.
    office.api.set_status(status_without_providers());
    status_expires().await;

    let error = steps().await;
    assert!(!error.to_string().contains("code-secret"), "{error}");

    // The code, its verifier, the refresh token and the client went nowhere.
    assert!(
        reached.all().is_empty(),
        "{provider} reached the fallback network: {:?}",
        reached.all()
    );
    assert_eq!(office.token_requests(), 2, "{provider}");
    // The tokens are still the ones saved, not replaced by anything.
    let after = office.saved(&mcp).await;
    assert_eq!(
        decrypt(&office.core, &after.oauth_refresh_token).as_deref(),
        Some("refresh-secret")
    );
}

macro_rules! provider_route_withdrawn {
    ($($name:ident: $provider:literal;)+) => {$(
        #[tokio::test]
        async fn $name() {
            provider_route_withdrawn_mid_flow($provider).await;
        }
    )+};
}

provider_route_withdrawn! {
    never_sends_the_oauth_exchange_elsewhere_when_a_subnet_route_of_the_provider_goes: "192.168.1.10";
    never_sends_the_oauth_exchange_elsewhere_when_a_public_address_behind_a_32_route_goes: "203.0.113.7";
    never_sends_the_oauth_exchange_elsewhere_when_the_device_of_the_provider_goes: "files.tail1234.ts.net";
    never_sends_the_oauth_exchange_elsewhere_when_the_short_name_of_the_provider_goes: "files";
}

#[tokio::test]
async fn a_provider_named_by_a_dns_name_still_works_when_the_tailnet_loses_other_routes() {
    let office = Office::on(status_with_providers(), Some("auth.example")).await;
    let (fallback, reached) = mock_fetch(public_provider(office.port));
    let upstream = office.upstream_falling_back_to(fallback);
    let mut mcp = office.oauth_mcp().await;
    let session = MemorySession::new();
    let redirect =
        Url::parse(&upstream.start_oauth_flow(&session, &mut mcp).await.unwrap()).unwrap();
    let state = query(&redirect, "state").unwrap();
    let oauth = read_oauth_session(&session, Some(&state)).unwrap();

    // Routes the provider never used go away.
    office.api.set_status(status_without_providers());
    status_expires().await;

    let mut mcp = office.saved(&mcp).await;
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "the-code", None)
        .await
        .unwrap();
    assert_access_token(&office, &mcp, "access-authorization_code");
    expire_token(&mut mcp);
    mcp.save(&*office.core.db).await.unwrap();
    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();
    assert_access_token(&office, &mcp, "access-refresh_token");
    assert!(
        reached
            .all()
            .iter()
            .all(|call| call.hostname() == "auth.example")
    );
    assert!(reached.all().iter().any(|call| call.path() == "/token"));
}

#[tokio::test]
async fn refuses_an_oauth_endpoint_at_an_ip_address_the_tailnet_does_not_route() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    // The network of the server answers for every one of these, as the
    // metadata service of a cloud or a service on the loopback: whatever
    // falls back to the real network (the default of the gateway) would be
    // sent there.
    let api = fixture.join(&tailnet, running_status(), None).await;
    for provider in [
        "127.0.0.1",
        "10.0.0.5",
        "169.254.169.254",
        "192.168.2.20",
        "100.64.0.9",
        "[::1]",
        "[fd00::1]",
        "[fe80::1]",
        "[::ffff:127.0.0.1]",
        "[::ffff:169.254.169.254]",
        "8.8.8.8",
    ] {
        let server = mcp_server(Some(provider)).await;
        api.bridge_dials_to(address_of(&server));
        let port = port_of(&server);
        let mut mcp = fixture
            .mcp(&tailnet, &format!("http://{NAS}:{port}/mcp"))
            .await;
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            start_oauth(&fixture.upstream(), &mut mcp),
        )
        .await
        .unwrap_or_else(|_| panic!("{provider}: a connection was attempted"));

        assert!(outcome.is_err(), "{provider}");
        // The only requests that reached the server went over the tailnet,
        // for the host of the MCP, and none registered or exchanged anything.
        for request in server.requests() {
            assert_eq!(
                request.header("host"),
                Some(&format!("{NAS}:{port}")[..]),
                "{provider}: {}",
                request.path()
            );
            assert!(
                request.path() != "/register" && request.path() != "/token",
                "{provider}"
            );
        }
    }
}

// What the gateway refuses before it connects

#[tokio::test]
async fn refuses_a_tailnet_mcp_when_tailscale_is_off_on_the_server() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://{NAS}:8080/mcp"))
        .await;
    let upstream = Upstream::builder(fixture.core.core.clone(), BuiltinRegistry::default())
        .tailnets(None)
        .build();

    let message = refused(&upstream.probe(&mut mcp).await.unwrap_err());
    assert!(message.contains("Tailscale is turned off"), "{message}");
    let message = refused(&start_oauth(&upstream, &mut mcp).await.unwrap_err());
    assert!(message.contains("Tailscale is turned off"), "{message}");
}

#[tokio::test]
async fn refuses_a_tailnet_that_was_deleted_or_is_of_another_team() {
    let office = Office::new().await;
    let other_team = office.other_team().await;
    let foreign = office.tailnet_of(other_team.id, "Theirs").await;
    let foreign_api = office
        .join(&foreign, running_status(), Some(&office.server))
        .await;
    let upstream = office.upstream();
    // The row an MCP in flight holds still names the first, which is gone.
    let mut deleted = office.mcp().await;
    office.tailnet.delete(&*office.core.db).await.unwrap();
    // The MCP is in the root team, the tailnet in another one.
    let url = format!("http://{NAS}:{}/mcp", office.port);
    let mut foreigner = office.fixture.mcp(&foreign, &url).await;
    assert_ne!(foreigner.team_id, foreign.team_id);

    for mcp in [&mut deleted, &mut foreigner] {
        let message = refused(&upstream.probe(mcp).await.unwrap_err());
        assert!(message.contains("no longer exists"), "{message}");
        refused(&start_oauth(&upstream, mcp).await.unwrap_err());
    }
    assert!(office.dials().is_empty());
    assert!(dials(&foreign_api).is_empty());
    assert!(office.server.requests().is_empty());
}

#[tokio::test]
async fn refuses_a_tailnet_that_is_not_running() {
    for (state, reason) in [
        ("NeedsLogin", "needs a login"),
        ("NeedsMachineAuth", "approve this server"),
        ("Stopped", "is stopped"),
        ("Starting", "is starting"),
    ] {
        let office = Office::on(status_in(state), None).await;
        let mut mcp = office.mcp().await;

        let message = refused(&office.upstream().probe(&mut mcp).await.unwrap_err());

        assert!(
            message.contains("Tailnet \"Office\" is not running"),
            "{state}: {message}"
        );
        assert!(message.contains(reason), "{state}: {message}");
        assert!(
            message.contains("Sign in again on the Tailnets page"),
            "{state}: {message}"
        );
        assert!(office.dials().is_empty(), "{state}");
        assert!(office.server.requests().is_empty(), "{state}");
    }
}

#[tokio::test]
async fn refuses_a_tailnet_nobody_started() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://{NAS}:8080/mcp"))
        .await;

    let message = refused(&fixture.upstream().probe(&mut mcp).await.unwrap_err());

    assert!(message.contains("is not running"), "{message}");
    assert!(message.contains("Tailnets page"), "{message}");
}

#[tokio::test]
async fn refuses_a_host_that_is_not_a_device_of_the_tailnet() {
    let office = Office::new().await;
    let upstream = office.upstream();

    // Another machine of the Internet, an address outside every route of the
    // tailnet, and the machine of the server itself.
    for host in [
        "example.com",
        "192.168.2.20",
        "127.0.0.1",
        "169.254.169.254",
    ] {
        let mut mcp = office.mcp_at(host).await;

        let message = refused(&upstream.probe(&mut mcp).await.unwrap_err());
        assert!(
            message.contains(&format!(
                "\"{host}\" is not a device or a subnet route of tailnet \"Office\""
            )),
            "{host}: {message}"
        );

        let message = refused(&start_oauth(&upstream, &mut mcp).await.unwrap_err());
        assert!(message.contains("is not a device or a subnet route"));
    }
    assert!(office.dials().is_empty());
    assert!(office.server.requests().is_empty());
}

// An MCP that is not on a tailnet

#[tokio::test]
async fn leaves_an_mcp_without_a_tailnet_on_the_network_it_used() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture.join(&tailnet, running_status(), None).await;
    // The requests are answered here: the tailnet is not asked for them.
    let (fetcher, calls) = mock_fetch(|call| {
        mcp_answer(
            call,
            &json!([{ "name": "ping", "inputSchema": { "type": "object" } }]),
            &json!({}),
        )
    });
    let upstream =
        fixture.upstream_with(|builder| builder.fetcher(fetcher).address_guard(public_names()));
    let mut mcp = create_mcp(&fixture.core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".to_owned());
    })
    .await;
    assert_eq!(mcp.tailnet_id, None);

    let tools = upstream.probe(&mut mcp).await.unwrap();

    assert_eq!(tools.len(), 1);
    assert!(!calls.all().is_empty());
    assert!(api.requests_to("/localapi/v0/status").is_empty());
    assert!(dials(&api).is_empty());
}

// The tailnets of the server

#[tokio::test]
async fn starts_the_tailnet_of_every_team_and_says_where_each_stands() {
    let fixture = Fixture::new().await;
    let other_team = fixture.other_team().await;
    let office = fixture.tailnet("Office").await;
    let theirs = fixture.tailnet_of(other_team.id, "Theirs").await;

    let upstream = fixture.upstream();
    let (starts, _apis) = tokio::join!(upstream.start_tailnets(), async {
        // The daemons answer a moment after they are spawned.
        let mut apis = Vec::new();
        for (tailnet, status) in [
            (&office, running_status()),
            (&theirs, status_in("NeedsLogin")),
        ] {
            let socket = fixture.socket(tailnet);
            while !socket.parent().unwrap().exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            apis.push(FakeLocalApi::bind(&socket, status));
        }
        apis
    });

    let starts: Vec<TailnetStart> = starts.unwrap();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].tailnet.name, "Office");
    assert!(matches!(starts[0].state, Ok(TailnetState::Running { .. })));
    assert_eq!(starts[1].tailnet.name, "Theirs");
    assert!(matches!(
        starts[1].state,
        Ok(TailnetState::NeedsLogin { .. })
    ));
    assert!(fixture.tailnets.handle(office.id).is_some());
    assert!(fixture.tailnets.handle(theirs.id).is_some());

    upstream.stop_tailnets().await;
    assert!(fixture.tailnets.handle(office.id).is_none());
    assert!(fixture.tailnets.handle(theirs.id).is_none());
}

#[tokio::test]
async fn starts_nothing_when_tailscale_is_off() {
    let fixture = Fixture::new().await;
    fixture.tailnet("Office").await;
    let upstream = Upstream::builder(fixture.core.core.clone(), BuiltinRegistry::default())
        .tailnets(None)
        .build();

    assert!(upstream.start_tailnets().await.unwrap().is_empty());
    upstream.stop_tailnets().await;
}
