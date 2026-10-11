//! JavaScript semantics that Vine's rules, and the rules the app writes on top
//! of them, rely on: how a value becomes a number or a string, what `trim`
//! removes, how long a string is, when two values are `===`.
//!
//! Rules ported from TypeScript should go through these helpers rather than
//! the nearest Rust method. `str::trim`, `str::len`, `f64::from_str` and
//! `\d` in the `regex` crate all differ from their JavaScript namesakes on
//! some input.

use std::cmp::Ordering;

use serde_json::{Map, Number, Value};

pub use crate::js_regex::{JsRegex, JsRegexError};

/// Translate a JavaScript regular expression. See [`JsRegex::new`].
pub fn regex(source: &str, flags: &str) -> Result<JsRegex, JsRegexError> {
    JsRegex::new(source, flags)
}

/// `WhiteSpace` and `LineTerminator` of ECMAScript: what
/// `String.prototype.trim` removes and what `\s` matches. Unlike
/// `char::is_whitespace`, it includes U+FEFF and leaves out U+0085.
pub fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_whitespace)
}

/// `String.prototype.length`: the number of UTF-16 code units. This is what
/// Vine's `minLength`, `maxLength` and `fixedLength` count.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `Number(text)` for a string.
pub fn string_to_number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return parse_radix(&text[2..], radix);
        }
    }
    let (sign, digits) = match bytes[0] {
        b'+' => (1.0, &text[1..]),
        b'-' => (-1.0, &text[1..]),
        _ => (1.0, text),
    };
    if digits == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(digits) {
        return f64::NAN;
    }
    digits
        .parse::<f64>()
        .map_or(f64::NAN, |number| sign * number)
}

/// `StrUnsignedDecimalLiteral` without `Infinity`: Rust's parser would also
/// take `inf`, `nan` and a sign after the one already removed.
fn is_decimal_literal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    let digits = |at: &mut usize| {
        let start = *at;
        while *at < bytes.len() && bytes[*at].is_ascii_digit() {
            *at += 1;
        }
        *at - start
    };
    let whole = digits(&mut at);
    let mut fraction = 0;
    if at < bytes.len() && bytes[at] == b'.' {
        at += 1;
        fraction = digits(&mut at);
    }
    if whole == 0 && fraction == 0 {
        return false;
    }
    if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
        at += 1;
        if at < bytes.len() && (bytes[at] == b'+' || bytes[at] == b'-') {
            at += 1;
        }
        if digits(&mut at) == 0 {
            return false;
        }
    }
    at == bytes.len()
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut number = 0.0;
    for digit in digits.chars() {
        match digit.to_digit(radix) {
            Some(value) => number = number * f64::from(radix) + f64::from(value),
            None => return f64::NAN,
        }
    }
    number
}

