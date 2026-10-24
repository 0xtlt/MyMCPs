//! The tools that manage the MCP registry, from the request of an agent to
//! `/mcp` to the row in the database: who sees them and who may call them,
//! and that they do what the MCPs page does and never return a secret.

#[path = "support/gateway.rs"]
mod support;

use mymcps_core::models::{
    McpAuthType, McpStatus, McpTransport, OauthClient, Tailnet, Team, TeamRole, User,
};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::mcp_management::{ManagementActor, ManagementError, McpRegistryManager};
use mymcps_net::{AddressGuard, CannedResponse, StaticResolver};
use mymcps_node::{NodeRunner, NodeRuntime};
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use mymcps_web::mcp_management::WebMcpRegistry;
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{
    create_admin, create_mcp, create_member, create_team, next_value,
};
use serde_json::{Value, json};
use std::sync::Arc;

use support::*;

const RESOURCE: &str = "http://localhost:3333/mcp";
const MANAGE: &str = "mcp:tools mcps:manage";
const REGISTRY_TOOLS: [&str; 6] = [
    "mcp_registry_list",
    "mcp_registry_create",
    "mcp_registry_update",
    "mcp_registry_delete",
    "mcp_registry_test",
    "mcp_registry_auth_link",
];

/// The MCPs of the tests: `broken.example` fails, any other host lists one tool.
fn upstreams(message: &UpstreamMessage) -> Reply {
    if message.host.starts_with("broken.") {
        return Reply::Error("Upstream exploded with sk-live-secret".into());
    }
    listing(&[("ping", Some("Ping"))])(message)
}

/// The app with the MCPs of `upstreams`, and a Node.js that never starts:
/// saving an npm MCP probes it, and these tests are about the registry.
async fn gateway() -> TestGateway {
    gateway_with(|_| {}).await
}

async fn gateway_with(adjust: impl FnOnce(&mut mymcps_core::Config)) -> TestGateway {
    gateway_guarded(adjust, public_names()).await
}

/// The app where `guard` tells which names are public.
async fn gateway_guarded(
    adjust: impl FnOnce(&mut mymcps_core::Config),
    guard: AddressGuard,
) -> TestGateway {
    let (fetcher, upstreams) = upstream_fetcher(upstreams);
    // `oauth.example` asks for an authorization.
    let fetcher = fetcher.answering(|request| {
        request
            .url
            .host_str()
            .is_some_and(|host| host.starts_with("oauth."))
            .then(|| CannedResponse::new(http::StatusCode::UNAUTHORIZED))
    });
    let app = TestApp::with_state(adjust, |core| {
        let runtime = NodeRuntime::new(&core.config).binary(|| {
            Err(mymcps_node::NodeError::Other(
                "Node.js is not started in this test".into(),
            ))
        });
        let upstream = Upstream::builder(core.clone(), mymcps_web::state::builtins())
            .fetcher(fetcher)
            .address_guard(guard)
            .node(NodeRunner::with_runtime(core.clone(), runtime))
            .build();
        AppState::with_upstream(core, upstream)
    })
    .await;
    TestGateway { app, upstreams }
}

/// An OAuth access token for the Root Team, granted `scopes` by `user`.
async fn grant(gateway: &TestGateway, user: &User, scopes: &str) -> String {
    grant_in(gateway, user, scopes, Team::ROOT_ID).await
}

async fn grant_in(gateway: &TestGateway, user: &User, scopes: &str, team_id: i64) -> String {
    let mut client = OauthClient {
        client_id: next_value("mcp_client"),
        client_name: "Claude".into(),
        redirect_uris: json!(["http://127.0.0.1/callback"]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: json!(["authorization_code", "refresh_token"]).to_string(),
        response_types: json!(["code"]).to_string(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&**gateway.db()).await.unwrap();
    access_token::create_oauth_grant(
        &**gateway.db(),
        NewOauthGrant {
            name: "Claude",
            client_id: client.id,
            client_supports_refresh: true,
            scopes,
            resource: RESOURCE,
            created_by: user.id,
            team_id,
        },
    )
    .await
    .unwrap()
    .plaintext
}

async fn list_tools(gateway: &TestGateway, plaintext: &str, mode: &str) -> Vec<String> {
    let result = gateway.rpc(plaintext, "tools/list", json!({}), mode).await;
    result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn registry_tools(names: &[String]) -> Vec<&str> {
    names
        .iter()
        .map(String::as_str)
        .filter(|name| REGISTRY_TOOLS.contains(name))
        .collect()
}

/// A tool call whose result is expected to be an answer.
async fn call_ok(gateway: &TestGateway, plaintext: &str, name: &str, arguments: Value) -> Value {
    let result = gateway.call(plaintext, name, arguments).await;
    assert_ne!(
        result["isError"],
        json!(true),
        "{name} failed: {}",
        result_text(&result)
    );
    result["structuredContent"].clone()
}

/// A tool call whose result is expected to be an error: its text.
async fn call_refused(
    gateway: &TestGateway,
    plaintext: &str,
    name: &str,
    arguments: Value,
) -> String {
    let result = gateway.call(plaintext, name, arguments).await;
    assert_eq!(result["isError"], json!(true), "{result}");
    assert!(result.get("structuredContent").is_none());
    result_text(&result).to_owned()
}

async fn mcp_in(
    gateway: &TestGateway,
    team_id: i64,
    slug: &str,
) -> Option<mymcps_core::models::Mcp> {
    sqlx::query_as("select * from `mcps` where `team_id` = ? and `slug` = ?")
        .bind(team_id)
        .bind(slug)
        .fetch_optional(&**gateway.db())
        .await
        .unwrap()
}

async fn mcp_named(gateway: &TestGateway, slug: &str) -> mymcps_core::models::Mcp {
    mcp_in(gateway, Team::ROOT_ID, slug)
        .await
        .unwrap_or_else(|| panic!("no MCP {slug}"))
}

fn decrypted(gateway: &TestGateway, column: &Option<String>) -> Option<String> {
    gateway.core().decrypt_secret(column.as_deref())
}

// --- who has the tools ---

#[tokio::test]
async fn lists_the_tools_only_for_a_token_that_may_manage_mcps() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;

    let plain = grant(&gateway, &admin, "mcp:tools").await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let manual = create_access_token(&gateway, admin.id, mymcps_core::models::ScopeMode::All, &[])
        .await
        .plaintext;

    for mode in ["eager", "lazy"] {
        assert!(registry_tools(&list_tools(&gateway, &plain, mode).await).is_empty());
        assert!(registry_tools(&list_tools(&gateway, &manual, mode).await).is_empty());
        let names = list_tools(&gateway, &manager, mode).await;
        assert_eq!(registry_tools(&names), REGISTRY_TOOLS, "{mode}");
        if mode == "lazy" {
            assert_eq!(names[..3], ["list_mcps", "tool_search", "call_tool"]);
        }
    }

    let tools = gateway
        .rpc(&manager, "tools/list", json!({}), "eager")
        .await["tools"]
        .clone();
    let annotations = |name: &str| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()["annotations"]
            .clone()
    };
    assert_eq!(annotations("mcp_registry_list")["readOnlyHint"], true);
    assert_eq!(annotations("mcp_registry_auth_link")["readOnlyHint"], true);
    assert_eq!(annotations("mcp_registry_delete")["destructiveHint"], true);
    assert_eq!(annotations("mcp_registry_create")["readOnlyHint"], false);
}

#[tokio::test]
async fn lets_in_a_grant_of_both_scopes_and_no_other() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let both = grant(&gateway, &admin, "mcps:manage mcp:tools").await;
    assert_eq!(
        registry_tools(&list_tools(&gateway, &both, "eager").await),
        REGISTRY_TOOLS
    );

    // The permission to manage MCPs does not replace the one to use them.
    for scopes in ["mcps:manage", "mcp:tools mcps:manage other:scope"] {
        let token = grant(&gateway, &admin, scopes).await;
        let response = gateway
            .post_message(
                &token,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
                &[],
            )
            .await;
        assert_eq!(response.status, http::StatusCode::UNAUTHORIZED, "{scopes}");
    }
}

