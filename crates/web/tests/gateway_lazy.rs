//! `tests/functional/gateway_lazy.spec.ts`: the lazy tool mode from the
//! request of an agent to `/mcp` to the MCP behind the gateway.

#[path = "support/gateway.rs"]
mod support;

use http::{Method, StatusCode};
use mymcps_core::models::{GatewayToolMode, InstanceSetting, ScopeMode};
use mymcps_web::testing::factories::{create_admin, create_mcp};
use serde_json::{Value, json};

use support::*;

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "gateway-lazy-test", "version": "1.0.0" },
        },
    })
}

fn tools_list_request() -> Value {
    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })
}

fn tool_call_request(name: &str, arguments: Option<Value>) -> Value {
    let mut params = json!({ "name": name });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": params })
}

/// The MCPs of the TypeScript tests: `issues.example` has two tools, any
/// other host one, and a call is answered with what was called.
fn upstreams(message: &UpstreamMessage) -> Reply {
    Reply::Result(match message.method.as_str() {
        "tools/list" if message.host.starts_with("issues.") => json!({
            "tools": [
                {
                    "name": "create_issue",
                    "description": "Create a new project issue",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "title": { "type": "string" } },
                        "required": ["title"],
                    },
                },
                {
                    "name": "list_issues",
                    "description": "List project work items",
                    "inputSchema": { "type": "object" },
                },
            ],
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "create_event",
                "description": "Create a calendar event",
                "inputSchema": { "type": "object" },
            }],
        }),
        _ => json!({
            "content": [{
                "type": "text",
                "text": json!({
                    "name": message.params["name"],
                    "args": message.params["arguments"],
                })
                .to_string(),
            }],
        }),
    })
}

fn tool_names(result: &Value) -> Vec<&str> {
    result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

const LAZY_TOOLS: [&str; 3] = ["list_mcps", "tool_search", "call_tool"];

#[tokio::test]
async fn rejects_an_unsupported_tool_mode_header_on_post_and_get() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let authorization = format!("Bearer {plaintext}");

    let post_response = gateway
        .post_message(
            &plaintext,
            tools_list_request(),
            &[("x-mymcps-tool-mode", "sometimes")],
        )
        .await;
    let get_response = gateway
        .mcp_request(
            Method::GET,
            &[
                ("authorization", &authorization),
                ("x-mymcps-tool-mode", "sometimes"),
            ],
            None,
        )
        .await;

    for response in [post_response, get_response] {
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json(),
            json!({
                "error": "invalid_tool_mode",
                "message": "X-MyMCPs-Tool-Mode must be either eager or lazy",
            })
        );
        assert_eq!(
            response.header("content-type"),
            Some("application/json; charset=utf-8")
        );
    }
}

#[tokio::test]
async fn uses_the_instance_default_when_the_header_is_absent_and_lets_the_header_override_it() {
    let gateway = TestGateway::new(upstreams).await;
    let db = gateway.db();
    let admin = create_admin(&gateway).await;
    let issues = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[issues.id])
        .await
        .plaintext;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.gateway_tool_mode = GatewayToolMode::Lazy;
    settings.save(&**db).await.unwrap();

    let default_response = gateway
        .post_message(&plaintext, tools_list_request(), &[])
        .await;
    assert_eq!(default_response.status, StatusCode::OK);
    assert_eq!(tool_names(&result_of(&default_response)), LAZY_TOOLS);
    assert!(gateway.upstreams.requests().is_empty());

    let overridden_response = gateway
        .post_message(
            &plaintext,
            tools_list_request(),
            &[("x-mymcps-tool-mode", "eager")],
        )
        .await;
    assert_eq!(overridden_response.status, StatusCode::OK);
    assert_eq!(
        tool_names(&result_of(&overridden_response)),
        ["issues__create_issue", "issues__list_issues"]
    );
    assert!(
        gateway
            .upstreams
            .methods()
            .contains(&"tools/list".to_owned())
    );
}

