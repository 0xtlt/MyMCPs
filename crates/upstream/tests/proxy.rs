//! An HTTP MCP that names a proxy sends everything through it, and nothing
//! without it: its MCP traffic and every request of its OAuth flow. The
//! proxy is a SOCKS5 server of the test, the MCP and its provider one local
//! server on a name that only the proxy can find.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::response::Response;
use chrono::Duration;
use mymcps_builtin::BuiltinRegistry;
use mymcps_core::models::{McpStatus, Proxy, ProxyKind, Team};
use mymcps_core::{TestCore, Timestamp};
use mymcps_upstream::{ProxyDirectory, Upstream, UpstreamError};
use serde_json::{Value, json};
use support::socks::{Socks5, socks5};
use support::*;
use url::Url;

/// What the Tailscale runtime is to the server: the URL of a proxy that is
/// connected, and the sentence that says why for one that is not.
#[derive(Default)]
struct Directory {
    answers: Mutex<HashMap<i64, Result<Url, String>>>,
    asked: AtomicUsize,
}

impl Directory {
    fn set(&self, proxy_id: i64, answer: Result<Url, String>) {
        self.answers.lock().unwrap().insert(proxy_id, answer);
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl ProxyDirectory for Directory {
    fn proxy_url(&self, proxy_id: i64) -> Result<Url, String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answers
            .lock()
            .unwrap()
            .get(&proxy_id)
            .cloned()
            .unwrap_or_else(|| Err("it is not known".to_owned()))
    }
}

const USER: &str = "7";
const SECRET: &str = "per-node-secret";
/// A name nothing resolves: only the proxy finds the machine.
const MACHINE: &str = "box.tailnet.invalid";

/// An MCP with its OAuth provider on one host, as T3 Code has them: dynamic
/// registration, PKCE, and a bearer token the MCP asks for.
async fn machine() -> LocalServer {
    local_server(|received| async move {
        let host = received.headers["host"].to_str().unwrap().to_owned();
        let origin = format!("http://{host}");
        let path = received.target.split('?').next().unwrap().to_owned();
        match path.as_str() {
            "/.well-known/oauth-protected-resource/mcp" => respond_json(
                200,
                json!({ "resource": format!("{origin}/mcp"), "authorization_servers": [origin] }),
            ),
            "/.well-known/oauth-authorization-server" => {
                respond_json(200, authorization_server(&origin))
            }
            "/register" => respond_json(200, registered_client("tailnet-client")),
            "/token" => {
                let refreshing = url::form_urlencoded::parse(received.body.as_bytes())
                    .any(|(key, value)| key == "grant_type" && value == "refresh_token");
                respond_json(
                    200,
                    json!({
                        "access_token": if refreshing { "tok-renewed" } else { "tok-first" },
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "refresh_token": "refresh-1",
                    }),
                )
            }
            "/mcp" => mcp_endpoint(&received),
            _ => respond(404, &[], "not found"),
        }
    })
    .await
}

fn mcp_endpoint(received: &Received) -> Response {
    let authorized = received
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("Bearer tok-"));
    if !authorized {
        return respond(401, &[("WWW-Authenticate", "Bearer")], "sign in");
    }
    let message: Value = serde_json::from_str(&received.body).unwrap_or(Value::Null);
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
            "serverInfo": { "name": "tailnet", "version": "1.0" },
        })),
        Some("tools/list") => answer(json!({ "tools": [{
            "name": "thread_list",
            "inputSchema": { "type": "object" },
        }] })),
        _ => respond(202, &[], ""),
    }
}

struct Setup {
    core: TestCore,
    upstream: Arc<Upstream>,
    directory: Arc<Directory>,
    machine: LocalServer,
    proxy: Socks5,
    proxy_row: Proxy,
}