#[tokio::test]
async fn takes_the_tools_away_from_a_person_who_is_no_longer_an_administrator() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    assert_eq!(
        registry_tools(&list_tools(&gateway, &manager, "eager").await),
        REGISTRY_TOOLS
    );

    sqlx::query("update `team_user` set `role` = ? where `user_id` = ?")
        .bind(TeamRole::Member)
        .bind(admin.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    for mode in ["eager", "lazy"] {
        assert!(registry_tools(&list_tools(&gateway, &manager, mode).await).is_empty());
    }
    let refusal = call_refused(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert!(
        refusal.contains("not an administrator or an owner"),
        "{refusal}"
    );

    // Promoted again, the same token has them back.
    sqlx::query("update `team_user` set `role` = ? where `user_id` = ?")
        .bind(TeamRole::Owner)
        .bind(admin.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    assert_eq!(
        registry_tools(&list_tools(&gateway, &manager, "lazy").await),
        REGISTRY_TOOLS
    );
}

#[tokio::test]
async fn refuses_a_call_the_token_is_not_allowed_without_touching_the_registry() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let member = create_member(&gateway).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;

    let plain = grant(&gateway, &admin, "mcp:tools").await;
    let manual = create_access_token(&gateway, admin.id, mymcps_core::models::ScopeMode::All, &[])
        .await
        .plaintext;
    let demoted = grant(&gateway, &member, MANAGE).await;

    for (plaintext, expected) in [
        (&plain, "not allowed to manage MCPs"),
        (&manual, "not allowed to manage MCPs"),
        (&demoted, "not an administrator or an owner"),
    ] {
        for (name, arguments) in [
            ("mcp_registry_list", json!({})),
            (
                "mcp_registry_create",
                json!({ "name": "Evil", "transport": "http", "url": "https://evil.example/mcp" }),
            ),
            ("mcp_registry_delete", json!({ "slug": "issues" })),
            (
                "mcp_registry_update",
                json!({ "slug": "issues", "enabled": false }),
            ),
            ("mcp_registry_test", json!({ "slug": "issues" })),
            ("mcp_registry_auth_link", json!({ "slug": "issues" })),
        ] {
            let refusal = call_refused(&gateway, plaintext, name, arguments).await;
            assert!(refusal.contains(expected), "{name}: {refusal}");
        }
        // A refusal in lazy mode is the same sentence, and still no crash.
        let lazy = gateway
            .rpc(
                plaintext,
                "tools/call",
                json!({ "name": "mcp_registry_list", "arguments": {} }),
                "lazy",
            )
            .await;
        assert_eq!(lazy["isError"], true);
        assert!(result_text(&lazy).contains(expected));
    }

    // Nothing changed, and nothing reached an MCP.
    assert!(mcp_in(&gateway, Team::ROOT_ID, "issues").await.is_some());
    assert!(mcp_in(&gateway, Team::ROOT_ID, "evil").await.is_none());
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn keeps_the_arguments_of_these_calls_out_of_the_call_log() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({
            "name": "Docs", "transport": "http", "url": "https://docs.example/mcp",
            "auth_type": "bearer", "bearer_token": "log-me-not",
        }),
    )
    .await;
    gateway.flush().await;
    assert!(gateway.call_logs().await.is_empty());
}

// --- list ---

#[tokio::test]
async fn lists_every_mcp_of_the_team_without_a_secret() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let core = gateway.core().clone();

    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.slug = "docs".into();
        mcp.http_url =
            Some("https://user:pw@docs.example/s/path-secret-9f3a/mcp?api_key=sk-12345".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = core.encrypt_secret(Some("bearer-secret-value"));
        mcp.enabled = false;
    })
    .await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Package".into();
        mcp.slug = "package".into();
        mcp.transport = McpTransport::Npm;
        mcp.http_url = None;
        mcp.npm_package = Some("@example/mcp".into());
        mcp.npm_env = merge_env(&core, &[("API_KEY", "env-secret-value")]);
        mcp.set_npm_args_list(&["--token=arg-secret-value".to_owned()]);
    })
    .await;
    let other_team = create_team(&gateway, "Other").await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Foreign".into();
        mcp.slug = "foreign".into();
        mcp.team_id = other_team.id;
    })
    .await;

    let result = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    let mcps = result["mcps"].as_array().unwrap();
    assert_eq!(
        mcps.iter()
            .map(|mcp| mcp["slug"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["docs", "package"]
    );
    assert_eq!(mcps[0]["enabled"], false);
    // Only the origin: the path of an MCP can hold the key of an account.
    assert_eq!(mcps[0]["url"], "https://docs.example/…");
    assert_eq!(mcps[0]["bearer_token_set"], true);
    assert_eq!(mcps[0]["header_value_set"], false);
    assert_eq!(
        mcps[1]["npm_env"],
        json!([{ "name": "API_KEY", "value_set": true }])
    );

    let text = result.to_string();
    assert_eq!(mcps[1]["npm_args_count"], 1);
    for secret in [
        "bearer-secret-value",
        "env-secret-value",
        "arg-secret-value",
        "path-secret-9f3a",
        "sk-12345",
        "pw@",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

fn merge_env(core: &mymcps_core::Core, entries: &[(&str, &str)]) -> Option<String> {
    use mymcps_core::secrets::{EnvironmentInput, merge_environment};
    let inputs: Vec<EnvironmentInput> = entries
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: (*name).into(),
            value: Some((*value).into()),
        })
        .collect();
    merge_environment(&core.encryption, None, &inputs)
}

// --- create ---

#[tokio::test]
async fn creates_an_http_mcp_with_a_bearer_token_and_tests_it() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({
            "name": "Docs",
            "description": "The docs",
            "transport": "http",
            "url": "https://docs.example/mcp",
            "auth_type": "bearer",
            "bearer_token": "bearer-secret-value",
        }),
    )
    .await;

    assert_eq!(result["mcp"]["slug"], "docs");
    assert_eq!(result["mcp"]["status"], "ready");
    assert_eq!(result["mcp"]["enabled"], true);
    assert_eq!(result["mcp"]["bearer_token_set"], true);
    assert_eq!(result["test"]["ok"], true);
    assert_eq!(result["test"]["tool_count"], 1);
    assert!(!result.to_string().contains("bearer-secret-value"));

    let saved = mcp_named(&gateway, "docs").await;
    assert_eq!(saved.created_by, admin.id);
    assert_eq!(saved.team_id, Team::ROOT_ID);
    assert_eq!(saved.status, McpStatus::Ready);
    assert_ne!(saved.auth_bearer.as_deref(), Some("bearer-secret-value"));
    assert_eq!(
        decrypted(&gateway, &saved.auth_bearer).as_deref(),
        Some("bearer-secret-value")
    );
    // The probe presented it to the MCP.
    let sent = gateway.upstreams.requests();
    assert!(sent.iter().any(|request| {
        request
            .headers
            .get("authorization")
            .is_some_and(|value| value == "Bearer bearer-secret-value")
    }));
}

