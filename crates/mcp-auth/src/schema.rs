//! The checks of the SDK's zod schemas (`shared/auth.js`), and the error they
//! fail with.
//!
//! A response that is not what its schema asks for is refused with a
//! `ZodError`, whose message is the list of issues as indented JSON. That
//! message is shown to the administrator, so the issues are written here with
//! the members, the order and the wording zod gives them.

use std::fmt;

use serde_json::{Map, Value, json};
use url::Url;

use crate::js;
use crate::json::{Json, array_index};

/// The `ZodError` a schema of the SDK throws for a document it refuses.
///
/// Its text is the JavaScript `message`: the issues, as JSON indented by two
/// spaces.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaError {
    issues: Vec<Issue>,
}

impl SchemaError {
    /// The issues, as zod reports them: objects with `code`, `path`,
    /// `message` and what the kind of issue adds.
    pub fn issues(&self) -> Vec<Value> {
        self.issues.iter().map(Issue::to_value).collect()
    }

    /// The `message` of the JavaScript error.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `JSON.stringify(issues, null, 2)`
        match serde_json::to_string_pretty(&self.issues()) {
            Ok(text) => formatter.write_str(&text),
            Err(_) => Err(fmt::Error),
        }
    }
}

impl std::error::Error for SchemaError {}

/// Where in a document an issue is: nowhere (the document itself), a member,
/// or an element of a member that is a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Path {
    key: Option<&'static str>,
    index: Option<usize>,
}

impl Path {
    fn member(key: &'static str) -> Self {
        Self {
            key: Some(key),
            index: None,
        }
    }

    fn element(self, index: usize) -> Self {
        Self {
            index: Some(index),
            ..self
        }
    }

    fn to_value(self) -> Value {
        let mut path = Vec::new();
        path.extend(self.key.map(Value::from));
        path.extend(self.index.map(Value::from));
        Value::Array(path)
    }
}

/// One issue of a `ZodError`. They are kept in this form and only written out
/// when someone reads them: a document can be refused for as many reasons as
/// it has elements.
#[derive(Debug, Clone, PartialEq)]
enum Issue {
    /// `invalid_type`
    Type {
        expected: &'static str,
        received: &'static str,
        path: Path,
    },
    /// `invalid_type` of `z.number()` for a number it refuses, which it
    /// names, since its type alone would not say why.
    Number { received: &'static str, path: Path },
    /// `invalid_format` of `z.url()`
    Url { path: Path },
    /// The check `SafeUrlSchema` adds for a string that is no URL, which
    /// stops the checks after it.
    UnparseableUrl { path: Path },
    /// The check of `SafeUrlSchema` on the scheme.
    UnsafeScheme { path: Path },
    /// `invalid_union` of `OptionalSafeUrlSchema`: neither a safe URL, for
    /// the reasons given, nor the empty string.
    NeitherUrlNorEmpty {
        key: &'static str,
        refused_as_url: Vec<Issue>,
    },
}

impl Issue {
    /// The issue with its members in the order zod writes them.
    fn to_value(&self) -> Value {
        match self {
            Issue::Type {
                expected,
                received,
                path,
            } => json!({
                "expected": expected,
                "code": "invalid_type",
                "path": path.to_value(),
                "message": format!("Invalid input: expected {expected}, received {received}"),
            }),
            Issue::Number { received, path } => json!({
                "expected": "number",
                "code": "invalid_type",
                "received": received,
                "path": path.to_value(),
                "message": format!("Invalid input: expected number, received {received}"),
            }),
            Issue::Url { path } => json!({
                "code": "invalid_format",
                "format": "url",
                "path": path.to_value(),
                "message": "Invalid URL",
            }),
            Issue::UnparseableUrl { path } => json!({
                "code": "custom",
                "message": "URL must be parseable",
                "fatal": true,
                "path": path.to_value(),
            }),
            Issue::UnsafeScheme { path } => json!({
                "code": "custom",
                "path": path.to_value(),
                "message": "URL cannot use javascript:, data:, or vbscript: scheme",
            }),
            Issue::NeitherUrlNorEmpty {
                key,
                refused_as_url,
            } => json!({
                "code": "invalid_union",
                "errors": [
                    refused_as_url.iter().map(Issue::to_value).collect::<Vec<_>>(),
                    [{
                        "code": "invalid_value",
                        "values": [""],
                        "path": [],
                        "message": "Invalid input: expected \"\"",
                    }],
                ],
                "path": [key],
                "message": "Invalid input",
            }),
        }
    }
}

/// How zod names the type of a value in "received ...".
fn received(value: Option<&Json>) -> &'static str {
    match value {
        None => "undefined",
        Some(Json::Null) => "null",
        Some(Json::Bool(_)) => "boolean",
        Some(Json::Number(value)) if value.is_nan() => "NaN",
        Some(Json::Number(value)) if value.is_infinite() => {
            if *value > 0.0 {
                "Infinity"
            } else {
                "-Infinity"
            }
        }
        Some(Json::Number(_)) => "number",
        Some(Json::String(_)) => "string",
        Some(Json::Array(_)) => "array",
        Some(Json::Object(_)) => "object",
    }
}

