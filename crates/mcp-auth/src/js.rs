//! The few JavaScript operations the SDK's validation and requests lean on,
//! with their exact results: `String.prototype.trim`, `Number(value)` and the
//! Latin-1 view `btoa` and header values have of a string.

use crate::json::Json;

/// `String.prototype.trim`: white space and line terminators of ECMAScript,
/// which are not the `White_Space` characters `str::trim` removes.
pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(is_white_space)
}

fn is_white_space(character: char) -> bool {
    matches!(
        character,
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

/// `Number(value)` for a value that came out of `JSON.parse`. `None` is the
/// `TypeError` of an object that cannot be converted to a primitive.
pub(crate) fn to_number(value: &Json) -> Option<f64> {
    match value {
        Json::Null => Some(0.0),
        Json::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Json::Number(value) => Some(*value),
        Json::String(value) => Some(string_to_number(value)),
        Json::Object(members) => {
            if shadows_to_string(members) {
                None
            } else {
                // "[object Object]"
                Some(f64::NAN)
            }
        }
        Json::Array(items) => {
            if items.iter().any(cannot_be_printed) {
                None
            } else {
                Some(array_to_number(items))
            }
        }
    }
}

/// An own `toString` member is not callable, and `valueOf` returns the object
/// itself: nothing is left to convert it with.
fn shadows_to_string(members: &[(String, Json)]) -> bool {
    members.iter().any(|(key, _)| key == "toString")
}

fn cannot_be_printed(value: &Json) -> bool {
    match value {
        Json::Object(members) => shadows_to_string(members),
        Json::Array(items) => items.iter().any(cannot_be_printed),
        _ => false,
    }
}

/// An array is converted through `join(",")`: only an array of at most one
/// element prints as something a number can be read from.
fn array_to_number(items: &[Json]) -> f64 {
    match items {
        [] | [Json::Null] => 0.0,
        // Printed and read back, which loses the sign of zero and nothing else.
        [Json::Number(value)] => *value + 0.0,
        [Json::String(value)] => string_to_number(value),
        [Json::Array(inner)] => array_to_number(inner),
        _ => f64::NAN,
    }
}

/// `StringToNumber` of ECMAScript.
pub(crate) fn string_to_number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix_bits = match bytes[1] {
            b'x' | b'X' => Some(4),
            b'o' | b'O' => Some(3),
            b'b' | b'B' => Some(1),
            _ => None,
        };
        if let Some(radix_bits) = radix_bits {
            return power_of_two_literal(&bytes[2..], radix_bits).unwrap_or(f64::NAN);
        }
    }

    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if !is_decimal_literal(unsigned.as_bytes()) {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// `digits [. digits] [exponent]` or `. digits [exponent]`.
fn is_decimal_literal(bytes: &[u8]) -> bool {
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let integer = digits(0);
    let mut at = integer;
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        fraction = digits(at + 1);
        at += 1 + fraction;
    }
    if integer + fraction == 0 {
        return false;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exponent = digits(at);
        if exponent == 0 {
            return false;
        }
        at += exponent;
    }
    at == bytes.len()
}

/// The digits of a hexadecimal, octal or binary literal, rounded to the
/// nearest double as the mathematical value is.
fn power_of_two_literal(digits: &[u8], radix_bits: u32) -> Option<f64> {
    let mut mantissa: u64 = 0;
    // Bits that no longer fit the mantissa: how many, and whether any is set.
    let mut dropped: u32 = 0;
    let mut sticky = false;
    for &byte in digits {
        let digit = char::from(byte).to_digit(1 << radix_bits)?;
        for bit in (0..radix_bits).rev() {
            let bit = (digit >> bit) & 1;
            if mantissa >> 63 == 0 {
                mantissa = (mantissa << 1) | u64::from(bit);
            } else {
                dropped = dropped.saturating_add(1);
                sticky |= bit == 1;
            }
        }
    }
    if sticky {
        // Enough to round the 64 bits kept to the 53 of a double correctly.
        mantissa |= 1;
    }
    let exponent = i32::try_from(dropped).unwrap_or(i32::MAX);
    Some(if exponent > 1100 {
        f64::INFINITY
    } else {
        // The conversion rounds to nearest, ties to even, as the specification asks.
        mantissa as f64 * 2f64.powi(exponent)
    })
}

/// The bytes of a string whose characters are all below U+0100, which is what
/// `btoa` encodes. `None` is its `InvalidCharacterError`.
pub(crate) fn latin1_bytes(text: &str) -> Option<Vec<u8>> {
    text.chars()
        .map(|character| u8::try_from(u32::from(character)).ok())
        .collect()
}

/// A header value as `fetch` reads it: one character per byte.
pub(crate) fn latin1_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(trim("\u{FEFF}\u{00A0} a\u{2028}\t"), "a");
        // U+0085 is white space to Rust and not to JavaScript.
        assert_eq!(trim("\u{0085}a"), "\u{0085}a");
    }

    #[test]
    fn reads_numbers_the_way_number_does() {
        assert_eq!(string_to_number(" 12 "), 12.0);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert_eq!(string_to_number("0b101"), 5.0);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(string_to_number("-.5"), -0.5);
        assert!(string_to_number("-0x10").is_nan());
        assert!(string_to_number("1_000").is_nan());
        assert!(string_to_number("infinity").is_nan());
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(
            string_to_number("0x20000000000001"),
            9_007_199_254_740_992.0
        );
        assert_eq!(
            string_to_number("0x20000000000003"),
            9_007_199_254_740_996.0
        );
    }

    #[test]
    fn encodes_latin1_only() {
        assert_eq!(latin1_bytes("a\u{e9}"), Some(vec![b'a', 0xE9]));
        assert_eq!(latin1_bytes("\u{20ac}"), None);
    }
}
