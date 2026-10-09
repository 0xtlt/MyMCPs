//! JSON as the Node gateway reads and writes it.
//!
//! Every value the TypeScript app forwarded went through `JSON.parse` and
//! `JSON.stringify`. Two effects of that round trip are visible on the wire
//! and are reproduced here: an object lists its integer-like keys first, in
//! ascending order, and a number is written the way JavaScript prints it
//! (`1.0` becomes `1`, `1e21` becomes `1e+21`).
//!
//! Integers beyond 2^53 keep their exact digits, where JavaScript would round
//! them to the nearest double.

use std::io;

use serde::Serialize;
use serde_json::{Map, Value};

/// Parse JSON text as `JSON.parse` would order it. See [`js_key_order`].
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    let mut value: Value = serde_json::from_str(text)?;
    js_key_order(&mut value);
    Ok(value)
}

/// Parse a body as `Response.json()` does: UTF-8 with invalid sequences
/// replaced, a leading byte order mark ignored.
pub fn parse_bytes(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    let text = String::from_utf8_lossy(bytes);
    parse(text.strip_prefix('\u{feff}').unwrap_or(&text))
}

/// Reorder object keys, at every depth, the way a JavaScript object
/// enumerates them: array indices first in ascending order, then the other
/// keys in insertion order.
pub fn js_key_order(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(js_key_order),
        Value::Object(map) => {
            map.values_mut().for_each(js_key_order);
            reorder(map);
        }
        _ => {}
    }
}

fn reorder(map: &mut Map<String, Value>) {
    // Each array index among the keys, with where its entry is now.
    let mut indices: Vec<(u32, usize)> = map
        .keys()
        .enumerate()
        .filter_map(|(position, key)| array_index(key).map(|index| (index, position)))
        .collect();
    if indices.is_empty() {
        return;
    }
    let ordered_already = indices
        .iter()
        .enumerate()
        .all(|(expected, (_, position))| *position == expected)
        && indices.windows(2).all(|pair| pair[0].0 < pair[1].0);
    if ordered_already {
        return;
    }

    // One pass over the entries, whatever their number: the keys come from
    // whoever wrote the JSON, and taking them out of the map one at a time
    // costs a pass each.
    indices.sort_unstable_by_key(|(index, _)| *index);
    let mut entries: Vec<Option<(String, Value)>> =
        std::mem::take(map).into_iter().map(Some).collect();
    for (_, position) in &indices {
        if let Some((key, value)) = entries[*position].take() {
            map.insert(key, value);
        }
    }
    map.extend(entries.into_iter().flatten());
}

/// An ECMAScript array index: a canonical decimal integer below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// Serialize as `JSON.stringify(value)` does.
pub fn to_string(value: &Value) -> String {
    let mut out = Vec::with_capacity(128);
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter);
    // Writing a `Value` to a `Vec` cannot fail: keys are strings and the writer is infallible.
    if value.serialize(&mut serializer).is_err() {
        return String::from("null");
    }
    String::from_utf8(out).unwrap_or_else(|_| String::from("null"))
}

/// Serialize as `JSON.stringify(value, null, 2)` does.
pub fn to_string_pretty(value: &Value) -> String {
    let mut out = Vec::with_capacity(256);
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut out,
        JsPrettyFormatter(serde_json::ser::PrettyFormatter::with_indent(b"  ")),
    );
    if value.serialize(&mut serializer).is_err() {
        return String::from("null");
    }
    String::from_utf8(out).unwrap_or_else(|_| String::from("null"))
}

struct JsFormatter;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }
}

struct JsPrettyFormatter(serde_json::ser::PrettyFormatter<'static>);

impl serde_json::ser::Formatter for JsPrettyFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// `Number.prototype.toString` for a finite double.
pub(crate) fn js_number(value: f64) -> String {
    if value == 0.0 {
        return String::from("0");
    }
    if !value.is_finite() {
        // `JSON.stringify` writes null; a `serde_json::Value` cannot hold one anyway.
        return String::from("null");
    }

    // Shortest digits that read back as the same double, as `d[.ddd]e[-]x`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let count = digits.len() as i32;
    // Position of the decimal point, counted from the first digit.
    let point = exponent + 1;

    let mut out = String::with_capacity(digits.len() + 8);
    if value < 0.0 {
        out.push('-');
    }
    if count <= point && point <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (point - count) as usize));
    } else if 0 < point && point <= 21 {
        out.push_str(&digits[..point as usize]);
        out.push('.');
        out.push_str(&digits[point as usize..]);
    } else if -6 < point && point <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-point) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if count > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&exponent.abs().to_string());
    }
    out
}

