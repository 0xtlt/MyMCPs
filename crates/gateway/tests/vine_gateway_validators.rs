//! `tests/unit/vine_gateway_validators.spec.ts`, apart from the namespaced
//! tool name, which the upstream crate validates.

use mymcps_core::models::GatewayToolMode;
use mymcps_gateway::lazy_tools::{
    CallToolInput, ToolSearchInput, parse_call_tool_input, parse_gateway_tool_mode,
    parse_tool_search_input,
};
use mymcps_gateway::validators::gateway::{CALL_TOOL, GATEWAY_TOOL_MODE, TOOL_SEARCH};
use serde_json::{Value, json};

const MCP_MESSAGE: &str = "mcp must be a non-empty MCP slug of at most 120 characters";
const QUERY_MESSAGE: &str = "query must be non-empty and at most 200 characters";
const LIMIT_MESSAGE: &str = "limit must be an integer between 1 and 20";
const TOOL_MESSAGE: &str = "tool must be a non-empty upstream tool name of at most 128 characters";
const ARGUMENTS_MESSAGE: &str = "arguments must be an object when provided";

/// Values that stand where a string is expected without being one. The first
/// is `undefined`.
fn not_strings() -> Vec<Option<Value>> {
    let mut values = vec![None];
    values.extend(
        [
            json!(null),
            json!(0),
            json!(1),
            json!(true),
            json!(["issues"]),
            json!([]),
            json!({}),
            json!({ "slug": "issues" }),
        ]
        .map(Some),
    );
    values
}

fn strings_and_not_strings(values: &[&str]) -> Vec<Option<Value>> {
    let mut all: Vec<Option<Value>> = values.iter().map(|value| Some(json!(value))).collect();
    all.extend(not_strings());
    all
}

/// `{ ...base, [key]: value }`, where `None` leaves the key out.
fn with(base: Value, key: &str, value: Option<Value>) -> Value {
    let mut object = base;
    if let (Some(object), Some(value)) = (object.as_object_mut(), value) {
        object.insert(key.to_owned(), value);
    }
    object
}

fn search(args: Value) -> Result<ToolSearchInput, String> {
    parse_tool_search_input(Some(&args))
}

fn search_input(mcp: &str, query: &str, limit: usize) -> ToolSearchInput {
    ToolSearchInput {
        mcp: mcp.to_owned(),
        query: query.to_owned(),
        limit,
    }
}

fn call_refusal(args: Value) -> Option<String> {
    parse_call_tool_input(Some(&args)).err()
}

// vine: gateway tool mode header

#[test]
fn reads_eager_and_lazy_whatever_their_case_and_surrounding_whitespace() {
    for (header, mode) in [
        ("eager", GatewayToolMode::Eager),
        ("lazy", GatewayToolMode::Lazy),
        ("LAZY", GatewayToolMode::Lazy),
        ("Eager", GatewayToolMode::Eager),
        (" LaZy ", GatewayToolMode::Lazy),
        ("\tlazy\n", GatewayToolMode::Lazy),
    ] {
        assert_eq!(
            GATEWAY_TOOL_MODE.validate_opt(&json!(header)).unwrap(),
            Some(json!(mode.as_str()))
        );
        assert_eq!(
            parse_gateway_tool_mode(Some(header), GatewayToolMode::Eager),
            Some(mode)
        );
    }
}

#[test]
fn leaves_the_mode_to_the_instance_when_the_header_is_absent_or_blank() {
    for header in [None, Some(""), Some(" "), Some(" \t ")] {
        let sent = header.map(Value::from);
        assert_eq!(GATEWAY_TOOL_MODE.validate_opt(&sent).unwrap(), None);
        assert_eq!(
            parse_gateway_tool_mode(header, GatewayToolMode::Eager),
            Some(GatewayToolMode::Eager)
        );
        assert_eq!(
            parse_gateway_tool_mode(header, GatewayToolMode::Lazy),
            Some(GatewayToolMode::Lazy)
        );
    }
}

