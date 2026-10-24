//! HTTP MCPs that reach their server through a tailnet, against the fake
//! `tailscaled` of `mymcps-tailnet`: its LocalAPI answers the status of a
//! tailnet that is up, and bridges every dial to a server on this machine, so
//! the requests of the gateway only reach it through the tailnet.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;

use mymcps_builtin::BuiltinRegistry;
use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, Tailnet, Team};
use mymcps_net::Fetcher;
use mymcps_tailnet::test_support::{FakeLocalApi, fake_tailscaled, running_status, status_in};
use mymcps_tailnet::{Tailnets, TailnetsConfig};
use mymcps_upstream::{TailnetStart, Upstream, UpstreamError};
use serde_json::{Value, json};
use support::*;
use url::Url;

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
        Upstream::builder(self.core.core.clone(), BuiltinRegistry::default())
            .tailnets(Some(self.tailnets.clone()))
            .build()
    }

    /// [`upstream`](Self::upstream) whose fallback network is `fetcher`: what
    /// reaches it did not go through a tailnet.
    fn upstream_falling_back_to(&self, fetcher: Fetcher) -> Arc<Upstream> {
        Upstream::builder(self.core.core.clone(), BuiltinRegistry::default())
            .fetcher(fetcher)
            .tailnets(Some(self.tailnets.clone()))
            .build()
    }

    async fn tailnet_of(&self, team_id: i64, name: &str) -> Tailnet {
        let mut tailnet = Tailnet {
            team_id,
            name: name.to_owned(),
            hostname: "mymcps".to_owned(),
            created_by: create_admin(&self.core).await,
            ..Default::default()
        };
        tailnet.insert(&*self.core.db).await.unwrap();
        tailnet
    }

    async fn tailnet(&self, name: &str) -> Tailnet {
        self.tailnet_of(Team::ROOT_ID, name).await
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
        let socket = self
            .core
            .dir
            .path()
            .join("tailnets")
            .join(tailnet.id.to_string())
            .join("ts.sock");
        let api = FakeLocalApi::bind(&socket, status);
        if let Some(server) = server {
            api.bridge_dials_to(address_of(server));
        }
        api
    }

    async fn mcp(&self, tailnet: &Tailnet, http_url: &str) -> Mcp {
        create_mcp(&self.core, |mcp| {
            mcp.http_url = Some(http_url.to_owned());
            mcp.tailnet_id = Some(tailnet.id);
        })
        .await
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

// The MCP transport

#[tokio::test]
async fn lists_the_tools_of_an_mcp_through_the_tailnet() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://nas.tail1234.ts.net:{port}/mcp"))
        .await;

    let tools = fixture.upstream().probe(&mut mcp).await.unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "ping");
    // The connection went to the tailscale address of the device, and the
    // server was still addressed by the host of the MCP.
    let dialed = dials(&api);
    assert!(!dialed.is_empty());
    assert!(
        dialed
            .iter()
            .all(|dial| *dial == ("100.64.0.2".to_owned(), port.to_string())),
        "{dialed:?}"
    );
    let received = server.requests();
    assert!(!received.is_empty());
    assert!(
        received
            .iter()
            .all(|request| request.header("host") == Some(&format!("nas.tail1234.ts.net:{port}")))
    );
}

#[tokio::test]
async fn pools_the_connections_to_a_tailnet_across_requests() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let mut mcp = fixture
        .mcp(
            &tailnet,
            &format!("http://nas.tail1234.ts.net:{}/mcp", port_of(&server)),
        )
        .await;
    let upstream = fixture.upstream();

    upstream.probe(&mut mcp).await.unwrap();
    let first = dials(&api).len();
    for _ in 0..3 {
        upstream.probe(&mut mcp).await.unwrap();
    }

    // Without the shared network each of the four would open its own.
    assert!(
        dials(&api).len() < first * 4,
        "{first} connections for the first listing, {} for four",
        dials(&api).len()
    );
}

#[tokio::test]
async fn reaches_a_device_by_its_subnet_route() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    // 192.168.1.0/24 is routed by the `router` peer of the fake status.
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://192.168.1.20:{port}/mcp"))
        .await;

    let tools = fixture.upstream().probe(&mut mcp).await.unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(
        dials(&api)[0],
        ("192.168.1.20".to_owned(), port.to_string())
    );
}

