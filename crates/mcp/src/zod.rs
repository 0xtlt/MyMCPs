//! The part of zod 4 that the SDK's schemas use.
//!
//! The SDK validates every message with zod, and what a client or an admin
//! reads comes out of that validation: a tool result loses the keys its
//! schema does not name, the keys that stay are written in schema order, and a
//! refused value is reported with zod's own issue list as the error message.
//! This module reproduces those three effects for the schemas in
//! `schemas.rs`, so they are checked against the real library with fixtures
//! rather than rewritten by hand for each message.
//!
//! `None` stands for JavaScript's `undefined`: an absent key.

use std::fmt;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::json;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Debug, Clone, PartialEq)]
enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct Issue {
    /// The issue's own fields in the order zod writes them. `path` and
    /// `message` always follow.
    fields: Vec<(&'static str, Value)>,
    path: Vec<PathSegment>,
    message: String,
    /// zod's `continue: true`: the value it was raised on is still usable.
    continues: bool,
}

impl Issue {
    fn invalid_type(expected: &'static str, input: Option<&Value>) -> Self {
        Self {
            fields: vec![
                ("expected", json!(expected)),
                ("code", json!("invalid_type")),
            ],
            path: Vec::new(),
            message: format!(
                "Invalid input: expected {expected}, received {}",
                parsed_type(input)
            ),
            continues: false,
        }
    }

    fn finalize(&self) -> Value {
        let mut issue = Map::new();
        for (name, value) in &self.fields {
            issue.insert((*name).to_owned(), value.clone());
        }
        let path = self
            .path
            .iter()
            .map(|segment| match segment {
                PathSegment::Key(key) => json!(key),
                PathSegment::Index(index) => json!(index),
            })
            .collect();
        issue.insert("path".to_owned(), Value::Array(path));
        issue.insert("message".to_owned(), json!(self.message));
        Value::Object(issue)
    }
}

/// What zod calls a payload: the value so far and the issues raised on it.
pub(crate) struct Outcome {
    value: Option<Value>,
    issues: Vec<Issue>,
    aborted: bool,
}

impl Outcome {
    fn ok(value: Option<Value>) -> Self {
        Self {
            value,
            issues: Vec::new(),
            aborted: false,
        }
    }

    fn failed(input: Option<&Value>, issue: Issue) -> Self {
        Self {
            value: input.cloned(),
            issues: vec![issue],
            aborted: false,
        }
    }

    fn is_aborted(&self) -> bool {
        self.aborted || self.issues.iter().any(|issue| !issue.continues)
    }
}

/// A failed parse. Its text is the `message` of a `ZodError`: the issues as
/// JSON, indented by two spaces.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    issues: Vec<Value>,
}

impl ValidationError {
    /// The issues, each with zod's `code`, `path` and `message`.
    pub fn issues(&self) -> &[Value] {
        &self.issues
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&json::to_string_pretty(&Value::Array(self.issues.clone())))
    }
}

impl std::error::Error for ValidationError {}

/// Whether an absent key is accepted, and whether something stands in for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionalIn {
    Optional,
    Defaulted,
}

#[derive(Debug, Clone)]
pub(crate) enum UnknownKeys {
    Strip,
    Strict,
    Catchall(Box<Schema>),
}

#[derive(Debug, Clone)]
enum Kind {
    String,
    Number,
    Boolean,
    Null,
    Unknown,
    Literal(Value),
    Enum(&'static [&'static str]),
    Array(Box<Schema>),
    Object {
        shape: Vec<(&'static str, Schema)>,
        unknown_keys: UnknownKeys,
    },
    /// `z.record(z.string(), value)`.
    Record(Box<Schema>),
    Union(Vec<Schema>),
    Intersection(Box<Schema>, Box<Schema>),
    Optional(Box<Schema>),
    Default(Box<Schema>, Value),
    /// `z.preprocess(transform, schema)`.
    Preprocess(fn(Option<&Value>) -> Option<Value>, Box<Schema>),
}