#[tokio::test]
async fn creates_a_disabled_mcp_with_a_custom_header() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({
            "name": "Header MCP", "transport": "http", "url": "https://header.example/mcp",
            "auth_type": "header", "header_name": "X-Api-Key", "header_value": "header-secret",
            "enabled": false,
        }),
    )
    .await;
    assert_eq!(result["mcp"]["enabled"], false);
    assert_eq!(result["mcp"]["header_name"], "X-Api-Key");
    assert_eq!(result["mcp"]["header_value_set"], true);
    assert!(!result.to_string().contains("header-secret"));

    let saved = mcp_named(&gateway, "header-mcp").await;
    assert_eq!(
        decrypted(&gateway, &saved.auth_header_value).as_deref(),
        Some("header-secret")
    );
}

/// What an agent is told to do about an npm MCP.
const NPM_ADVICE: &str = "Ask the user to do it on the MCPs page";

#[tokio::test]
async fn refuses_to_create_an_npm_mcp_whatever_the_shape_of_the_call() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    let cases = [
        json!({ "name": "A", "transport": "npm", "npm_package": "@example/mcp" }),
        json!({ "name": "A", "transport": "npm" }),
        json!({
            "name": "A", "transport": "npm", "npm_package": "@example/environment-mcp",
            "npm_version": "1.2.3", "npm_args": ["--verbose"],
            "npm_env": [{ "name": "API_KEY", "value": "env-secret-one" }],
        }),
        // Settings of a package under another transport are a try as well.
        json!({ "name": "A", "transport": "http", "url": "https://a.example/mcp", "npm_package": "@example/mcp" }),
        json!({ "name": "A", "transport": "http", "url": "https://a.example/mcp", "npm_env": [{ "name": "K", "value": "v" }] }),
    ];
    for arguments in cases {
        let refusal =
            call_refused(&gateway, &manager, "mcp_registry_create", arguments.clone()).await;
        assert!(refusal.contains(NPM_ADVICE), "{arguments}: {refusal}");
        assert!(!refusal.contains("env-secret-one"));
    }
    // Nothing was saved, nothing was installed or reached.
    let result = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert_eq!(result["mcps"], json!([]));
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn names_the_argument_that_is_wrong() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    let cases = [
        (
            json!({ "name": "A", "transport": "http", "url": "ftp://docs.example/mcp" }),
            "Invalid url: MCP URL must use HTTP or HTTPS",
        ),
        (
            json!({ "name": "A", "transport": "http", "url": "not a url" }),
            "Invalid url",
        ),
        (json!({ "name": "A", "transport": "http" }), "Invalid url"),
        (
            json!({ "name": "A", "transport": "http", "url": "https://a.example/mcp", "auth_type": "header", "header_value": "x" }),
            "Invalid header_name",
        ),
        (
            json!({ "name": "A", "transport": "builtin" }),
            "configure built-in MCPs from the MyMCPs page",
        ),
        (
            json!({ "transport": "http", "url": "https://a.example/mcp" }),
            "name",
        ),
        (
            json!({ "name": "A", "transport": "http", "url": "https://a.example/mcp", "enabled": "yes" }),
            "enabled must be true or false",
        ),
        (
            json!({ "name": "A", "transport": "http", "url": "https://a.example/mcp", "bearer": "x" }),
            "bearer is not an argument of this tool",
        ),
        (
            json!({ "name": "A", "transport": "npm", "npm_package": "@example/mcp", "npm_args": ["two words"] }),
            "npm_args must be a list",
        ),
    ];
    for (arguments, expected) in cases {
        let refusal =
            call_refused(&gateway, &manager, "mcp_registry_create", arguments.clone()).await;
        assert!(refusal.contains(expected), "{arguments}: {refusal}");
    }
    // Nothing was saved, and no MCP was reached.
    let result = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert_eq!(result["mcps"], json!([]));
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn gives_a_new_mcp_a_slug_no_other_mcp_of_the_team_has() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    for expected in ["docs", "docs-2"] {
        let result = call_ok(
            &gateway,
            &manager,
            "mcp_registry_create",
            json!({ "name": "Docs", "transport": "http", "url": "https://docs.example/mcp" }),
        )
        .await;
        assert_eq!(result["mcp"]["slug"], expected);
    }
}

// --- update ---

async fn bearer_mcp(gateway: &TestGateway, admin: &User) -> mymcps_core::models::Mcp {
    let core = gateway.core().clone();
    create_mcp(gateway, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.slug = "docs".into();
        mcp.description = Some("Old".into());
        mcp.http_url = Some("https://docs.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = core.encrypt_secret(Some("bearer-secret-value"));
        mcp.enabled = false;
    })
    .await
}

#[tokio::test]
async fn updates_only_what_it_is_given_and_keeps_the_secrets() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "description": "New", "url": "https://docs.example/v2/mcp" }),
    )
    .await;
    assert_eq!(result["mcp"]["slug"], "docs");
    assert_eq!(result["cleared_secrets"], json!([]));
    assert_eq!(result["previous_slug"], Value::Null);
    assert_eq!(result["test"]["ok"], true);

    let saved = mcp_named(&gateway, "docs").await;
    assert_eq!(saved.description.as_deref(), Some("New"));
    assert_eq!(
        saved.http_url.as_deref(),
        Some("https://docs.example/v2/mcp")
    );
    // Not given: kept, the bearer token and the switch included.
    assert_eq!(saved.auth_type, McpAuthType::Bearer);
    assert!(!saved.enabled);
    assert_eq!(
        decrypted(&gateway, &saved.auth_bearer).as_deref(),
        Some("bearer-secret-value")
    );

    // A blank description clears it; a new token replaces the saved one.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "description": "", "bearer_token": "second-token", "enabled": true }),
    )
    .await;
    let saved = mcp_named(&gateway, "docs").await;
    assert_eq!(saved.description, None);
    assert!(saved.enabled);
    assert_eq!(
        decrypted(&gateway, &saved.auth_bearer).as_deref(),
        Some("second-token")
    );
}