#[test]
fn refuses_a_header_that_names_no_mode_the_gateway_has() {
    for header in [
        "sometimes",
        "lazyy",
        "laz",
        "lazy, eager",
        "eager lazy",
        "\"lazy\"",
        "0",
    ] {
        assert!(GATEWAY_TOOL_MODE.validate(&json!(header)).is_err());
        assert_eq!(
            parse_gateway_tool_mode(Some(header), GatewayToolMode::Lazy),
            None
        );
    }
    for header in [json!(0), json!(1), json!(true), json!(["lazy"]), json!({})] {
        assert!(GATEWAY_TOOL_MODE.validate(&header).is_err(), "{header}");
    }
}

// vine: lazy gateway tool_search arguments

#[test]
fn trims_the_slug_and_the_query_and_defaults_the_limit_to_10() {
    assert_eq!(
        search(json!({ "mcp": " issues ", "query": " create issue " })),
        Ok(search_input("issues", "create issue", 10))
    );
    assert_eq!(
        search(json!({ "mcp": "issues", "query": "q", "limit": 20, "extra": true })),
        Ok(search_input("issues", "q", 20))
    );
    assert_eq!(
        TOOL_SEARCH
            .validate(&json!({ "mcp": "x".repeat(120), "query": "q".repeat(200), "limit": 1 }))
            .unwrap(),
        json!({ "mcp": "x".repeat(120), "query": "q".repeat(200), "limit": 1 })
    );
    // The bounds apply to what is left after trimming.
    assert!(
        search(json!({
            "mcp": format!(" {} ", "x".repeat(120)),
            "query": format!(" {} ", "q".repeat(200)),
        }))
        .is_ok()
    );
}

#[test]
fn tells_the_agent_what_the_slug_must_be() {
    assert_eq!(parse_tool_search_input(None), Err(MCP_MESSAGE.to_owned()));
    assert_eq!(search(json!({})), Err(MCP_MESSAGE.to_owned()));
    let long = "x".repeat(121);
    let padded = format!(" {long} ");
    for mcp in strings_and_not_strings(&["", " ", " \n\t", &long, &padded]) {
        assert_eq!(
            search(with(json!({ "query": "issue" }), "mcp", mcp.clone())),
            Err(MCP_MESSAGE.to_owned()),
            "{mcp:?}"
        );
    }
}

#[test]
fn tells_the_agent_what_the_query_must_be() {
    assert_eq!(
        search(json!({ "mcp": "issues" })),
        Err(QUERY_MESSAGE.to_owned())
    );
    let long = "q".repeat(201);
    let padded = format!(" {long} ");
    for query in strings_and_not_strings(&["", " ", &long, &padded]) {
        assert_eq!(
            search(with(json!({ "mcp": "issues" }), "query", query.clone())),
            Err(QUERY_MESSAGE.to_owned()),
            "{query:?}"
        );
    }
}

#[test]
fn takes_an_integer_from_1_to_20_as_limit_and_nothing_that_only_looks_like_one() {
    for (limit, read) in [
        (json!(1), 1),
        (json!(2), 2),
        (json!(10), 10),
        (json!(20), 20),
        (json!(5.0), 5),
    ] {
        assert_eq!(
            search(json!({ "mcp": "issues", "query": "q", "limit": limit })),
            Ok(search_input("issues", "q", read))
        );
    }
    // NaN and the infinities of the TypeScript test cannot be written in JSON.
    for limit in [
        json!(0),
        json!(-1),
        json!(21),
        json!(1.5),
        json!(1e21),
        json!("5"),
        json!(""),
        json!(" "),
        json!(null),
        json!(true),
        json!(false),
        json!([5]),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            search(json!({ "mcp": "issues", "query": "q", "limit": limit })),
            Err(LIMIT_MESSAGE.to_owned()),
            "{limit}"
        );
    }
}

