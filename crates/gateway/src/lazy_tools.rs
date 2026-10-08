//! The lazy tool mode of the gateway: instead of every tool of every MCP,
//! an agent is given three tools to list the MCPs it may use, to search the
//! tools of one, and to call one of those.

use std::sync::LazyLock;

use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed};
use mymcps_core::models::{GatewayToolMode, Mcp};
use mymcps_vine as vine;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::validators::gateway::{CALL_TOOL, GATEWAY_TOOL_MODE, TOOL_SEARCH};

/// A tool of an upstream MCP, as far as this module reads it: a name, a
/// description when it has one, and the schema of its arguments. The type
/// that listing an MCP returns implements it.
pub trait ToolDefinition {
    fn name(&self) -> &str;
    fn description(&self) -> Option<&str>;
    fn input_schema(&self) -> &Value;
}

impl<T: ToolDefinition + ?Sized> ToolDefinition for &T {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> Option<&str> {
        (**self).description()
    }

    fn input_schema(&self) -> &Value {
        (**self).input_schema()
    }
}

impl ToolDefinition for mymcps_upstream::UpstreamTool {
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

/// What an agent is told about an MCP it may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpCatalogEntry {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub status: String,
}

/// The tools of the lazy gateway, as `tools/list` answers them: each is an
/// object with `name`, `description` and `inputSchema`.
pub static LAZY_GATEWAY_TOOLS: LazyLock<Vec<Map<String, Value>>> = LazyLock::new(|| {
    let tools = json!([
        {
            "name": "list_mcps",
            "description":
                "List the MCP servers available to this access token. Use a returned slug with tool_search and call_tool.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        },
        {
            "name": "tool_search",
            "description":
                "Search tool definitions from one available MCP server without invoking them. Select the MCP by slug from the server catalog or list_mcps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mcp": {
                        "type": "string",
                        "description": "Exact MCP slug from the available MCP catalog.",
                    },
                    "query": {
                        "type": "string",
                        "description": "Words describing the tool capability to find.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "default": 10,
                        "description": "Maximum number of matching tool definitions to return.",
                    },
                },
                "required": ["mcp", "query"],
                "additionalProperties": false,
            },
        },
        {
            "name": "call_tool",
            "description":
                "Invoke an exact upstream tool. Use the MCP slug and tool name returned by tool_search; arguments must match that tool input schema.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mcp": {
                        "type": "string",
                        "description": "Exact MCP slug from the available MCP catalog.",
                    },
                    "tool": {
                        "type": "string",
                        "description": "Exact upstream tool name returned by tool_search.",
                    },
                    "arguments": {
                        "type": "object",
                        "description": "Arguments matching the selected upstream tool input schema.",
                        "additionalProperties": true,
                    },
                },
                "required": ["mcp", "tool"],
                "additionalProperties": false,
            },
        },
    ]);
    match tools {
        Value::Array(tools) => tools
            .into_iter()
            .filter_map(|tool| match tool {
                Value::Object(tool) => Some(tool),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
});

/// The tool mode a request asks for, or `None` when its header names no mode
/// the gateway has. `value` is the `X-MyMCPs-Tool-Mode` header, and
/// `default_mode` the mode of the instance, for a request that sends none.
pub fn parse_gateway_tool_mode(
    value: Option<&str>,
    default_mode: GatewayToolMode,
) -> Option<GatewayToolMode> {
    let header = value.map(Value::from);
    match GATEWAY_TOOL_MODE.validate_opt(&header) {
        Ok(Some(mode)) => mode.as_str().and_then(GatewayToolMode::parse),
        Ok(None) => Some(default_mode),
        Err(_) => None,
    }
}

pub fn mcp_catalog<'a>(mcps: impl IntoIterator<Item = &'a Mcp>) -> Vec<McpCatalogEntry> {
    mcps.into_iter()
        .map(|mcp| McpCatalogEntry {
            name: mcp.name.clone(),
            slug: mcp.slug.clone(),
            description: mcp.description.clone(),
            status: mcp.status.to_string(),
        })
        .collect()
}

