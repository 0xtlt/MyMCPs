//! Vine rules shared by the tools of built-in MCPs, for the arguments agents
//! call them with: the port of `app/validators/builtin_tools.ts`.
//!
//! A tool describes its arguments as a schema made of the types below, in
//! the order they are checked: of several wrong ones, the agent is told
//! about the first. Each type says what is wrong in one sentence naming the
//! argument, takes the ways agents write a value (a quoted number, an empty
//! string for an argument left out), and says how it reads in a JSON Schema.
//!
//! | TypeScript | Rust | The tool receives |
//! |---|---|---|
//! | `toolVine` | [`TOOL_VINE`] | |
//! | `toolVine.create({ a: ..., b: ... })` | `TOOL_VINE.create(vine::object! { "a" => ..., "b" => ... })` | |
//! | `new VineArgument<T>(rule())` | [`argument(rule())`](argument), a [`VineArgument`] | |
//! | `isBlank(value)` | [`is_blank(value)`](is_blank) | |
//! | `blankAsMissing` | [`blank_as_missing`], as `.parse(blank_as_missing)` | |
//! | `integer({ min: 1 })` | [`integer(1..)`](integer) | an integer (`i64`, `u32`, ...) |
//! | `integer({ min: 1, max: 100 })` | `integer(1..=100)` | an integer |
//! | `number({ min: -90, max: 90 })` | [`number(-90..=90)`](number) | a number (`f64`) |
//! | `boolean()` | [`boolean()`] | a `bool` |
//! | `choice(['riding', 'running'])` | [`choice(["riding", "running"])`](choice) | a `String`, or an enum that deserializes from it |
//! | `text(max)` | [`text(max)`](text) | a `String`, as written |
//! | `trimmedText(max)` | [`trimmed_text(max)`](trimmed_text) | a `String`, trimmed and not empty |
//! | `line(max)` | [`line(max)`](line()) | a `String` on one line |
//! | `pattern(/^\d{1,19}$/, hint)` | [`pattern(r"^\d{1,19}$", hint)`](pattern) | a `String`, trimmed |
//! | `pattern(new RegExp(source, 'u'), hint)` | [`pattern_with(vine::js::regex(source, "u")?, hint)`](pattern_with) | a `String`, trimmed |
//! | `uploadedFileName(max)` | [`uploaded_file_name(max)`](uploaded_file_name) | a `String` |
//! | `mediaType()` | [`media_type()`] | a `String` |
//! | `isoDate()` | [`iso_date()`] | a `chrono::DateTime<Utc>` |
//! | `localTimestamp()` | [`local_timestamp()`] | a `String` such as `2026-10-03T19:30:00Z` |
//! | `listLength({ min: 1, max: 100, sentence })` | [`list_length(1..=100, sentence)`](list_length) | |
//! | `listLength({ max: 10, sentence })` | `list_length(..=10, sentence)` | |
//! | `noArgumentsValidator` | [`NO_ARGUMENTS_VALIDATOR`] | [`NoArguments`] |
//! | `.optional()`, `.use(rule())`, `.parse(fn)` | `.optional()`, `.use_rule(rule())`, `.parse(\|value, context\| ...)` | `Option<T>` for an optional argument |
//!
//! A tool that needs a rule of its own writes it with `vine::rule`, as the
//! ones here are written, and reads the context of the call with
//! `field.meta::<C>()`, where `C` is the context type of the tool
//! (`BuiltinToolContext` or `BuiltinPasswordContext`).
//!
//! # A tool's arguments, and what the tool receives
//!
//! ```
//! use std::sync::LazyLock;
//!
//! use chrono::{DateTime, Utc};
//! use mymcps_builtin::arguments::{TOOL_VINE, boolean, choice, integer, iso_date, pattern, trimmed_text};
//! use mymcps_builtin::tool_input::tool_input;
//! use mymcps_vine as vine;
//! use serde::Deserialize;
//! use serde_json::json;
//!
//! // const pagination = () => ({
//! //   page: integer({ min: 1 }).optional(),
//! //   per_page: integer({ min: 1, max: STRAVA_LIMITS.pageSize }).optional(),
//! // })
//! fn pagination() -> Vec<(String, vine::Schema)> {
//!     vine::properties! {
//!         "page" => integer(1..).optional(),
//!         "per_page" => integer(1..=100).optional(),
//!     }
//! }
//!
//! // export const listActivitiesValidator = toolVine.create({
//! //   after: isoDate().optional(),
//! //   name: trimmedText(255),
//! //   gear_id: pattern(/^[bg]\d{1,20}$/, 'a gear identifier such as b1234567').optional(),
//! //   activity_type: choice(['riding', 'running']).optional(),
//! //   commute: boolean().optional(),
//! //   ...pagination(),
//! // })
//! static LIST_ACTIVITIES_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
//!     TOOL_VINE.create(vine::object! {
//!         "after" => iso_date().optional(),
//!         "name" => trimmed_text(255),
//!         "gear_id" => pattern(r"^[bg]\d{1,20}$", "a gear identifier such as b1234567").optional(),
//!         "activity_type" => choice(["riding", "running"]).optional(),
//!         "commute" => boolean().optional(),
//!         ..pagination(),
//!     })
//! });
//!
//! // What `run` and `describe` receive: `Infer<typeof listActivitiesValidator>`.
//! #[derive(Debug, Deserialize)]
//! struct ListActivities {
//!     after: Option<DateTime<Utc>>,
//!     name: String,
//!     gear_id: Option<String>,
//!     activity_type: Option<ActivityType>,
//!     commute: Option<bool>,
//!     page: Option<u32>,
//!     per_page: Option<u32>,
//! }
//!
//! #[derive(Debug, PartialEq, Deserialize)]
//! #[serde(rename_all = "lowercase")]
//! enum ActivityType {
//!     Riding,
//!     Running,
//! }
//!
//! // Agents quote numbers and booleans, and send "" for what they leave out.
//! let arguments = json!({
//!     "after": "2026-09-01",
//!     "name": " Evening ride ",
//!     "gear_id": "",
//!     "activity_type": "riding",
//!     "commute": "true",
//!     "per_page": "50",
//!     "unknown": 1,
//! });
//! let input: ListActivities = tool_input(&LIST_ACTIVITIES_VALIDATOR, &arguments).unwrap();
//! assert_eq!(input.after.unwrap().timestamp(), 1_788_220_800);
//! assert_eq!(input.name, "Evening ride");
//! assert_eq!(input.gear_id, None);
//! assert_eq!(input.activity_type, Some(ActivityType::Riding));
//! assert_eq!(input.commute, Some(true));
//! assert_eq!((input.page, input.per_page), (None, Some(50)));
//!
//! // The first argument that is wrong, in the order of the schema.
//! let refused = tool_input::<ListActivities>(
//!     &LIST_ACTIVITIES_VALIDATOR,
//!     &json!({ "after": "last week", "per_page": 500 }),
//! );
//! assert_eq!(
//!     refused.unwrap_err().to_string(),
//!     "after must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z"
//! );
//!
//! // What the validator enforces, to compare with what the tool advertises.
//! let enforced = LIST_ACTIVITIES_VALIDATOR.to_json_schema();
//! assert_eq!(enforced["required"], json!(["name"]));
//! assert_eq!(enforced["properties"]["per_page"], json!({ "type": "integer", "minimum": 1, "maximum": 100 }));
//! ```
//!
//! # Where the port differs
//!
//! Dates are read as Luxon's `DateTime.fromISO` reads them, with three
//! bounds: a time zone given by name is not read, nor a date in a year
//! chrono cannot hold, and the server is taken to run in UTC. See
//! [`iso_date`] and [`local_timestamp`].

