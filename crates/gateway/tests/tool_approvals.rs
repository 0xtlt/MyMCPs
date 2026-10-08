//! `tests/unit/tool_approvals.spec.ts`, for how a call is read: the hash
//! that identifies it and the rows a person reads of it. Which tools ask is
//! tested with the policy, in the upstream crate. The Vine schemas of
//! `app/validators/approvals.ts` are compared here with what Vine answers.

use mymcps_builtin::ApprovalDetail;
use mymcps_core::models::{Mcp, McpTransport};
use mymcps_gateway::approvals::{
    Decision, SavedApprovalSummary, argument_details, arguments_hash, arguments_summary,
};
use mymcps_gateway::validators::approvals::{
    APPROVAL_DECISION, APPROVAL_PARAMS, SAVED_APPROVAL_SUMMARY, UPDATE_TOOL_APPROVALS,
};
use mymcps_vine::Validator;
use serde_json::{Map, Value, json};

fn mcp() -> Mcp {
    Mcp {
        name: "CRM".into(),
        transport: McpTransport::Http,
        ..Default::default()
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(entries) => entries,
        other => panic!("not an object: {other}"),
    }
}

fn hash(arguments: Value) -> String {
    arguments_hash(Some(&object(arguments)))
}

fn rows(details: &[ApprovalDetail]) -> Vec<(&str, &str)> {
    details
        .iter()
        .map(|detail| (detail.label.as_str(), detail.value.as_str()))
        .collect()
}

// Tool approvals: reading a call

#[test]
fn identifies_arguments_whatever_the_order_of_their_keys() {
    let reference = hash(json!({ "id": 42, "filter": { "tags": ["a", "b"], "dry_run": false } }));

    assert_eq!(
        reference,
        hash(json!({ "filter": { "dry_run": false, "tags": ["a", "b"] }, "id": 42 }))
    );
    assert_eq!(arguments_hash(None), hash(json!({})));
    // Another value, another type, or another order in a list is another call.
    assert_ne!(
        reference,
        hash(json!({ "id": 43, "filter": { "tags": ["a", "b"], "dry_run": false } }))
    );
    assert_ne!(
        reference,
        hash(json!({ "id": "42", "filter": { "tags": ["a", "b"], "dry_run": false } }))
    );
    assert_ne!(
        reference,
        hash(json!({ "id": 42, "filter": { "tags": ["b", "a"], "dry_run": false } }))
    );
}