#[tokio::test]
async fn sets_how_long_a_tool_call_waits_and_keeps_it_when_not_given() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "timeout_seconds": 240 }),
    )
    .await;
    assert_eq!(result["mcp"]["timeout_seconds"], 240);
    assert_eq!(
        mcp_named(&gateway, "docs").await.tool_timeout_seconds,
        Some(240)
    );

    // Another change leaves it as it is.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "description": "New" }),
    )
    .await;
    assert_eq!(
        mcp_named(&gateway, "docs").await.tool_timeout_seconds,
        Some(240)
    );

    for (timeout, refusal) in [
        (
            json!(301),
            "Invalid timeout_seconds: A remote MCP waits at most 300 seconds for a tool call",
        ),
        (
            json!(0),
            "timeout_seconds must be a whole number of seconds, 1 or more, or null for the default",
        ),
        (
            json!("240"),
            "timeout_seconds must be a whole number of seconds, 1 or more, or null for the default",
        ),
    ] {
        let refused = call_refused(
            &gateway,
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "timeout_seconds": timeout }),
        )
        .await;
        assert!(refused.contains(refusal), "{refused}");
    }
    assert_eq!(
        mcp_named(&gateway, "docs").await.tool_timeout_seconds,
        Some(240)
    );

    // Null goes back to the default.
    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "timeout_seconds": null }),
    )
    .await;
    assert_eq!(result["mcp"]["timeout_seconds"], 60);
    assert_eq!(mcp_named(&gateway, "docs").await.tool_timeout_seconds, None);
}

/// A tailnet of a team, as if connected earlier.
async fn tailnet_in(gateway: &TestGateway, team_id: i64, user: &User, name: &str) -> Tailnet {
    let mut tailnet = Tailnet {
        team_id,
        name: name.into(),
        hostname: format!("mymcps-{}", name.to_lowercase()),
        created_by: user.id,
        ..Default::default()
    };
    tailnet.insert(&**gateway.db()).await.unwrap();
    tailnet
}

#[tokio::test]
async fn reads_and_sets_the_tailnet_of_an_http_mcp_by_name() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;
    let pro = tailnet_in(&gateway, Team::ROOT_ID, &admin, "Pro").await;
    let other_team = create_team(&gateway, "Other").await;
    tailnet_in(&gateway, other_team.id, &admin, "Theirs").await;

    let listed = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert_eq!(listed["mcps"][0]["tailnet"], Value::Null);

    // By name, whatever its case.
    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "tailnet": "pro" }),
    )
    .await;
    assert_eq!(result["mcp"]["tailnet"], "Pro");
    assert_eq!(mcp_named(&gateway, "docs").await.tailnet_id, Some(pro.id));
    let listed = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert_eq!(listed["mcps"][0]["tailnet"], "Pro");

    // Another change leaves it as it is.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "description": "New" }),
    )
    .await;
    assert_eq!(mcp_named(&gateway, "docs").await.tailnet_id, Some(pro.id));

    // The team's own tailnets only, and the refusal says which they are.
    for name in ["Theirs", "nope"] {
        let refused = call_refused(
            &gateway,
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "tailnet": name }),
        )
        .await;
        assert!(
            refused.contains(&format!("Tailnet \"{name}\" was not found in this team")),
            "{refused}"
        );
        assert!(refused.contains("Its tailnets: Pro."), "{refused}");
    }
    assert_eq!(mcp_named(&gateway, "docs").await.tailnet_id, Some(pro.id));

    // A new MCP can name one, and null or an empty name goes back to the internet.
    let created = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({ "name": "Grafana", "transport": "http", "url": "https://grafana.tail1234.ts.net/mcp", "tailnet": "Pro" }),
    )
    .await;
    assert_eq!(created["mcp"]["tailnet"], "Pro");
    assert_eq!(
        mcp_named(&gateway, "grafana").await.tailnet_id,
        Some(pro.id)
    );
    for cleared in [json!(null), json!("")] {
        call_ok(
            &gateway,
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "tailnet": cleared }),
        )
        .await;
        assert_eq!(mcp_named(&gateway, "docs").await.tailnet_id, None);
        call_ok(
            &gateway,
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "tailnet": "Pro" }),
        )
        .await;
    }

    // A tailnet is a name.
    let refused = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "tailnet": 7 }),
    )
    .await;
    assert!(
        refused.contains("tailnet must be the name of a tailnet of the team"),
        "{refused}"
    );
}

#[tokio::test]
async fn names_no_tailnet_for_a_team_that_has_none_and_none_for_an_npm_mcp() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let refused = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "tailnet": "pro" }),
    )
    .await;
    assert!(refused.contains("this team has no tailnet"), "{refused}");

    tailnet_in(&gateway, Team::ROOT_ID, &admin, "pro").await;
    let package = package_mcp(&gateway, &admin).await;
    let refused = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": package.slug, "tailnet": "pro" }),
    )
    .await;
    assert!(
        refused.contains("Only an HTTP MCP can be reached through a tailnet"),
        "{refused}"
    );
    assert_eq!(mcp_named(&gateway, &package.slug).await.tailnet_id, None);
}

#[tokio::test]
async fn clears_the_secrets_when_the_origin_changes_and_says_so() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "url": "https://elsewhere.example/mcp" }),
    )
    .await;
    assert_eq!(result["cleared_secrets"], json!(["bearer_token"]));
    assert_eq!(result["mcp"]["bearer_token_set"], false);
    let notes = result["notes"].to_string();
    assert!(
        notes.contains("bearer_token") && notes.contains("origin"),
        "{notes}"
    );
    assert_eq!(mcp_named(&gateway, "docs").await.auth_bearer, None);

    // Sent again together with the new origin, they are kept and not reported.
    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "url": "https://third.example/mcp", "bearer_token": "fresh-token" }),
    )
    .await;
    assert_eq!(result["cleared_secrets"], json!([]));
    let saved = mcp_named(&gateway, "docs").await;
    assert_eq!(
        decrypted(&gateway, &saved.auth_bearer).as_deref(),
        Some("fresh-token")
    );
}

#[tokio::test]
async fn follows_a_rename_with_the_new_slug() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "name": "Handbook" }),
    )
    .await;
    assert_eq!(result["mcp"]["slug"], "handbook");
    assert_eq!(result["previous_slug"], "docs");
    assert!(result["notes"].to_string().contains("handbook"));
    assert!(mcp_in(&gateway, Team::ROOT_ID, "docs").await.is_none());
}

