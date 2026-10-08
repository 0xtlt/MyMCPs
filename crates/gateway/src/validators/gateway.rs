//! Vine schemas for what agents send to the MCP gateway, apart from the
//! JSON-RPC messages themselves, which the MCP protocol layer validates. A
//! blank value is refused or read as absent by every schema here, so they
//! answer the same with or without the app-wide conversion of blank strings
//! to null.
//!
//! The schema of a namespaced tool name (`<slug>__<tool>`) lives with the
//! upstream manager, which is the one that splits such names.

use std::collections::HashMap;
use std::sync::LazyLock;

use mymcps_vine as vine;
use serde_json::{Map, Value, json};
use vine::{FieldRef, MessagesProvider, Rule, Validator, VineString};

/// An agent that gets an argument wrong reads one sentence saying what that
/// argument must be, whichever rule refused it.
fn argument_messages<const N: usize>(
    messages: [(&'static str, &'static str); N],
) -> impl MessagesProvider {
    let messages = HashMap::from(messages);
    move |default_message: &str,
          _rule: &str,
          field: FieldRef<'_>,
          _args: Option<&Map<String, Value>>| {
        messages
            .get(field.wildcard_path)
            .copied()
            .unwrap_or(default_message)
            .to_owned()
    }
}

/// Vine reads null as a value left out. In the arguments of a tool call it is
/// a value the agent sent, and it is not a valid one.
fn not_null() -> Rule {
    vine::rule(|_, field| {
        if field.is_null() {
            field.report("The {{ field }} field must not be null", "notNull");
        }
    })
    .implicit()
}

fn mcp_slug_argument() -> VineString {
    vine::string().trim().min_length(1).max_length(120)
}

/// The `X-MyMCPs-Tool-Mode` request header. Case and surrounding whitespace
/// are ignored, and a blank header leaves the choice to the instance.
pub static GATEWAY_TOOL_MODE: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(
        vine::enum_(["eager", "lazy"])
            .parse(|value, _| match value {
                Some(Value::String(mode)) => {
                    let mode = vine::js::trim(&mode).to_lowercase();
                    (!mode.is_empty()).then_some(Value::String(mode))
                }
                other => other,
            })
            .optional(),
    )
});

/// Arguments of the lazy gateway's `tool_search`.
pub static TOOL_SEARCH: LazyLock<Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "mcp" => mcp_slug_argument(),
            "query" => vine::string().trim().min_length(1).max_length(200),
            "limit" => vine::number()
                .strict()
                .parse(|value, _| Some(value.unwrap_or_else(|| json!(10))))
                .without_decimals()
                .range([1, 20]),
        })
        .messages_provider(argument_messages([
            (
                "mcp",
                "mcp must be a non-empty MCP slug of at most 120 characters",
            ),
            (
                "query",
                "query must be non-empty and at most 200 characters",
            ),
            ("limit", "limit must be an integer between 1 and 20"),
        ]))
});

/// Arguments of the lazy gateway's `call_tool`. What is passed on to the
/// upstream tool is only required to be an object: its content is the
/// upstream's to judge.
pub static CALL_TOOL: LazyLock<Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "mcp" => mcp_slug_argument(),
            "tool" => vine::string().trim().min_length(1).max_length(128),
            "arguments" => vine::object! {}.use_rule(not_null()).optional(),
        })
        .messages_provider(argument_messages([
            (
                "mcp",
                "mcp must be a non-empty MCP slug of at most 120 characters",
            ),
            (
                "tool",
                "tool must be a non-empty upstream tool name of at most 128 characters",
            ),
            ("arguments", "arguments must be an object when provided"),
        ]))
});