/// The hashes Node 24 computes with `argumentsHash`: the requests an
/// instance holds when it moves to this version name their call by them.
#[test]
fn computes_the_hashes_the_node_app_stored() {
    for (arguments, expected) in [
        (
            r#"{"id":42,"filter":{"tags":["a","b"],"dry_run":false}}"#,
            "9f63c7d7d7671879bec424e9822add672dee154acd8caf95f894a6fdfa6650a9",
        ),
        (
            "{}",
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
        ),
        // Keys that read as array indexes come first, in the order of their value.
        (
            r#"{"10":1,"9":2,"b":3,"a":{"2":true,"1":null,"z":[{"y":1,"x":2}]}}"#,
            "71394295b1e924db577aa47b6e955324b1bd3937882677b19a2bcde47f241ed3",
        ),
        // Numbers are written as JavaScript writes them.
        (
            r#"{"price":71.5,"big":1e21,"small":0.000001,"neg":-0,"whole":5.0,"exp":1e3,"text":"é😀\n\"\\\u0001\u007f "}"#,
            "262e79ff67daf0fbd64e25c0c0e1bfcf8f095382065886defa093c0b9d8286fb",
        ),
        // Keys are sorted by their UTF-16 code units.
        (
            "{\"\u{ff5e}\":1,\"😀\":2,\"a\":3,\"Z\":4,\"_\":5}",
            "12a9ee5fcced06400db7a359c4074a3f6a73d60698362651d1213d32de14dc57",
        ),
        (
            r#"{"id":42}"#,
            "17b4db064e17f4878e391177e6ca623b798911f34014bc9e78920993d7dd27ad",
        ),
        (
            r#"{"weight":71.5}"#,
            "e1348167643a6fd41ffedd595db2c9d23996507ae2d0d0d1629f6e7cc593f1b4",
        ),
        (
            r#"{"n":9007199254740991,"m":-9007199254740991,"list":[3,2,1,[],{}],"nil":null}"#,
            "af02a1caec9beabd8d351e5f004add6b1b07a45ea449699337efe15f193af1e1",
        ),
        (
            r#"{"01":1,"1":2,"4294967294":3,"4294967295":4,"-1":5,"1.5":6}"#,
            "b07cce30193ecd643fdf4e56a03eeb1f14d97e0b78385673f73aedcff8269903",
        ),
    ] {
        let parsed: Value = serde_json::from_str(arguments).unwrap();
        assert_eq!(hash(parsed), expected, "{arguments}");
    }
    assert_eq!(
        arguments_hash(None),
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
}

#[test]
fn tells_apart_whole_numbers_that_javascript_would_round_to_the_same() {
    // Both are forwarded with every digit, so they are two calls.
    assert_ne!(
        hash(json!({ "id": 9_007_199_254_740_993_u64 })),
        hash(json!({ "id": 9_007_199_254_740_992_u64 }))
    );
    // The same number written with or without a fraction is one call.
    assert_eq!(hash(json!({ "id": 5.0 })), hash(json!({ "id": 5 })));
}

#[test]
fn puts_every_value_on_a_row_named_by_where_it_is() {
    let arguments = object(json!({
        "amount": 250,
        "note": "",
        "nothing": null,
        "tags": [],
        "options": {},
        "items": [{ "name": "a" }, { "name": "b", "flags": [true] }],
    }));
    let listed = argument_details(Some(&arguments));

    assert_eq!(listed.hidden, 0);
    assert_eq!(
        rows(&listed.details),
        [
            ("amount", "250"),
            ("note", "(empty text)"),
            ("nothing", "null"),
            ("tags", "(empty list)"),
            ("options", "(empty object)"),
            ("items[0].name", "a"),
            ("items[1].name", "b"),
            ("items[1].flags[0]", "true"),
        ]
    );
    assert!(listed.details.iter().all(|detail| detail.before.is_none()));

    let none = argument_details(None);
    assert_eq!((none.details.len(), none.hidden), (0, 0));
}

#[test]
fn lists_values_as_javascript_lists_and_writes_them() {
    let arguments = object(json!({
        "b": 1.5,
        "10": "ten",
        "2": false,
        "": { "inner": 1e21, "": 0.000001 },
        "whole": 5.0,
        "large": 9_007_199_254_740_993_u64,
        "negative": -3,
    }));

    assert_eq!(
        rows(&argument_details(Some(&arguments)).details),
        [
            // Keys that read as array indexes come first.
            ("2", "false"),
            ("10", "ten"),
            ("b", "1.5"),
            // A value under an empty name is named by what is inside it.
            ("inner", "1e+21"),
            ("", "0.000001"),
            ("whole", "5"),
            ("large", "9007199254740993"),
            ("negative", "-3"),
        ]
    );
}

#[test]
fn cuts_what_is_too_long_to_read_and_says_so() {
    let mut arguments: Map<String, Value> = (0..75)
        .map(|index| (format!("field{index}"), json!(1)))
        .collect();
    arguments.insert("body".into(), json!("x".repeat(1500)));
    let summary = arguments_summary(&mcp(), "import", Some(&arguments), Some(" Imports. "));

    assert_eq!(summary.title, "Run the tool \"import\" of CRM");
    assert!(!summary.interpreted);
    assert_eq!(summary.tool_description.as_deref(), Some("Imports."));
    assert_eq!(summary.details.len(), 60);
    assert_eq!(
        summary.warnings,
        Some(vec![
            "Only the first 60 values are listed, and 16 more are not. Read the exact arguments before you decide."
                .to_owned()
        ])
    );

    let long = object(json!({ "body": "x".repeat(1500) }));
    assert_eq!(
        argument_details(Some(&long)).details[0].value,
        format!("{}… (500 more characters)", "x".repeat(1000))
    );
    // Lengths are counted as JavaScript counts them.
    let emoji = object(json!({ "body": "😀".repeat(600) }));
    assert_eq!(
        argument_details(Some(&emoji)).details[0].value,
        format!("{}… (200 more characters)", "😀".repeat(500))
    );
}

#[test]
fn keeps_no_description_for_a_tool_that_says_nothing_of_itself() {
    for description in [None, Some(""), Some("  \n\t ")] {
        let summary = arguments_summary(&mcp(), "import", None, description);
        assert_eq!(summary.tool_description, None, "{description:?}");
        assert_eq!(summary.warnings, None);
        assert!(summary.details.is_empty());
    }

    let long = "d".repeat(2500);
    let summary = arguments_summary(&mcp(), "import", None, Some(&long));
    assert_eq!(summary.tool_description, Some("d".repeat(2000)));
}

#[test]
fn stores_a_summary_under_the_names_the_pages_read() {
    let arguments = object(json!({ "id": 42 }));
    let summary = arguments_summary(
        &mcp(),
        "delete_contact",
        Some(&arguments),
        Some("Delete a contact for good."),
    );

    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({
            "interpreted": false,
            "title": "Run the tool \"delete_contact\" of CRM",
            "details": [{ "label": "id", "value": "42" }],
            "toolDescription": "Delete a contact for good.",
        })
    );
    // What was stored is read back through the schema.
    assert_eq!(
        SAVED_APPROVAL_SUMMARY
            .validate_as::<SavedApprovalSummary>(&serde_json::to_value(&summary).unwrap())
            .unwrap(),
        summary
    );
}