impl Setup {
    /// The machine, a connected proxy to it, and an upstream that knows it.
    async fn new() -> Self {
        let core = TestCore::new().await;
        let machine = machine().await;
        let proxy = socks5(USER, SECRET, Some(machine_address(&machine))).await;
        let proxy_row = proxy_in(&core, Team::ROOT_ID, "Home").await;
        let directory = Arc::new(Directory::default());
        directory.set(proxy_row.id, Ok(proxy.url()));
        let (addresses, _) = resolve_names(&[
            (MACHINE, &["203.0.113.10"]),
            ("auth.internal.example", &["192.168.10.4"]),
        ]);
        let upstream = Upstream::builder(core.core.clone(), BuiltinRegistry::default())
            .address_guard(addresses)
            .proxies(directory.clone())
            .build();
        Self {
            core,
            upstream,
            directory,
            machine,
            proxy,
            proxy_row,
        }
    }

    /// The URL of the MCP on the machine, by the name only the proxy knows.
    fn mcp_url(&self) -> String {
        self.url_on(MACHINE)
    }

    fn url_on(&self, host: &str) -> String {
        let port = self.machine.origin().rsplit(':').next().unwrap().to_owned();
        format!("http://{host}:{port}/mcp")
    }

    async fn mcp(&self, proxy_id: Option<i64>, http_url: &str) -> mymcps_core::models::Mcp {
        let http_url = http_url.to_owned();
        create_mcp(&self.core, move |mcp| {
            mcp.http_url = Some(http_url);
            mcp.proxy_id = proxy_id;
            mcp.status = McpStatus::Draft;
            mcp.oauth_required = true;
        })
        .await
    }

    /// The paths the machine received, in order.
    fn paths(&self) -> Vec<String> {
        self.machine
            .requests()
            .iter()
            .map(|request| request.target.split('?').next().unwrap().to_owned())
            .collect()
    }
}

fn machine_address(machine: &LocalServer) -> std::net::SocketAddr {
    machine
        .origin()
        .trim_start_matches("http://")
        .parse()
        .unwrap()
}

async fn proxy_in(core: &TestCore, team_id: i64, name: &str) -> Proxy {
    let mut proxy = Proxy {
        team_id,
        name: name.to_owned(),
        kind: ProxyKind::Tailscale,
        tailscale_hostname: "mymcps".to_owned(),
        ..Default::default()
    };
    proxy.insert(&*core.db).await.unwrap();
    proxy
}

