//! Port of `tests/unit/vine_builtin_tools.spec.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use mymcps_builtin::arguments::{
    NO_ARGUMENTS_VALIDATOR, NoArguments, TOOL_VINE, blank_as_missing, boolean, choice, integer,
    iso_date, line, list_length, local_timestamp, number, pattern, text, trimmed_text,
};
use mymcps_builtin::tool_input::{tool_input, tool_input_with};
use mymcps_builtin::{BuiltinError, BuiltinTool, ToolInput};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// What a tool gets for the argument `x`.
#[derive(Debug, PartialEq)]
enum Argument {
    Value(Value),
    Missing,
    /// The sentence the agent reads instead.
    Refused(String),
}

fn refused(sentence: &str) -> Argument {
    Argument::Refused(sentence.to_owned())
}

/// `{ x: value }`, where `None` is a value of `undefined`.
fn x(value: Option<Value>) -> Value {
    match value {
        Some(value) => json!({ "x": value }),
        None => json!({}),
    }
}

fn argument(schema: impl Into<vine::Schema>) -> impl Fn(Option<Value>) -> Argument {
    let validator = TOOL_VINE.create(vine::object! { "x" => schema });
    move |value| match tool_input::<Value>(&validator, &x(value)) {
        Ok(input) => input
            .get("x")
            .cloned()
            .map_or(Argument::Missing, Argument::Value),
        Err(BuiltinError::Tool(sentence)) => Argument::Refused(sentence),
        Err(error) => panic!("{error}"),
    }
}

fn given(value: Value) -> Option<Value> {
    Some(value)
}

// Vine rules of built-in tools: numbers and choices

#[test]
fn takes_a_whole_number_quoted_or_not_and_nothing_that_only_reads_like_one() {
    let per_page = argument(integer(1..=100).optional());
    let sentence = "x must be an integer between 1 and 100";

    for (value, expected) in [
        (json!(5), 5),
        (json!("5"), 5),
        (json!(" 7 "), 7),
        (json!("007"), 7),
        (json!(100), 100),
        (json!(1.0), 1),
    ] {
        let read = per_page(given(value.clone()));
        assert_eq!(read, Argument::Value(json!(expected)), "{value}");
        // A whole number, whichever way it was written.
        assert!(
            matches!(read, Argument::Value(number) if number.is_u64()),
            "{value}"
        );
    }
    for value in [None, given(json!(null)), given(json!(""))] {
        assert_eq!(per_page(value.clone()), Argument::Missing, "{value:?}");
    }
    // Vine's own number type would read every one of these as a number.
    for value in [
        json!(0),
        json!(101),
        json!(1.5),
        json!("1.0"),
        json!("1e1"),
        json!("0x10"),
        json!("+5"),
        json!(" "),
        json!(true),
        json!([5]),
        json!({}),
        json!("５"),
    ] {
        assert_eq!(per_page(given(value.clone())), refused(sentence), "{value}");
    }
}

#[test]
fn names_the_bounds_of_a_whole_number_and_stays_within_the_safe_integers() {
    let id = argument(integer(1..));
    let max_safe_integer = 9_007_199_254_740_991_u64;

    assert_eq!(
        id(given(json!("15000000001"))),
        Argument::Value(json!(15_000_000_001_u64))
    );
    assert_eq!(
        id(given(json!(max_safe_integer))),
        Argument::Value(json!(max_safe_integer))
    );
    for value in [
        json!(0),
        json!(-1),
        json!("abc"),
        json!("12/../../athlete"),
        json!(max_safe_integer + 1),
        json!("9007199254740993"),
    ] {
        assert_eq!(
            id(given(value.clone())),
            refused("x must be an integer of at least 1"),
            "{value}"
        );
    }
    assert_eq!(id(None), refused("x is required"));
    assert_eq!(id(given(json!(""))), refused("x is required"));

    let category = argument(integer(0..=5));
    assert_eq!(
        category(given(json!(6))),
        refused("x must be an integer between 0 and 5")
    );
}

