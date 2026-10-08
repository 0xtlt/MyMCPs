//! `tests/fixtures/zod.json` holds what the real schemas of the SDK (zod
//! 4.6.5) answered for a corpus of values: the parsed value as `JSON.stringify`
//! writes it, or the message of the `ZodError`. Long messages are stored as a
//! hash.

use serde::Deserialize;

use crate::{json, schemas};

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    schema: String,
    input: Option<String>,
    #[serde(default)]
    absent: bool,
    output: Option<String>,
    message: Option<String>,
    #[serde(rename = "messageHash")]
    message_hash: Option<String>,
    #[serde(rename = "messageLength")]
    message_length: Option<usize>,
}

fn fnv1a64(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[test]
fn parses_and_refuses_values_exactly_as_the_sdk_schemas_do() {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("../tests/fixtures/zod.json")).expect("fixture file");
    assert!(fixtures.cases.len() > 500);

    let mut failures = Vec::new();
    for (index, case) in fixtures.cases.iter().enumerate() {
        let schema = schemas::by_name(&case.schema).expect("known schema");
        let input = match (&case.input, case.absent) {
            (Some(text), false) => Some(json::parse(text).expect("fixture input is JSON")),
            _ => None,
        };
        let describe = |got: &str, expected: &str| {
            format!(
                "case {index} ({}) input {}\n  expected: {expected}\n  got:      {got}",
                case.schema,
                case.input.as_deref().unwrap_or("undefined"),
            )
        };

        match schema.parse(input.as_ref()) {
            Ok(value) => {
                let got = value.as_ref().map(json::to_string);
                if case.message.is_some() || case.message_hash.is_some() {
                    failures.push(describe(
                        got.as_deref().unwrap_or("undefined"),
                        case.message.as_deref().unwrap_or("a refusal"),
                    ));
                } else if got != case.output {
                    failures.push(describe(
                        got.as_deref().unwrap_or("undefined"),
                        case.output.as_deref().unwrap_or("undefined"),
                    ));
                }
            }
            Err(error) => {
                let got = error.to_string();
                match (&case.message, &case.message_hash) {
                    (Some(expected), _) => {
                        if &got != expected {
                            failures.push(describe(&got, expected));
                        }
                    }
                    (None, Some(hash)) => {
                        if &fnv1a64(&got) != hash || Some(got.len()) != case.message_length {
                            failures.push(describe(&got, &format!("a message with hash {hash}")));
                        }
                    }
                    (None, None) => failures.push(describe(
                        &got,
                        case.output.as_deref().unwrap_or("undefined"),
                    )),
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} cases differ from the SDK:\n{}",
        failures.len(),
        fixtures.cases.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