#[tokio::test]
async fn never_sends_the_credentials_elsewhere_when_the_route_disappears_mid_session() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    // Whatever the tailnet does not carry would reach this: nothing may.
    let (fallback, reached) = mock_fetch(|_| not_found());
    let upstream = fixture.upstream_falling_back_to(fallback);
    let mut mcp = create_mcp(&fixture.core, |mcp| {
        mcp.http_url = Some(format!(
            "http://nas.tail1234.ts.net:{}/mcp",
            port_of(&server)
        ));
        mcp.tailnet_id = Some(tailnet.id);
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = encrypt(&fixture.core, "s3cret");
    })
    .await;

    let connected = upstream.connect_http_upstream(&mut mcp).await.unwrap();
    assert_eq!(connected.client.list_tools().await.unwrap().tools.len(), 1);
    // The client also opens a stream with a GET in the background, whose
    // moment is not ours: the POSTs are what the test counts.
    let posts = |server: &LocalServer| {
        server
            .requests()
            .iter()
            .filter(|request| request.method == "POST")
            .count()
    };
    let sent = posts(&server);
    assert_eq!(sent, 3, "initialize, initialized, tools/list");
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.header("authorization") == Some("Bearer s3cret"))
    );

    // The subnet route (here, the device) is withdrawn while the tailnet stays
    // up, and the daemon's status that was kept expires.
    let mut status = running_status();
    status["Peer"] = json!({});
    api.set_status(status);
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;

    let error = connected.client.list_tools().await.unwrap_err();
    assert!(
        error.to_string().contains("nas.tail1234.ts.net"),
        "the error says which host the tailnet stopped carrying: {error}"
    );
    // Nothing was sent over the public network, nor over the tailnet.
    assert!(reached.all().is_empty(), "{:?}", reached.all());
    assert_eq!(posts(&server), sent);
    connected.close().await;
}

// The OAuth flow

#[tokio::test]
async fn discovers_and_registers_with_the_provider_through_the_tailnet() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://nas.tail1234.ts.net:{port}/mcp"))
        .await;
    mcp.status = McpStatus::Draft;
    mcp.oauth_required = true;

    let session = MemorySession::new();
    let redirect = fixture
        .upstream()
        .start_oauth_flow(&session, &mut mcp)
        .await
        .unwrap();

    let authorization_url = Url::parse(&redirect).unwrap();
    assert_eq!(
        origin(&authorization_url),
        format!("http://nas.tail1234.ts.net:{port}")
    );
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("tailnet-client"));
    // Discovery and registration all went over the connection of the tailnet.
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path()))
        .collect();
    assert!(
        paths.contains(&"GET /.well-known/oauth-protected-resource/mcp".to_owned()),
        "{paths:?}"
    );
    assert!(paths.contains(&"POST /register".to_owned()), "{paths:?}");
    assert_eq!(
        dials(&api),
        [("100.64.0.2".to_owned(), port.to_string())],
        "one pooled connection carried every request"
    );
}

#[tokio::test]
async fn sends_the_requests_to_another_origin_of_the_oauth_flow_to_the_public_network() {
    let fixture = Fixture::new().await;
    // The MCP names a provider that is not a device of the tailnet.
    let server = mcp_server(Some("auth.example")).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    // The provider answers on the public network only.
    let issuer = format!("http://auth.example:{port}");
    let (fallback, reached) = mock_fetch({
        let issuer = issuer.clone();
        move |call| match (call.method.as_str(), call.path().as_str()) {
            ("GET", "/.well-known/oauth-authorization-server") => {
                json_response(authorization_server(&issuer))
            }
            ("POST", "/register") => json_response(registered_client("public-client")),
            _ => not_found(),
        }
    });
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://nas.tail1234.ts.net:{port}/mcp"))
        .await;
    mcp.status = McpStatus::Draft;
    mcp.oauth_required = true;

    let redirect = fixture
        .upstream_falling_back_to(fallback)
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    assert_eq!(origin(&Url::parse(&redirect).unwrap()), issuer);
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
        server
            .requests()
            .iter()
            .any(|request| request.path() == "/.well-known/oauth-protected-resource/mcp")
    );
    assert!(!dials(&api).is_empty());
}