async fn package_mcp(gateway: &TestGateway, admin: &User) -> mymcps_core::models::Mcp {
    let core = gateway.core().clone();
    create_mcp(gateway, admin.id, |mcp| {
        mcp.name = "Package".into();
        mcp.slug = "package".into();
        mcp.transport = McpTransport::Npm;
        mcp.http_url = None;
        mcp.npm_package = Some("@example/one".into());
        mcp.npm_version = Some("1.0.0".into());
        mcp.set_npm_args_list(&["--flag".to_owned()]);
        mcp.npm_env = merge_env(&core, &[("API_KEY", "env-secret-one")]);
    })
    .await
}

#[tokio::test]
async fn does_not_let_an_agent_change_what_an_npm_mcp_runs() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let before = package_mcp(&gateway, &admin).await;
    create_http_mcp(&gateway, admin.id, "Docs", "docs").await;
    gateway.upstreams.clear();

    for arguments in [
        json!({ "slug": "package", "npm_package": "@example/two" }),
        json!({ "slug": "package", "npm_version": "2.0.0" }),
        json!({ "slug": "package", "npm_args": ["--flag", "--extra"] }),
        json!({ "slug": "package", "npm_args": [] }),
        // An address in the environment is where the key it holds would go.
        json!({ "slug": "package", "npm_env": [{ "name": "API_KEY" }, { "name": "API_URL", "value": "https://evil.example" }] }),
        json!({ "slug": "package", "npm_env": [{ "name": "API_KEY", "value": "other" }] }),
        json!({ "slug": "package", "npm_env": [] }),
        // An http MCP does not become one, and an npm one does not take another transport's package.
        json!({ "slug": "docs", "transport": "npm", "npm_package": "@example/mcp" }),
        json!({ "slug": "docs", "npm_package": "@example/mcp" }),
        json!({ "slug": "package", "transport": "npm", "npm_package": "@example/two" }),
    ] {
        let refusal =
            call_refused(&gateway, &manager, "mcp_registry_update", arguments.clone()).await;
        assert!(refusal.contains(NPM_ADVICE), "{arguments}: {refusal}");
    }

    let after = mcp_named(&gateway, "package").await;
    assert_eq!(after.npm_package, before.npm_package);
    assert_eq!(after.npm_version, before.npm_version);
    assert_eq!(after.npm_args, before.npm_args);
    assert_eq!(after.npm_env, before.npm_env);
    assert_eq!(
        mcp_named(&gateway, "docs").await.transport,
        McpTransport::Http
    );
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn lets_an_agent_rename_and_switch_off_an_npm_mcp_and_say_what_it_already_has() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    package_mcp(&gateway, &admin).await;

    // Repeating what is saved is not a change, a variable without a value keeps its own.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({
            "slug": "package", "name": "Package renamed", "description": "Runs the package",
            "enabled": false, "npm_package": "@example/one", "npm_version": "1.0.0",
            "npm_args": ["--flag"], "npm_env": [{ "name": "API_KEY" }],
        }),
    )
    .await;
    let saved = mcp_named(&gateway, "package-renamed").await;
    assert!(!saved.enabled);
    assert_eq!(saved.npm_package.as_deref(), Some("@example/one"));
    assert_eq!(saved.npm_args_list(), ["--flag"]);
    assert_eq!(
        saved.npm_environment(&gateway.core().encryption).unwrap(),
        [("API_KEY".to_owned(), "env-secret-one".to_owned())]
    );

    call_ok(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "package-renamed" }),
    )
    .await;
    assert!(
        mcp_in(&gateway, Team::ROOT_ID, "package-renamed")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn refuses_an_update_the_page_would_refuse() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    bearer_mcp(&gateway, &admin).await;

    let refusal = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "url": "ftp://docs.example/mcp" }),
    )
    .await;
    assert!(refusal.contains("Invalid url"), "{refusal}");
    let refusal = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "docs", "auth_type": "header" }),
    )
    .await;
    assert!(refusal.contains("Invalid header_name"), "{refusal}");
    // Nothing of the refused update was saved.
    let saved = mcp_named(&gateway, "docs").await;
    assert_eq!(saved.auth_type, McpAuthType::Bearer);
    assert_eq!(saved.http_url.as_deref(), Some("https://docs.example/mcp"));
}

// --- other teams ---

#[tokio::test]
async fn answers_the_mcp_of_another_team_like_a_missing_one() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let other_team = create_team(&gateway, "Other").await;
    let foreign = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Foreign".into();
        mcp.slug = "foreign".into();
        mcp.team_id = other_team.id;
        mcp.oauth_required = true;
    })
    .await;

    let missing = call_refused(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "nothing" }),
    )
    .await;
    for (name, arguments) in [
        (
            "mcp_registry_update",
            json!({ "slug": "foreign", "enabled": false }),
        ),
        ("mcp_registry_delete", json!({ "slug": "foreign" })),
        ("mcp_registry_test", json!({ "slug": "foreign" })),
        ("mcp_registry_auth_link", json!({ "slug": "foreign" })),
    ] {
        let refusal = call_refused(&gateway, &manager, name, arguments).await;
        assert_eq!(refusal, missing.replace("nothing", "foreign"), "{name}");
    }
    let still = mcp_in(&gateway, other_team.id, "foreign").await.unwrap();
    assert_eq!(still.id, foreign.id);
    assert!(still.enabled);
    assert!(gateway.upstreams.requests().is_empty());

    // The same slug in this team is a different MCP, and is the one acted on.
    create_http_mcp(&gateway, admin.id, "Foreign", "foreign").await;
    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "foreign" }),
    )
    .await;
    assert_ne!(result["mcp"]["id"], json!(foreign.id));
    assert!(mcp_in(&gateway, other_team.id, "foreign").await.is_some());
}

#[tokio::test]
async fn acts_in_the_team_the_token_was_granted_for() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let other_team = create_team(&gateway, "Other").await;
    mymcps_web::testing::factories::join_team(&gateway, other_team.id, &admin, TeamRole::Admin)
        .await;
    let manager = grant_in(&gateway, &admin, MANAGE, other_team.id).await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({ "name": "Docs", "transport": "http", "url": "https://docs.example/mcp" }),
    )
    .await;
    assert_eq!(result["mcp"]["slug"], "docs");
    assert!(mcp_in(&gateway, other_team.id, "docs").await.is_some());
    assert!(mcp_in(&gateway, Team::ROOT_ID, "docs").await.is_none());
}

// --- delete ---

#[tokio::test]
async fn deletes_an_mcp_by_slug() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "issues" }),
    )
    .await;
    assert_eq!(result["deleted"], true);
    assert_eq!(result["mcp"]["slug"], "issues");
    assert!(mcp_in(&gateway, Team::ROOT_ID, "issues").await.is_none());

    let refusal = call_refused(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "issues" }),
    )
    .await;
    assert!(refusal.contains("not found"), "{refusal}");
}

// --- test ---