#[test]
fn takes_a_number_quoted_or_not_within_its_bounds() {
    let latitude = argument(number(-90..=90).optional());
    let sentence = "x must be a number between -90 and 90";

    for (value, expected) in [
        (json!(45.5), json!(45.5)),
        (json!("45.5"), json!(45.5)),
        (json!(" -90 "), json!(-90)),
        (json!("1e1"), json!(10)),
        (json!(0), json!(0)),
    ] {
        assert_eq!(
            latitude(given(value.clone())),
            Argument::Value(expected),
            "{value}"
        );
    }
    assert_eq!(latitude(given(json!(""))), Argument::Missing);
    for value in [
        json!(120),
        json!(-90.01),
        json!(" "),
        json!("north"),
        json!("Infinity"),
        json!(true),
        json!(false),
        json!([45]),
        json!({}),
    ] {
        assert_eq!(latitude(given(value.clone())), refused(sentence), "{value}");
    }

    let distance = argument(number(0..=10_000_000));
    assert_eq!(
        distance(given(json!(-5))),
        refused("x must be a number between 0 and 10000000")
    );
    assert_eq!(distance(given(json!(null))), refused("x is required"));
}

#[test]
fn takes_true_and_false_quoted_or_not_and_no_other_way_to_say_them() {
    let flag = argument(boolean().optional());

    assert_eq!(flag(given(json!(true))), Argument::Value(json!(true)));
    assert_eq!(flag(given(json!("true"))), Argument::Value(json!(true)));
    assert_eq!(flag(given(json!(false))), Argument::Value(json!(false)));
    assert_eq!(flag(given(json!("false"))), Argument::Value(json!(false)));
    assert_eq!(flag(given(json!(""))), Argument::Missing);
    assert_eq!(flag(given(json!(null))), Argument::Missing);
    // Vine's own boolean type would take all of these.
    for value in [
        json!(1),
        json!(0),
        json!("1"),
        json!("0"),
        json!("on"),
        json!("off"),
        json!("TRUE"),
        json!(" true "),
    ] {
        assert_eq!(
            flag(given(value.clone())),
            refused("x must be true or false"),
            "{value}"
        );
    }
}

#[test]
fn lists_the_choices_of_an_argument_that_has_a_few() {
    let activity_type = argument(choice(["riding", "running"]).optional());

    assert_eq!(
        activity_type(given(json!("running"))),
        Argument::Value(json!("running"))
    );
    assert_eq!(activity_type(given(json!(""))), Argument::Missing);
    for value in [
        json!("Riding"),
        json!(" riding"),
        json!("walking"),
        json!(1),
        json!(["riding"]),
    ] {
        assert_eq!(
            activity_type(given(value.clone())),
            refused("x must be one of: riding, running"),
            "{value}"
        );
    }
}

// Vine rules of built-in tools: text

#[test]
fn keeps_text_as_written_an_empty_one_included() {
    let description = argument(text(10).optional());

    for written in ["", "  two  ", "a\nb", "xxxxxxxxxx"] {
        assert_eq!(
            description(given(json!(written))),
            Argument::Value(json!(written))
        );
    }
    assert_eq!(description(given(json!(null))), Argument::Missing);
    for value in [
        json!("xxxxxxxxxxx"),
        json!(5),
        json!(true),
        json!(["a"]),
        json!({}),
    ] {
        assert_eq!(
            description(given(value.clone())),
            refused("x must be text of at most 10 characters"),
            "{value}"
        );
    }

    let body = argument(text(10).parse(blank_as_missing));
    assert_eq!(body(given(json!(""))), refused("x is required"));
    assert_eq!(body(given(json!(" "))), Argument::Value(json!(" ")));
}

