//! What the tools do to the JSON Google answers with, done as JavaScript
//! does it.
//!
//! The TypeScript reads a row as `any`. A field Google left out is
//! `undefined`, and `Number(x ?? 0)`, `String(x)`, `x || null` and
//! `JSON.stringify` each make something of it: a count of 0, the text
//! `undefined`, a `null`, no key at all. The tools here go through the same
//! steps, so that they answer the same JSON for the same rows.
//!
//! `None` stands for `undefined` throughout. Reading a field of what is not
//! an object gives `undefined`, as it does in JavaScript for a number, a
//! boolean or a text, and as `?.` makes it for a `null`.

use mymcps_builtin::BuiltinError;
use mymcps_vine as vine;
use serde_json::{Map, Value};

/// `value.key` and `value?.key`.
pub(crate) fn get<'a>(value: impl Into<Option<&'a Value>>, key: &str) -> Option<&'a Value> {
    match value.into()? {
        Value::Object(object) => object.get(key),
        _ => None,
    }
}

/// What `??` keeps: a value that is neither `undefined` nor `null`.
pub(crate) fn defined(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| !value.is_null())
}

/// `value ?? null`.
pub(crate) fn or_null(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

/// `Boolean(value)`.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|number| number != 0.0 && !number.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `value || null`.
pub(crate) fn truthy_or_null(value: Option<&Value>) -> Value {
    value
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `String(value)`, which is also how a template string writes a value.
pub(crate) fn string(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".to_owned(), vine::js::to_string)
}

/// `Number(value ?? fallback)`.
pub(crate) fn number_or(value: Option<&Value>, fallback: f64) -> f64 {
    defined(value).map_or(fallback, |value| vine::js::to_number(Some(value)))
}

/// A JavaScript number as `JSON.stringify` writes it: a whole number without
/// a fraction, and `null` for what is not a number.
pub(crate) fn json_number(value: f64) -> Value {
    vine::js::number(value)
}

/// `Math.round`: to the nearest integer, and up when halfway between two.
pub(crate) fn math_round(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let floor = value.floor();
    // The difference is exact, or rounded without crossing the half.
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// `text.slice(0, units)`, where JavaScript counts UTF-16 code units. A
/// character cut in two by the count is left out, since half of one is not
/// text here.
pub(crate) fn slice_start(text: &str, units: usize) -> &str {
    let mut taken = 0;
    for (at, character) in text.char_indices() {
        taken += character.len_utf16();
        if taken > units {
            return &text[..at];
        }
    }
    text
}

/// `text?.slice(0, units)`: `None` for a text Google left out.
pub(crate) fn slice_of<'a>(
    value: Option<&'a Value>,
    units: usize,
    what: &str,
) -> Result<Option<&'a str>, BuiltinError> {
    match defined(value) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(slice_start(text, units))),
        Some(_) => Err(unreadable(what)),
    }
}

/// The part of a row its query selected from, such as `row.campaign!`.
pub(crate) fn resource<'a>(
    row: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Value, BuiltinError> {
    row.get(name)
        .ok_or_else(|| unreadable(&format!("a row without its {name}")))
}

/// The items of `value ?? []`, for a value the TypeScript then calls an
/// array method on.
pub(crate) fn items<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a [Value], BuiltinError> {
    match defined(value) {
        None => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(unreadable(what)),
    }
}

/// `value?.[0]`, for a list or a text.
pub(crate) fn first(value: Option<&Value>) -> Option<Value> {
    match value? {
        Value::Array(items) => items.first().cloned(),
        Value::String(text) => text
            .chars()
            .next()
            .map(|character| Value::String(character.to_string())),
        _ => None,
    }
}

/// Where the TypeScript threw a `TypeError`: on an answer that is not made
/// as the Google Ads API documents it, such as a row without the resource
/// its query selects from. The agent only learns that the call failed.
pub(crate) fn unreadable(what: &str) -> BuiltinError {
    BuiltinError::internal(format!(
        "Google Ads answered with {what} the tool cannot read"
    ))
}

/// An object literal: its keys in the order they are written, without the
/// ones whose value is `undefined`, which `JSON.stringify` leaves out.
#[derive(Debug, Default)]
pub(crate) struct Object(Map<String, Value>);

