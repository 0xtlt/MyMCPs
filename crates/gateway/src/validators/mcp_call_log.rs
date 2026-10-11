//! Vine schemas of the call log: the query strings of the Logs and Analytics
//! pages, and the slug a refused call is stored under.

use std::sync::LazyLock;

use mymcps_vine as vine;
use vine::Validator;

fn static_regex(source: &str) -> vine::JsRegex {
    vine::js::regex(source, "").expect("a valid static pattern")
}

pub static LOGS_QUERY: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "range" => vine::enum_(["24h", "7d", "30d", "all"]).optional(),
        "outcome" => vine::enum_(["success", "error"]).optional(),
        "mcp" => vine::string().trim().max_length(120).optional(),
        "token" => vine::string().trim().max_length(16).optional(),
        "page" => vine::number().without_decimals().positive().optional(),
        "pageSize" => vine::number().without_decimals().in_([10, 25, 50, 100]).optional(),
        "logId" => vine::number().without_decimals().positive().optional(),
        "timeZone" => vine::string().trim().max_length(100).optional(),
    })
});

pub static ANALYTICS_QUERY: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "range" => vine::enum_(["24h", "7d", "30d", "custom"]).optional(),
        "start" => vine::string().trim().max_length(40).optional(),
        "end" => vine::string().trim().max_length(40).optional(),
        "timeZone" => vine::string().trim().max_length(100).optional(),
    })
});

/// An MCP slug as a caller named it. The call log stores it only in the form a
/// real slug can take.
pub static LOGGED_MCP_SLUG: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::string().regex(static_regex(r"^[a-z0-9-]{1,120}$")))
});