#[test]
fn trims_text_and_counts_what_is_left_empty_as_left_out() {
    let name = argument(trimmed_text(10));

    assert_eq!(
        name(given(json!(" Evening "))),
        Argument::Value(json!("Evening"))
    );
    assert_eq!(name(given(json!("a\nb"))), Argument::Value(json!("a\nb")));
    assert_eq!(name(given(json!("   "))), refused("x is required"));
    assert_eq!(name(given(json!(""))), refused("x is required"));
    // The limit is on what was written, spaces included.
    assert_eq!(
        name(given(json!(format!(" {}", "x".repeat(9))))),
        Argument::Value(json!("x".repeat(9)))
    );
    assert_eq!(
        name(given(json!(format!(" {}", "x".repeat(10))))),
        refused("x must be text of at most 10 characters")
    );
    assert_eq!(
        name(given(json!(5))),
        refused("x must be text of at most 10 characters")
    );
}

#[test]
fn refuses_control_characters_in_a_single_line_of_text() {
    let mailbox = argument(line(10).optional());

    assert_eq!(
        mailbox(given(json!(" Archive "))),
        Argument::Value(json!("Archive"))
    );
    assert_eq!(
        mailbox(given(json!("Boîte"))),
        Argument::Value(json!("Boîte"))
    );
    assert_eq!(mailbox(given(json!(" \n "))), Argument::Missing);
    for value in [
        "a\nb",
        "a\rb",
        "a\tb",
        "a\u{0000}b",
        "a\u{007f}b",
        "a\u{0085}b",
    ] {
        assert_eq!(
            mailbox(given(json!(value))),
            refused("x must be a single line of text"),
            "{value:?}"
        );
    }
    assert_eq!(
        mailbox(given(json!("x".repeat(11)))),
        refused("x must be text of at most 10 characters")
    );
    assert_eq!(
        argument(line(10))(given(json!("  "))),
        refused("x is required")
    );
}

#[test]
fn takes_an_identifier_only_when_it_matches_its_pattern_in_full() {
    let gear = argument(pattern(
        r"^[bg]\d{1,20}$",
        "a gear identifier such as b1234567",
    ));
    let sentence = "x must be a gear identifier such as b1234567";

    assert_eq!(
        gear(given(json!("b1234567"))),
        Argument::Value(json!("b1234567"))
    );
    assert_eq!(gear(given(json!(" g1 "))), Argument::Value(json!("g1")));
    for value in [
        json!("../athlete"),
        json!("b12?x=1"),
        json!("B12"),
        json!("b"),
        json!(" "),
        json!(12),
        json!(true),
        json!(["b1"]),
    ] {
        assert_eq!(gear(given(value.clone())), refused(sentence), "{value}");
    }
    assert_eq!(gear(given(json!(""))), refused("x is required"));

    // An identifier made of digits may come as a number.
    let part = argument(pattern(r"^\d{1,3}(\.\d{1,3}){0,9}$", "a part"));
    assert_eq!(part(given(json!(2))), Argument::Value(json!("2")));
    assert_eq!(part(given(json!(1.2))), Argument::Value(json!("1.2")));
    assert_eq!(part(given(json!(1000))), refused("x must be a part"));
}

