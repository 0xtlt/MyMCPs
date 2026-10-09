//! Which tools of an MCP run when an agent calls them, and which wait for a
//! person. It works the same for every MCP: the gateway holds the call before
//! it reaches the MCP. (`mcp_tools_controller.ts`)

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::get;
use http::{HeaderMap, StatusCode};
use mymcps_core::models::Mcp;
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_gateway::validators::approvals::UPDATE_TOOL_APPROVALS;
use mymcps_upstream::approvals::{
    MAX_CHOOSABLE_NAME_UNITS, ToolApprovalChoice, ToolApprovalMode, assign_tool_approvals,
    can_be_chosen, default_tool_approval, saved_tool_approvals,
};
use mymcps_vine::ValidationError;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{AppError, is_fetch};
use crate::forms::refusal;
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::{fragment, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::route_params::record_id;
use crate::views::mcp_tools::{McpToolsPage, Notice, ToolRow, mcp_tools_page, tools_fragment};
use crate::views::shell::PageContext;

const MAX_DESCRIPTION_CHARS: usize = 600;

const NOT_FOUND: &str = "MCP not found";

const SAVED: &str = "Tool approvals saved";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new().route("/mcps/{id}/tools", get(index).put(update)),
        ..Default::default()
    }
}

async fn find_mcp(state: &AppState, id: &str) -> Result<Option<Mcp>, sqlx::Error> {
    match record_id(id) {
        Some(id) => Mcp::find(&*state.core.db, id).await,
        None => Ok(None),
    }
}

fn not_found(session: &Session, headers: &HeaderMap, query: Option<&str>) -> Response {
    session.flash("error", NOT_FOUND);
    navigate(headers, &with_query("/mcps", query))
}

/// The first `max` UTF-16 units of a text, as `String.prototype.slice` cut
/// it, without the half of a character the cut would leave.
fn first_units(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

struct ListedTool {
    name: String,
    description: Option<String>,
}

/// The tools an MCP has. A built-in MCP knows them without connecting, so they
/// can be set up before its account is. Any other MCP is asked for its list.
async fn list_tools(state: &AppState, mcp: &mut Mcp) -> (Vec<ListedTool>, Option<String>) {
    if let Some(definition) = state.upstream.builtin_mcp(mcp.builtin_key.as_deref()) {
        let tools = definition
            .tools()
            .into_iter()
            .map(|tool| ListedTool {
                name: tool.name.to_string(),
                description: Some(tool.description.to_string()),
            })
            .collect();
        return (tools, None);
    }

    match state.upstream.probe(mcp).await {
        Ok(tools) => {
            let tools = tools
                .into_iter()
                .map(|tool| ListedTool {
                    name: tool.name,
                    description: tool.description,
                })
                .collect();
            (tools, None)
        }
        Err(error) => {
            let diagnostic =
                sanitize_mcp_diagnostic(&state.core.encryption, &error.to_string(), mcp);
            let diagnostic = if diagnostic.is_empty() {
                "Unknown error".to_string()
            } else {
                diagnostic
            };
            (Vec::new(), Some(diagnostic))
        }
    }
}

/// What the page shows of an MCP: its tools, each with what is saved for it.
async fn tools_page(state: &AppState, mcp: &mut Mcp) -> McpToolsPage {
    let (tools, list_error) = list_tools(state, mcp).await;
    let builtins = state.upstream.builtins();
    let saved = saved_tool_approvals(mcp);
    // A tool with a saved choice stays listed while its MCP cannot be
    // reached, or after the MCP dropped it, so the choice can still be seen.
    let mut unlisted: Vec<&str> = saved
        .iter()
        .flat_map(|saved| saved.names())
        .filter(|name| !tools.iter().any(|tool| tool.name == *name))
        .collect();
    // As `Array.prototype.sort` orders names.
    unlisted.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));

    let row = |name: &str, description: Option<&str>, is_listed: bool| {
        let default_mode = default_tool_approval(builtins, mcp, name);
        let can_be_chosen = can_be_chosen(name);
        ToolRow {
            // A name no choice is saved for is only shown, and not at any length.
            name: if can_be_chosen {
                name.to_string()
            } else {
                first_units(name, MAX_CHOOSABLE_NAME_UNITS).to_string()
            },
            description: description
                .map(|description| first_units(description, MAX_DESCRIPTION_CHARS).to_string()),
            mode: match &saved {
                Some(saved) => saved.get(name).unwrap_or(default_mode),
                None => ToolApprovalMode::Ask,
            },
            default_mode,
            is_listed,
            can_be_chosen,
        }
    };
    let rows = tools
        .iter()
        .map(|tool| row(&tool.name, tool.description.as_deref(), true))
        .chain(unlisted.iter().map(|name| row(name, None, false)))
        .collect();

    McpToolsPage {
        mcp_id: mcp.id,
        mcp_name: mcp.name.clone(),
        tools: rows,
        list_error,
        saved_unreadable: saved.is_none(),
    }
}