/// `Number(value)` for a JSON value. `None` stands for `undefined`.
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => string_to_number(text),
        Some(array @ Value::Array(_)) => string_to_number(&to_string(array)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// The number a JSON number is in JavaScript, or `None` for any other type
/// (`typeof value === 'number'`).
pub fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// A JavaScript number as a JSON value. A whole number is written without a
/// fraction, as `JSON.stringify` does, so `"5"` read by `vine.number()`
/// comes out as `5` and not `5.0`. `NaN` and the infinities become `null`.
pub fn number(value: f64) -> Value {
    const I64_BOUND: f64 = 9_223_372_036_854_775_808.0;
    if value.is_finite() && value.fract() == 0.0 && (-I64_BOUND..I64_BOUND).contains(&value) {
        // Truncation is exact: the value is whole and within the range.
        return Value::Number(Number::from(value as i64));
    }
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// `Number.isInteger`.
pub fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// `Number.isSafeInteger`.
pub fn is_safe_integer(value: f64) -> bool {
    is_integer(value) && value.abs() <= 9_007_199_254_740_991.0
}

/// The shortest digits that read back as the same finite, positive number,
/// and where its decimal point stands: the number is `0.<digits>` times ten
/// to the power of the second value.
///
/// When two writings are as short and as close, ECMAScript takes the one
/// that ends with an even digit (`1000000000000000.25` is written `…000.2`).
/// The printer of `serde_json` does, the one of the standard library does
/// not, so the digits are read from the former.
fn shortest_digits(value: f64) -> (String, i32) {
    let text = serde_json::Number::from_f64(value)
        .map(|number| number.to_string())
        .unwrap_or_default();
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((&text, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let written = format!("{whole}{fraction}");
    let significant = written.trim_start_matches('0');
    let leading = written.len() - significant.len();
    let point = i32::try_from(whole.len()).unwrap_or(i32::MAX) + exponent
        - i32::try_from(leading).unwrap_or(0);
    (significant.trim_end_matches('0').to_owned(), point)
}

/// `JSON.stringify(value)`. A number that is not whole, very large or very
/// small is written as JavaScript writes it (`0.000001`, not `1e-6`), so
/// that what was text in the Node app is the same text here.
pub fn json_stringify(value: &Value) -> String {
    struct JavaScriptNumbers;

    impl serde_json::ser::Formatter for JavaScriptNumbers {
        fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            writer.write_all(number_to_string(value).as_bytes())
        }
    }

    let mut text = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut text, JavaScriptNumbers);
    // Writing a `Value` to memory cannot fail, and what it writes is UTF-8.
    if serde::Serialize::serialize(value, &mut serializer).is_err() {
        return value.to_string();
    }
    String::from_utf8(text).unwrap_or_else(|_| value.to_string())
}

/// `String(number)`.
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let (digits, point) = shortest_digits(value.abs());
    let count = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let exponent = point - 1;

    let mut text = String::new();
    if value < 0.0 {
        text.push('-');
    }
    if count <= point && point <= 21 {
        text.push_str(&digits);
        text.push_str(&"0".repeat(usize::try_from(point - count).unwrap_or(0)));
    } else if 0 < point && point <= 21 {
        let split = usize::try_from(point).unwrap_or(0);
        text.push_str(&digits[..split]);
        text.push('.');
        text.push_str(&digits[split..]);
    } else if -6 < point && point <= 0 {
        text.push_str("0.");
        text.push_str(&"0".repeat(usize::try_from(-point).unwrap_or(0)));
        text.push_str(&digits);
    } else {
        text.push_str(&digits[..1]);
        if count > 1 {
            text.push('.');
            text.push_str(&digits[1..]);
        }
        text.push('e');
        text.push(if exponent < 0 { '-' } else { '+' });
        text.push_str(&exponent.abs().to_string());
    }
    text
}

/// `String(value)` for a JSON value: an array is its items joined by commas,
/// an object is `[object Object]`.
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    to_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// `a === b`. Two arrays or two objects are never equal: in JavaScript they
/// would have to be the same instance, which values read from JSON never are.
pub fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

/// `a === b` where either side may be `undefined` (`None`).
pub fn strict_equals_opt(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => strict_equals(a, b),
        _ => false,
    }
}

/// `list.includes(value)`.
pub fn includes(list: &[Value], value: &Value) -> bool {
    list.iter().any(|item| strict_equals(item, value))
}

/// How `<`, `>`, `<=` and `>=` order two values: two strings by their UTF-16
/// code units, anything else as numbers. `None` when either side is `NaN`,
/// for which every comparison is false.
pub fn compare(a: Option<&Value>, b: Option<&Value>) -> Option<Ordering> {
    let primitive = |value: Option<&Value>| match value {
        Some(compound @ (Value::Array(_) | Value::Object(_))) => {
            Some(Value::String(to_string(compound)))
        }
        other => other.cloned(),
    };
    let (a, b) = (primitive(a), primitive(b));
    if let (Some(Value::String(a)), Some(Value::String(b))) = (&a, &b) {
        return Some(a.encode_utf16().cmp(b.encode_utf16()));
    }
    to_number(a.as_ref()).partial_cmp(&to_number(b.as_ref()))
}

/// The index a property key stands for when JavaScript treats it as an array
/// index, which it lists before every other key.
fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && !key.is_empty());
    if !canonical || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// `Object.keys(object)`: keys that are array indexes first, in ascending
/// order, then the others in the order they were written.
pub fn own_keys(object: &Map<String, Value>) -> Vec<&String> {
    let mut indexes: Vec<(u32, &String)> = Vec::new();
    let mut names: Vec<&String> = Vec::new();
    for key in object.keys() {
        match array_index(key) {
            Some(index) => indexes.push((index, key)),
            None => names.push(key),
        }
    }
    if indexes.is_empty() {
        return names;
    }
    indexes.sort_by_key(|(index, _)| *index);
    indexes
        .into_iter()
        .map(|(_, key)| key)
        .chain(names)
        .collect()
}

