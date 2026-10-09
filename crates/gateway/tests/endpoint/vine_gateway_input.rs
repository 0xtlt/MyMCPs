//! `tests/functional/vine_gateway_input.spec.ts`: what agents send to the
//! gateway apart from JSON-RPC itself, read through `/mcp`.

use http::StatusCode;
use mymcps_core::models::{
    CallErrorCategory, CallOutcome, GatewayToolMode, InstanceSetting, ScopeMode,
};
use serde_json::{Value, json};

use crate::support::mcp::*;
use crate::support::*;

const MCP_MESSAGE: &str = "mcp must be a non-empty MCP slug of at most 120 characters";
const QUERY_MESSAGE: &str = "query must be non-empty and at most 200 characters";
const LIMIT_MESSAGE: &str = "limit must be an integer between 1 and 20";
const TOOL_MESSAGE: &str = "tool must be a non-empty upstream tool name of at most 128 characters";
const ARGUMENTS_MESSAGE: &str = "arguments must be an object when provided";

fn tools_list() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })
}

fn tool_call(name: &str, arguments: Option<Value>) -> Value {
    let mut params = json!({ "name": name });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params })
}

/// One upstream MCP that answers every tool call with `called`.
fn issues(message: &UpstreamMessage) -> Reply {
    Reply::Result(match message.method.as_str() {
        "tools/list" => json!({
            "tools": [
                { "name": "create_issue", "description": "Create an issue", "inputSchema": { "type": "object" } },
                { "name": "list_issues", "description": "List issues", "inputSchema": { "type": "object" } },
            ],
        }),
        _ => json!({ "content": [{ "type": "text", "text": "called" }] }),
    })
}

/// A gateway with the `issues` MCP, and the token of an agent that may use it.
async fn issues_gateway() -> (TestMcpGateway, String) {
    let gateway = TestMcpGateway::new(issues).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_http_mcp(db, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::Selected, &[mcp.id])
        .await
        .plaintext;
    (gateway, plaintext)
}

