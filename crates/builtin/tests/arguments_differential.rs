//! The argument types against the TypeScript they are ported from.
//!
//! `fixtures/builtin_tools.json` holds what the types of
//! `app/validators/builtin_tools.ts` answer, run from that very file by
//! Node, for forty schemas over some two hundred values each: what the tool
//! receives, or the sentence the agent reads, and how each schema reads as
//! a JSON Schema. The schemas go by name: each is built here with the Rust
//! API and was built there with the TypeScript one. The script that wrote
//! the fixture is not part of the repository, since it runs Node on the
//! TypeScript app.

use mymcps_builtin::BuiltinError;
use mymcps_builtin::arguments::{
    NO_ARGUMENTS_VALIDATOR, TOOL_VINE, blank_as_missing, boolean, choice, integer, iso_date, line,
    list_length, local_timestamp, media_type, number, pattern, pattern_with, text, trimmed_text,
    uploaded_file_name,
};
use mymcps_builtin::tool_input::tool_input;
use mymcps_vine as vine;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/builtin_tools.json");

fn schema(name: &str) -> vine::Schema {
    match name {
        "integer(1..)" => integer(1..).into(),
        "integer(1..).optional" => integer(1..).optional().into(),
        "integer(1..=100).optional" => integer(1..=100).optional().into(),
        "integer(0..=5)" => integer(0..=5).into(),
        "integer(-5..=5)" => integer(-5..=5).into(),
        "integer(1..=4294967295)" => integer(1..=4_294_967_295).into(),
        "number(-90..=90).optional" => number(-90..=90).optional().into(),
        "number(0.01..=1000000)" => number(0.01..=1_000_000.0).into(),
        "number(0..=10000000)" => number(0..=10_000_000).into(),
        "number(20..=400).optional" => number(20..=400).optional().into(),
        "boolean" => boolean().into(),
        "boolean.optional" => boolean().optional().into(),
        "choice" => choice(["riding", "running"]).into(),
        "choice.optional" => choice(["ENABLED", "PAUSED", "ALL"]).optional().into(),
        "text(10)" => text(10).into(),
        "text(10).optional" => text(10).optional().into(),
        "text(10).blankAsMissing" => text(10).parse(blank_as_missing).into(),
        "text(0).optional" => text(0).optional().into(),
        "trimmedText(10)" => trimmed_text(10).into(),
        "trimmedText(10).optional" => trimmed_text(10).optional().into(),
        "line(10)" => line(10).into(),
        "line(10).optional" => line(10).optional().into(),
        "pattern(gear)" => pattern(r"^[bg]\d{1,20}$", "a gear identifier such as b1234567").into(),
        "pattern(part).optional" => pattern(
            r"^\d{1,3}(\.\d{1,3}){0,9}$",
            "the part of an attachment, such as 2",
        )
        .optional()
        .into(),
        "pattern(id)" => pattern(r"^\d{1,19}$", "the numeric ID of a campaign").into(),
        "pattern(customer).optional" => pattern(
            r"^\d{3}-?\d{3}-?\d{4}$",
            "a Google Ads account ID such as 123-456-7890",
        )
        .optional()
        .into(),
        "pattern(path, u)" => {
            let expression = vine::js::regex(r"^[^\s/]{1,15}$", "u").unwrap();
            pattern_with(expression, "a display path of at most 15 characters").into()
        }
        "pattern(sport)" => {
            pattern(r"^[A-Z][A-Za-z]{1,39}$", "a Strava sport type like Run").into()
        }
        "uploadedFileName(20)" => uploaded_file_name(20).into(),
        "uploadedFileName(20).optional" => uploaded_file_name(20).optional().into(),
        "mediaType" => media_type().into(),
        "mediaType.optional" => media_type().optional().into(),
        "isoDate" => iso_date().into(),
        "isoDate.optional" => iso_date().optional().into(),
        "localTimestamp" => local_timestamp().into(),
        "localTimestamp.optional" => local_timestamp().optional().into(),
        "array(integer(1..=9)).listLength(1..=3).optional" => vine::array(integer(1..=9))
            .use_rule(list_length(
                1..=3,
                "{{ field }} must be a list of 1 to 3 ids",
            ))
            .optional()
            .into(),
        "array(integer(1..)).arrayOrEmpty.listLength(1..=3)" => vine::array(integer(1..))
            .parse(|value, _| Some(value.filter(Value::is_array).unwrap_or_else(|| json!([]))))
            .use_rule(list_length(
                1..=3,
                "{{ field }} must be a list of 1 to 3 message UIDs",
            ))
            .into(),
        "array(text(10)).listLength(..=2)" => vine::array(text(10))
            .use_rule(list_length(
                ..=2,
                "{{ field }} must be a list of at most 2 texts",
            ))
            .into(),
        "array(line(10)).listLength(0..=2).optional" => vine::array(line(10))
            .use_rule(list_length(
                0..=2,
                "{{ field }} must be a list of at most 2 lines",
            ))
            .optional()
            .into(),
        other => panic!("unknown schema {other}"),
    }
}