#[tokio::test]
async fn lets_the_provider_be_another_device_of_the_tailnet() {
    let fixture = Fixture::new().await;
    // The MCP names a provider at an address inside the subnet the router
    // routes. For an MCP that is not on a tailnet that is a private address
    // it may not name.
    let server = mcp_server(Some("192.168.1.10")).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    let mut mcp = fixture
        .mcp(&tailnet, &format!("http://nas.tail1234.ts.net:{port}/mcp"))
        .await;
    mcp.status = McpStatus::Draft;
    mcp.oauth_required = true;

    let redirect = fixture
        .upstream()
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    assert_eq!(
        origin(&Url::parse(&redirect).unwrap()),
        format!("http://192.168.1.10:{port}")
    );
    assert!(
        dials(&api).contains(&("192.168.1.10".to_owned(), port.to_string())),
        "{:?}",
        dials(&api)
    );
}

#[tokio::test]
async fn an_oauth_flow_that_the_tailnet_cannot_carry_fails_as_a_network_error() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    // The daemon does not bridge: it asks the client to dial by itself.
    let api = fixture.join(&tailnet, running_status(), None).await;
    let mut mcp = fixture
        .mcp(
            &tailnet,
            &format!("http://nas.tail1234.ts.net:{}/mcp", port_of(&server)),
        )
        .await;
    mcp.status = McpStatus::Draft;
    mcp.oauth_required = true;

    let error = fixture
        .upstream()
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap_err();

    // Discovery found nothing, and said that instead of a refusal.
    assert!(!error.is_restricted_endpoint(), "{error}");
    assert!(error.to_string().contains("OAuth"), "{error}");
    assert!(server.requests().is_empty());
    assert!(!dials(&api).is_empty());
}

// What the gateway refuses before it connects

#[tokio::test]
async fn refuses_a_tailnet_mcp_when_tailscale_is_off_on_the_server() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    let mut mcp = fixture
        .mcp(&tailnet, "http://nas.tail1234.ts.net:8080/mcp")
        .await;
    let upstream = Upstream::builder(fixture.core.core.clone(), BuiltinRegistry::default())
        .tailnets(None)
        .build();

    let message = refused(&upstream.probe(&mut mcp).await.unwrap_err());
    assert!(message.contains("Tailscale is turned off"), "{message}");
    let message = refused(
        &upstream
            .start_oauth_flow(&MemorySession::new(), &mut mcp)
            .await
            .unwrap_err(),
    );
    assert!(message.contains("Tailscale is turned off"), "{message}");
}

#[tokio::test]
async fn refuses_a_tailnet_that_was_deleted() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let mut mcp = fixture
        .mcp(
            &tailnet,
            &format!("http://nas.tail1234.ts.net:{}/mcp", port_of(&server)),
        )
        .await;
    // The row an MCP in flight holds still names it.
    tailnet.delete(&*fixture.core.db).await.unwrap();

    let message = refused(&fixture.upstream().probe(&mut mcp).await.unwrap_err());

    assert!(message.contains("no longer exists"), "{message}");
    assert!(dials(&api).is_empty());
}