#[tokio::test]
async fn tests_a_connection_and_reports_what_it_found() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Good".into();
        mcp.slug = "good".into();
        mcp.http_url = Some("https://good.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let core = gateway.core().clone();
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Broken".into();
        mcp.slug = "broken".into();
        mcp.http_url = Some("https://broken.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = core.encrypt_secret(Some("sk-live-secret"));
    })
    .await;

    let good = call_ok(
        &gateway,
        &manager,
        "mcp_registry_test",
        json!({ "slug": "good" }),
    )
    .await;
    assert_eq!(good["status"], "ready");
    assert_eq!(good["ok"], true);
    assert_eq!(good["error"], Value::Null);
    assert_eq!(good["oauth_required"], false);
    assert_eq!(good["auth_link"], Value::Null);
    assert_eq!(good["tool_count"], 1);
    assert_eq!(mcp_named(&gateway, "good").await.status, McpStatus::Ready);

    let broken = call_ok(
        &gateway,
        &manager,
        "mcp_registry_test",
        json!({ "slug": "broken" }),
    )
    .await;
    assert_eq!(broken["status"], "error");
    assert_eq!(broken["ok"], false);
    let error = broken["error"].as_str().unwrap();
    assert!(error.contains("Upstream exploded"), "{error}");
    assert!(!error.contains("sk-live-secret"), "{error}");
    assert_eq!(
        mcp_named(&gateway, "broken").await.last_error.as_deref(),
        Some(error)
    );
}

// --- auth link ---

#[tokio::test]
async fn hands_out_the_link_of_an_mcp_that_waits_for_oauth() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let waiting = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Waiting".into();
        mcp.slug = "waiting".into();
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
    })
    .await;
    let plain = create_http_mcp(&gateway, admin.id, "Plain", "plain").await;

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_auth_link",
        json!({ "slug": "waiting" }),
    )
    .await;
    assert_eq!(result["uses_oauth"], true);
    assert_eq!(result["connected"], false);
    assert_eq!(
        result["auth_link"],
        format!("http://localhost:3333/mcps/{}", waiting.id)
    );
    assert!(
        result["instructions"]
            .as_str()
            .unwrap()
            .contains("signed in to MyMCPs")
    );
    // The link opens the MCP's dialog; it is not the route that starts the flow.
    assert!(
        !result["auth_link"]
            .as_str()
            .unwrap()
            .contains("oauth/start")
    );

    let result = call_ok(
        &gateway,
        &manager,
        "mcp_registry_auth_link",
        json!({ "slug": "plain" }),
    )
    .await;
    assert_eq!(result["uses_oauth"], false);
    assert_eq!(result["auth_link"], Value::Null);
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("does not sign in with OAuth")
    );
    let _ = plain;

    // An MCP that an upstream just refused with 401 shows the link in its test too.
    let listing = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    let waiting_entry = listing["mcps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mcp| mcp["slug"] == "waiting")
        .unwrap();
    assert_eq!(waiting_entry["uses_oauth"], true);
    assert_eq!(waiting_entry["oauth_connected"], false);
}

#[tokio::test]
async fn gives_the_link_with_the_test_of_an_mcp_that_asks_for_oauth() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    let created = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({ "name": "Notes", "transport": "http", "url": "https://oauth.example/mcp" }),
    )
    .await;
    let id = created["mcp"]["id"].as_i64().unwrap();
    let link = format!("http://localhost:3333/mcps/{id}");
    assert_eq!(created["test"]["ok"], false);
    assert_eq!(created["test"]["oauth_required"], true);
    assert_eq!(created["test"]["auth_link"], link);
    assert_eq!(created["mcp"]["status"], "draft");
    assert_eq!(created["mcp"]["uses_oauth"], true);

    let tested = call_ok(
        &gateway,
        &manager,
        "mcp_registry_test",
        json!({ "slug": "notes" }),
    )
    .await;
    assert_eq!(tested["oauth_required"], true);
    assert_eq!(tested["auth_link"], link);
    assert_eq!(tested["error"], "OAuth authorization required");

    let linked = call_ok(
        &gateway,
        &manager,
        "mcp_registry_auth_link",
        json!({ "slug": "notes" }),
    )
    .await;
    assert_eq!(linked["auth_link"], link);
}

#[tokio::test]
async fn says_so_when_the_instance_has_no_public_address() {
    // An OAuth token cannot be used without an address, so the tool is asked
    // directly: the address is read from the configuration at each call.
    let gateway = gateway_with(|config| config.app_url = None).await;
    let admin = create_admin(&gateway).await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Waiting".into();
        mcp.slug = "waiting".into();
        mcp.oauth_required = true;
    })
    .await;

    let registry = WebMcpRegistry::new(gateway.app.state.upstream.clone());
    let actor = ManagementActor {
        user_id: admin.id,
        team_id: Team::ROOT_ID,
        access_token_id: 1,
    };
    let refusal = registry.auth_link(&actor, "waiting").await.unwrap_err();
    let ManagementError::Refused(message) = refusal else {
        panic!("{refusal:?}");
    };
    assert!(message.contains("APP_URL"), "{message}");
}

// --- built-in MCPs ---

#[tokio::test]
async fn leaves_the_setup_of_built_in_mcps_to_the_page() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let strava = create_strava_mcp(&gateway, admin.id, false, false).await;

    let refusal = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "strava", "enabled": false }),
    )
    .await;
    assert!(
        refusal.contains("configure built-in MCPs from the MyMCPs page"),
        "{refusal}"
    );
    assert!(mcp_named(&gateway, "strava").await.enabled);

    // It is listed, with no credential, and has a link to connect it.
    let listing = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    let entry = &listing["mcps"][0];
    assert_eq!(entry["transport"], "builtin");
    assert_eq!(entry["builtin_key"], "strava");
    assert_eq!(entry["uses_oauth"], true);
    let text = listing.to_string();
    assert!(
        !text.contains("strava-client-secret") && !text.contains("123456"),
        "{text}"
    );
    let link = call_ok(
        &gateway,
        &manager,
        "mcp_registry_auth_link",
        json!({ "slug": "strava" }),
    )
    .await;
    assert_eq!(
        link["auth_link"],
        format!("http://localhost:3333/mcps/{}", strava.id)
    );

    // And it can be deleted.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_delete",
        json!({ "slug": "strava" }),
    )
    .await;
    assert!(mcp_in(&gateway, Team::ROOT_ID, "strava").await.is_none());
}

// --- addresses ---

/// The names that resolve inside the network of the instance, and one that does not.
fn guard_with_internal_names() -> AddressGuard {
    AddressGuard::new(Arc::new(
        StaticResolver::new()
            .with("internal.example", &["10.0.0.5".parse().unwrap()])
            .with(
                "rebinding.example",
                &["203.0.113.7".parse().unwrap(), "127.0.0.1".parse().unwrap()],
            )
            .with("public.example", &["203.0.113.10".parse().unwrap()]),
    ))
}