use std::ops::{RangeFrom, RangeInclusive, RangeToInclusive};
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Timelike, Utc};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use vine::{
    FieldRef, JsRegex, MessagesProvider, ParseContext, Rule, SimpleMessagesProvider, Validator,
    Vine, VineBoolean, VineCustom, VineEnum,
};

/// Says what is wrong in one sentence the agent can act on, naming the
/// argument. An item of a list is named after its list: `uids`, not `2`.
struct ArgumentMessages(SimpleMessagesProvider);

impl MessagesProvider for ArgumentMessages {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        let argument = field.wildcard_path.split('.').next().unwrap_or_default();
        let field = field.with_name(if argument.is_empty() {
            "arguments"
        } else {
            argument
        });
        match args
            .and_then(|args| args.get("choices"))
            .and_then(Value::as_array)
        {
            Some(choices) => {
                let choices: Vec<String> = choices.iter().map(vine::js::to_string).collect();
                let mut args = args.cloned().unwrap_or_default();
                args.insert("choices".to_owned(), Value::String(choices.join(", ")));
                self.0.get_message(message, rule, field, Some(&args))
            }
            None => self.0.get_message(message, rule, field, args),
        }
    }
}

/// Arguments are JSON written by an agent, not the fields of a form, so they
/// get a Vine of their own: the one the pages use turns an empty string into
/// null, and here an empty description is how a description gets cleared.
pub static TOOL_VINE: LazyLock<Vine> = LazyLock::new(|| {
    // The rules of Vine's own types that the schemas use. The others below bring their sentence.
    Vine::new().messages_provider(ArgumentMessages(SimpleMessagesProvider::new([
        ("required", "{{ field }} is required"),
        ("object", "{{ field }} must be an object"),
        ("boolean", "{{ field }} must be true or false"),
        ("enum", "{{ field }} must be one of: {{ choices }}"),
    ])))
});