fn invalid_type(expected: &'static str, value: Option<&Json>, path: Path) -> Issue {
    Issue::Type {
        expected,
        received: received(value),
        path,
    }
}

/// One check of a schema: the member as it is accepted, or `None` after
/// noting in `issues` why it is refused. `path` is where the member is.
type Check<T> = fn(Option<&Json>, Path, &mut Vec<Issue>) -> Option<T>;

/// A URL as `z.url()` returns it when it accepts one: trimmed, and without
/// the tabs and line breaks the URL parser ignores.
fn accepted_url(text: &str) -> Option<String> {
    let trimmed = js::trim(text);
    Url::parse(trimmed).ok()?;
    Some(trimmed.replace(['\t', '\n', '\r'], ""))
}

fn string(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<String> {
    match value {
        Some(Json::String(text)) => Some(text.clone()),
        other => {
            issues.push(invalid_type("string", other, path));
            None
        }
    }
}

/// `z.string().url()`
fn url(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<String> {
    let text = string(value, path, issues)?;
    let accepted = accepted_url(&text);
    if accepted.is_none() {
        issues.push(Issue::Url { path });
    }
    accepted
}

enum UnsafeUrl {
    NotAUrl,
    Scheme,
}

/// `SafeUrlSchema`: a URL that is not `javascript:`, `data:` or `vbscript:`.
fn check_safe_url(text: &str) -> Result<String, UnsafeUrl> {
    let accepted = accepted_url(text).ok_or(UnsafeUrl::NotAUrl)?;
    let scheme = Url::parse(&accepted).map(|url| url.scheme().to_owned());
    if matches!(scheme.as_deref(), Ok("javascript" | "data" | "vbscript")) {
        return Err(UnsafeUrl::Scheme);
    }
    Ok(accepted)
}

/// The two issues of a string `SafeUrlSchema` cannot parse: the one of
/// `z.url()`, and the one of the check the SDK adds.
fn not_a_url(path: Path) -> [Issue; 2] {
    [Issue::Url { path }, Issue::UnparseableUrl { path }]
}

fn safe_url(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<String> {
    let text = string(value, path, issues)?;
    match check_safe_url(&text) {
        Ok(accepted) => Some(accepted),
        Err(UnsafeUrl::NotAUrl) => {
            issues.extend(not_a_url(path));
            None
        }
        Err(UnsafeUrl::Scheme) => {
            issues.push(Issue::UnsafeScheme { path });
            None
        }
    }
}

fn list(
    value: Option<&Json>,
    path: Path,
    issues: &mut Vec<Issue>,
    item: Check<String>,
) -> Option<Vec<String>> {
    let Some(Json::Array(items)) = value else {
        issues.push(invalid_type("array", value, path));
        return None;
    };
    let before = issues.len();
    let mut accepted = Vec::with_capacity(items.len());
    for (index, element) in items.iter().enumerate() {
        if let Some(text) = item(Some(element), path.element(index), issues) {
            accepted.push(text);
        }
    }
    (issues.len() == before).then_some(accepted)
}

fn strings(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<Vec<String>> {
    list(value, path, issues, string)
}

fn safe_urls(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<Vec<String>> {
    list(value, path, issues, safe_url)
}

fn boolean(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<bool> {
    match value {
        Some(Json::Bool(value)) => Some(*value),
        other => {
            issues.push(invalid_type("boolean", other, path));
            None
        }
    }
}

/// `z.number()`
fn number(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<f64> {
    match value {
        Some(Json::Number(value)) if value.is_finite() => Some(*value),
        Some(number @ Json::Number(_)) => {
            issues.push(Issue::Number {
                received: received(Some(number)),
                path,
            });
            None
        }
        other => {
            issues.push(invalid_type("number", other, path));
            None
        }
    }
}

/// `z.coerce.number()`: `Number(value)` first.
fn coerced_number(value: Option<&Json>, path: Path, issues: &mut Vec<Issue>) -> Option<f64> {
    match value.and_then(js::to_number) {
        Some(coerced) => number(Some(&Json::Number(coerced)), path, issues),
        // `Number()` threw: the value is judged as it came.
        None => number(value, path, issues),
    }
}

/// Reads the members of one object in the order of its schema.
///
/// A getter returns `None` when the member is refused, after noting why. An
/// optional member that is absent is `Some(None)`: `null` is not absent, and
/// is refused like any other value of the wrong type.
pub(crate) struct ObjectReader<'a> {
    members: &'a [(String, Json)],
    known: Vec<&'static str>,
    issues: Vec<Issue>,
}

impl<'a> ObjectReader<'a> {
    pub(crate) fn new(value: &'a Json) -> Result<Self, SchemaError> {
        match value {
            Json::Object(members) => Ok(Self {
                members,
                known: Vec::new(),
                issues: Vec::new(),
            }),
            other => Err(SchemaError {
                issues: vec![invalid_type("object", Some(other), Path::default())],
            }),
        }
    }

    fn member(&mut self, key: &'static str) -> Option<&'a Json> {
        self.known.push(key);
        self.members
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    fn required<T>(&mut self, key: &'static str, check: Check<T>) -> Option<T> {
        let value = self.member(key);
        check(value, Path::member(key), &mut self.issues)
    }

    fn optional<T>(&mut self, key: &'static str, check: Check<T>) -> Option<Option<T>> {
        match self.member(key) {
            None => Some(None),
            value => check(value, Path::member(key), &mut self.issues).map(Some),
        }
    }

    pub(crate) fn string(&mut self, key: &'static str) -> Option<String> {
        self.required(key, string)
    }

    pub(crate) fn opt_string(&mut self, key: &'static str) -> Option<Option<String>> {
        self.optional(key, string)
    }

    pub(crate) fn safe_url(&mut self, key: &'static str) -> Option<String> {
        self.required(key, safe_url)
    }

    pub(crate) fn opt_safe_url(&mut self, key: &'static str) -> Option<Option<String>> {
        self.optional(key, safe_url)
    }

    pub(crate) fn url(&mut self, key: &'static str) -> Option<String> {
        self.required(key, url)
    }

    pub(crate) fn opt_url(&mut self, key: &'static str) -> Option<Option<String>> {
        self.optional(key, url)
    }

    pub(crate) fn strings(&mut self, key: &'static str) -> Option<Vec<String>> {
        self.required(key, strings)
    }

    pub(crate) fn opt_strings(&mut self, key: &'static str) -> Option<Option<Vec<String>>> {
        self.optional(key, strings)
    }

    pub(crate) fn safe_urls(&mut self, key: &'static str) -> Option<Vec<String>> {
        self.required(key, safe_urls)
    }

    pub(crate) fn opt_safe_urls(&mut self, key: &'static str) -> Option<Option<Vec<String>>> {
        self.optional(key, safe_urls)
    }

    pub(crate) fn opt_boolean(&mut self, key: &'static str) -> Option<Option<bool>> {
        self.optional(key, boolean)
    }

    pub(crate) fn opt_number(&mut self, key: &'static str) -> Option<Option<f64>> {
        self.optional(key, number)
    }

    pub(crate) fn opt_coerced_number(&mut self, key: &'static str) -> Option<Option<f64>> {
        self.optional(key, coerced_number)
    }

    /// `z.any().optional()`: whatever is there, `null` included.
    pub(crate) fn opt_any(&mut self, key: &'static str) -> Option<Option<Value>> {
        Some(self.member(key).map(Json::to_value))
    }

    /// `OptionalSafeUrlSchema`: a safe URL, or the empty string that older
    /// registrations send, which counts as absent.
    pub(crate) fn opt_safe_url_or_empty(&mut self, key: &'static str) -> Option<Option<String>> {
        let Some(value) = self.member(key) else {
            return Some(None);
        };
        let refused_as_url = match value {
            Json::String(text) => match check_safe_url(text) {
                Ok(accepted) => return Some(Some(accepted)),
                // A URL of a refused scheme is no candidate for the empty
                // string either: zod reports it alone, not as a failed union.
                Err(UnsafeUrl::Scheme) => {
                    self.issues.push(Issue::UnsafeScheme {
                        path: Path::member(key),
                    });
                    return None;
                }
                Err(UnsafeUrl::NotAUrl) if text.is_empty() => return Some(None),
                Err(UnsafeUrl::NotAUrl) => not_a_url(Path::default()).to_vec(),
            },
            other => vec![invalid_type("string", Some(other), Path::default())],
        };
        self.issues.push(Issue::NeitherUrlNorEmpty {
            key,
            refused_as_url,
        });
        None
    }

    /// Ends a `z.object()`: members the schema does not name are dropped.
    pub(crate) fn finish_strip(self) -> Result<(), SchemaError> {
        if self.issues.is_empty() {
            Ok(())
        } else {
            Err(SchemaError {
                issues: self.issues,
            })
        }
    }

    /// Ends a `z.looseObject()`: members the schema does not name are kept.
    pub(crate) fn finish_loose(self) -> Result<Map<String, Value>, SchemaError> {
        if !self.issues.is_empty() {
            return Err(SchemaError {
                issues: self.issues,
            });
        }
        Ok(self
            .members
            .iter()
            // zod never copies this one, which would set the prototype of its output.
            .filter(|(key, _)| key != "__proto__" && !self.known.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.to_value()))
            .collect())
    }
}

/// What a member the reader accepted is missing: only reached if a getter
/// returned `None` without an issue, which none does.
pub(crate) fn no_issue() -> SchemaError {
    SchemaError { issues: Vec::new() }
}

/// Writes a document back with its members in the order zod gives its output:
/// the members of the schema in the order of the schema, between the unknown
/// members that are array indices and the other unknown members.
#[derive(Default)]
pub(crate) struct ObjectWriter {
    members: Map<String, Value>,
}

impl ObjectWriter {
    pub(crate) fn loose(extra: &Map<String, Value>) -> Self {
        let mut writer = Self::default();
        for (key, value) in extra {
            if array_index(key).is_some() {
                writer.members.insert(key.clone(), value.clone());
            }
        }
        writer
    }

    pub(crate) fn put(&mut self, key: &str, value: &impl Member) {
        if let Some(value) = value.to_member() {
            self.members.insert(key.to_owned(), value);
        }
    }

    pub(crate) fn finish_strip(self) -> Map<String, Value> {
        self.members
    }

    pub(crate) fn finish_loose(mut self, extra: &Map<String, Value>) -> Map<String, Value> {
        for (key, value) in extra {
            if array_index(key).is_none() && !self.members.contains_key(key) {
                self.members.insert(key.clone(), value.clone());
            }
        }
        self.members
    }
}

/// A member of a document, or nothing when it is absent.
pub(crate) trait Member {
    fn to_member(&self) -> Option<Value>;
}

impl Member for String {
    fn to_member(&self) -> Option<Value> {
        Some(Value::String(self.clone()))
    }
}

impl Member for Vec<String> {
    fn to_member(&self) -> Option<Value> {
        Some(json!(self))
    }
}

impl Member for bool {
    fn to_member(&self) -> Option<Value> {
        Some(Value::Bool(*self))
    }
}

impl Member for f64 {
    fn to_member(&self) -> Option<Value> {
        Some(crate::json::number_to_value(*self))
    }
}

impl Member for Value {
    fn to_member(&self) -> Option<Value> {
        Some(self.clone())
    }
}

impl<T: Member> Member for Option<T> {
    fn to_member(&self) -> Option<Value> {
        self.as_ref().and_then(Member::to_member)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn a_list_of_wrong_elements_is_refused_without_writing_its_issues_out() {
        // Half a megabyte of numbers where URLs are expected.
        let document = format!(r#"{{"redirect_uris":[{}0]}}"#, "0,".repeat(250_000));
        let document = json::parse(&document).unwrap();
        let mut reader = ObjectReader::new(&document).unwrap();
        assert_eq!(reader.safe_urls("redirect_uris"), None);
        let error = reader.finish_strip().unwrap_err();
        assert_eq!(error.issues.len(), 250_001);
        assert!(std::mem::size_of::<Issue>() <= 80);
        assert_eq!(
            error.issues[250_000].to_value(),
            json!({
                "expected": "string",
                "code": "invalid_type",
                "path": ["redirect_uris", 250_000],
                "message": "Invalid input: expected string, received number",
            })
        );
    }
}