#[tokio::test]
async fn refuses_the_tailnet_of_another_team() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let mut other_team = Team {
        name: "Other".to_owned(),
        ..Default::default()
    };
    other_team.insert(&*fixture.core.db).await.unwrap();
    let foreign = fixture.tailnet_of(other_team.id, "Theirs").await;
    let api = fixture
        .join(&foreign, running_status(), Some(&server))
        .await;
    // The MCP is in the root team, the tailnet in another one.
    let mut mcp = fixture
        .mcp(
            &foreign,
            &format!("http://nas.tail1234.ts.net:{}/mcp", port_of(&server)),
        )
        .await;
    assert_ne!(mcp.team_id, foreign.team_id);

    let upstream = fixture.upstream();
    let message = refused(&upstream.probe(&mut mcp).await.unwrap_err());
    assert!(message.contains("no longer exists"), "{message}");
    refused(
        &upstream
            .start_oauth_flow(&MemorySession::new(), &mut mcp)
            .await
            .unwrap_err(),
    );
    assert!(dials(&api).is_empty());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn refuses_a_tailnet_that_is_not_running() {
    for (state, reason) in [
        ("NeedsLogin", "needs a login"),
        ("NeedsMachineAuth", "approve this server"),
        ("Stopped", "is stopped"),
        ("Starting", "is starting"),
    ] {
        let fixture = Fixture::new().await;
        let server = mcp_server(None).await;
        let tailnet = fixture.tailnet("Office").await;
        let api = fixture
            .join(&tailnet, status_in(state), Some(&server))
            .await;
        let mut mcp = fixture
            .mcp(
                &tailnet,
                &format!("http://nas.tail1234.ts.net:{}/mcp", port_of(&server)),
            )
            .await;

        let message = refused(&fixture.upstream().probe(&mut mcp).await.unwrap_err());

        assert!(
            message.contains("Tailnet \"Office\" is not running"),
            "{state}: {message}"
        );
        assert!(message.contains(reason), "{state}: {message}");
        assert!(
            message.contains("Sign in again on the Tailnets page"),
            "{state}: {message}"
        );
        assert!(dials(&api).is_empty(), "{state}");
        assert!(server.requests().is_empty(), "{state}");
    }
}

#[tokio::test]
async fn refuses_a_tailnet_nobody_started() {
    let fixture = Fixture::new().await;
    let tailnet = fixture.tailnet("Office").await;
    let mut mcp = fixture
        .mcp(&tailnet, "http://nas.tail1234.ts.net:8080/mcp")
        .await;

    let message = refused(&fixture.upstream().probe(&mut mcp).await.unwrap_err());

    assert!(message.contains("is not running"), "{message}");
    assert!(message.contains("Tailnets page"), "{message}");
}

#[tokio::test]
async fn refuses_a_host_that_is_not_a_device_of_the_tailnet() {
    let fixture = Fixture::new().await;
    let server = mcp_server(None).await;
    let tailnet = fixture.tailnet("Office").await;
    let api = fixture
        .join(&tailnet, running_status(), Some(&server))
        .await;
    let port = port_of(&server);
    let upstream = fixture.upstream();

    // Another machine of the Internet, an address outside every route of the
    // tailnet, and the machine of the server itself.
    for host in [
        "example.com",
        "192.168.2.20",
        "127.0.0.1",
        "169.254.169.254",
    ] {
        let mut mcp = fixture
            .mcp(&tailnet, &format!("http://{host}:{port}/mcp"))
            .await;

        let message = refused(&upstream.probe(&mut mcp).await.unwrap_err());
        assert!(
            message.contains(&format!(
                "\"{host}\" is not a device or a subnet route of tailnet \"Office\""
            )),
            "{host}: {message}"
        );

        let message = refused(
            &upstream
                .start_oauth_flow(&MemorySession::new(), &mut mcp)
                .await
                .unwrap_err(),
        );
        assert!(message.contains("is not a device or a subnet route"));
    }
    assert!(dials(&api).is_empty());
    assert!(server.requests().is_empty());
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
    let upstream = Upstream::builder(fixture.core.core.clone(), BuiltinRegistry::default())
        .fetcher(fetcher)
        .address_guard(public_names())
        .tailnets(Some(fixture.tailnets.clone()))
        .build();
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
    let mut other_team = Team {
        name: "Other".to_owned(),
        ..Default::default()
    };
    other_team.insert(&*fixture.core.db).await.unwrap();
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
            let socket = fixture
                .core
                .dir
                .path()
                .join("tailnets")
                .join(tailnet.id.to_string())
                .join("ts.sock");
            while !socket.parent().unwrap().exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            apis.push(FakeLocalApi::bind(&socket, status));
        }
        apis
    });

    let starts: Vec<TailnetStart> = starts.unwrap();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].tailnet.name, "Office");
    assert!(matches!(
        starts[0].state,
        Ok(mymcps_tailnet::TailnetState::Running { .. })
    ));
    assert_eq!(starts[1].tailnet.name, "Theirs");
    assert!(matches!(
        starts[1].state,
        Ok(mymcps_tailnet::TailnetState::NeedsLogin { .. })
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