fn single_line(value: &str) -> String {
    value
        .split(vine::js::is_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the gateway tells an agent when a lazy session starts: the MCPs it
/// may use, one per line.
pub fn lazy_gateway_instructions<'a>(mcps: impl IntoIterator<Item = &'a Mcp>) -> String {
    let mut lines = vec!["Available MCPs:".to_string()];
    let catalog = mcp_catalog(mcps);
    if catalog.is_empty() {
        lines.push("- None available for this access token.".to_string());
    }
    for mcp in catalog {
        let description = match mcp.description.as_deref() {
            Some(description) if !description.is_empty() => single_line(description),
            _ => single_line(&mcp.name),
        };
        lines.push(format!("- {}: {description}", mcp.slug));
    }
    lines.push(String::new());
    lines.push(
        "Use list_mcps to get the up-to-date catalog, then tool_search to discover an MCP's tools."
            .to_string(),
    );
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchInput {
    pub mcp: String,
    pub query: String,
    pub limit: usize,
}

/// The arguments of `tool_search`, or the sentence that tells the agent which
/// argument to correct: the first one that is wrong.
pub fn parse_tool_search_input(args: Option<&Value>) -> Result<ToolSearchInput, String> {
    let input = TOOL_SEARCH
        .validate(arguments_or_none(args))
        .map_err(first_message)?;
    Ok(ToolSearchInput {
        mcp: text(&input, "mcp"),
        query: text(&input, "query"),
        limit: input
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or_default(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallToolInput<'a> {
    pub mcp: String,
    pub tool: String,
    /// The upstream tool receives the object the agent sent, not a copy of
    /// it that validation went through.
    pub arguments: Option<&'a Map<String, Value>>,
}

/// The arguments of `call_tool`, or the sentence that tells the agent which
/// argument to correct: the first one that is wrong.
pub fn parse_call_tool_input(args: Option<&Value>) -> Result<CallToolInput<'_>, String> {
    let input = CALL_TOOL
        .validate(arguments_or_none(args))
        .map_err(first_message)?;
    Ok(CallToolInput {
        mcp: text(&input, "mcp"),
        tool: text(&input, "tool"),
        arguments: args
            .and_then(|args| args.get("arguments"))
            .and_then(Value::as_object),
    })
}

static NO_ARGUMENTS: LazyLock<Value> = LazyLock::new(|| json!({}));

/// A call without arguments is one with none set.
fn arguments_or_none(args: Option<&Value>) -> &Value {
    args.filter(|args| !args.is_null()).unwrap_or(&NO_ARGUMENTS)
}

fn first_message(error: vine::ValidationError) -> String {
    error
        .messages
        .into_iter()
        .next()
        .map(|first| first.message)
        .unwrap_or_default()
}

fn text(input: &Value, name: &str) -> String {
    input
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn search_tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|character: char| !matches!(character, 'a'..='z' | '0'..='9'))
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn tool_search_score(tool: &impl ToolDefinition, normalized_query: &str, tokens: &[String]) -> u32 {
    let name = tool.name().to_lowercase();
    let description = tool.description().unwrap_or_default().to_lowercase();
    let mut score = 0;

    if name == normalized_query {
        score += 1_000;
    } else if name.starts_with(normalized_query) {
        score += 600;
    } else if name.contains(normalized_query) {
        score += 400;
    }
    if description.contains(normalized_query) {
        score += 200;
    }

    for token in tokens {
        if name == *token {
            score += 100;
        } else if name.contains(token.as_str()) {
            score += 50;
        }
        if description.contains(token.as_str()) {
            score += 10;
        }
    }

    score
}

/// The order of `String.prototype.localeCompare`, in which the Node app
/// listed tools of equal score: letters before their capitals, punctuation
/// before digits, digits before letters.
static NAME_ORDER: LazyLock<Option<CollatorBorrowed<'static>>> =
    LazyLock::new(|| Collator::try_new(Default::default(), CollatorOptions::default()).ok());

/// The tools that match the query, best first, `limit` at most.
pub fn search_upstream_tools<'a, T: ToolDefinition>(
    tools: &'a [T],
    query: &str,
    limit: usize,
) -> Vec<&'a T> {
    let tokens = search_tokens(query);
    let normalized_query = query.to_lowercase();
    let mut matches: Vec<(&T, u32)> = tools
        .iter()
        .map(|tool| (tool, tool_search_score(tool, &normalized_query, &tokens)))
        .filter(|(_, score)| *score > 0)
        .collect();
    matches.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| match NAME_ORDER.as_ref() {
                Some(order) => order.compare(left.name(), right.name()),
                None => left.name().cmp(right.name()),
            })
    });
    matches
        .into_iter()
        .take(limit)
        .map(|(tool, _)| tool)
        .collect()
}
