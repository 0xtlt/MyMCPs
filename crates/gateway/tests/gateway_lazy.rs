//! `tests/functional/gateway_lazy.spec.ts`, for what the lazy gateway does
//! without an upstream: the tools it lists, what it tells an agent about its
//! MCPs, and how it ranks the tools of one.

mod support;

use std::time::Duration;

use mymcps_core::models::{
    CallOutcome, GatewayToolMode, InstanceSetting, Mcp, McpStatus, ScopeMode,
};
use mymcps_gateway::access_token;
use mymcps_gateway::call_log::McpCallLogInput;
use mymcps_gateway::lazy_tools::{
    LAZY_GATEWAY_TOOLS, ToolDefinition, lazy_gateway_instructions, mcp_catalog,
    parse_call_tool_input, parse_gateway_tool_mode, parse_tool_search_input, search_upstream_tools,
};
use serde_json::{Value, json};
use support::*;

/// A tool as listing an upstream returns it.
#[derive(Debug, Clone, PartialEq)]
struct ListedTool {
    name: String,
    description: Option<String>,
    input_schema: Value,
}

impl ToolDefinition for ListedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn input_schema(&self) -> &Value {
        &self.input_schema
    }
}

fn tool(name: &str, description: Option<&str>) -> ListedTool {
    ListedTool {
        name: name.to_owned(),
        description: description.map(str::to_owned),
        input_schema: json!({ "type": "object" }),
    }
}

fn names<'a>(tools: impl IntoIterator<Item = &'a ListedTool>) -> Vec<&'a str> {
    tools.into_iter().map(|tool| tool.name.as_str()).collect()
}

/// The tools of the `issues` upstream of the TypeScript tests.
fn issue_tools() -> Vec<ListedTool> {
    vec![
        ListedTool {
            name: "create_issue".into(),
            description: Some("Create a new project issue".into()),
            input_schema: json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            }),
        },
        tool("list_issues", Some("List project work items")),
    ]
}

#[tokio::test]
async fn rejects_an_unsupported_tool_mode_header() {
    let gateway = TestGateway::new().await;
    let settings = InstanceSetting::current(&**gateway.db()).await.unwrap();

    assert_eq!(
        parse_gateway_tool_mode(Some("sometimes"), settings.gateway_tool_mode),
        None
    );
}

#[tokio::test]
async fn uses_the_instance_default_when_the_header_is_absent_and_lets_the_header_override_it() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    assert_eq!(
        parse_gateway_tool_mode(None, settings.gateway_tool_mode),
        Some(GatewayToolMode::Eager)
    );

    settings.gateway_tool_mode = GatewayToolMode::Lazy;
    settings.save(&**db).await.unwrap();
    let settings = gateway.call_log.settings().await.unwrap();

    assert_eq!(
        parse_gateway_tool_mode(None, settings.gateway_tool_mode),
        Some(GatewayToolMode::Lazy)
    );
    assert_eq!(
        parse_gateway_tool_mode(Some("eager"), settings.gateway_tool_mode),
        Some(GatewayToolMode::Eager)
    );
}

#[tokio::test]
async fn shares_only_allowed_mcp_summaries_during_lazy_initialization() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let allowed = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Issue Tracker".into();
        mcp.slug = "issues".into();
        mcp.http_url = Some("https://issues.example/mcp".into());
        mcp.status = McpStatus::Ready;
        mcp.description = Some("Project issues\nwithout exposing credentials".into());
    })
    .await;
    create_mcp(db, admin.id, |mcp| {
        mcp.name = "Private Calendar".into();
        mcp.slug = "calendar".into();
        mcp.http_url = Some("https://calendar.example/mcp".into());
        mcp.last_error = Some("Bearer top-secret-value".into());
    })
    .await;
    let created = create_access_token(db, admin.id, ScopeMode::Selected, &[allowed.id]).await;

    assert_eq!(
        parse_gateway_tool_mode(Some(" LaZy "), GatewayToolMode::Eager),
        Some(GatewayToolMode::Lazy)
    );
    let mcps = access_token::resolve_allowed_mcps(db, &created.token)
        .await
        .unwrap();
    let instructions = lazy_gateway_instructions(&mcps);

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
}