#[tokio::test]
async fn does_not_reach_the_network_of_the_instance_on_the_word_of_an_agent() {
    let gateway = gateway_guarded(|_| {}, guard_with_internal_names()).await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    for url in [
        "http://127.0.0.1:3333/mcp",
        "http://[::1]/mcp",
        "http://0.0.0.0/mcp",
        // The address of the metadata service of the clouds, in every spelling.
        "http://169.254.169.254/latest/meta-data/",
        "http://[::ffff:169.254.169.254]/latest/meta-data/",
        "http://[::ffff:a9fe:a9fe]/",
        "http://10.0.0.5/mcp",
        "http://192.168.1.10:8080/mcp",
        "http://100.64.0.1/mcp",
        "http://[fd00::1]/mcp",
        "http://[fe80::1]/mcp",
        "https://user:pw@127.0.0.1/mcp",
        // Names that resolve there, wholly or in part.
        "https://internal.example/mcp",
        "https://rebinding.example/mcp",
    ] {
        let refusal = call_refused(
            &gateway,
            &manager,
            "mcp_registry_create",
            json!({ "name": "Inside", "transport": "http", "url": url }),
        )
        .await;
        assert!(
            refusal.starts_with("Invalid url: the host is a loopback, private or link-local"),
            "{url}: {refusal}"
        );
    }
    // The form refuses a name without a dot, and a number for an address,
    // before the guard is asked.
    for url in [
        "http://localhost/mcp",
        "http://LOCALHOST./mcp",
        "http://localhost:3333/",
    ] {
        let refusal = call_refused(
            &gateway,
            &manager,
            "mcp_registry_create",
            json!({ "name": "Inside", "transport": "http", "url": url }),
        )
        .await;
        assert!(refusal.starts_with("Invalid url"), "{url}: {refusal}");
    }
    assert!(gateway.upstreams.requests().is_empty(), "no request left");
    let result = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert_eq!(result["mcps"], json!([]));

    // A public name is still welcome.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({ "name": "Outside", "transport": "http", "url": "https://public.example/mcp" }),
    )
    .await;
}

#[tokio::test]
async fn does_not_let_an_agent_move_an_mcp_into_the_network_of_the_instance() {
    let gateway = gateway_guarded(|_| {}, guard_with_internal_names()).await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.slug = "docs".into();
        mcp.http_url = Some("https://public.example/mcp".into());
    })
    .await;
    // Put by a person, on the MCPs page: it stays what it is.
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Home".into();
        mcp.slug = "home".into();
        mcp.http_url = Some("http://192.168.1.10:8123/mcp".into());
    })
    .await;
    gateway.upstreams.clear();

    for url in [
        "http://169.254.169.254/",
        "https://internal.example/mcp",
        "http://[::1]/",
    ] {
        let refusal = call_refused(
            &gateway,
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "url": url }),
        )
        .await;
        assert!(
            refusal.starts_with("Invalid url: the host is a loopback"),
            "{url}: {refusal}"
        );
    }
    assert_eq!(
        mcp_named(&gateway, "docs").await.http_url.as_deref(),
        Some("https://public.example/mcp")
    );
    assert!(gateway.upstreams.requests().is_empty());

    // What a person set can be renamed and tested, not pointed elsewhere inside.
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "home", "description": "Home Assistant", "url": "http://192.168.1.10:8123/mcp" }),
    )
    .await;
    call_ok(
        &gateway,
        &manager,
        "mcp_registry_test",
        json!({ "slug": "home" }),
    )
    .await;
    let refusal = call_refused(
        &gateway,
        &manager,
        "mcp_registry_update",
        json!({ "slug": "home", "url": "http://192.168.1.11:8123/mcp" }),
    )
    .await;
    assert!(
        refusal.starts_with("Invalid url: the host is a loopback"),
        "{refusal}"
    );
}

#[tokio::test]
async fn keeps_the_secrets_of_an_url_out_of_what_the_agent_reads_back() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;

    // `broken.` fails its connection: the error is saved and told.
    let created = call_ok(
        &gateway,
        &manager,
        "mcp_registry_create",
        json!({
            "name": "Broken", "transport": "http",
            "url": "https://agent:url-password@broken.example/s/path-secret-9f3a/mcp?key=query-secret-abc",
            "auth_type": "bearer", "bearer_token": "bearer-secret-value",
        }),
    )
    .await;
    let listed = call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    let tested = call_ok(
        &gateway,
        &manager,
        "mcp_registry_test",
        json!({ "slug": "broken" }),
    )
    .await;
    for answer in [&created, &listed, &tested] {
        let text = answer.to_string();
        for secret in [
            "url-password",
            "path-secret-9f3a",
            "query-secret-abc",
            "bearer-secret-value",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }
    assert_eq!(created["mcp"]["url"], "https://broken.example/…");
}

// --- allowance ---

#[tokio::test]
async fn holds_an_access_token_to_a_number_of_registry_calls_per_minute() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let other = grant(&gateway, &admin, MANAGE).await;

    for _ in 0..30 {
        call_ok(&gateway, &manager, "mcp_registry_list", json!({})).await;
    }
    let refusal = call_refused(&gateway, &manager, "mcp_registry_list", json!({})).await;
    assert!(
        refusal.contains("Too many calls to the MCP registry tools"),
        "{refusal}"
    );
    // Another connection has its own allowance, and the tools of the MCPs are not counted.
    call_ok(&gateway, &other, "mcp_registry_list", json!({})).await;
    let tools = list_tools(&gateway, &manager, "eager").await;
    assert!(!tools.is_empty());
}

// --- the other ways in ---

/// A token that was never allowed the tools, and one that is, try every way
/// to reach them but their name: neither changes the registry.
#[tokio::test]
async fn reaches_the_tools_by_their_name_only() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let plain = grant(&gateway, &admin, "mcp:tools").await;
    let evil = json!({ "name": "Evil", "transport": "http", "url": "https://evil.example/mcp" });

    // The protocol trims a name: padding is the tool itself, and as much
    // behind its permission as the tool. Left to the token without one.
    for padded in [
        " mcp_registry_create",
        "mcp_registry_create\n",
        "\tmcp_registry_create ",
    ] {
        gateway
            .post_message(
                &plain,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": padded, "arguments": evil } }),
                &[],
            )
            .await;
    }
    assert!(mcp_in(&gateway, Team::ROOT_ID, "evil").await.is_none());

    for token in [&plain, &manager] {
        for mode in ["eager", "lazy"] {
            for name in [
                "MCP_REGISTRY_CREATE",
                "Mcp_Registry_Create",
                "mcp_registry_create\u{0}",
                "mcp_registry_create\u{200b}",
                "mcp_registry__create",
                "mcp_registry_create__create",
                "mcp_registry-create",
            ] {
                // Whatever it answers, it is not the tool.
                gateway
                    .post_message(
                        token,
                        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": name, "arguments": evil } }),
                        &[("x-mymcps-tool-mode", mode)],
                    )
                    .await;
            }
            // The lazy meta-tool names an MCP and one of its tools.
            for (mcp, tool) in [
                ("mcp_registry_create", "mcp_registry_create"),
                ("mcp_registry", "create"),
                ("mcp", "registry_create"),
                ("", "mcp_registry_create"),
            ] {
                gateway
                    .post_message(
                        token,
                        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": "call_tool",
                                "arguments": { "mcp": mcp, "tool": tool, "arguments": evil } } }),
                        &[("x-mymcps-tool-mode", mode)],
                    )
                    .await;
            }
        }
    }
    assert!(mcp_in(&gateway, Team::ROOT_ID, "evil").await.is_none());
    assert!(gateway.upstreams.requests().is_empty());

    // A batch is no JSON-RPC message the gateway reads: no call in it runs.
    for token in [&plain, &manager] {
        let batch = gateway
            .post_message(
                token,
                json!([
                    { "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": "mcp_registry_create", "arguments": evil } },
                    { "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                      "params": { "name": "mcp_registry_list", "arguments": {} } },
                ]),
                &[],
            )
            .await;
        assert_eq!(batch.status, http::StatusCode::BAD_REQUEST);
    }
    assert!(mcp_in(&gateway, Team::ROOT_ID, "evil").await.is_none());
}

