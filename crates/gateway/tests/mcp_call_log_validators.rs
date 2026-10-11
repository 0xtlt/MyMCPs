//! `app/validators/mcp_call_log.ts` has no test of its own in the Node app:
//! these are the answers its validators give there, to the same input.

use mymcps_gateway::validators::mcp_call_log::{ANALYTICS_QUERY, LOGGED_MCP_SLUG, LOGS_QUERY};
use mymcps_vine::Validator;
use serde_json::{Value, json};

/// The first error of an input the validator refuses, as (field, rule, message).
fn refusal(validator: &Validator, input: Value) -> (String, String, String) {
    let error = validator.validate(&input).unwrap_err();
    assert_eq!(error.messages.len(), 1, "{input}");
    let first = &error.messages[0];
    (
        first.field.clone(),
        first.rule.clone(),
        first.message.clone(),
    )
}

fn refused(field: &str, rule: &str, message: &str) -> (String, String, String) {
    (field.to_owned(), rule.to_owned(), message.to_owned())
}

#[test]
fn reads_the_query_string_of_the_logs_page() {
    for (input, output) in [
        (json!({}), json!({})),
        (
            json!({
                "range": "7d",
                "outcome": "error",
                "mcp": " issues ",
                "token": "",
                "page": "2",
                "pageSize": "25",
                "logId": "7",
                "timeZone": " Europe/Paris ",
                "extra": 1,
            }),
            json!({
                "range": "7d",
                "outcome": "error",
                "mcp": "issues",
                "page": 2,
                "pageSize": 25,
                "logId": 7,
                "timeZone": "Europe/Paris",
            }),
        ),
        (
            json!({ "range": "7d", "page": "3", "pageSize": "10", "outcome": "success" }),
            json!({ "range": "7d", "outcome": "success", "page": 3, "pageSize": 10 }),
        ),
        (json!({ "pageSize": 100 }), json!({ "pageSize": 100 })),
        // A blank parameter is one left out.
        (json!({ "range": "", "page": " " }), json!({})),
        (json!({ "mcp": "   " }), json!({})),
    ] {
        assert_eq!(
            LOGS_QUERY.validate(&input).unwrap().to_string(),
            output.to_string()
        );
    }
}

#[test]
fn refuses_a_query_string_the_logs_page_cannot_read() {
    for (input, error) in [
        (
            json!({ "range": "custom" }),
            refused("range", "enum", "The selected range is invalid"),
        ),
        (
            json!({ "range": ["7d"] }),
            refused("range", "enum", "The selected range is invalid"),
        ),
        (
            json!({ "outcome": "ok" }),
            refused("outcome", "enum", "The selected outcome is invalid"),
        ),
        (
            json!({ "page": "0" }),
            refused("page", "positive", "The page field must be positive"),
        ),
        (
            json!({ "page": "1.5" }),
            refused(
                "page",
                "withoutDecimals",
                "The page field must be an integer",
            ),
        ),
        (
            json!({ "page": "abc" }),
            refused("page", "number", "The page field must be a number"),
        ),
        (
            json!({ "page": ["1", "2"] }),
            refused("page", "number", "The page field must be a number"),
        ),
        (
            json!({ "pageSize": "30" }),
            refused("pageSize", "in", "The selected pageSize is invalid"),
        ),
        (
            json!({ "logId": "-1" }),
            refused("logId", "positive", "The logId field must be positive"),
        ),
        (
            json!({ "mcp": "x".repeat(121) }),
            refused(
                "mcp",
                "maxLength",
                "The mcp field must not be greater than 120 characters",
            ),
        ),
        (
            json!({ "token": "x".repeat(17) }),
            refused(
                "token",
                "maxLength",
                "The token field must not be greater than 16 characters",
            ),
        ),
        (
            json!({ "timeZone": "x".repeat(101) }),
            refused(
                "timeZone",
                "maxLength",
                "The timeZone field must not be greater than 100 characters",
            ),
        ),
    ] {
        assert_eq!(refusal(&LOGS_QUERY, input), error);
    }
}

#[test]
fn reads_the_query_string_of_the_analytics_page() {
    assert_eq!(ANALYTICS_QUERY.validate(&json!({})).unwrap(), json!({}));
    assert_eq!(
        ANALYTICS_QUERY
            .validate(&json!({
                "range": "custom",
                "start": " 2026-01-01 ",
                "end": "2026-01-31",
                "timeZone": "UTC",
            }))
            .unwrap()
            .to_string(),
        json!({
            "range": "custom",
            "start": "2026-01-01",
            "end": "2026-01-31",
            "timeZone": "UTC",
        })
        .to_string()
    );
    assert_eq!(
        ANALYTICS_QUERY.validate(&json!({ "end": "" })).unwrap(),
        json!({})
    );

    assert_eq!(
        refusal(&ANALYTICS_QUERY, json!({ "range": "all" })),
        refused("range", "enum", "The selected range is invalid")
    );
    assert_eq!(
        refusal(&ANALYTICS_QUERY, json!({ "start": "x".repeat(41) })),
        refused(
            "start",
            "maxLength",
            "The start field must not be greater than 40 characters"
        )
    );
    assert_eq!(
        refusal(&ANALYTICS_QUERY, json!({ "range": "24h", "start": 5 })),
        refused("start", "string", "The start field must be a string")
    );
}

#[test]
fn takes_a_slug_only_in_the_form_a_real_slug_has() {
    let long = "x".repeat(120);
    for slug in ["issues", "a-b-9", long.as_str()] {
        assert_eq!(LOGGED_MCP_SLUG.validate(&json!(slug)).unwrap(), json!(slug));
    }

    for (slug, rule) in [
        (Some(json!("")), "required"),
        (Some(json!(" ")), "required"),
        (Some(json!(null)), "required"),
        (None, "required"),
        (Some(json!("Issues")), "regex"),
        (Some(json!("x".repeat(121))), "regex"),
        (Some(json!("issues\n")), "regex"),
        (Some(json!(5)), "string"),
    ] {
        let error = LOGGED_MCP_SLUG.validate(&slug).unwrap_err();
        assert_eq!(error.messages[0].rule, rule, "{slug:?}");
    }
}