#[test]
fn reads_the_decision_a_page_submits() {
    assert_eq!(Decision::parse("approve"), Some(Decision::Approve));
    assert_eq!(Decision::parse("deny"), Some(Decision::Deny));
    for other in ["", "maybe", "Approve", " approve"] {
        assert_eq!(Decision::parse(other), None, "{other:?}");
    }
}

// The schemas of `app/validators/approvals.ts`

/// What Vine 4.4 answers for each input, written by Node 24.
const REFERENCE: &str = include_str!("fixtures/approval_validators_reference.json");

fn assert_answers_as_vine(name: &str, validator: &Validator) {
    let reference: Value = serde_json::from_str(REFERENCE).unwrap();
    let cases = reference[name].as_array().unwrap();
    assert!(!cases.is_empty());

    for case in cases {
        let input = &case["input"];
        match validator.validate(input) {
            Ok(output) => {
                assert!(case["errors"].is_null(), "{name}: {input} was accepted");
                assert_eq!(
                    output.to_string(),
                    case["output"].to_string(),
                    "{name}: {input}"
                );
            }
            Err(error) => {
                let errors: Vec<Value> = error
                    .messages
                    .iter()
                    .map(|error| {
                        json!({
                            "field": error.field,
                            "rule": error.rule,
                            "message": error.message,
                        })
                    })
                    .collect();
                assert_eq!(Value::Array(errors), case["errors"], "{name}: {input}");
            }
        }
    }
}

#[test]
fn reads_the_choices_of_the_tools_page_as_vine_does() {
    assert_answers_as_vine("UPDATE_TOOL_APPROVALS", &UPDATE_TOOL_APPROVALS);

    let many =
        |count: usize| json!({ "tools": vec![json!({ "name": "tool", "mode": "ask" }); count] });
    assert!(UPDATE_TOOL_APPROVALS.validate(&many(2000)).is_ok());
    assert!(UPDATE_TOOL_APPROVALS.validate(&many(2001)).is_err());
}

#[test]
fn reads_the_id_of_an_approval_link_as_vine_does() {
    assert_answers_as_vine("APPROVAL_PARAMS", &APPROVAL_PARAMS);
}

#[test]
fn reads_a_decision_as_vine_does() {
    assert_answers_as_vine("APPROVAL_DECISION", &APPROVAL_DECISION);
}

#[test]
fn reads_a_saved_summary_as_vine_does() {
    assert_answers_as_vine("SAVED_APPROVAL_SUMMARY", &SAVED_APPROVAL_SUMMARY);
}