#[derive(Debug, Clone)]
enum Check {
    /// `.int()`: an integer in the safe range.
    Int,
    Min(i64),
    Max(i64),
    /// `z.custom(test)` and `.refine(test, { message })`.
    Custom {
        test: fn(Option<&Value>) -> bool,
        message: Option<&'static str>,
    },
    /// A string format checked with a pattern, reported with the pattern's JavaScript source.
    Format {
        format: &'static str,
        label: &'static str,
        source: &'static str,
        regex: fn() -> &'static Regex,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Schema {
    kind: Kind,
    checks: Vec<Check>,
}

impl From<Kind> for Schema {
    fn from(kind: Kind) -> Self {
        Self {
            kind,
            checks: Vec::new(),
        }
    }
}

pub(crate) fn string() -> Schema {
    Kind::String.into()
}

pub(crate) fn number() -> Schema {
    Kind::Number.into()
}

pub(crate) fn boolean() -> Schema {
    Kind::Boolean.into()
}

pub(crate) fn null() -> Schema {
    Kind::Null.into()
}

pub(crate) fn unknown() -> Schema {
    Kind::Unknown.into()
}

pub(crate) fn literal(value: &str) -> Schema {
    Kind::Literal(json!(value)).into()
}

pub(crate) fn enumeration(values: &'static [&'static str]) -> Schema {
    Kind::Enum(values).into()
}

pub(crate) fn array(element: Schema) -> Schema {
    Kind::Array(Box::new(element)).into()
}