#[test]
fn reads_iso_8601_dates_as_utc_unless_they_say_otherwise() {
    let after = argument(iso_date().optional());

    assert_eq!(
        after(given(json!("2026-09-01"))),
        Argument::Value(json!("2026-09-01T00:00:00.000Z"))
    );
    assert_eq!(
        after(given(json!(" 2026-10-01T12:00:00+02:00 "))),
        Argument::Value(json!("2026-10-01T10:00:00.000Z"))
    );
    assert_eq!(after(given(json!(""))), Argument::Missing);
    for value in [
        json!("last week"),
        json!("2026-13-01"),
        json!("2026-02-30"),
        json!(" "),
        json!(1_767_225_600),
        json!(true),
    ] {
        assert_eq!(
            after(given(value.clone())),
            refused(
                "x must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z"
            ),
            "{value}"
        );
    }

    // The tool reads the instant where the TypeScript one had a Luxon `DateTime`.
    #[derive(Deserialize)]
    struct Input {
        x: Option<DateTime<Utc>>,
    }
    let validator = TOOL_VINE.create(vine::object! { "x" => iso_date().optional() });
    let input: Input =
        tool_input(&validator, &json!({ "x": "2026-10-01T12:00:00+02:00" })).unwrap();
    assert_eq!(
        input.x.map(|instant| instant.timestamp()),
        Some(1_790_848_800)
    );
    let input: Input = tool_input(&validator, &json!({})).unwrap();
    assert_eq!(input.x, None);
}

#[test]
fn keeps_the_clock_reading_of_a_local_time_whatever_offset_it_names() {
    let start = argument(local_timestamp());

    assert_eq!(
        start(given(json!("2026-10-03T19:30:00+02:00"))),
        Argument::Value(json!("2026-10-03T19:30:00Z"))
    );
    assert_eq!(
        start(given(json!("2026-10-03T08:00:00"))),
        Argument::Value(json!("2026-10-03T08:00:00Z"))
    );
    assert_eq!(
        start(given(json!("2026-10-03"))),
        Argument::Value(json!("2026-10-03T00:00:00Z"))
    );
    assert_eq!(
        start(given(json!("tomorrow"))),
        refused("x must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00")
    );
    assert_eq!(start(None), refused("x is required"));
}

// Vine rules of built-in tools: what the agent reads

fn agent_validator() -> vine::Validator {
    TOOL_VINE.create(vine::object! {
        "first" => integer(1..),
        "second" => boolean().optional(),
        "ids" => vine::array(integer(1..=9))
            .use_rule(list_length(1..=3, "{{ field }} must be a list of 1 to 3 ids"))
            .optional(),
    })
}

/// The sentence the agent reads, or `None` when the arguments are fine.
fn refusal(arguments: Option<Value>) -> Option<String> {
    match tool_input::<Value>(&agent_validator(), arguments.as_ref()) {
        Ok(_) => None,
        Err(BuiltinError::Tool(sentence)) => Some(sentence),
        Err(error) => panic!("{error}"),
    }
}

#[test]
fn reports_one_argument_at_a_time_in_the_order_the_schema_lists_them() {
    assert_eq!(
        refusal(given(json!({ "second": "maybe", "ids": "x" }))).as_deref(),
        Some("first is required")
    );
    assert_eq!(
        refusal(given(json!({ "first": 0, "second": "maybe" }))).as_deref(),
        Some("first must be an integer of at least 1")
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "second": "maybe", "ids": [] }))).as_deref(),
        Some("second must be true or false")
    );
    assert_eq!(refusal(given(json!({ "first": 1 }))), None);
}

#[test]
fn names_an_item_of_a_list_after_the_list() {
    let list = "ids must be a list of 1 to 3 ids";
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [] }))).as_deref(),
        Some(list)
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, 2, 3, 4] }))).as_deref(),
        Some(list)
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, "x"] }))).as_deref(),
        Some("ids must be an integer between 1 and 9")
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, null] }))).as_deref(),
        Some("ids is required")
    );
    let input: Value =
        tool_input(&agent_validator(), &json!({ "first": 1, "ids": ["2", 3] })).unwrap();
    assert_eq!(input, json!({ "first": 1, "ids": [2, 3] }));
}