impl Object {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `key: value`.
    pub(crate) fn set(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.to_owned(), value);
        self
    }

    /// `key: value` for a text.
    pub(crate) fn text(self, key: &str, value: impl Into<String>) -> Self {
        self.set(key, Value::String(value.into()))
    }

    /// `key: value` for a boolean.
    pub(crate) fn flag(self, key: &str, value: bool) -> Self {
        self.set(key, Value::Bool(value))
    }

    /// `key: value` for a JavaScript number, written as `JSON.stringify`
    /// writes it. Never `set` an `f64`: 150 would read `150.0`.
    pub(crate) fn number(self, key: &str, value: f64) -> Self {
        self.set(key, json_number(value))
    }

    /// `key: value` for a value read from a row, which may be `undefined`.
    pub(crate) fn field(mut self, key: &str, value: Option<&Value>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.to_owned(), value.clone());
        }
        self
    }

    /// `...other`.
    pub(crate) fn spread(mut self, other: Object) -> Self {
        self.0.extend(other.0);
        self
    }

    pub(crate) fn into_value(self) -> Value {
        Value::Object(self.0)
    }
}

impl From<Object> for Value {
    fn from(object: Object) -> Self {
        object.into_value()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rounds_halves_up_as_math_round_does() {
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(-2.5), -2.0);
        assert_eq!(math_round(2.4), 2.0);
        assert_eq!(math_round(-2.6), -3.0);
        assert_eq!(math_round(0.5), 1.0);
        assert_eq!(math_round(-0.5), 0.0);
        assert_eq!(math_round(-1.5), -1.0);
        // Adding a half first would round this one up.
        assert_eq!(math_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(math_round(-0.499_999_999_999_999_94), 0.0);
        assert_eq!(math_round(-0.500_000_000_000_000_1), -1.0);
        assert_eq!(math_round(9_007_199_254_740_991.0), 9_007_199_254_740_991.0);
        assert!(math_round(f64::NAN).is_nan());
    }

    #[test]
    fn tells_what_javascript_holds_true() {
        for falsy in [json!(null), json!(false), json!(0), json!(0.0), json!("")] {
            assert!(!truthy(Some(&falsy)), "{falsy}");
        }
        for truthy_value in [json!(true), json!(-1), json!("0"), json!([]), json!({})] {
            assert!(truthy(Some(&truthy_value)), "{truthy_value}");
        }
        assert!(!truthy(None));
        assert_eq!(truthy_or_null(Some(&json!(0))), Value::Null);
        assert_eq!(truthy_or_null(Some(&json!(1.2))), json!(1.2));
    }

    #[test]
    fn writes_values_as_string_and_number_do() {
        assert_eq!(string(None), "undefined");
        assert_eq!(string(Some(&json!(null))), "null");
        assert_eq!(string(Some(&json!(111))), "111");
        assert_eq!(string(Some(&json!("111"))), "111");
        assert_eq!(number_or(None, 1.0), 1.0);
        assert_eq!(number_or(Some(&json!(null)), 1.0), 1.0);
        assert_eq!(number_or(Some(&json!("12000")), 0.0), 12000.0);
        assert!(number_or(Some(&json!("many")), 0.0).is_nan());
        assert_eq!(json_number(150.0), json!(150));
        assert_eq!(json_number(f64::NAN), Value::Null);
    }

    #[test]
    fn reads_fields_of_objects_only() {
        let row = json!({ "campaign": { "id": "111", "budget": null }, "count": 3 });
        assert_eq!(
            get(&row, "campaign").and_then(|campaign| get(campaign, "id")),
            Some(&json!("111"))
        );
        assert_eq!(get(get(&row, "count"), "id"), None);
        assert_eq!(
            get(get(get(&row, "campaign"), "budget"), "amountMicros"),
            None
        );
        assert_eq!(get(None, "id"), None);
    }

    #[test]
    fn cuts_text_by_utf16_code_units() {
        assert_eq!(slice_start("2026-03-01 00:00:00", 10), "2026-03-01");
        assert_eq!(slice_start("short", 10), "short");
        assert_eq!(slice_start("ab😀cd", 4), "ab😀");
        assert_eq!(slice_start("ab😀cd", 3), "ab");
    }

    #[test]
    fn leaves_undefined_out_of_an_object() {
        let object = Object::new()
            .text("id", "1")
            .field("name", None)
            .field("status", Some(&json!(null)))
            .number("cost", 150.0)
            .flag("manager", false)
            .spread(Object::new().text("id", "2").set("clicks", json!(3)));
        assert_eq!(
            object.into_value().to_string(),
            r#"{"id":"2","status":null,"cost":150,"manager":false,"clicks":3}"#
        );
    }
}
