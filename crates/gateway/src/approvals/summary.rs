//! What a person reads of a call before deciding it, when MyMCPs has only
//! the arguments to show.

use mymcps_builtin::{ApprovalDetail, ApprovalSummary};
use mymcps_core::models::Mcp;
use mymcps_vine as vine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::js::slice_utf16;

/// A summary as it is kept with its request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedApprovalSummary {
    /// Whether MyMCPs knows what the tool does and read the call itself. When it
    /// does not, the summary is the arguments as they are and nothing more.
    pub interpreted: bool,
    /// One sentence, such as `Change the daily budget of campaign "Spring sale"`.
    pub title: String,
    /// What the call sets, and for a change, the value it replaces.
    pub details: Vec<ApprovalDetail>,
    /// What the person should weigh before deciding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    /// What the MCP says its tool does. Written by the MCP, never by the agent.
    pub tool_description: Option<String>,
}

impl SavedApprovalSummary {
    /// The summary a built-in tool made of its own call.
    pub fn interpreted(described: ApprovalSummary) -> Self {
        Self {
            interpreted: true,
            title: described.title,
            details: described.details,
            warnings: described.warnings,
            tool_description: None,
        }
    }
}

const MAX_DETAILS: usize = 60;
const MAX_VALUE_CHARS: usize = 1000;
const MAX_DESCRIPTION_CHARS: usize = 2000;

fn shown(value: &str) -> String {
    let length = vine::js::utf16_len(value);
    if length > MAX_VALUE_CHARS {
        format!(
            "{}… ({} more characters)",
            slice_utf16(value, MAX_VALUE_CHARS),
            length - MAX_VALUE_CHARS
        )
    } else {
        value.to_owned()
    }
}

fn leaf_value(value: &Value) -> String {
    match value {
        Value::String(text) if text.is_empty() => "(empty text)".to_owned(),
        Value::String(text) => shown(text),
        Value::Array(_) => "(empty list)".to_owned(),
        Value::Object(_) => "(empty object)".to_owned(),
        // A whole number is forwarded with every digit it came with, and
        // that is the number the person approves.
        Value::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
        other => vine::js::to_string(other),
    }
}

/// The rows of [`argument_details`], and how many values were left out of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentDetails {
    pub details: Vec<ApprovalDetail>,
    pub hidden: usize,
}

fn visit(value: &Value, path: String, details: &mut Vec<ApprovalDetail>, total: &mut usize) {
    match value {
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                visit(item, format!("{path}[{index}]"), details, total);
            }
        }
        Value::Object(entries) if !entries.is_empty() => {
            for key in vine::js::own_keys(entries) {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                visit(&entries[key], child_path, details, total);
            }
        }
        leaf => {
            *total += 1;
            if details.len() < MAX_DETAILS {
                details.push(ApprovalDetail::new(path, leaf_value(leaf)));
            }
        }
    }
}

/// Every value of the arguments on a row of its own, named by where it is:
/// `budget.amount`, `keywords[0].text`. No sentence is made out of them, so
/// the rows say what the agent sent and nothing it would like them to say.
pub fn argument_details(args: Option<&Map<String, Value>>) -> ArgumentDetails {
    let mut details = Vec::new();
    let mut total = 0;
    if let Some(args) = args {
        for key in vine::js::own_keys(args) {
            visit(&args[key], key.clone(), &mut details, &mut total);
        }
    }
    ArgumentDetails {
        hidden: total - details.len(),
        details,
    }
}

/// The summary of a call to a tool MyMCPs cannot read: one of a connected MCP,
/// or a built-in one that has nothing to add to its arguments.
pub fn arguments_summary(
    mcp: &Mcp,
    tool_name: &str,
    args: Option<&Map<String, Value>>,
    tool_description: Option<&str>,
) -> SavedApprovalSummary {
    let ArgumentDetails { details, hidden } = argument_details(args);
    SavedApprovalSummary {
        interpreted: false,
        title: format!("Run the tool \"{tool_name}\" of {}", mcp.name),
        details,
        warnings: (hidden > 0).then(|| {
            vec![format!(
                "Only the first {MAX_DETAILS} values are listed, and {hidden} more are not. Read the exact arguments before you decide."
            )]
        }),
        tool_description: tool_description
            .map(|description| slice_utf16(vine::js::trim(description), MAX_DESCRIPTION_CHARS))
            .filter(|description| !description.is_empty())
            .map(str::to_owned),
    }
}
