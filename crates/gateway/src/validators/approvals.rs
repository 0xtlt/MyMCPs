//! Vine schemas for tool approvals: what the pages submit, what a link names,
//! and what the app reads back from its own database.
//!
//! The modes saved for an MCP are read where the policy is: they are
//! re-exported here from `mymcps_upstream::validators`.

use std::sync::LazyLock;

use mymcps_builtin::arguments::TOOL_VINE;
use mymcps_vine as vine;
use vine::Validator;

use mymcps_upstream::approvals::MAX_CHOOSABLE_NAME_UNITS;
pub use mymcps_upstream::validators::{SAVED_TOOL_APPROVALS_VALIDATOR, TOOL_APPROVAL_MODES};

fn static_regex(source: &str) -> vine::JsRegex {
    vine::js::regex(source, "").expect("a valid static pattern")
}

/// The tools page sends every tool a choice can be saved for, with the mode
/// chosen for it.
pub static UPDATE_TOOL_APPROVALS: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "tools" => vine::array(vine::object! {
            "name" => vine::string().min_length(1).max_length(MAX_CHOOSABLE_NAME_UNITS),
            "mode" => vine::enum_(TOOL_APPROVAL_MODES),
        })
        .max_length(2000),
    })
});

/// `:id` of an approval link: 24 random bytes in base64url.
pub static APPROVAL_PARAMS: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "id" => vine::string().regex(static_regex(r"^[A-Za-z0-9_-]{32}$")),
    })
});

pub static APPROVAL_DECISION: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "decision" => vine::enum_(["approve", "deny"]),
    })
});

/// What MyMCPs read in a call when it was made, as stored with the request.
/// Values are kept as written: an empty one is a value.
pub static SAVED_APPROVAL_SUMMARY: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "interpreted" => vine::boolean().strict(),
        "title" => vine::string(),
        "details" => vine::array(vine::object! {
            "label" => vine::string(),
            "value" => vine::string(),
            "before" => vine::string().optional(),
        }),
        "warnings" => vine::array(vine::string()).optional(),
        "toolDescription" => vine::string().nullable(),
    })
});