async fn call_tool(
    gateway: &TestMcpGateway,
    plaintext: &str,
    name: &str,
    arguments: Option<Value>,
    mode: &str,
) -> Value {
    let response = gateway
        .post(
            plaintext,
            tool_call(name, arguments),
            &[("x-mymcps-tool-mode", mode)],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    response.result()
}

#[tokio::test]
async fn falls_back_to_the_instance_tool_mode_for_a_blank_header_and_refuses_unknown_modes() {
    let gateway = TestMcpGateway::offline().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.gateway_tool_mode = GatewayToolMode::Lazy;
    settings.save(&**db).await.unwrap();
    let listed = async |headers: &[(&str, &str)]| {
        let response = gateway.post(&plaintext, tools_list(), headers).await;
        assert_eq!(response.status, StatusCode::OK);
        response.result()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let lazy_tools = ["list_mcps", "tool_search", "call_tool"];

    assert_eq!(listed(&[]).await, lazy_tools);
    assert_eq!(listed(&[("x-mymcps-tool-mode", "")]).await, lazy_tools);
    assert_eq!(listed(&[("x-mymcps-tool-mode", "   ")]).await, lazy_tools);
    assert_eq!(listed(&[("x-mymcps-tool-mode", "LAZY")]).await, lazy_tools);
    assert!(
        listed(&[("x-mymcps-tool-mode", " Eager ")])
            .await
            .is_empty()
    );

    let refused = async |headers: &[(&str, &str)]| {
        let response = gateway.post(&plaintext, tools_list(), headers).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{headers:?}");
        assert_eq!(
            response.json(),
            json!({
                "error": "invalid_tool_mode",
                "message": "X-MyMCPs-Tool-Mode must be either eager or lazy",
            })
        );
    };
    for mode in [
        "sometimes",
        "lazy, eager",
        "eager lazy",
        "\"lazy\"",
        "lazyy",
    ] {
        refused(&[("x-mymcps-tool-mode", mode)]).await;
    }
    // A header sent twice reads as its values joined, which is no mode.
    refused(&[
        ("x-mymcps-tool-mode", "lazy"),
        ("x-mymcps-tool-mode", "lazy"),
    ])
    .await;
}

#[tokio::test]
async fn tells_the_agent_which_tool_search_argument_to_correct() {
    let (gateway, plaintext) = issues_gateway().await;
    let search = async |arguments: Option<Value>| {
        call_tool(&gateway, &plaintext, "tool_search", arguments, "lazy").await
    };

    for (arguments, message) in [
        (None, MCP_MESSAGE),
        (Some(json!({})), MCP_MESSAGE),
        (Some(json!({ "mcp": 5, "query": "issue" })), MCP_MESSAGE),
        (
            Some(json!({ "mcp": ["issues"], "query": "issue" })),
            MCP_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "x".repeat(121), "query": "issue" })),
            MCP_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "", "query": "", "limit": 0 })),
            MCP_MESSAGE,
        ),
        (Some(json!({ "mcp": "issues" })), QUERY_MESSAGE),
        (
            Some(json!({ "mcp": "issues", "query": null })),
            QUERY_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "q".repeat(201), "limit": 0 })),
            QUERY_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "issue", "limit": 0 })),
            LIMIT_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "issue", "limit": 21 })),
            LIMIT_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "issue", "limit": 1.5 })),
            LIMIT_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "issue", "limit": "5" })),
            LIMIT_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "query": "issue", "limit": null })),
            LIMIT_MESSAGE,
        ),
    ] {
        let result = search(arguments.clone()).await;
        assert_eq!(result["isError"], true, "{arguments:?}");
        assert_eq!(result_text(&result), message, "{arguments:?}");
    }

    let found = search(Some(
        json!({ "mcp": " issues ", "query": " issue ", "surplus": true }),
    ))
    .await;
    assert!(found.get("isError").is_none());
    assert_eq!(found["structuredContent"]["query"], "issue");
    assert_eq!(
        found["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let limited = search(Some(
        json!({ "mcp": "issues", "query": "issue", "limit": 1 }),
    ))
    .await;
    assert_eq!(
        limited["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn tells_the_agent_which_call_tool_argument_to_correct_and_logs_the_refusal() {
    let (gateway, plaintext) = issues_gateway().await;
    let call = async |arguments: Option<Value>| {
        call_tool(&gateway, &plaintext, "call_tool", arguments, "lazy").await
    };
    let refusals = [
        (None, MCP_MESSAGE),
        (Some(json!({ "tool": "create_issue" })), MCP_MESSAGE),
        (
            Some(json!({ "mcp": "", "tool": "", "arguments": [] })),
            MCP_MESSAGE,
        ),
        (Some(json!({ "mcp": "issues" })), TOOL_MESSAGE),
        (
            Some(json!({ "mcp": "issues", "tool": "t".repeat(129), "arguments": [] })),
            TOOL_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "tool": "create_issue", "arguments": null })),
            ARGUMENTS_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "tool": "create_issue", "arguments": [] })),
            ARGUMENTS_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "tool": "create_issue", "arguments": "title" })),
            ARGUMENTS_MESSAGE,
        ),
        (
            Some(json!({ "mcp": "issues", "tool": "create_issue", "arguments": 0 })),
            ARGUMENTS_MESSAGE,
        ),
    ];

    for (arguments, message) in &refusals {
        let result = call(arguments.clone()).await;
        assert_eq!(result["isError"], true, "{arguments:?}");
        assert_eq!(result_text(&result), *message, "{arguments:?}");
    }
    assert!(gateway.upstreams.calls().is_empty());

    call(Some(json!({ "mcp": " issues ", "tool": " create_issue " }))).await;
    call(Some(json!({
        "mcp": "issues",
        "tool": "create_issue",
        "arguments": { "title": "Hello", "labels": ["a", { "deep": [1, null] }], "count": 0 },
    })))
    .await;
    assert_eq!(
        gateway.upstreams.calls(),
        [
            json!({ "name": "create_issue", "arguments": {} }),
            json!({
                "name": "create_issue",
                "arguments": { "title": "Hello", "labels": ["a", { "deep": [1, null] }], "count": 0 },
            }),
        ]
    );

    gateway.gateway.call_log.flush().await;
    let logs = call_logs(gateway.db()).await;
    let mut expected: Vec<(CallOutcome, Option<CallErrorCategory>, Option<&str>)> = refusals
        .iter()
        .map(|(_, message)| {
            (
                CallOutcome::Error,
                Some(CallErrorCategory::InvalidTool),
                Some(*message),
            )
        })
        .collect();
    expected.extend([(CallOutcome::Success, None, None); 2]);
    assert_eq!(
        logs.iter()
            .map(|log| (
                log.outcome,
                log.error_category,
                log.error_summary.as_deref()
            ))
            .collect::<Vec<_>>(),
        expected
    );
    // A refused call is logged under the name the agent called.
    assert_eq!(logs[0].requested_tool_name, "call_tool");
    assert_eq!(logs[0].tool_name, None);
    assert_eq!(logs[0].mcp_id, None);
    assert_eq!(logs[9].requested_tool_name, "issues__create_issue");
}

#[tokio::test]
async fn splits_an_eager_tool_name_at_its_first_separator_and_refuses_names_without_a_slug() {
    let (gateway, plaintext) = issues_gateway().await;
    let call = async |name: &str| {
        call_tool(
            &gateway,
            &plaintext,
            name,
            Some(json!({ "title": "Hello" })),
            "eager",
        )
        .await
    };

    for name in [
        "__create_issue",
        "___create_issue",
        "issues_create_issue",
        "issues",
    ] {
        let result = call(name).await;
        assert_eq!(result["isError"], true, "{name}");
        assert_eq!(result_text(&result), "Invalid tool name", "{name}");
    }
    for name in [
        "missing__create_issue",
        "_issues__create_issue",
        "issues __tool",
    ] {
        let result = call(name).await;
        assert_eq!(result["isError"], true, "{name}");
        assert_eq!(
            result_text(&result),
            "MCP not allowed for this token",
            "{name}"
        );
    }

    for name in [
        "issues__create_issue",
        "issues__create__issue",
        "issues___x",
    ] {
        assert!(call(name).await.get("isError").is_none(), "{name}");
    }
    // A tool without a name is one no choice can be saved for on the Tool
    // approvals page: it asks for a person, and a tool that asks is looked
    // up first. The Node app sent the call on.
    let unnamed = call("issues__").await;
    assert_eq!(unnamed["isError"], true);
    assert_eq!(result_text(&unnamed), "Issues has no tool named \"\"");
    assert_eq!(
        gateway
            .upstreams
            .calls()
            .iter()
            .map(|sent| sent["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["create_issue", "create__issue", "_x"]
    );
}