/// What the tool receives, or the sentence the agent reads, written as the
/// script wrote it.
fn answer(validator: &vine::Validator, data: Option<&Value>) -> Value {
    match tool_input::<Value>(validator, data) {
        Ok(input) => json!(["ok", input]),
        Err(BuiltinError::Tool(sentence)) => json!(["err", sentence]),
        Err(error) => panic!("{error}"),
    }
}

/// Deep equality where `5` and `5.0` are the same number, as they are in JavaScript.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((left_key, left), (right_key, right))| {
                        left_key == right_key && same(left, right)
                    })
        }
        _ => left == right,
    }
}

#[test]
fn answers_what_the_typescript_rules_answer() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 40);
    let mut compared = 0;
    let mut differences = Vec::new();

    for case in cases {
        let name = case["schema"].as_str().unwrap();
        let validator = TOOL_VINE.create(vine::object! { "x" => schema(name) });

        let described = &validator.to_json_schema()["properties"]["x"];
        if !same(described, &case["jsonSchema"]) {
            differences.push(format!(
                "{name}\n    typescript: {}\n    rust: {described}",
                case["jsonSchema"]
            ));
        }

        let values = fixture[case["values"].as_str().unwrap()]
            .as_array()
            .unwrap();
        let results = case["results"].as_array().unwrap();
        assert_eq!(values.len(), results.len(), "{name}");
        for (value, outcome) in values.iter().zip(results) {
            let expected = &case["outcomes"][usize::try_from(outcome.as_u64().unwrap()).unwrap()];
            // An empty list stands for a value of `undefined`.
            let arguments = match value.get(0) {
                Some(value) => json!({ "x": value }),
                None => json!({}),
            };
            let actual = answer(&validator, Some(&arguments));
            compared += 1;
            if !same(&actual, expected) {
                differences.push(format!(
                    "{name}\n    value: {value}\n    typescript: {expected}\n    rust: {actual}"
                ));
            }
        }
    }

    assert!(compared > 7_800, "only {compared} values compared");
    assert!(
        differences.is_empty(),
        "{} of {compared} answers differ from the TypeScript ones:\n{}",
        differences.len(),
        differences
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn refuses_what_is_not_a_set_of_arguments_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let required = TOOL_VINE.create(vine::object! {
        "first" => integer(1..),
        "second" => boolean().optional(),
    });
    assert!(same(
        NO_ARGUMENTS_VALIDATOR.to_json_schema(),
        &fixture["schemas"]["noArguments"]
    ));
    assert!(same(
        required.to_json_schema(),
        &fixture["schemas"]["required"]
    ));

    let roots = fixture["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 10);
    for root in roots {
        let data = root[0].get(0);
        assert!(
            same(&answer(&NO_ARGUMENTS_VALIDATOR, data), &root[1]),
            "no arguments, given {}",
            root[0]
        );
        assert!(
            same(&answer(&required, data), &root[2]),
            "arguments, given {}",
            root[0]
        );
    }
}