/// An argument checked by the rules below. Vine's string and number types
/// cannot tell the agent what was expected: their type check knows neither
/// the bounds nor the format to name. Each rule also says how its argument
/// reads in a JSON Schema, to compare with the one the tool advertises.
pub type VineArgument = VineCustom;

/// `new VineArgument(rule)`: an argument that is whatever its rule accepts.
pub fn argument(rule: Rule) -> VineArgument {
    vine::custom([rule])
}

/// `value === undefined || value === null || value === ''`. `None` stands
/// for `undefined`.
pub fn is_blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

/// An empty string is one of the ways agents leave an argument out. Written
/// to be given to `.parse(...)` as it is.
pub fn blank_as_missing(value: Option<Value>, _: &ParseContext<'_>) -> Option<Value> {
    value.filter(|value| value.as_str() != Some(""))
}

/// The bounds of [`integer`]: `1..` or `1..=100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegerRange {
    pub min: i64,
    pub max: Option<i64>,
}

impl From<RangeFrom<i64>> for IntegerRange {
    fn from(range: RangeFrom<i64>) -> Self {
        Self {
            min: range.start,
            max: None,
        }
    }
}

impl From<RangeInclusive<i64>> for IntegerRange {
    fn from(range: RangeInclusive<i64>) -> Self {
        Self {
            min: *range.start(),
            max: Some(*range.end()),
        }
    }
}

/// `Number.MAX_SAFE_INTEGER`
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A bound of a range as the number JavaScript compares with. The bounds
/// of the tools are far below 2^53, where the conversion is exact.
fn bound(value: i64) -> f64 {
    value as f64
}

fn integer_rule(range: IntegerRange) -> Rule {
    static QUOTED: LazyLock<JsRegex> =
        LazyLock::new(|| vine::js::regex(r"^-?\d+$", "").expect("static regex"));
    let IntegerRange { min, max } = range;
    vine::rule(move |value, field| {
        // Agents often quote large identifiers.
        let parsed = match value {
            Value::String(text) if QUOTED.test(vine::js::trim(text)) => {
                Some(vine::js::string_to_number(text))
            }
            other => vine::js::as_f64(other),
        };
        let accepted = parsed.filter(|parsed| {
            vine::js::is_safe_integer(*parsed)
                && *parsed >= bound(min)
                && *parsed <= max.map_or(MAX_SAFE_INTEGER, bound)
        });
        let Some(parsed) = accepted else {
            match max {
                None => field.report_with(
                    "{{ field }} must be an integer of at least {{ min }}",
                    "integer",
                    json!({ "min": min }),
                ),
                Some(max) => field.report_with(
                    "{{ field }} must be an integer between {{ min }} and {{ max }}",
                    "integer",
                    json!({ "min": min, "max": max }),
                ),
            }
            return;
        };
        field.mutate(vine::js::number(parsed));
    })
    .json_schema(move |schema| {
        schema.insert("type".to_owned(), json!("integer"));
        schema.insert("minimum".to_owned(), json!(min));
        if let Some(max) = max {
            schema.insert("maximum".to_owned(), json!(max));
        }
    })
}

/// A whole number, also when it is quoted. `integer(1..)` has no upper
/// bound but the largest integer JavaScript counts exactly.
pub fn integer(range: impl Into<IntegerRange>) -> VineArgument {
    argument(integer_rule(range.into())).parse(blank_as_missing)
}

/// The bounds of [`number`]: `-90..=90` or `0.01..=1000.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberRange {
    pub min: f64,
    pub max: f64,
}