#[tokio::test]
async fn shares_only_allowed_mcp_summaries_during_lazy_initialization_without_upstream_calls() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let allowed = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Issue Tracker".into();
        mcp.slug = "issues".into();
        mcp.http_url = Some("https://issues.example/mcp".into());
        mcp.description = Some("Project issues\nwithout exposing credentials".into());
    })
    .await;
    create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Private Calendar".into();
        mcp.slug = "calendar".into();
        mcp.http_url = Some("https://calendar.example/mcp".into());
        mcp.last_error = Some("Bearer top-secret-value".into());
    })
    .await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[allowed.id])
        .await
        .plaintext;

    let response = gateway
        .post_message(
            &plaintext,
            initialize_request(),
            &[("x-mymcps-tool-mode", " LaZy ")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let result = result_of(&response);
    let instructions = result["instructions"].as_str().unwrap();

    assert_eq!(
        result["serverInfo"],
        json!({ "name": "mymcps", "version": mymcps_core::VERSION })
    );
    assert_eq!(result["capabilities"], json!({ "tools": {} }));
    assert_eq!(
        instructions,
        [
            "Available MCPs:",
            "- issues: Project issues without exposing credentials",
            "",
            "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools.",
        ]
        .join("\n")
    );
    assert!(!instructions.contains("Private Calendar"));
    assert!(!instructions.contains("top-secret-value"));
    assert!(gateway.upstreams.requests().is_empty());

    // The eager gateway has nothing to say before its tools are listed.
    let eager = gateway
        .post_message(&plaintext, initialize_request(), &[])
        .await;
    assert!(result_of(&eager).get("instructions").is_none());
}

#[tokio::test]
async fn lists_only_lazy_gateway_tools_without_probing_upstream_mcps() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let mcp = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[mcp.id])
        .await
        .plaintext;

    let response = gateway
        .post_message(
            &plaintext,
            tools_list_request(),
            &[("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let result = result_of(&response);

    assert_eq!(tool_names(&result), LAZY_TOOLS);
    assert_eq!(
        result["tools"][1]["inputSchema"]["required"],
        json!(["mcp", "query"])
    );
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn returns_the_allowed_mcp_catalog_through_list_mcps_without_upstream_calls() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let issues = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "Issues".into();
        mcp.slug = "issues".into();
        mcp.http_url = Some("https://issues.example/mcp".into());
        mcp.description = Some("Issue tracking".into());
    })
    .await;
    create_http_mcp(&gateway, admin.id, "Calendar", "calendar").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[issues.id])
        .await
        .plaintext;

    let response = gateway
        .post_message(
            &plaintext,
            tool_call_request("list_mcps", None),
            &[("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let result = result_of(&response);

    assert_eq!(
        result["structuredContent"]["mcps"],
        json!([
            { "name": "Issues", "slug": "issues", "description": "Issue tracking", "status": "ready" },
        ])
    );
    // The text an agent reads is the same catalog, as `JSON.stringify` wrote it.
    assert_eq!(
        result_text(&result),
        r#"{"mcps":[{"name":"Issues","slug":"issues","description":"Issue tracking","status":"ready"}]}"#
    );
    assert!(result.get("isError").is_none());
    assert!(gateway.upstreams.requests().is_empty());
    // Asking for the catalog is not a call of a tool of an MCP.
    gateway.flush().await;
    assert!(gateway.call_logs().await.is_empty());
}

#[tokio::test]
async fn searches_only_the_selected_allowed_mcp_and_returns_matching_schemas() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let issues = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let calendar = create_http_mcp(&gateway, admin.id, "Calendar", "calendar").await;
    let plaintext = create_access_token(
        &gateway,
        admin.id,
        ScopeMode::Selected,
        &[issues.id, calendar.id],
    )
    .await
    .plaintext;

    let response = gateway
        .post_message(
            &plaintext,
            tool_call_request(
                "tool_search",
                Some(json!({ "mcp": "issues", "query": "create issue", "limit": 1 })),
            ),
            &[("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let result = result_of(&response);
    let found = &result["structuredContent"];

    assert_eq!(found["mcp"]["slug"], "issues");
    assert_eq!(found["tools"].as_array().unwrap().len(), 1);
    assert_eq!(found["tools"][0]["name"], "create_issue");
    assert!(found["tools"][0]["inputSchema"].get("properties").is_some());
    assert_eq!(
        result_text(&result),
        r#"{"mcp":{"name":"Issues","slug":"issues","description":null,"status":"ready"},"query":"create issue","tools":[{"name":"create_issue","description":"Create a new project issue","inputSchema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}}]}"#
    );
    let requests = gateway.upstreams.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.host == "issues.example")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == "tools/list")
    );
    let handshake = requests
        .iter()
        .find(|request| request.method == "initialize")
        .unwrap();
    assert_eq!(
        handshake.params["clientInfo"]["version"],
        mymcps_core::VERSION
    );
    // A search calls no tool, and is not logged as one.
    gateway.flush().await;
    assert!(gateway.call_logs().await.is_empty());
}

#[tokio::test]
async fn answers_a_search_it_cannot_make_without_saying_why() {
    let gateway = TestGateway::new(|message| match message.method.as_str() {
        "tools/list" => Reply::Error("Bearer very-secret-token was refused".into()),
        _ => Reply::Result(json!({})),
    })
    .await;
    let admin = create_admin(&gateway).await;
    let issues = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[issues.id])
        .await
        .plaintext;
    let search = |mcp: &'static str| {
        gateway.rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "mcp": mcp, "query": "issue" } }),
            "lazy",
        )
    };

    let unreachable = search("issues").await;
    assert_eq!(unreachable["isError"], true);
    assert_eq!(
        result_text(&unreachable),
        "Unable to search tools for MCP \"issues\""
    );

    let not_allowed = search("calendar").await;
    assert_eq!(not_allowed["isError"], true);
    assert_eq!(
        result_text(&not_allowed),
        "MCP \"calendar\" is not available to this access token"
    );

    let unknown = gateway
        .rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "issues__create_issue", "arguments": {} }),
            "lazy",
        )
        .await;
    assert_eq!(unknown["isError"], true);
    assert_eq!(result_text(&unknown), "Invalid lazy gateway tool name");
    assert!(gateway.call_logs().await.is_empty());
}