/// `GET /mcps/{id}/tools`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, uri.query()));
    };
    let tools = tools_page(&state, &mut mcp).await;
    Ok(page(mcp_tools_page(&context, &tools)))
}

#[derive(Deserialize)]
struct ToolApprovals {
    tools: Vec<ToolApprovalChoice>,
}

/// The list a form sends as `tools[0][name]`, `tools[0][mode]`, and so on,
/// arrives keyed by index once it is longer than the body parser keeps as a
/// list. Read it as the list it is, in the order of its indexes.
fn listed_by_index(input: &mut Map<String, Value>) {
    let Some(Value::Object(entries)) = input.get("tools") else {
        return;
    };
    let indexes: Option<Vec<usize>> = entries.keys().map(|key| key.parse().ok()).collect();
    let Some(mut indexes) = indexes else {
        return;
    };
    indexes.sort_unstable();
    let tools = indexes
        .iter()
        .filter_map(|index| entries.get(&index.to_string()).cloned())
        .collect();
    input.insert("tools".into(), Value::Array(tools));
}

/// The form says how many tools it holds before it lists them. Fewer than
/// that arrived when the body parser stopped reading: saving what is there
/// would let the tools it did not read run without asking.
fn arrived_whole(input: &Map<String, Value>) -> Result<(), ValidationError> {
    let expected = match input.get("toolCount") {
        Some(Value::String(count)) => count.parse::<usize>().ok(),
        Some(Value::Number(count)) => count.as_u64().and_then(|count| count.try_into().ok()),
        _ => None,
    };
    let arrived = input
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    match expected {
        Some(expected) if arrived < expected => Err(ValidationError::single(
            "tools",
            "array.minLength",
            format!(
                "Only {arrived} of the {expected} tools of this page reached the server, so nothing was saved"
            ),
        )),
        _ => Ok(()),
    }
}

/// The choice submitted for each tool of the page.
fn submitted_choices(
    mut input: Map<String, Value>,
) -> Result<Vec<ToolApprovalChoice>, mymcps_vine::Error> {
    listed_by_index(&mut input);
    arrived_whole(&input).map_err(mymcps_vine::Error::Validation)?;
    let submitted: ToolApprovals = UPDATE_TOOL_APPROVALS.validate_as(&Value::Object(input))?;
    Ok(submitted.tools)
}

/// `PUT /mcps/{id}/tools`
pub async fn update(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, uri.query()));
    };
    let page_path = format!("/mcps/{}/tools", mcp.id);

    let choices = match submitted_choices(input) {
        Ok(choices) => choices,
        Err(error) => {
            let error = refusal(error)?;
            // Only the first refusal is said: a session cannot hold one for
            // each of two thousand tools.
            let (field, message) = error
                .messages
                .first()
                .map(|first| (first.field.clone(), first.message.clone()))
                .unwrap_or_default();
            if is_fetch(&headers) {
                let tools = tools_page(&state, &mut mcp).await;
                return Ok(fragment(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    tools_fragment(&context, &tools, Some(Notice::Refused(&message))),
                ));
            }
            session.flash(
                "errors",
                Map::from_iter([(field, Value::from(message.clone()))]),
            );
            session.flash("error", &message);
            return Ok(redirect_back(&headers, &page_path));
        }
    };

    assign_tool_approvals(state.upstream.builtins(), &mut mcp, &choices);
    mcp.save(&*state.core.db).await?;

    // The page's script puts the saved form in place, where the person was.
    if is_fetch(&headers) {
        let tools = tools_page(&state, &mut mcp).await;
        return Ok(fragment(
            StatusCode::OK,
            tools_fragment(&context, &tools, Some(Notice::Saved(SAVED))),
        ));
    }
    session.flash("success", SAVED);
    Ok(redirect_to(&with_query(&page_path, uri.query())))
}