impl<N: Into<f64> + Copy> From<RangeInclusive<N>> for NumberRange {
    fn from(range: RangeInclusive<N>) -> Self {
        Self {
            min: (*range.start()).into(),
            max: (*range.end()).into(),
        }
    }
}

fn number_rule(range: NumberRange) -> Rule {
    let NumberRange { min, max } = range;
    vine::rule(move |value, field| {
        let parsed = match value {
            Value::String(text) if !vine::js::trim(text).is_empty() => {
                Some(vine::js::string_to_number(text))
            }
            other => vine::js::as_f64(other),
        };
        match parsed.filter(|parsed| parsed.is_finite() && *parsed >= min && *parsed <= max) {
            Some(parsed) => field.mutate(vine::js::number(parsed)),
            None => field.report_with(
                "{{ field }} must be a number between {{ min }} and {{ max }}",
                "number",
                json!({ "min": vine::js::number(min), "max": vine::js::number(max) }),
            ),
        }
    })
    .json_schema(move |schema| {
        schema.insert("type".to_owned(), json!("number"));
        schema.insert("minimum".to_owned(), vine::js::number(min));
        schema.insert("maximum".to_owned(), vine::js::number(max));
    })
}

/// A number, also when it is quoted.
pub fn number(range: impl Into<NumberRange>) -> VineArgument {
    argument(number_rule(range.into())).parse(blank_as_missing)
}

/// JSON booleans, and the same two words quoted. Vine's own conversion would
/// also take 1 and "on".
pub fn boolean() -> VineBoolean {
    vine::boolean().strict().parse(
        |value, context| match value.as_ref().and_then(Value::as_str) {
            Some("true") => Some(Value::Bool(true)),
            Some("false") => Some(Value::Bool(false)),
            _ => blank_as_missing(value, context),
        },
    )
}

/// One of a few words.
pub fn choice(values: impl IntoIterator<Item = impl Into<Value>>) -> VineEnum {
    vine::enum_(values).parse(blank_as_missing)
}

fn text_rule(max: usize) -> Rule {
    vine::rule(move |value, field| {
        if !value
            .as_str()
            .is_some_and(|text| vine::js::utf16_len(text) <= max)
        {
            field.report_with(
                "{{ field }} must be text of at most {{ max }} characters",
                "text",
                json!({ "max": max }),
            );
        }
    })
    .json_schema(move |schema| {
        schema.insert("type".to_owned(), json!("string"));
        schema.insert("maxLength".to_owned(), json!(max));
    })
}

/// Text as written: an empty one is a value. `max` counts as JavaScript's
/// `length` does, in UTF-16 code units.
pub fn text(max: usize) -> VineArgument {
    argument(text_rule(max))
}

/// Text without the spaces around it. Left out when nothing remains.
pub fn trimmed_text(max: usize) -> VineArgument {
    // Text that is too long is left for the rule to refuse.
    text(max).parse(move |value, _| match value {
        Some(Value::String(text)) if vine::js::utf16_len(&text) <= max => {
            let trimmed = vine::js::trim(&text);
            (!trimmed.is_empty()).then(|| Value::String(trimmed.to_owned()))
        }
        other => other,
    })
}

fn single_line_rule() -> Rule {
    vine::rule(|value, field| {
        // `/\p{Cc}/u`
        if value
            .as_str()
            .is_some_and(|text| text.chars().any(char::is_control))
        {
            field.report("{{ field }} must be a single line of text", "line");
        }
    })
}

/// One line of text: it ends up in a mail header or an IMAP command.
pub fn line(max: usize) -> VineArgument {
    trimmed_text(max).use_rule(single_line_rule())
}

fn json_schema_string(schema: &mut Map<String, Value>) {
    schema.insert("type".to_owned(), json!("string"));
}

fn pattern_rule(expression: JsRegex, hint: &str) -> Rule {
    let hint = hint.to_owned();
    vine::rule(move |value, field| {
        let written = match value {
            Value::Number(_) => Some(vine::js::to_string(value)),
            Value::String(text) => Some(text.clone()),
            _ => None,
        };
        match written.map(|written| vine::js::trim(&written).to_owned()) {
            Some(written) if expression.test(&written) => field.mutate(written),
            _ => field.report_with(
                "{{ field }} must be {{ hint }}",
                "pattern",
                json!({ "hint": hint }),
            ),
        }
    })
    .json_schema(json_schema_string)
}