/// `z.object(shape)`: unknown keys are dropped.
pub(crate) fn object(shape: Vec<(&'static str, Schema)>) -> Schema {
    Kind::Object {
        shape,
        unknown_keys: UnknownKeys::Strip,
    }
    .into()
}

pub(crate) fn record(value: Schema) -> Schema {
    Kind::Record(Box::new(value)).into()
}

pub(crate) fn union(options: Vec<Schema>) -> Schema {
    Kind::Union(options).into()
}

pub(crate) fn intersection(left: Schema, right: Schema) -> Schema {
    Kind::Intersection(Box::new(left), Box::new(right)).into()
}

pub(crate) fn custom(test: fn(Option<&Value>) -> bool) -> Schema {
    Schema {
        kind: Kind::Unknown,
        checks: vec![Check::Custom {
            test,
            message: None,
        }],
    }
}

pub(crate) fn preprocess(transform: fn(Option<&Value>) -> Option<Value>, schema: Schema) -> Schema {
    Kind::Preprocess(transform, Box::new(schema)).into()
}

impl Schema {
    pub(crate) fn optional(self) -> Schema {
        Kind::Optional(Box::new(self)).into()
    }

    pub(crate) fn default_value(self, value: Value) -> Schema {
        Kind::Default(Box::new(self), value).into()
    }

    pub(crate) fn int(mut self) -> Schema {
        self.checks.push(Check::Int);
        self
    }

    pub(crate) fn min(mut self, minimum: i64) -> Schema {
        self.checks.push(Check::Min(minimum));
        self
    }

    pub(crate) fn max(mut self, maximum: i64) -> Schema {
        self.checks.push(Check::Max(maximum));
        self
    }

    pub(crate) fn refine(
        mut self,
        test: fn(Option<&Value>) -> bool,
        message: &'static str,
    ) -> Schema {
        self.checks.push(Check::Custom {
            test,
            message: Some(message),
        });
        self
    }

    pub(crate) fn format(
        mut self,
        format: &'static str,
        label: &'static str,
        source: &'static str,
        regex: fn() -> &'static Regex,
    ) -> Schema {
        self.checks.push(Check::Format {
            format,
            label,
            source,
            regex,
        });
        self
    }

    /// `.loose()`: unknown keys are kept as they are.
    pub(crate) fn loose(self) -> Schema {
        self.unknown_keys(UnknownKeys::Catchall(Box::new(unknown())))
    }

    /// `.strict()`: unknown keys are an issue.
    pub(crate) fn strict(self) -> Schema {
        self.unknown_keys(UnknownKeys::Strict)
    }

    pub(crate) fn catchall(self, schema: Schema) -> Schema {
        self.unknown_keys(UnknownKeys::Catchall(Box::new(schema)))
    }

    fn unknown_keys(mut self, mode: UnknownKeys) -> Schema {
        if let Kind::Object { unknown_keys, .. } = &mut self.kind {
            *unknown_keys = mode;
        }
        self
    }

    /// `.extend(shape)`: a key already in the shape keeps its place.
    pub(crate) fn extend(mut self, additions: Vec<(&'static str, Schema)>) -> Schema {
        if let Kind::Object { shape, .. } = &mut self.kind {
            for (key, schema) in additions {
                match shape.iter_mut().find(|(existing, _)| *existing == key) {
                    Some(entry) => entry.1 = schema,
                    None => shape.push((key, schema)),
                }
            }
        }
        self
    }

    /// `schema.safeParse(value)`.
    pub(crate) fn parse(&self, input: Option<&Value>) -> Result<Option<Value>, ValidationError> {
        let outcome = self.run(input);
        if outcome.issues.is_empty() {
            // The parsed value is a JavaScript object: its integer-like keys come first.
            Ok(outcome.value.map(|mut value| {
                json::js_key_order(&mut value);
                value
            }))
        } else {
            Err(ValidationError {
                issues: outcome.issues.iter().map(Issue::finalize).collect(),
            })
        }
    }

    /// Parse a value that is known to be present and to stay present.
    pub(crate) fn parse_value(&self, input: &Value) -> Result<Value, ValidationError> {
        self.parse(Some(input))
            .map(|value| value.unwrap_or(Value::Null))
    }

    pub(crate) fn accepts(&self, input: Option<&Value>) -> bool {
        self.run(input).issues.is_empty()
    }

    fn optional_in(&self) -> Option<OptionalIn> {
        match &self.kind {
            Kind::Optional(inner) => Some(match inner.optional_in() {
                Some(OptionalIn::Defaulted) => OptionalIn::Defaulted,
                _ => OptionalIn::Optional,
            }),
            Kind::Default(..) => Some(OptionalIn::Defaulted),
            Kind::Union(options) => {
                let modes: Vec<_> = options.iter().filter_map(Schema::optional_in).collect();
                if modes.contains(&OptionalIn::Defaulted) {
                    Some(OptionalIn::Defaulted)
                } else if modes.is_empty() {
                    None
                } else {
                    Some(OptionalIn::Optional)
                }
            }
            // A transform runs on `undefined` too.
            Kind::Preprocess(..) => Some(OptionalIn::Optional),
            _ => None,
        }
    }

    fn optional_out(&self) -> bool {
        match &self.kind {
            Kind::Optional(_) => true,
            Kind::Union(options) => options.iter().any(Schema::optional_out),
            Kind::Preprocess(_, schema) => schema.optional_out(),
            _ => false,
        }
    }

    fn run(&self, input: Option<&Value>) -> Outcome {
        let mut outcome = self.run_kind(input);
        // Checks only see a value its type accepted.
        let mut aborted = outcome.is_aborted();
        for check in &self.checks {
            if aborted {
                break;
            }
            let before = outcome.issues.len();
            check.apply(&mut outcome);
            aborted = outcome.issues[before..]
                .iter()
                .any(|issue| !issue.continues);
        }
        outcome
    }

    fn run_kind(&self, input: Option<&Value>) -> Outcome {
        match &self.kind {
            Kind::String => match input {
                Some(Value::String(_)) => Outcome::ok(input.cloned()),
                _ => Outcome::failed(input, Issue::invalid_type("string", input)),
            },
            Kind::Number => match input {
                Some(Value::Number(_)) => Outcome::ok(input.cloned()),
                _ => Outcome::failed(input, Issue::invalid_type("number", input)),
            },
            Kind::Boolean => match input {
                Some(Value::Bool(_)) => Outcome::ok(input.cloned()),
                _ => Outcome::failed(input, Issue::invalid_type("boolean", input)),
            },
            Kind::Null => match input {
                Some(Value::Null) => Outcome::ok(input.cloned()),
                _ => Outcome::failed(input, Issue::invalid_type("null", input)),
            },
            Kind::Unknown => Outcome::ok(input.cloned()),
            Kind::Literal(expected) => {
                if input == Some(expected) {
                    return Outcome::ok(input.cloned());
                }
                Outcome::failed(
                    input,
                    Issue {
                        fields: vec![
                            ("code", json!("invalid_value")),
                            ("values", json!([expected])),
                        ],
                        path: Vec::new(),
                        message: format!(
                            "Invalid input: expected {}",
                            stringify_primitive(expected)
                        ),
                        continues: false,
                    },
                )
            }
            Kind::Enum(values) => {
                if let Some(Value::String(text)) = input
                    && values.contains(&text.as_str())
                {
                    return Outcome::ok(input.cloned());
                }
                let message = match values {
                    [only] => format!("Invalid input: expected \"{only}\""),
                    _ => format!(
                        "Invalid option: expected one of {}",
                        values
                            .iter()
                            .map(|value| format!("\"{value}\""))
                            .collect::<Vec<_>>()
                            .join("|")
                    ),
                };
                Outcome::failed(
                    input,
                    Issue {
                        fields: vec![("code", json!("invalid_value")), ("values", json!(values))],
                        path: Vec::new(),
                        message,
                        continues: false,
                    },
                )
            }
            Kind::Array(element) => {
                let Some(Value::Array(items)) = input else {
                    return Outcome::failed(input, Issue::invalid_type("array", input));
                };
                let mut issues = Vec::new();
                let mut parsed = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    let outcome = element.run(Some(item));
                    prefix_into(&mut issues, outcome.issues, PathSegment::Index(index));
                    parsed.push(outcome.value.unwrap_or(Value::Null));
                }
                Outcome {
                    value: Some(Value::Array(parsed)),
                    issues,
                    aborted: false,
                }
            }
            Kind::Object {
                shape,
                unknown_keys,
            } => run_object(shape, unknown_keys, input),
            Kind::Record(value_schema) => {
                let Some(Value::Object(entries)) = input else {
                    return Outcome::failed(input, Issue::invalid_type("record", input));
                };
                let mut issues = Vec::new();
                let mut parsed = Map::new();
                for (key, value) in entries {
                    // Assigning `__proto__` would replace the prototype of the result.
                    if key == "__proto__" {
                        continue;
                    }
                    let outcome = value_schema.run(Some(value));
                    prefix_into(&mut issues, outcome.issues, PathSegment::Key(key.clone()));
                    if let Some(value) = outcome.value {
                        parsed.insert(key.clone(), value);
                    }
                }
                Outcome {
                    value: Some(Value::Object(parsed)),
                    issues,
                    aborted: false,
                }
            }
            Kind::Union(options) => run_union(options, input),
            Kind::Intersection(left, right) => {
                let left = left.run(input);
                let right = right.run(input);
                let mut issues = left.issues;
                issues.extend(right.issues);
                let value = match (left.value, right.value) {
                    (Some(left), Some(right)) => Some(merge_values(left, right)),
                    (left, right) => left.or(right),
                };
                Outcome {
                    value,
                    issues,
                    aborted: false,
                }
            }
            Kind::Optional(inner) => match input {
                None if inner.optional_in() != Some(OptionalIn::Defaulted) => Outcome::ok(None),
                None => {
                    // A default that fails to apply leaves the key absent.
                    let outcome = inner.run(None);
                    Outcome::ok(if outcome.issues.is_empty() {
                        outcome.value
                    } else {
                        None
                    })
                }
                Some(_) => inner.run(input),
            },
            Kind::Default(inner, default) => match input {
                None => Outcome::ok(Some(default.clone())),
                Some(_) => {
                    let mut outcome = inner.run(input);
                    if outcome.value.is_none() {
                        outcome.value = Some(default.clone());
                    }
                    outcome
                }
            },
            Kind::Preprocess(transform, schema) => schema.run(transform(input).as_ref()),
        }
    }
}

fn run_object(
    shape: &[(&'static str, Schema)],
    unknown_keys: &UnknownKeys,
    input: Option<&Value>,
) -> Outcome {
    let Some(Value::Object(entries)) = input else {
        return Outcome::failed(input, Issue::invalid_type("object", input));
    };
    let mut issues = Vec::new();
    let mut parsed = Map::new();

    for (key, schema) in shape {
        let present = entries.contains_key(*key);
        let outcome = schema.run(entries.get(*key));
        let segment = || PathSegment::Key((*key).to_owned());
        match (schema.optional_in(), schema.optional_out()) {
            (Some(optional_in), true) => {
                // Whatever an optional key made of its absence is dropped, issues included.
                if outcome.issues.is_empty() || present {
                    prefix_into(&mut issues, outcome.issues, segment());
                    let assigned = match optional_in {
                        OptionalIn::Optional => present,
                        OptionalIn::Defaulted => outcome.value.is_some() || present,
                    };
                    if assigned && let Some(value) = outcome.value {
                        parsed.insert((*key).to_owned(), value);
                    }
                }
            }
            (None, _) => {
                let failed = !outcome.issues.is_empty();
                prefix_into(&mut issues, outcome.issues, segment());
                if !present && !failed {
                    issues.push(Issue {
                        fields: vec![
                            ("code", json!("invalid_type")),
                            ("expected", json!("nonoptional")),
                        ],
                        path: vec![segment()],
                        message: "Invalid input: expected nonoptional, received undefined"
                            .to_owned(),
                        continues: false,
                    });
                }
                if present && let Some(value) = outcome.value {
                    parsed.insert((*key).to_owned(), value);
                }
            }
            (Some(_), false) => {
                prefix_into(&mut issues, outcome.issues, segment());
                if let Some(value) = outcome.value {
                    parsed.insert((*key).to_owned(), value);
                }
            }
        }
    }

    let catchall = match unknown_keys {
        UnknownKeys::Strip => {
            return Outcome {
                value: Some(Value::Object(parsed)),
                issues,
                aborted: false,
            };
        }
        UnknownKeys::Strict => None,
        UnknownKeys::Catchall(schema) => Some(schema),
    };
    let mut unrecognized = Vec::new();
    for (key, value) in entries {
        if shape.iter().any(|(declared, _)| declared == key) {
            continue;
        }
        let Some(schema) = catchall else {
            unrecognized.push(json!(key));
            continue;
        };
        if key == "__proto__" {
            continue;
        }
        let outcome = schema.run(Some(value));
        prefix_into(&mut issues, outcome.issues, PathSegment::Key(key.clone()));
        if let Some(value) = outcome.value {
            parsed.insert(key.clone(), value);
        }
    }
    if !unrecognized.is_empty() {
        let quoted: Vec<String> = unrecognized.iter().map(stringify_primitive).collect();
        issues.push(Issue {
            message: format!(
                "Unrecognized key{}: {}",
                if quoted.len() > 1 { "s" } else { "" },
                quoted.join(", ")
            ),
            fields: vec![
                ("code", json!("unrecognized_keys")),
                ("keys", Value::Array(unrecognized)),
            ],
            path: Vec::new(),
            // Says something about the shape of the input, not about the parsed value.
            continues: true,
        });
    }
    Outcome {
        value: Some(Value::Object(parsed)),
        issues,
        aborted: false,
    }
}

fn run_union(options: &[Schema], input: Option<&Value>) -> Outcome {
    if let [only] = options {
        return only.run(input);
    }
    let mut outcomes = Vec::with_capacity(options.len());
    for option in options {
        let outcome = option.run(input);
        if outcome.issues.is_empty() {
            return outcome;
        }
        outcomes.push(outcome);
    }

    // One option that only has issues it can live with is the one that was meant.
    let usable: Vec<usize> = outcomes
        .iter()
        .enumerate()
        .filter(|(_, outcome)| !outcome.is_aborted())
        .map(|(index, _)| index)
        .collect();
    if let [index] = usable[..] {
        return outcomes.swap_remove(index);
    }

    let errors: Vec<Value> = outcomes
        .iter()
        .map(|outcome| Value::Array(outcome.issues.iter().map(Issue::finalize).collect()))
        .collect();
    Outcome::failed(
        input,
        Issue {
            fields: vec![
                ("code", json!("invalid_union")),
                ("errors", Value::Array(errors)),
            ],
            path: Vec::new(),
            message: "Invalid input".to_owned(),
            continues: false,
        },
    )
}

impl Check {
    fn apply(&self, outcome: &mut Outcome) {
        let value = outcome.value.as_ref();
        match self {
            Check::Int => {
                let Some(Value::Number(number)) = value else {
                    return;
                };
                let Some(float) = number.as_f64() else { return };
                let integer = number.is_i64() || number.is_u64() || float.fract() == 0.0;
                if !integer {
                    outcome.issues.push(Issue {
                        fields: vec![
                            ("expected", json!("int")),
                            ("format", json!("safeint")),
                            ("code", json!("invalid_type")),
                        ],
                        path: Vec::new(),
                        message: "Invalid input: expected int, received number".to_owned(),
                        continues: false,
                    });
                } else if float.abs() > MAX_SAFE_INTEGER {
                    let (code, bound, limit, sign) = if float > 0.0 {
                        ("too_big", "maximum", 9_007_199_254_740_991_i64, "<=")
                    } else {
                        ("too_small", "minimum", -9_007_199_254_740_991_i64, ">=")
                    };
                    let size = if float > 0.0 { "big" } else { "small" };
                    outcome.issues.push(Issue {
                        fields: vec![
                            ("code", json!(code)),
                            (bound, json!(limit)),
                            (
                                "note",
                                json!("Integers must be within the safe integer range."),
                            ),
                            ("origin", json!("int")),
                            ("inclusive", json!(true)),
                        ],
                        path: Vec::new(),
                        message: format!("Too {size}: expected int to be {sign}{limit}"),
                        continues: true,
                    });
                }
            }
            Check::Min(minimum) => {
                let Some(float) = value.and_then(Value::as_f64) else {
                    return;
                };
                if float < *minimum as f64 {
                    outcome.issues.push(Issue {
                        fields: vec![
                            ("origin", json!("number")),
                            ("code", json!("too_small")),
                            ("minimum", json!(minimum)),
                            ("inclusive", json!(true)),
                        ],
                        path: Vec::new(),
                        message: format!("Too small: expected number to be >={minimum}"),
                        continues: true,
                    });
                }
            }
            Check::Max(maximum) => {
                let Some(float) = value.and_then(Value::as_f64) else {
                    return;
                };
                if float > *maximum as f64 {
                    outcome.issues.push(Issue {
                        fields: vec![
                            ("origin", json!("number")),
                            ("code", json!("too_big")),
                            ("maximum", json!(maximum)),
                            ("inclusive", json!(true)),
                        ],
                        path: Vec::new(),
                        message: format!("Too big: expected number to be <={maximum}"),
                        continues: true,
                    });
                }
            }
            Check::Custom { test, message } => {
                if !test(value) {
                    outcome.issues.push(Issue {
                        fields: vec![("code", json!("custom"))],
                        path: Vec::new(),
                        message: message.unwrap_or("Invalid input").to_owned(),
                        continues: true,
                    });
                }
            }
            Check::Format {
                format,
                label,
                source,
                regex,
            } => {
                let Some(Value::String(text)) = value else {
                    return;
                };
                if !regex().is_match(text) {
                    outcome.issues.push(Issue {
                        fields: vec![
                            ("origin", json!("string")),
                            ("code", json!("invalid_format")),
                            ("format", json!(format)),
                            ("pattern", json!(source)),
                        ],
                        path: Vec::new(),
                        message: format!("Invalid {label}"),
                        continues: true,
                    });
                }
            }
        }
    }
}

fn prefix_into(issues: &mut Vec<Issue>, nested: Vec<Issue>, segment: PathSegment) {
    for mut issue in nested {
        issue.path.insert(0, segment.clone());
        issues.push(issue);
    }
}

/// zod's `mergeValues` for the two sides of an intersection. Both sides were
/// parsed from the same input, so they can only differ in which keys they kept.
fn merge_values(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Object(mut left), Value::Object(right)) => {
            // `{ ...a, ...b }` keeps the position a key had in `a`.
            for (key, right_value) in right {
                match left.get_mut(&key) {
                    Some(slot) => *slot = merge_values(slot.take(), right_value),
                    None => {
                        left.insert(key, right_value);
                    }
                }
            }
            Value::Object(left)
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => Value::Array(
            left.into_iter()
                .zip(right)
                .map(|(left, right)| merge_values(left, right))
                .collect(),
        ),
        (left, _) => left,
    }
}

fn parsed_type(input: Option<&Value>) -> &'static str {
    match input {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn stringify_primitive(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => json::to_string(other),
    }
}