#[test]
fn describes_an_mcp_by_its_name_when_it_has_no_description() {
    let mcp = |slug: &str, name: &str, description: Option<&str>| Mcp {
        slug: slug.into(),
        name: name.into(),
        description: description.map(str::to_owned),
        ..Default::default()
    };

    assert_eq!(
        lazy_gateway_instructions(&[
            mcp("issues", "  Issue\tTracker ", None),
            mcp("mail", "Mail", Some("")),
            mcp("notes", "Notes", Some(" Take \r\n\n notes\u{a0}here ")),
        ]),
        [
            "Available MCPs:",
            "- issues: Issue Tracker",
            "- mail: Mail",
            "- notes: Take notes here",
            "",
            "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools.",
        ]
        .join("\n")
    );
    assert_eq!(
        lazy_gateway_instructions(&[]),
        [
            "Available MCPs:",
            "- None available for this access token.",
            "",
            "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools.",
        ]
        .join("\n")
    );
}

#[test]
fn lists_only_lazy_gateway_tools() {
    assert_eq!(
        LAZY_GATEWAY_TOOLS
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["list_mcps", "tool_search", "call_tool"]
    );
    for tool in LAZY_GATEWAY_TOOLS.iter() {
        assert_eq!(
            tool.keys().collect::<Vec<_>>(),
            ["name", "description", "inputSchema"]
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    // As the Node app sent them, byte for byte.
    assert_eq!(
        Value::from(LAZY_GATEWAY_TOOLS.clone()).to_string(),
        r#"[{"name":"list_mcps","description":"List the MCP servers available to this access token. Use a returned slug with tool_search and call_tool.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},{"name":"tool_search","description":"Search tool definitions from one available MCP server without invoking them. Select the MCP by slug from the server catalog or list_mcps.","inputSchema":{"type":"object","properties":{"mcp":{"type":"string","description":"Exact MCP slug from the available MCP catalog."},"query":{"type":"string","description":"Words describing the tool capability to find."},"limit":{"type":"integer","minimum":1,"maximum":20,"default":10,"description":"Maximum number of matching tool definitions to return."}},"required":["mcp","query"],"additionalProperties":false}},{"name":"call_tool","description":"Invoke an exact upstream tool. Use the MCP slug and tool name returned by tool_search; arguments must match that tool input schema.","inputSchema":{"type":"object","properties":{"mcp":{"type":"string","description":"Exact MCP slug from the available MCP catalog."},"tool":{"type":"string","description":"Exact upstream tool name returned by tool_search."},"arguments":{"type":"object","description":"Arguments matching the selected upstream tool input schema.","additionalProperties":true}},"required":["mcp","tool"],"additionalProperties":false}}]"#
    );
}

#[tokio::test]
async fn returns_the_allowed_mcp_catalog_through_list_mcps() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let issues = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Issues".into();
        mcp.slug = "issues".into();
        mcp.http_url = Some("https://issues.example/mcp".into());
        mcp.description = Some("Issue tracking".into());
    })
    .await;
    create_mcp(db, admin.id, |mcp| mcp.name = "Not selected".into()).await;
    let created = create_access_token(db, admin.id, ScopeMode::Selected, &[issues.id]).await;

    let mcps = access_token::resolve_allowed_mcps(db, &created.token)
        .await
        .unwrap();
    let catalog = json!({ "mcps": mcp_catalog(&mcps) });

    assert_eq!(
        catalog.to_string(),
        r#"{"mcps":[{"name":"Issues","slug":"issues","description":"Issue tracking","status":"ready"}]}"#
    );
    assert_eq!(
        json!(mcp_catalog(&[Mcp {
            name: "Draft".into(),
            slug: "draft".into(),
            ..Default::default()
        }])),
        json!([{ "name": "Draft", "slug": "draft", "description": null, "status": "draft" }])
    );
}

#[test]
fn searches_the_tools_of_an_mcp_and_returns_matching_schemas() {
    let tools = issue_tools();
    let args = json!({ "mcp": "issues", "query": "create issue", "limit": 1 });
    let input = parse_tool_search_input(Some(&args)).unwrap();
    assert_eq!(input.mcp, "issues");

    let matches = search_upstream_tools(&tools, &input.query, input.limit);

    assert_eq!(names(matches.iter().copied()), ["create_issue"]);
    assert!(matches[0].input_schema().get("properties").is_some());
    assert_eq!(
        names(search_upstream_tools(&tools, &input.query, 10)),
        ["create_issue", "list_issues"]
    );
}