/// Connect an MCP with OAuth the way the pages do.
async fn authorize(setup: &Setup, mcp: &mut mymcps_core::models::Mcp) {
    let session = MemorySession::new();
    let url = Url::parse(
        &setup
            .upstream
            .start_oauth_flow(&session, mcp)
            .await
            .unwrap(),
    )
    .unwrap();
    let state = query(&url, "state").unwrap();
    let pending = setup
        .upstream
        .read_oauth_session(&session, Some(&state))
        .unwrap();
    setup
        .upstream
        .exchange_authorization_code(mcp, &pending, "authorization-code", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn sends_the_mcp_and_its_whole_oauth_flow_through_the_proxy() {
    let setup = Setup::new().await;
    let mut mcp = setup.mcp(Some(setup.proxy_row.id), &setup.mcp_url()).await;

    // Discovery, registration and the exchange of the code.
    authorize(&setup, &mut mcp).await;
    assert_eq!(
        decrypt(
            &setup.core,
            &find_mcp(&setup.core, mcp.id).await.oauth_access_token
        )
        .as_deref(),
        Some("tok-first")
    );
    // The handshake and the listing.
    setup
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .unwrap();
    assert_eq!(mcp.status, McpStatus::Ready);
    assert_eq!(mcp.last_error, None);
    // A tool call.
    let tools = setup.upstream.probe(&mut mcp).await.unwrap();
    assert_eq!(tools[0].name, "thread_list");
    // And the renewal of the token.
    mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    mcp.save(&*setup.core.db).await.unwrap();
    setup
        .upstream
        .refresh_oauth_access_token(&mut mcp)
        .await
        .unwrap();
    assert_eq!(
        decrypt(
            &setup.core,
            &find_mcp(&setup.core, mcp.id).await.oauth_access_token
        )
        .as_deref(),
        Some("tok-renewed")
    );

    let paths = setup.paths();
    for expected in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
        "/register",
        "/token",
        "/mcp",
    ] {
        assert!(
            paths.iter().any(|path| path == expected),
            "{expected}: {paths:?}"
        );
    }
    assert_eq!(paths.iter().filter(|path| *path == "/token").count(), 2);

    // Every connection was asked of the proxy, by the name of the machine,
    // which the proxy resolves: nothing else leaves this process.
    let connects = setup.proxy.connects();
    assert!(!connects.is_empty());
    let host = format!(
        "{MACHINE}:{}",
        setup.machine.origin().rsplit(':').next().unwrap()
    );
    assert!(
        connects.iter().all(|connect| *connect == host),
        "{connects:?}"
    );
    assert!(setup.directory.asked() > 0);
}

#[tokio::test]
async fn leaves_an_mcp_without_a_proxy_alone() {
    let setup = Setup::new().await;
    // The machine can be reached directly by its address.
    let direct_url = format!("{}/mcp", setup.machine.origin());
    let mut mcp = setup.mcp(None, &direct_url).await;

    authorize(&setup, &mut mcp).await;
    setup
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .unwrap();

    assert_eq!(mcp.status, McpStatus::Ready, "{:?}", mcp.last_error);
    assert!(!setup.paths().is_empty());
    assert!(setup.proxy.connects().is_empty());
    assert_eq!(setup.directory.asked(), 0);
}

#[tokio::test]
async fn fails_with_the_reason_and_sends_nothing_when_the_proxy_is_not_connected() {
    let setup = Setup::new().await;
    setup.directory.set(
        setup.proxy_row.id,
        Err("it needs a Tailscale login".to_owned()),
    );
    // Reachable directly, by its address: a fallback would show.
    let mut mcp = setup
        .mcp(
            Some(setup.proxy_row.id),
            &format!("{}/mcp", setup.machine.origin()),
        )
        .await;
    let sentence = "Proxy \"Home\" is not connected: it needs a Tailscale login";

    let error = setup.upstream.probe(&mut mcp).await.unwrap_err();
    assert!(matches!(&error, UpstreamError::Proxy(message) if message == sentence));
    assert_eq!(error.to_string(), sentence);
    assert!(!error.is_unauthorized());
    let error = setup
        .upstream
        .call_tool(&mut mcp, "thread_list", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), sentence);
    let error = setup
        .upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), sentence);

    // A saved sign-in that has to be renewed is no reason to go direct either.
    mcp.oauth_issuer = Some(setup.machine.origin());
    mcp.oauth_client_id = Some("tailnet-client".into());
    mcp.oauth_client_auth_method = Some("none".into());
    mcp.oauth_access_token = encrypt(&setup.core, "tok-old");
    mcp.oauth_refresh_token = encrypt(&setup.core, "refresh-old");
    mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    mcp.save(&*setup.core.db).await.unwrap();
    let error = setup
        .upstream
        .refresh_oauth_access_token(&mut mcp)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), sentence);
    let row = find_mcp(&setup.core, mcp.id).await;
    assert_eq!(
        decrypt(&setup.core, &row.oauth_refresh_token).as_deref(),
        Some("refresh-old")
    );

    // The test of the MCP says so, and does not ask for an authorization.
    setup
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .unwrap();
    let saved = find_mcp(&setup.core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert_eq!(saved.last_error.as_deref(), Some(sentence));
    assert!(!saved.oauth_required);

    assert!(setup.machine.requests().is_empty(), "nothing was sent");
    assert!(setup.proxy.connects().is_empty());
}

#[tokio::test]
async fn fails_when_the_server_runs_no_proxies() {
    let core = TestCore::new().await;
    let proxy_row = proxy_in(&core, Team::ROOT_ID, "Home").await;
    let upstream = Upstream::new(core.core.clone(), BuiltinRegistry::default());
    let mut mcp = create_mcp(&core, |mcp| mcp.proxy_id = Some(proxy_row.id)).await;

    let error = upstream.probe(&mut mcp).await.unwrap_err();

    assert!(error.is_proxy_error(), "{error}");
    assert!(
        error
            .to_string()
            .starts_with("Proxy \"Home\" is not connected: ")
    );
}