/// `Number(id)` for a JSON-RPC id, when that is a non-negative integer.
///
/// The SDK looks a response up by `Number(response.id)`, so a server that
/// echoes the id as the string `"3"` still answers request 3.
pub(crate) fn id_as_request_number(id: &Value) -> Option<u64> {
    let number = match id {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => string_to_number(text)?,
        _ => return None,
    };
    if number >= 0.0 && number.fract() == 0.0 && number <= 9_007_199_254_740_991.0 {
        Some(number as u64)
    } else {
        None
    }
}

/// ECMAScript `StringToNumber`, without the forms that give NaN or infinity.
fn string_to_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = trimmed.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix)
                .ok()
                .filter(|_| !digits.starts_with('+'))
                .map(|value| value as f64);
        }
    }

    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !all_digits(whole) || !all_digits(fraction) {
        return None;
    }
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if digits.is_empty() || !all_digits(digits) {
            return None;
        }
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_integer_keys_first_like_a_javascript_object() {
        let value =
            parse(r#"{"b":1,"10":2,"2":{"z":0,"1":0},"a":3,"01":4,"4294967295":5}"#).unwrap();
        assert_eq!(
            to_string(&value),
            r#"{"2":{"1":0,"z":0},"10":2,"b":1,"a":3,"01":4,"4294967295":5}"#
        );
    }

    #[test]
    fn orders_the_keys_of_a_large_object_in_one_pass() {
        use std::fmt::Write;

        // Written backwards after another key, so that every index moves.
        let count = 200_000;
        let mut text = String::from("{\"last\":true");
        for index in (0..count).rev() {
            write!(text, ",\"{index}\":{index}").unwrap();
        }
        text.push('}');

        let started = std::time::Instant::now();
        let value = parse(&text).unwrap();
        // Moved one at a time, this many keys took over a minute.
        assert!(started.elapsed() < std::time::Duration::from_secs(20));

        let object = value.as_object().unwrap();
        assert_eq!(object.len(), count + 1);
        let mut keys = object.keys();
        for index in 0..count {
            assert_eq!(keys.next().unwrap(), &index.to_string());
        }
        assert_eq!(keys.next().unwrap(), "last");
        assert_eq!(object["4242"], 4242);
    }

    #[test]
    fn keeps_the_first_position_of_a_duplicated_key() {
        let value = parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(to_string(&value), r#"{"a":3,"b":2}"#);
    }

    #[test]
    fn writes_numbers_the_way_javascript_prints_them() {
        for (text, expected) in [
            ("1.0", "1"),
            ("-0.0", "0"),
            ("1e2", "100"),
            ("0.1", "0.1"),
            ("1.5e300", "1.5e+300"),
            ("1e21", "1e+21"),
            ("123456789012345680000.0", "123456789012345680000"),
            ("1e-7", "1e-7"),
            ("0.000001", "0.000001"),
            ("-2.5", "-2.5"),
            ("1.2345678e-10", "1.2345678e-10"),
            ("42", "42"),
            ("9007199254740993", "9007199254740993"),
        ] {
            assert_eq!(to_string(&parse(text).unwrap()), expected, "{text}");
        }
    }

    #[test]
    fn pretty_prints_like_json_stringify_with_two_spaces() {
        let value = parse(r#"{"a":[],"b":{},"c":[1,{"d":1.0}]}"#).unwrap();
        assert_eq!(
            to_string_pretty(&value),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": 1\n    }\n  ]\n}"
        );
    }

    #[test]
    fn reads_a_response_id_the_way_number_does() {
        assert_eq!(id_as_request_number(&Value::from(3)), Some(3));
        assert_eq!(id_as_request_number(&parse("3.0").unwrap()), Some(3));
        assert_eq!(id_as_request_number(&Value::from("3")), Some(3));
        assert_eq!(id_as_request_number(&Value::from(" 0x10 ")), Some(16));
        assert_eq!(id_as_request_number(&Value::from("")), Some(0));
        assert_eq!(id_as_request_number(&Value::from("1e1")), Some(10));
        assert_eq!(id_as_request_number(&Value::from("abc")), None);
        assert_eq!(id_as_request_number(&Value::from("inf")), None);
        assert_eq!(id_as_request_number(&Value::from(-1)), None);
        assert_eq!(id_as_request_number(&parse("1.5").unwrap()), None);
        assert_eq!(id_as_request_number(&Value::Null), None);
    }
}