#[test]
fn ranks_tools_by_how_well_their_name_and_description_match() {
    let tools = [
        tool("list_events", Some("List calendar events")),
        tool("search", Some("Find an event by its title")),
        tool("event", None),
        tool("delete_event", Some("Delete an event")),
        tool("EVENT_LOG", Some("Read the log")),
        tool("unrelated", Some("Nothing to see")),
    ];

    // What the Node app answers. The whole name comes first; a name that
    // holds the query with a description that does too outranks a name that
    // only starts with it; a description alone comes last.
    assert_eq!(
        names(search_upstream_tools(&tools, "event", 10)),
        [
            "event",
            "delete_event",
            "list_events",
            "EVENT_LOG",
            "search"
        ]
    );
    assert_eq!(
        names(search_upstream_tools(&tools, "EVENT", 2)),
        ["event", "delete_event"]
    );
    // Each word of the query counts on its own.
    assert_eq!(
        names(search_upstream_tools(&tools, "delete, the event!", 10)),
        [
            "delete_event",
            "event",
            "EVENT_LOG",
            "list_events",
            "search"
        ]
    );
    assert_eq!(
        names(search_upstream_tools(&tools, "nothing", 10)),
        ["unrelated"]
    );
    assert_eq!(
        search_upstream_tools(&tools, "weather", 10),
        Vec::<&ListedTool>::new()
    );
    assert_eq!(
        search_upstream_tools(&tools, "event", 0),
        Vec::<&ListedTool>::new()
    );
}

/// The order Node 24 gives these names with `localeCompare`.
#[test]
fn lists_tools_of_equal_score_in_the_order_of_locale_compare() {
    let sorted = [
        "",
        " ",
        "_a",
        "-a",
        "!",
        "#",
        "~",
        "😀",
        "0",
        "10",
        "1a",
        "2",
        "9",
        "a",
        "A",
        "ä",
        "a b",
        "a_",
        "a-",
        "a:b",
        "a.b",
        "a@b",
        "a/b",
        "a😀",
        "a$b",
        "a1",
        "ab",
        "aB",
        "Ab",
        "AB",
        "ab10",
        "ab2",
        "b",
        "B",
        "create_event",
        "create_issue",
        "CREATE_ISSUE",
        "createIssue",
        "ǆ",
        "e",
        "é",
        "f",
        "i",
        "I",
        "İ",
        "ı",
        "list",
        "List",
        "list_issues",
        "list-issues",
        "list.issues",
        "list/issues",
        "list2",
        "listIssues",
        "resume",
        "resumé",
        "résumé",
        "ss",
        "ß",
        "z",
        "Z",
        "α",
        "я",
        "日本",
    ];
    let mut shuffled: Vec<&str> = sorted.to_vec();
    shuffled.reverse();
    shuffled.rotate_left(17);
    // Every tool scores the same: its description holds the query.
    let tools: Vec<ListedTool> = shuffled
        .into_iter()
        .map(|name| tool(name, Some("matching description")))
        .collect();

    assert_eq!(
        names(search_upstream_tools(&tools, "matching", 100)),
        sorted
    );
}

#[tokio::test]
async fn records_a_call_through_call_tool_under_its_real_target() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let issues = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Issues".into();
        mcp.slug = "issues".into();
        mcp.http_url = Some("https://issues.example/mcp".into());
    })
    .await;
    let created = create_access_token(db, admin.id, ScopeMode::Selected, &[issues.id]).await;

    let args = json!({
        "mcp": "issues",
        "tool": "create_issue",
        "arguments": { "title": "Lazy discovery works" },
    });
    let input = parse_call_tool_input(Some(&args)).unwrap();
    assert_eq!(
        input.arguments.cloned().map(Value::Object),
        Some(json!({ "title": "Lazy discovery works" }))
    );
    gateway.call_log.record(McpCallLogInput {
        access_token: created.token.clone(),
        mcp: Some(issues.clone()),
        requested_tool_name: format!("{}__{}", input.mcp, input.tool),
        tool_name: Some(input.tool.clone()),
        args: input.arguments.cloned().map(Value::Object),
        outcome: CallOutcome::Success,
        duration: Duration::from_millis(12),
        ..Default::default()
    });
    gateway.call_log.flush().await;

    let logs = call_logs(db).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].mcp_slug.as_deref(), Some("issues"));
    assert_eq!(logs[0].requested_tool_name, "issues__create_issue");
    assert_eq!(logs[0].tool_name.as_deref(), Some("create_issue"));
    assert_eq!(logs[0].duration_ms, 12);
}