/// Identifiers end up in request paths, so only an exact pattern match is
/// accepted. `expression` is the JavaScript regular expression, without its
/// slashes: `pattern(r"^\d{1,19}$", "the numeric ID of a campaign")`. A
/// number is taken as its digits.
///
/// # Panics
///
/// When `expression` is not a regular expression [`vine::js::regex`] can
/// translate: a mistake in the source of a tool, which the first use of its
/// validator brings out.
pub fn pattern(expression: &'static str, hint: &str) -> VineArgument {
    pattern_with(vine::js::regex(expression, "").expect("static regex"), hint)
}

/// [`pattern`] for an expression that has flags or is not a constant.
pub fn pattern_with(expression: JsRegex, hint: &str) -> VineArgument {
    argument(pattern_rule(expression, hint)).parse(blank_as_missing)
}

fn file_name_rule() -> Rule {
    vine::rule(|value, field| {
        if value
            .as_str()
            .is_some_and(|name| name.contains(['\\', '/']))
        {
            field.report(
                "{{ field }} must be the name of the file, such as report.pdf, without its folder",
                "fileName",
            );
        }
    })
}

/// The name of a file sent to an upload link. It never names a file on the instance.
pub fn uploaded_file_name(max: usize) -> VineArgument {
    line(max).use_rule(file_name_rule())
}

pub fn media_type() -> VineArgument {
    pattern(
        r"^[\w.+-]{1,100}\/[\w.+-]{1,100}$",
        "a media type, such as application/pdf",
    )
}

fn iso_date_rule() -> Rule {
    vine::rule(
        |value, field| match value.as_str().and_then(|text| read_iso_date(text, Utc::now())) {
            Some(instant) => field.mutate(to_iso(instant)),
            None => field.report(
                "{{ field }} must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z",
                "isoDate",
            ),
        },
    )
    .json_schema(json_schema_string)
}

/// An ISO 8601 date or datetime. Values without an offset are read as UTC.
///
/// The TypeScript rule hands the tool a Luxon `DateTime` in UTC. Here the
/// tool receives the same instant as Luxon's `toISO()` writes it, such as
/// `2026-10-01T10:00:00.000Z`, which a `chrono::DateTime<Utc>` field of its
/// input deserializes.
///
/// What is a date is what `DateTime.fromISO` takes, down to its quirks: a
/// calendar date (`2026-10-01`, `20261001`, `2026-10`, `2026`), a week date
/// (`2026-W40-4`), an ordinal date (`2026-274`), each with an optional time
/// (`T12`, `T12:00`, `T12:00:00.5`, `T24:00`) and offset (`Z`, `+02`,
/// `+02:00`, `+0200`), or a time alone, which is a time of today. Two
/// bounds:
///
/// - a time zone by name, such as `2026-10-01T12:00[Europe/Paris]`, is not
///   read: this needs the time zone database, which the port does without;
/// - an instant chrono cannot hold, before the year -262143 or after
///   262142, is not read, where Luxon goes on to the years around ±270000.
pub fn iso_date() -> VineArgument {
    argument(iso_date_rule()).parse(blank_as_missing)
}

fn local_timestamp_rule() -> Rule {
    vine::rule(|value, field| {
        match value
            .as_str()
            .and_then(|text| read_local_timestamp(text, Utc::now()))
        {
            Some(clock) => field.mutate(clock),
            None => field.report(
                "{{ field }} must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00",
                "localTimestamp",
            ),
        }
    })
    .json_schema(json_schema_string)
}

/// A wall-clock time in the user's own timezone, which some APIs take as an
/// ISO 8601 string ending in `Z`. The clock reading is kept as written: an
/// offset in the input is not converted to UTC.
///
/// The tool receives the reading as `2026-10-03T19:30:00Z`. A value
/// without an offset is read by Luxon in the time zone of the server, where
/// a clock time that a daylight saving change skips is moved forward. The
/// server is taken to run in UTC, which is what the Docker image does and
/// where no time is skipped: the reading is always the one written.
pub fn local_timestamp() -> VineArgument {
    argument(local_timestamp_rule()).parse(blank_as_missing)
}

/// What [`iso_date`] reads: `DateTime.fromISO(text.trim(), { zone: 'utc' })`,
/// or `None` when that is not a valid date. `now` is the current time, for
/// a time written without its date.
pub fn read_iso_date(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let reading = luxon::read(vine::js::trim(text), now.timestamp_millis())?;
    let instant = reading.local - reading.offset_minutes * 60_000;
    luxon::in_range(instant)
        .then(|| DateTime::from_timestamp_millis(instant))
        .flatten()
}