/// The person removes the permission on the Access tokens page; the access
/// token the agent already holds stops working as a manager on its next
/// request, with no wait for it to expire.
#[tokio::test]
async fn loses_the_tools_on_the_next_request_once_the_permission_is_removed() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    for mode in ["eager", "lazy"] {
        assert_eq!(
            registry_tools(&list_tools(&gateway, &manager, mode).await),
            REGISTRY_TOOLS
        );
    }

    let token_id: i64 =
        sqlx::query_scalar("select `id` from `access_tokens` where `oauth_scopes` = ?")
            .bind(MANAGE)
            .fetch_one(&**gateway.db())
            .await
            .unwrap();
    let removed = gateway
        .app
        .post(&format!("/tokens/{token_id}/remove-mcp-management"))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(removed.status, http::StatusCode::FOUND);

    for mode in ["eager", "lazy"] {
        let names = list_tools(&gateway, &manager, mode).await;
        assert!(registry_tools(&names).is_empty(), "{mode}");
        assert!(!names.is_empty(), "the MCPs are still served");
    }
    for (name, arguments) in [
        ("mcp_registry_delete", json!({ "slug": "issues" })),
        ("mcp_registry_list", json!({})),
    ] {
        let refusal = call_refused(&gateway, &manager, name, arguments).await;
        assert!(refusal.contains("not allowed to manage MCPs"), "{refusal}");
    }
    assert!(mcp_in(&gateway, Team::ROOT_ID, "issues").await.is_some());
}

#[tokio::test]
async fn refuses_the_tools_once_the_person_has_left_the_team_or_the_account_is_gone() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let other_team = create_team(&gateway, "Other").await;
    mymcps_web::testing::factories::join_team(&gateway, other_team.id, &admin, TeamRole::Admin)
        .await;
    let in_root = grant(&gateway, &admin, MANAGE).await;
    let in_other = grant_in(&gateway, &admin, MANAGE, other_team.id).await;
    assert_eq!(
        registry_tools(&list_tools(&gateway, &in_other, "eager").await),
        REGISTRY_TOOLS
    );

    // Out of the other team: that team's token is done, the root team's is not.
    sqlx::query("delete from `team_user` where `team_id` = ? and `user_id` = ?")
        .bind(other_team.id)
        .bind(admin.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    assert!(registry_tools(&list_tools(&gateway, &in_other, "eager").await).is_empty());
    let refusal = call_refused(&gateway, &in_other, "mcp_registry_list", json!({})).await;
    assert!(
        refusal.contains("not an administrator or an owner"),
        "{refusal}"
    );
    assert_eq!(
        registry_tools(&list_tools(&gateway, &in_root, "eager").await),
        REGISTRY_TOOLS
    );

    // Out of every team.
    sqlx::query("delete from `team_user` where `user_id` = ?")
        .bind(admin.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    assert!(registry_tools(&list_tools(&gateway, &in_root, "lazy").await).is_empty());
    call_refused(
        &gateway,
        &in_root,
        "mcp_registry_create",
        json!({
            "name": "Evil", "transport": "http", "url": "https://evil.example/mcp",
        }),
    )
    .await;

    // The account deleted: the token is refused, or has no tool.
    sqlx::query("delete from `users` where `id` = ?")
        .bind(admin.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    let response = gateway
        .post_message(
            &in_root,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "mcp_registry_create", "arguments": {
                    "name": "Evil", "transport": "http", "url": "https://evil.example/mcp" } } }),
            &[],
        )
        .await;
    if response.status == http::StatusCode::OK {
        assert_eq!(result_of(&response)["isError"], true);
    }
    assert!(mcp_in(&gateway, Team::ROOT_ID, "evil").await.is_none());
}

// --- the link ---

#[tokio::test]
async fn builds_the_link_from_the_configured_address_whatever_the_request_says() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let manager = grant(&gateway, &admin, MANAGE).await;
    let waiting = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Waiting".into();
        mcp.slug = "waiting".into();
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
        mcp.name = "https://evil.example/".into();
    })
    .await;

    let response = gateway
        .post_message(
            &manager,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "mcp_registry_auth_link", "arguments": { "slug": waiting.slug } } }),
            &[
                ("host", "evil.example"),
                ("x-forwarded-host", "evil.example"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-port", "8443"),
                ("forwarded", "host=evil.example;proto=https"),
                ("origin", "https://evil.example"),
                ("referer", "https://evil.example/"),
            ],
        )
        .await;
    let result = result_of(&response);
    assert_eq!(result["isError"], Value::Null, "{result}");
    let content = &result["structuredContent"];
    assert_eq!(
        content["auth_link"],
        format!("http://localhost:3333/mcps/{}", waiting.id)
    );
    // Nothing the request or the MCP holds is in the link.
    assert!(!content["auth_link"].as_str().unwrap().contains("evil"));
}

#[tokio::test]
async fn opens_the_link_of_an_mcp_only_for_the_team_it_is_in() {
    let gateway = gateway().await;
    let admin = create_admin(&gateway).await;
    let other_team = create_team(&gateway, "Other").await;
    mymcps_web::testing::factories::join_team(&gateway, other_team.id, &admin, TeamRole::Admin)
        .await;
    let foreign = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Foreign".into();
        mcp.slug = "foreign".into();
        mcp.team_id = other_team.id;
        mcp.oauth_required = true;
    })
    .await;

    // The person works in the Root Team: the MCP of the other one is a missing MCP
    // for the page, and nothing starts an authorization for it.
    let opened = gateway
        .app
        .get(&format!("/mcps/{}", foreign.id))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(opened.redirect_path().as_deref(), Some("/mcps"));
    let started = gateway
        .app
        .get(&format!("/mcps/{}/oauth/start", foreign.id))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(started.redirect_path().as_deref(), Some("/mcps"));
    assert!(gateway.upstreams.requests().is_empty());
}
