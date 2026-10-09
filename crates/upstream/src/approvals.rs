//! Which tools run when an agent calls them, and which wait for a person.

use mymcps_builtin::BuiltinRegistry;
use mymcps_core::models::Mcp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::http_client::UpstreamTool;
use crate::validators::SAVED_TOOL_APPROVALS_VALIDATOR;

/// `auto` runs the call. `ask` holds it until a person approves it in MyMCPs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolApprovalMode {
    Auto,
    Ask,
}

impl ToolApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

pub const APPROVAL_NOTE: &str = "Needs approval: the first call is not run and returns a link for a person to approve in MyMCPs. Once they have, call the tool again with the same arguments.";

/// The modes saved for an MCP, by tool name, in the order they were saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedToolApprovals(Vec<(String, ToolApprovalMode)>);

impl SavedToolApprovals {
    pub fn get(&self, tool_name: &str) -> Option<ToolApprovalMode> {
        self.0
            .iter()
            .find(|(name, _)| name == tool_name)
            .map(|(_, mode)| *mode)
    }

    /// The tools a choice is saved for.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, ToolApprovalMode)> {
        self.0.iter().map(|(name, mode)| (name.as_str(), *mode))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One choice the tools page submits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolApprovalChoice {
    pub name: String,
    pub mode: ToolApprovalMode,
}

/// The modes saved for this MCP, by tool name. `None` when what is saved
/// cannot be read, which no tool may take as leave to run.
pub fn saved_tool_approvals(mcp: &Mcp) -> Option<SavedToolApprovals> {
    let Some(saved) = mcp
        .tool_approvals
        .as_deref()
        .filter(|saved| !saved.is_empty())
    else {
        return Some(SavedToolApprovals::default());
    };

    let parsed: Value = serde_json::from_str(saved).ok()?;
    let saved = SAVED_TOOL_APPROVALS_VALIDATOR.validate(&parsed).ok()?;
    saved
        .as_object()?
        .iter()
        .map(|(name, mode)| Some((name.clone(), ToolApprovalMode::parse(mode.as_str()?)?)))
        .collect::<Option<Vec<_>>>()
        .map(SavedToolApprovals)
}

/// The longest tool name the tools page saves a choice for, in UTF-16 units.
pub const MAX_CHOOSABLE_NAME_UNITS: usize = 254;

/// Whether an admin can choose what this tool does. The tools page sends the
/// name of each tool as a form value, which arrives trimmed and is refused
/// when it is blank or longer than this. An MCP names its tools as it likes.
pub fn can_be_chosen(tool_name: &str) -> bool {
    let name = mymcps_vine::js::trim(tool_name);
    !name.is_empty() && name.encode_utf16().count() <= MAX_CHOOSABLE_NAME_UNITS
}

/// What a tool does until the admin chooses: built-in tools that commit money
/// or go live ask, and every other tool runs. A tool the admin cannot choose
/// for asks: its MCP must not decide, by how it names a tool, that it runs
/// unattended for good.
pub fn default_tool_approval(
    builtins: &BuiltinRegistry,
    mcp: &Mcp,
    tool_name: &str,
) -> ToolApprovalMode {
    if !can_be_chosen(tool_name) {
        return ToolApprovalMode::Ask;
    }
    let asks = builtins
        .get(mcp.builtin_key.as_deref())
        .and_then(|definition| definition.tool(tool_name))
        .is_some_and(|tool| tool.asks_approval);
    if asks {
        ToolApprovalMode::Ask
    } else {
        ToolApprovalMode::Auto
    }
}

fn mode_of(
    builtins: &BuiltinRegistry,
    saved: Option<&SavedToolApprovals>,
    mcp: &Mcp,
    tool_name: &str,
) -> ToolApprovalMode {
    let Some(saved) = saved else {
        return ToolApprovalMode::Ask;
    };
    saved
        .get(tool_name)
        .unwrap_or_else(|| default_tool_approval(builtins, mcp, tool_name))
}

pub fn tool_approval_mode(
    builtins: &BuiltinRegistry,
    mcp: &Mcp,
    tool_name: &str,
) -> ToolApprovalMode {
    mode_of(builtins, saved_tool_approvals(mcp).as_ref(), mcp, tool_name)
}

/// The mode of each of these tools, in one read of what is saved.
pub fn tool_approval_modes<'a>(
    builtins: &BuiltinRegistry,
    mcp: &Mcp,
    tool_names: impl IntoIterator<Item = &'a str>,
) -> Vec<(&'a str, ToolApprovalMode)> {
    let saved = saved_tool_approvals(mcp);
    tool_names
        .into_iter()
        .map(|name| (name, mode_of(builtins, saved.as_ref(), mcp, name)))
        .collect()
}

/// Keep the choices that differ from the defaults, so that a tool the admin
/// never touched follows its default if a later version changes it.
pub fn assign_tool_approvals(
    builtins: &BuiltinRegistry,
    mcp: &mut Mcp,
    choices: &[ToolApprovalChoice],
) {
    let mut saved = Map::new();
    for choice in choices {
        if choice.mode != default_tool_approval(builtins, mcp, &choice.name) {
            saved.insert(choice.name.clone(), Value::from(choice.mode.as_str()));
        }
    }
    mcp.tool_approvals = (!saved.is_empty()).then(|| Value::Object(saved).to_string());
}

/// Tell agents which tools wait for a person, before they plan around them.
pub fn with_approval_notes(
    builtins: &BuiltinRegistry,
    mcp: &Mcp,
    tools: Vec<UpstreamTool>,
) -> Vec<UpstreamTool> {
    let saved = saved_tool_approvals(mcp);
    tools
        .into_iter()
        .map(|mut tool| {
            if mode_of(builtins, saved.as_ref(), mcp, &tool.name) == ToolApprovalMode::Ask {
                tool.description = Some(match tool.description.filter(|text| !text.is_empty()) {
                    Some(description) => format!("{description}\n\n{APPROVAL_NOTE}"),
                    None => APPROVAL_NOTE.to_owned(),
                });
            }
            tool
        })
        .collect()
}