#[test]
fn drops_the_arguments_a_tool_does_not_know_and_refuses_what_is_not_a_set_of_them() {
    let input: Value = tool_input(
        &agent_validator(),
        &json!({ "first": "1", "extra": true, "constructor": 1 }),
    )
    .unwrap();
    assert_eq!(input, json!({ "first": 1 }));
    let input: Value = tool_input(&NO_ARGUMENTS_VALIDATOR, &json!({ "anything": 1 })).unwrap();
    assert_eq!(input, json!({}));
    let input: NoArguments =
        tool_input(&NO_ARGUMENTS_VALIDATOR, &json!({ "anything": 1 })).unwrap();
    assert_eq!(input, NoArguments {});
    assert_eq!(
        refusal(given(json!(null))).as_deref(),
        Some("arguments is required")
    );
    assert_eq!(refusal(None).as_deref(), Some("arguments is required"));
    assert_eq!(
        refusal(given(json!([1]))).as_deref(),
        Some("arguments must be an object")
    );
    assert_eq!(
        refusal(given(json!("first"))).as_deref(),
        Some("arguments must be an object")
    );
}

#[test]
fn leaves_an_empty_string_alone_unlike_the_vine_of_the_pages() {
    // start/validator.ts turns empty strings into null for HTML forms.
    let form = vine::global().create(vine::object! { "description" => vine::string().optional() });
    let tool = TOOL_VINE.create(vine::object! { "description" => text(100).optional() });
    assert_eq!(
        form.validate(&json!({ "description": "" })).unwrap(),
        json!({})
    );
    let input: Value = tool_input(&tool, &json!({ "description": "" })).unwrap();
    assert_eq!(input, json!({ "description": "" }));
}

#[test]
fn says_how_each_argument_reads_in_a_json_schema() {
    let described = TOOL_VINE.create(vine::object! {
        "id" => integer(1..),
        "per_page" => integer(1..=100).optional(),
        "weight" => number(20..=400),
        "starred" => boolean().optional(),
        "kind" => choice(["riding", "running"]).optional(),
        "name" => trimmed_text(255),
        "mailbox" => line(255).optional(),
        "gear_id" => pattern("^b$", "b").optional(),
        "after" => iso_date().optional(),
        "start" => local_timestamp(),
        "ids" => vine::array(integer(1..)).use_rule(list_length(1..=3, "")).optional(),
    });

    assert_eq!(
        described.to_json_schema(),
        &json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "minimum": 1 },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100 },
                "weight": { "type": "number", "minimum": 20, "maximum": 400 },
                "starred": { "type": "boolean" },
                "kind": { "enum": ["riding", "running"] },
                "name": { "type": "string", "maxLength": 255 },
                "mailbox": { "type": "string", "maxLength": 255 },
                "gear_id": { "type": "string" },
                "after": { "type": "string" },
                "start": { "type": "string" },
                "ids": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 1 },
                    "minItems": 1,
                    "maxItems": 3,
                },
            },
            "required": ["id", "weight", "name", "start"],
            "additionalProperties": false,
        })
    );
}

// Vine rules of built-in tools: tools

struct Greeting {
    greeting: &'static str,
}

#[derive(Deserialize)]
struct Greet {
    name: String,
    times: Option<usize>,
}

fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

fn greet_schema() -> Value {
    json!({ "type": "object", "properties": { "name": { "type": "string" } } })
}

fn greet() -> BuiltinTool<Greeting> {
    BuiltinTool::new(
        "greet",
        "Greets.",
        greet_schema(),
        TOOL_VINE.create(vine::object! {
            "name" => trimmed_text(20),
            "times" => integer(1..=3).optional(),
        }),
        |input: Greet, context: Arc<Greeting>| async move {
            let greeting = format!("{} {}", context.greeting, input.name);
            Ok(json!(vec![greeting; input.times.unwrap_or(1)]))
        },
    )
}

#[tokio::test]
async fn runs_a_tool_with_its_arguments_as_the_validator_returns_them() {
    let tool = greet();
    let context = Arc::new(Greeting { greeting: "Hi" });

    let greetings = tool
        .run(
            arguments(json!({ "name": "  Ada ", "times": "2", "other": 1 })),
            context,
        )
        .await;
    assert_eq!(greetings.unwrap(), json!(["Hi Ada", "Hi Ada"]));
    assert_eq!(tool.input.json_schema()["required"], json!(["name"]));
}