/// What [`local_timestamp`] reads:
/// `DateTime.fromISO(text.trim(), { setZone: true }).toFormat("yyyy-MM-dd'T'HH:mm:ss'Z'")`,
/// or `None` when that is not a valid date.
pub fn read_local_timestamp(text: &str, now: DateTime<Utc>) -> Option<String> {
    let reading = luxon::read(vine::js::trim(text), now.timestamp_millis())?;
    let (year, month, day) = luxon::civil_from_days(reading.local.div_euclid(luxon::MS_PER_DAY));
    let in_day = reading.local.rem_euclid(luxon::MS_PER_DAY);
    let (hours, minutes, seconds) = (in_day / 3_600_000, in_day / 60_000 % 60, in_day / 1000 % 60);
    // `yyyy` pads the year to four digits, after its sign.
    let sign = if year < 0 { "-" } else { "" };
    Some(format!(
        "{sign}{:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z",
        year.abs()
    ))
}

/// Luxon's `toISO()` for a time in UTC: `2026-10-01T10:00:00.000Z`, and six
/// digits after a sign for a year that does not fit in four.
pub fn to_iso(instant: DateTime<Utc>) -> String {
    let year = match instant.year() {
        year @ 0..=9999 => format!("{year:04}"),
        year => format!("{}{:06}", if year < 0 { '-' } else { '+' }, year.abs()),
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        instant.month(),
        instant.day(),
        instant.hour(),
        instant.minute(),
        instant.second(),
        instant.timestamp_subsec_millis(),
    )
}

/// How many items a list may have: `1..=100`, or `..=10` for a list that
/// may be empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLength {
    pub min: Option<usize>,
    pub max: usize,
}

impl From<RangeInclusive<usize>> for ListLength {
    fn from(range: RangeInclusive<usize>) -> Self {
        Self {
            min: Some(*range.start()),
            max: *range.end(),
        }
    }
}

impl From<RangeToInclusive<usize>> for ListLength {
    fn from(range: RangeToInclusive<usize>) -> Self {
        Self {
            min: None,
            max: range.end,
        }
    }
}

/// How many items a list may have. `sentence` is what the agent reads when
/// it has fewer or more, since each list names its items its own way.
pub fn list_length(length: impl Into<ListLength>, sentence: &str) -> Rule {
    let ListLength { min, max } = length.into();
    let sentence = sentence.to_owned();
    vine::rule(move |value, field| {
        let length = value.as_array().map_or(0, Vec::len);
        if length < min.unwrap_or(0) || length > max {
            field.report(&sentence, "listLength");
        }
    })
    .json_schema(move |schema| {
        if let Some(min) = min {
            schema.insert("minItems".to_owned(), json!(min));
        }
        schema.insert("maxItems".to_owned(), json!(max));
    })
}

/// For the tools that take no arguments: whatever they are passed is ignored.
pub static NO_ARGUMENTS_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| TOOL_VINE.create(vine::object! {}));

/// What a tool that takes no arguments receives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct NoArguments {}

/// What Luxon's `DateTime.fromISO` makes of a string: `parseISODate` of its
/// `regexParser.js`, then `DateTime.fromObject`.
mod luxon {
    use std::sync::LazyLock;

    use mymcps_vine::JsRegex;
    use regex::Captures;

    pub(super) const MS_PER_DAY: i64 = 86_400_000;
    /// The largest time value a JavaScript `Date` holds, on either side of the epoch.
    const MAX_TIME: i64 = 8_640_000_000_000_000;

    const OFFSET: &str = r"(?:([Zz])|([+-]\d\d)(?::?(\d\d))?)";
    const IANA: &str =
        r"[A-Za-z_+-]{1,256}(?::?\/[A-Za-z0-9_+-]{1,256}(?:\/[A-Za-z0-9_+-]{1,256})?)?";
    const TIME: &str = r"(\d\d)(?::?(\d\d)(?::?(\d\d)(?:[.,](\d{1,30}))?)?)?";
    const YMD: &str = r"([+-]\d{6}|\d{4})(?:-?(\d\d)(?:-?(\d\d))?)?";
    const WEEK: &str = r"(\d{4})-?W(\d\d)(?:-?(\d))?";
    const ORDINAL: &str = r"(\d{4})-?(\d{3})";