/// Put the keys of an object in the order JavaScript lists them. A no-op
/// unless one of them is an array index.
pub(crate) fn order_keys(object: Map<String, Value>) -> Map<String, Value> {
    if !object.keys().any(|key| array_index(key).is_some()) {
        return object;
    }
    let order: Vec<String> = own_keys(&object).into_iter().cloned().collect();
    let mut object = object;
    let mut ordered = Map::with_capacity(order.len());
    for key in order {
        if let Some(value) = object.remove(&key) {
            ordered.insert(key, value);
        }
    }
    ordered
}

/// A value with the keys of every object it holds in the order JavaScript
/// lists them, which is the order `JSON.stringify` writes them in.
pub(crate) fn order_keys_deep(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(order_keys_deep).collect()),
        Value::Object(object) => {
            let ordered = order_keys(object);
            Value::Object(
                ordered
                    .into_iter()
                    .map(|(key, value)| (key, order_keys_deep(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// What Node 24 prints for `String(number)`.
    #[test]
    // The first number is exact, and written with every digit on purpose.
    #[allow(clippy::excessive_precision)]
    fn writes_numbers_as_javascript_does() {
        for (number, text) in [
            // Two writings as short and as close: the even digit.
            (1_000_000_000_000_000.25, "1000000000000000.2"),
            (0.000_001, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (1e21, "1e+21"),
            (1.2345e22, "1.2345e+22"),
            (150.0, "150"),
            (0.1 + 0.2, "0.30000000000000004"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (1200.0, "1200"),
            (0.000_025, "0.000025"),
            (4.35, "4.35"),
            (9_007_199_254_740_993.0, "9007199254740992"),
            (1e16, "10000000000000000"),
            (123_456.789, "123456.789"),
            (0.000_012_34, "0.00001234"),
            (-1.5e300, "-1.5e+300"),
            (-0.0, "0"),
        ] {
            assert_eq!(number_to_string(number), text);
        }
    }

    /// What Node 24 prints for `JSON.stringify(value)`.
    #[test]
    fn writes_json_as_javascript_does() {
        let value = json!({
            "a": 1e-6,
            "b": [1.5, 150.0, 1e21, -0.0, 2e-7, 7],
            "c": "é\u{2028}\u{1f}\"\\/",
            "d": null,
            "e": true,
        });
        assert_eq!(
            json_stringify(&value),
            "{\"a\":0.000001,\"b\":[1.5,150,1e+21,0,2e-7,7],\"c\":\"é\u{2028}\\u001f\\\"\\\\/\",\"d\":null,\"e\":true}"
        );
        assert_eq!(json_stringify(&json!("plain")), "\"plain\"");
    }

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(trim("\u{FEFF} a\u{00A0}\n"), "a");
        // Rust would trim NEL, JavaScript does not.
        assert_eq!(trim("\u{0085}a"), "\u{0085}a");
    }

    #[test]
    fn counts_utf16_code_units() {
        assert_eq!(utf16_len("été"), 3);
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn writes_whole_numbers_without_a_fraction() {
        assert_eq!(number(5.0), json!(5));
        assert_eq!(number(-0.0), json!(0));
        assert_eq!(number(1.5), json!(1.5));
        assert_eq!(number(f64::NAN), Value::Null);
        assert!(number(1e21).is_f64());
    }

    #[test]
    fn lists_array_indexes_before_other_keys() {
        let object = json!({ "b": 1, "2": 2, "a": 3, "1": 4, "01": 5, "4294967295": 6 });
        let keys: Vec<&str> = own_keys(object.as_object().unwrap())
            .into_iter()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["1", "2", "b", "a", "01", "4294967295"]);
        let ordered = order_keys(object.as_object().unwrap().clone());
        assert_eq!(
            ordered.keys().map(String::as_str).collect::<Vec<_>>(),
            ["1", "2", "b", "a", "01", "4294967295"]
        );
    }

    #[test]
    fn compares_like_the_relational_operators() {
        assert_eq!(
            compare(Some(&json!("10")), Some(&json!("9"))),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare(Some(&json!("10")), Some(&json!(9))),
            Some(Ordering::Greater)
        );
        assert_eq!(compare(None, Some(&json!(9))), None);
        assert_eq!(
            compare(Some(&json!(null)), Some(&json!(1))),
            Some(Ordering::Less)
        );
    }
}