#[tokio::test]
async fn calls_an_allowed_upstream_tool_and_logs_its_real_target() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let issues = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[issues.id])
        .await
        .plaintext;

    let response = gateway
        .post_message(
            &plaintext,
            tool_call_request(
                "call_tool",
                Some(json!({
                    "mcp": "issues",
                    "tool": "create_issue",
                    "arguments": { "title": "Lazy discovery works" },
                })),
            ),
            &[("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    gateway.flush().await;

    assert!(result_of(&response).to_string().contains("create_issue"));
    assert_eq!(
        gateway.upstreams.calls(),
        [json!({ "name": "create_issue", "arguments": { "title": "Lazy discovery works" } })]
    );
    let logs = gateway.call_logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].mcp_slug.as_deref(), Some("issues"));
    assert_eq!(logs[0].requested_tool_name, "issues__create_issue");
    assert_eq!(logs[0].tool_name.as_deref(), Some("create_issue"));
}

#[tokio::test]
async fn preserves_eager_namespaced_discovery_when_the_header_is_absent() {
    let gateway = TestGateway::new(upstreams).await;
    let admin = create_admin(&gateway).await;
    let issues = create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::Selected, &[issues.id])
        .await
        .plaintext;

    let response = gateway
        .post_message(&plaintext, tools_list_request(), &[])
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let result = result_of(&response);

    assert_eq!(
        tool_names(&result),
        ["issues__create_issue", "issues__list_issues"]
    );
    assert_eq!(
        result["tools"][0],
        json!({
            "name": "issues__create_issue",
            "description": "Create a new project issue",
            "inputSchema": {
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            },
        })
    );
    assert!(
        gateway
            .upstreams
            .methods()
            .contains(&"tools/list".to_owned())
    );
}

#[tokio::test]
async fn names_a_tool_that_says_nothing_of_itself_after_its_mcp() {
    let gateway = TestGateway::new(listing(&[("echo", None)])).await;
    let admin = create_admin(&gateway).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let listed = gateway
        .rpc(&plaintext, "tools/list", json!({}), "eager")
        .await;

    assert_eq!(
        listed["tools"],
        json!([{
            "name": "issues__echo",
            "description": "Tool echo from issues",
            "inputSchema": { "type": "object" },
        }])
    );
}