#[tokio::test]
async fn refuses_the_proxy_of_another_team() {
    let setup = Setup::new().await;
    let mut other = Team {
        name: "Other".into(),
        ..Default::default()
    };
    other.insert(&*setup.core.db).await.unwrap();
    let theirs = proxy_in(&setup.core, other.id, "Theirs").await;
    setup.directory.set(theirs.id, Ok(setup.proxy.url()));
    let mut mcp = setup.mcp(Some(theirs.id), &setup.mcp_url()).await;

    let error = setup.upstream.probe(&mut mcp).await.unwrap_err();

    assert!(error.is_proxy_error(), "{error}");
    assert_eq!(
        error.to_string(),
        "Proxy \"Theirs\" is not one of the proxies of this MCP's team"
    );
    assert!(setup.proxy.connects().is_empty());
}

#[tokio::test]
async fn follows_the_proxy_when_its_node_connects_again() {
    let setup = Setup::new().await;
    let mut mcp = setup.mcp(Some(setup.proxy_row.id), &setup.mcp_url()).await;
    authorize(&setup, &mut mcp).await;
    setup.upstream.probe(&mut mcp).await.unwrap();
    // The stream the transport opens on its own may still be connecting.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let before = setup.proxy.connects().len();
    assert!(before > 0);

    // The node restarted: another front, another secret.
    let again = socks5("7", "another-secret", Some(machine_address(&setup.machine))).await;
    setup.directory.set(setup.proxy_row.id, Ok(again.url()));
    setup.upstream.probe(&mut mcp).await.unwrap();

    assert_eq!(setup.proxy.connects().len(), before);
    assert!(!again.connects().is_empty());
}

#[tokio::test]
async fn treats_an_mcp_behind_a_proxy_like_one_on_a_private_address() {
    // The provider is named by a host that resolves to a private address
    // here, and is on the tailnet: the proxy resolves it, not this server.
    let setup = Setup::new().await;
    let machine = local_server(|received| async move {
        let host = received.headers["host"].to_str().unwrap().to_owned();
        let provider = format!("http://auth.internal.example:{}", host.rsplit(':').next().unwrap());
        match received.target.as_str() {
            "/.well-known/oauth-protected-resource/mcp" => respond_json(
                200,
                json!({ "resource": format!("http://{host}/mcp"), "authorization_servers": [provider] }),
            ),
            "/.well-known/oauth-authorization-server" => {
                respond_json(200, authorization_server(&provider))
            }
            "/register" => respond_json(200, registered_client("tailnet-client")),
            _ => respond(404, &[], "not found"),
        }
    })
    .await;
    let proxy = socks5(USER, SECRET, Some(machine_address(&machine))).await;
    setup.directory.set(setup.proxy_row.id, Ok(proxy.url()));
    let port = machine.origin().rsplit(':').next().unwrap().to_owned();
    let mut mcp = setup
        .mcp(
            Some(setup.proxy_row.id),
            &format!("http://{MACHINE}:{port}/mcp"),
        )
        .await;

    let url = setup
        .upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    assert!(
        url.starts_with(&format!("http://auth.internal.example:{port}/authorize")),
        "{url}"
    );
    let connects = proxy.connects();
    assert!(
        connects.contains(&format!("auth.internal.example:{port}")),
        "{connects:?}"
    );
    assert!(
        connects.contains(&format!("{MACHINE}:{port}")),
        "{connects:?}"
    );

    // The same documents, for an MCP that is not behind a proxy, name an
    // endpoint inside the network the MCP is not part of.
    let (addresses, _) = resolve_names(&[
        (MACHINE, &["203.0.113.10"]),
        ("auth.internal.example", &["192.168.10.4"]),
    ]);
    let (upstream, calls) = upstream_with(&setup.core, addresses, Default::default(), |call| {
        if call.path() == "/.well-known/oauth-protected-resource/mcp" {
            json_response(json!({
                "resource": format!("http://{MACHINE}/mcp"),
                "authorization_servers": ["http://auth.internal.example"],
            }))
        } else {
            not_found()
        }
    });
    let mut direct = setup.mcp(None, &format!("http://{MACHINE}/mcp")).await;
    let error = upstream
        .start_oauth_flow(&MemorySession::new(), &mut direct)
        .await
        .unwrap_err();
    assert!(error.is_restricted_endpoint(), "{error}");
    assert!(!calls.hosts().contains(&"auth.internal.example".to_owned()));
}