#[test]
fn reports_the_first_wrong_argument_only() {
    assert_eq!(
        search(json!({ "mcp": "", "query": "", "limit": 0 })),
        Err(MCP_MESSAGE.to_owned())
    );
    assert_eq!(
        search(json!({ "mcp": "issues", "query": "", "limit": 0 })),
        Err(QUERY_MESSAGE.to_owned())
    );
    assert_eq!(
        search(json!({ "mcp": "issues", "query": "q", "limit": 0 })),
        Err(LIMIT_MESSAGE.to_owned())
    );
}

// vine: lazy gateway call_tool arguments

#[test]
fn trims_the_slug_and_the_tool_name_and_makes_the_arguments_optional() {
    let args = json!({ "mcp": " issues ", "tool": " create_issue " });
    assert_eq!(
        parse_call_tool_input(Some(&args)),
        Ok(CallToolInput {
            mcp: "issues".to_owned(),
            tool: "create_issue".to_owned(),
            arguments: None,
        })
    );
    assert_eq!(
        call_refusal(json!({ "mcp": "issues", "tool": "t".repeat(128) })),
        None
    );
    assert_eq!(
        call_refusal(json!({ "mcp": "issues", "tool": format!(" {} ", "t".repeat(128)) })),
        None
    );
}

#[test]
fn hands_the_upstream_tool_the_very_object_the_agent_sent() {
    let sent =
        json!({ "title": "", "nested": { "list": [1, null, { "deep": true }] }, "": "empty key" });
    let args = json!({ "mcp": "issues", "tool": "create_issue", "arguments": sent });
    let input = parse_call_tool_input(Some(&args)).unwrap();

    assert!(std::ptr::eq(
        input.arguments.unwrap(),
        args["arguments"].as_object().unwrap()
    ));
    assert_eq!(
        args["arguments"],
        json!({ "title": "", "nested": { "list": [1, null, { "deep": true }] }, "": "empty key" })
    );

    assert!(
        CALL_TOOL
            .validate(&json!({ "mcp": "issues", "tool": "create_issue", "arguments": {} }))
            .is_ok()
    );
}

#[test]
fn tells_the_agent_which_of_the_slug_the_tool_and_the_arguments_is_wrong() {
    assert_eq!(
        parse_call_tool_input(None).err(),
        Some(MCP_MESSAGE.to_owned())
    );
    let long = "x".repeat(121);
    for mcp in strings_and_not_strings(&["", " ", &long]) {
        assert_eq!(
            call_refusal(with(json!({ "tool": "create_issue" }), "mcp", mcp.clone())),
            Some(MCP_MESSAGE.to_owned()),
            "{mcp:?}"
        );
    }

    assert_eq!(
        call_refusal(json!({ "mcp": "issues" })),
        Some(TOOL_MESSAGE.to_owned())
    );
    let long = "t".repeat(129);
    let padded = format!(" {long} ");
    for tool in strings_and_not_strings(&["", " ", &long, &padded]) {
        assert_eq!(
            call_refusal(with(json!({ "mcp": "issues" }), "tool", tool.clone())),
            Some(TOOL_MESSAGE.to_owned()),
            "{tool:?}"
        );
    }

    // Null is an argument the agent sent, unlike an argument left out.
    for sent in [
        json!(null),
        json!([]),
        json!([{}]),
        json!("text"),
        json!(""),
        json!(" "),
        json!(0),
        json!(1),
        json!(true),
        json!(false),
    ] {
        assert_eq!(
            call_refusal(json!({ "mcp": "issues", "tool": "create_issue", "arguments": sent })),
            Some(ARGUMENTS_MESSAGE.to_owned()),
            "{sent}"
        );
    }

    assert_eq!(
        call_refusal(json!({ "mcp": "", "tool": "", "arguments": [] })),
        Some(MCP_MESSAGE.to_owned())
    );
    assert_eq!(
        call_refusal(json!({ "mcp": "issues", "tool": "", "arguments": [] })),
        Some(TOOL_MESSAGE.to_owned())
    );
}