    /// The four shapes Luxon tries, in its order: the first that matches
    /// decides, also when what it matched turns out not to be a date.
    struct Patterns {
        ymd: JsRegex,
        week: JsRegex,
        ordinal: JsRegex,
        time: JsRegex,
    }

    static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| {
        let zone = format!(r"(?:{OFFSET}?(?:\[({IANA})\])?)?");
        let time = format!("{TIME}{zone}");
        let time_extension = format!("(?:[Tt]{time})?");
        let compile = |source: String| JsRegex::new(&source, "").expect("static regex");
        Patterns {
            ymd: compile(format!("^{YMD}{time_extension}$")),
            week: compile(format!("^{WEEK}{time_extension}$")),
            ordinal: compile(format!("^{ORDINAL}{time_extension}$")),
            time: compile(format!("^{time}$")),
        }
    });

    enum Date {
        Calendar {
            year: i64,
            month: i64,
            day: i64,
        },
        Week {
            year: i64,
            week: i64,
            weekday: Option<i64>,
        },
        Ordinal {
            year: i64,
            ordinal: i64,
        },
        Today,
    }

    struct Time {
        hour: i64,
        minute: i64,
        second: i64,
        /// May be 1000, which Luxon then refuses.
        millisecond: i64,
    }

    enum Zone {
        /// No offset was written: the zone the caller reads dates in.
        Unspecified,
        /// An offset from UTC, in minutes.
        Fixed(i64),
        /// A zone by its name, between brackets.
        Named,
    }

    /// A date and time as its clock reads, and how far that clock is from UTC.
    pub(super) struct Reading {
        /// Milliseconds since the epoch of the reading, as if it were UTC.
        pub(super) local: i64,
        pub(super) offset_minutes: i64,
    }

    pub(super) fn in_range(time: i64) -> bool {
        time.abs() <= MAX_TIME
    }

    fn integer(captures: &Captures<'_>, group: usize) -> Option<i64> {
        captures
            .get(group)
            .and_then(|matched| matched.as_str().parse().ok())
    }

    /// `parseMillis`: the fraction of a second as whole milliseconds, by
    /// the same floating-point steps, which lose one now and then.
    fn milliseconds(fraction: &str) -> i64 {
        let seconds: f64 = format!("0.{fraction}").parse().unwrap_or(0.0);
        // At most 1000: the fraction is below or equal to one.
        (seconds * 1000.0).floor() as i64
    }

    /// The time and the zone that follow a date, from the group `cursor` on.
    fn time_and_zone(captures: &Captures<'_>, cursor: usize) -> (Time, Zone) {
        let time = Time {
            hour: integer(captures, cursor).unwrap_or(0),
            minute: integer(captures, cursor + 1).unwrap_or(0),
            second: integer(captures, cursor + 2).unwrap_or(0),
            millisecond: captures
                .get(cursor + 3)
                .map_or(0, |fraction| milliseconds(fraction.as_str())),
        };
        let hours = captures.get(cursor + 5).map(|matched| matched.as_str());
        let zone = if captures.get(cursor + 7).is_some() {
            // The name wins over an offset written before it.
            Zone::Named
        } else if captures.get(cursor + 4).is_none() && hours.is_none() {
            Zone::Unspecified
        } else {
            // `signedOffset`: no bound on either part, and `-00:30` is behind UTC.
            let minutes = integer(captures, cursor + 6).unwrap_or(0);
            let behind = hours.is_some_and(|hours| hours.starts_with('-'));
            let ahead = hours
                .and_then(|hours| hours[1..].parse::<i64>().ok())
                .unwrap_or(0)
                * 60
                + minutes;
            Zone::Fixed(if behind { -ahead } else { ahead })
        };
        (time, zone)
    }

    fn parse(text: &str) -> Option<(Date, Time, Zone)> {
        let patterns = &*PATTERNS;
        if let Some(captures) = patterns.ymd.as_regex().captures(text) {
            let date = Date::Calendar {
                year: integer(&captures, 1)?,
                month: integer(&captures, 2).unwrap_or(1),
                day: integer(&captures, 3).unwrap_or(1),
            };
            let (time, zone) = time_and_zone(&captures, 4);
            return Some((date, time, zone));
        }
        if let Some(captures) = patterns.week.as_regex().captures(text) {
            let date = Date::Week {
                year: integer(&captures, 1)?,
                week: integer(&captures, 2)?,
                weekday: integer(&captures, 3),
            };
            let (time, zone) = time_and_zone(&captures, 4);
            return Some((date, time, zone));
        }
        if let Some(captures) = patterns.ordinal.as_regex().captures(text) {
            let date = Date::Ordinal {
                year: integer(&captures, 1)?,
                ordinal: integer(&captures, 2)?,
            };
            let (time, zone) = time_and_zone(&captures, 3);
            return Some((date, time, zone));
        }
        let captures = patterns.time.as_regex().captures(text)?;
        let (time, zone) = time_and_zone(&captures, 1);
        Some((Date::Today, time, zone))
    }

    fn is_leap_year(year: i64) -> bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }

    fn days_in_year(year: i64) -> i64 {
        if is_leap_year(year) { 366 } else { 365 }
    }

    fn days_in_month(year: i64, month: i64) -> i64 {
        match month {
            2 if is_leap_year(year) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// Days since the epoch of a day of the proleptic Gregorian calendar.
    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let year_of_era = y.rem_euclid(400);
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// Year, month and day of a number of days since the epoch.
    pub(super) fn civil_from_days(days: i64) -> (i64, i64, i64) {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        (year_of_era + era * 400 + i64::from(month <= 2), month, day)
    }

    /// The ISO weekday, from 1 for Monday, of the fourth of January, which
    /// is always in the first week of its year.
    fn weekday_of_january_4(year: i64) -> i64 {
        // The epoch was a Thursday.
        (days_from_civil(year, 1, 4) + 3).rem_euclid(7) + 1
    }

    /// `weeksInWeekYear`: 52, or 53 in the years that have a long one.
    fn weeks_in_week_year(year: i64) -> i64 {
        let first_week_offset = |year: i64| 3 - weekday_of_january_4(year);
        (days_in_year(year) - first_week_offset(year) + first_week_offset(year + 1)) / 7
    }

    /// The day a date stands for, in days since the epoch, or `None` when
    /// one of its parts is out of range.
    fn day(date: &Date, today: i64) -> Option<i64> {
        match *date {
            Date::Calendar { year, month, day } => {
                let valid =
                    (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
                valid.then(|| days_from_civil(year, month, day))
            }
            Date::Week {
                year,
                week,
                weekday,
            } => {
                // Luxon asks whether the week year or the week number is
                // truthy: neither is in `0000-W00`, which it then reads as
                // no date at all, and so as today.
                let has_week =
                    year != 0 || week != 0 || weekday.is_some_and(|weekday| weekday != 0);
                if !has_week {
                    return Some(today);
                }
                let weekday = weekday.unwrap_or(1);
                let valid =
                    (1..=weeks_in_week_year(year)).contains(&week) && (1..=7).contains(&weekday);
                // `weekToGregorian`: a day of the year, which may be in the year before or after.
                let ordinal = week * 7 + weekday - weekday_of_january_4(year) - 3;
                valid.then(|| days_from_civil(year, 1, 1) + ordinal - 1)
            }
            Date::Ordinal { year, ordinal } => (1..=days_in_year(year))
                .contains(&ordinal)
                .then(|| days_from_civil(year, 1, 1) + ordinal - 1),
            Date::Today => Some(today),
        }
    }

    /// `DateTime.fromISO(text)`, read in UTC when the text names no offset.
    /// `now` is the time value of the present, for a time without a date.
    pub(super) fn read(text: &str, now: i64) -> Option<Reading> {
        let (date, time, zone) = parse(text)?;
        let offset_minutes = match zone {
            Zone::Unspecified => 0,
            Zone::Fixed(minutes) => minutes,
            Zone::Named => return None,
        };
        // Today is the day it is where the date is read.
        let today = (now + offset_minutes * 60_000).div_euclid(MS_PER_DAY);
        let day = day(&date, today)?;

        let Time {
            hour,
            minute,
            second,
            millisecond,
        } = time;
        // Midnight may be written as the 24th hour of the day before.
        let valid_hour = (0..=23).contains(&hour)
            || (hour == 24 && minute == 0 && second == 0 && millisecond == 0);
        let valid = valid_hour
            && (0..=59).contains(&minute)
            && (0..=59).contains(&second)
            && (0..=999).contains(&millisecond);
        if !valid {
            return None;
        }
        let local =
            day * MS_PER_DAY + hour * 3_600_000 + minute * 60_000 + second * 1000 + millisecond;
        in_range(local).then_some(Reading {
            local,
            offset_minutes,
        })
    }
}