#[tokio::test]
async fn refuses_wrong_arguments_with_a_builtin_tool_error_before_the_tool_runs() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let counted = BuiltinTool::new(
        "greet",
        "Greets.",
        greet_schema(),
        TOOL_VINE.create(vine::object! { "name" => trimmed_text(20) }),
        |_: Value, _: Arc<Greeting>| async move {
            RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        },
    );
    let context = || Arc::new(Greeting { greeting: "Hi" });

    let error = counted
        .run(arguments(json!({})), context())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "name is required");
    let error = counted
        .run(arguments(json!({ "name": "x".repeat(21) })), context())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "name must be text of at most 20 characters"
    );
    let error = counted
        .run(arguments(json!({ "name": 5 })), context())
        .await
        .unwrap_err();
    assert!(matches!(error, BuiltinError::Tool(_)));
    assert!(error.is_tool_error());
    // What a person is asked to approve goes through the same check.
    let error = counted
        .describe_call(arguments(json!({})), context())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "name is required");
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);

    assert_eq!(
        counted
            .run(arguments(json!({ "name": "Ada" })), context())
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        counted
            .describe_call(arguments(json!({ "name": "Ada" })), context())
            .await
            .unwrap(),
        None
    );
    assert_eq!(RUNS.load(Ordering::SeqCst), 1);
}

struct Known {
    known: Vec<&'static str>,
}

fn known_rule() -> vine::Rule {
    vine::rule(|value, field| {
        let known = field.meta::<Known>().is_some_and(|context| {
            value
                .as_str()
                .is_some_and(|name| context.known.contains(&name))
        });
        if !known {
            field.report("{{ field }} must be someone we know", "known");
        }
    })
}

#[tokio::test]
async fn passes_the_context_of_the_call_to_the_rules_that_depend_on_it() {
    let validator =
        TOOL_VINE.create(vine::object! { "name" => trimmed_text(20).use_rule(known_rule()) });
    let context = Known { known: vec!["Ada"] };

    let input: Value = tool_input_with(&validator, &json!({ "name": " Ada " }), &context).unwrap();
    assert_eq!(input, json!({ "name": "Ada" }));
    let error =
        tool_input_with::<Value, _>(&validator, &json!({ "name": "Bob" }), &context).unwrap_err();
    assert_eq!(error.to_string(), "name must be someone we know");
    // Without the context, the rule knows nobody.
    assert!(tool_input::<Value>(&validator, &json!({ "name": "Ada" })).is_err());

    // A tool hands its rules the context it runs with.
    let tool = BuiltinTool::new(
        "greet",
        "Greets.",
        greet_schema(),
        validator,
        |input: Value, _: Arc<Known>| async move { Ok(input) },
    );
    let context = Arc::new(context);
    let input = tool
        .run(arguments(json!({ "name": "Ada" })), context.clone())
        .await
        .unwrap();
    assert_eq!(input, json!({ "name": "Ada" }));
    let error = tool
        .run(arguments(json!({ "name": "Bob" })), context)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "name must be someone we know");
}

#[test]
fn takes_a_validator_kept_in_a_static_as_the_input_of_a_tool() {
    let tool = BuiltinTool::new(
        "get_athlete",
        "Reads the athlete.",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, _: Arc<Greeting>| async move { Ok(Value::Null) },
    );
    assert_eq!(
        tool.input.json_schema(),
        json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false })
    );
    let context = Greeting { greeting: "Hi" };
    assert_eq!(
        tool.input.validate(json!({ "anything": 1 }), &context),
        Ok(json!({}))
    );
    assert_eq!(
        tool.input.validate(json!([]), &context),
        Err("arguments must be an object".to_owned())
    );
}
