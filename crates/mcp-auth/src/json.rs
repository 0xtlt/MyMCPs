//! `JSON.parse` as Node runs it.
//!
//! The SDK reads every response with `response.json()` or `JSON.parse`, and
//! the message of the `SyntaxError` reaches the administrator: an MCP whose
//! host answers a discovery URL with its HTML page fails with
//! `Unexpected token '<', "<!DOCTYPE "... is not valid JSON`. This parser
//! follows V8's, so that it accepts and refuses the same documents, reports
//! the same message at the same position, and builds the same value: a
//! repeated key keeps its first place and its last value, keys that are array
//! indices come first, and a number is a double.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// A value as `JSON.parse` returns it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    /// Members in the order JavaScript enumerates them.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value as `JSON.stringify` would write it back: a number that is
    /// not finite becomes `null`.
    pub(crate) fn to_value(&self) -> serde_json::Value {
        match self {
            Json::Null => serde_json::Value::Null,
            Json::Bool(value) => serde_json::Value::Bool(*value),
            Json::Number(value) => number_to_value(*value),
            Json::String(value) => serde_json::Value::String(value.clone()),
            Json::Array(items) => {
                serde_json::Value::Array(items.iter().map(Json::to_value).collect())
            }
            Json::Object(members) => serde_json::Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }

    /// Reads a value another parser produced. Members are put back in
    /// JavaScript's order.
    pub(crate) fn from_value(value: &serde_json::Value) -> Json {
        match value {
            serde_json::Value::Null => Json::Null,
            serde_json::Value::Bool(value) => Json::Bool(*value),
            serde_json::Value::Number(value) => Json::Number(value.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(value) => Json::String(value.clone()),
            serde_json::Value::Array(items) => {
                Json::Array(items.iter().map(Json::from_value).collect())
            }
            serde_json::Value::Object(members) => {
                let mut object = ObjectBuilder::default();
                for (key, value) in members {
                    object.insert(key.clone(), Json::from_value(value));
                }
                Json::Object(object.finish())
            }
        }
    }
}

/// A JavaScript number as JSON carries it: an integer is written without a
/// fraction, and what JSON cannot express becomes `null`.
pub(crate) fn number_to_value(value: f64) -> serde_json::Value {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER {
        // Exact: the value is an integer below 2^53.
        return serde_json::Value::Number(serde_json::Number::from(value as i64));
    }
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// The `SyntaxError` of `JSON.parse`. Its text is the JavaScript `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSyntaxError {
    message: String,
}

impl JsonSyntaxError {
    /// The `message` of the JavaScript error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for JsonSyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JsonSyntaxError {}

/// Deepest nesting read. V8 has no limit of its own, but this value is walked
/// recursively afterwards, and no metadata document or token response comes
/// near it.
const MAX_DEPTH: usize = 512;

/// The characters around an unexpected token that the message quotes.
const MAX_CONTEXT_CHARACTERS: usize = 10;

/// `response.json()` and `response.text()`: the body is decoded as UTF-8, a
/// byte order mark is dropped and invalid sequences become U+FFFD.
pub(crate) fn decode_body(body: &[u8]) -> String {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    String::from_utf8_lossy(body).into_owned()
}

pub(crate) fn parse(text: &str) -> Result<Json, JsonSyntaxError> {
    // Positions in the messages count UTF-16 code units, as in JavaScript.
    let source: Vec<u16> = text.encode_utf16().collect();
    let mut parser = Parser {
        source: &source,
        cursor: 0,
    };
    let value = parser.value(0)?;
    parser.skip_whitespace();
    if parser.cursor < source.len() {
        return Err(parser.error(parser.peek(), Some(Template::NonWhitespaceAfterJson)));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    String,
    Number,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    True,
    False,
    Null,
    Colon,
    Comma,
    Whitespace,
    Illegal,
    EndOfString,
}

fn token_of(unit: u16) -> Token {
    match u8::try_from(unit) {
        Ok(b'"') => Token::String,
        Ok(b'-' | b'0'..=b'9') => Token::Number,
        Ok(b'{') => Token::LeftBrace,
        Ok(b'}') => Token::RightBrace,
        Ok(b'[') => Token::LeftBracket,
        Ok(b']') => Token::RightBracket,
        Ok(b't') => Token::True,
        Ok(b'f') => Token::False,
        Ok(b'n') => Token::Null,
        Ok(b':') => Token::Colon,
        Ok(b',') => Token::Comma,
        Ok(b' ' | b'\t' | b'\n' | b'\r') => Token::Whitespace,
        _ => Token::Illegal,
    }
}

#[derive(Debug, Clone, Copy)]
enum Template {
    PropertyNameOrRightBrace,
    CommaOrRightBracket,
    CommaOrRightBrace,
    ColonAfterPropertyName,
    UnterminatedString,
    DoubleQuotedPropertyName,
    ExponentPartMissingNumber,
    UnterminatedFractionalNumber,
    NoNumberAfterMinusSign,
    BadControlCharacter,
    BadUnicodeEscape,
    BadEscapedCharacter,
    NonWhitespaceAfterJson,
}

impl Template {
    fn text(self) -> &'static str {
        match self {
            Template::PropertyNameOrRightBrace => "Expected property name or '}' in JSON",
            Template::CommaOrRightBracket => "Expected ',' or ']' after array element in JSON",
            Template::CommaOrRightBrace => "Expected ',' or '}' after property value in JSON",
            Template::ColonAfterPropertyName => "Expected ':' after property name in JSON",
            Template::UnterminatedString => "Unterminated string in JSON",
            Template::DoubleQuotedPropertyName => "Expected double-quoted property name in JSON",
            Template::ExponentPartMissingNumber => "Exponent part is missing a number in JSON",
            Template::UnterminatedFractionalNumber => "Unterminated fractional number in JSON",
            Template::NoNumberAfterMinusSign => "No number after minus sign in JSON",
            Template::BadControlCharacter => "Bad control character in string literal in JSON",
            Template::BadUnicodeEscape => "Bad Unicode escape in JSON",
            Template::BadEscapedCharacter => "Bad escaped character in JSON",
            Template::NonWhitespaceAfterJson => "Unexpected non-whitespace character after JSON",
        }
    }
}

struct Parser<'a> {
    source: &'a [u16],
    cursor: usize,
}

impl Parser<'_> {
    fn current(&self) -> Option<u16> {
        self.source.get(self.cursor).copied()
    }

    /// Moves to the next code unit and returns it.
    fn next(&mut self) -> Option<u16> {
        self.cursor += 1;
        self.current()
    }

    fn peek(&self) -> Token {
        self.current().map_or(Token::EndOfString, token_of)
    }

    fn skip_whitespace(&mut self) {
        while self.peek() == Token::Whitespace {
            self.cursor += 1;
        }
    }

    /// Skips whitespace, then consumes `token` if it is next.
    fn check(&mut self, token: Token) -> bool {
        self.skip_whitespace();
        if self.peek() != token {
            return false;
        }
        self.cursor += 1;
        true
    }

    fn expect(&mut self, token: Token, otherwise: Template) -> Result<(), JsonSyntaxError> {
        if self.peek() != token {
            return Err(self.error(self.peek(), Some(otherwise)));
        }
        self.cursor += 1;
        Ok(())
    }

    fn expect_next(&mut self, token: Token, otherwise: Template) -> Result<(), JsonSyntaxError> {
        self.skip_whitespace();
        self.expect(token, otherwise)
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonSyntaxError> {
        if depth > MAX_DEPTH {
            return Err(JsonSyntaxError {
                message: format!("JSON is nested more than {MAX_DEPTH} levels deep"),
            });
        }
        self.skip_whitespace();
        match self.peek() {
            Token::String => {
                self.cursor += 1;
                Ok(Json::String(self.string()?))
            }
            Token::Number => self.number(),
            Token::LeftBrace => {
                self.cursor += 1;
                let mut object = ObjectBuilder::default();
                if self.check(Token::RightBrace) {
                    return Ok(Json::Object(object.finish()));
                }
                self.expect_next(Token::String, Template::PropertyNameOrRightBrace)?;
                loop {
                    let key = self.string()?;
                    self.expect_next(Token::Colon, Template::ColonAfterPropertyName)?;
                    let value = self.value(depth + 1)?;
                    object.insert(key, value);
                    if !self.check(Token::Comma) {
                        break;
                    }
                    self.expect_next(Token::String, Template::DoubleQuotedPropertyName)?;
                }
                self.expect(Token::RightBrace, Template::CommaOrRightBrace)?;
                Ok(Json::Object(object.finish()))
            }
            Token::LeftBracket => {
                self.cursor += 1;
                let mut items = Vec::new();
                if self.check(Token::RightBracket) {
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if !self.check(Token::Comma) {
                        break;
                    }
                }
                self.expect(Token::RightBracket, Template::CommaOrRightBracket)?;
                Ok(Json::Array(items))
            }
            Token::True => self.literal("true", Json::Bool(true)),
            Token::False => self.literal("false", Json::Bool(false)),
            Token::Null => self.literal("null", Json::Null),
            token => Err(self.error(token, None)),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, JsonSyntaxError> {
        // The first character is what announced the literal.
        self.cursor += 1;
        for expected in word.bytes().skip(1) {
            match self.current() {
                None => return Err(self.error(Token::EndOfString, None)),
                Some(unit) if unit != u16::from(expected) => {
                    return Err(self.error(token_of(unit), None));
                }
                Some(_) => self.cursor += 1,
            }
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonSyntaxError> {
        let start = self.cursor;
        let mut unit = self.current();
        if unit == Some(u16::from(b'-')) {
            unit = self.next();
        }

        if unit == Some(u16::from(b'0')) {
            // A leading zero is only allowed as the whole integer part.
            if self.next().is_some_and(is_digit) {
                return Err(self.error(Token::Number, None));
            }
        } else {
            let digits = self.cursor;
            self.skip_digits();
            if self.cursor == digits {
                return Err(self.error(Token::Illegal, Some(Template::NoNumberAfterMinusSign)));
            }
        }

        if self.current() == Some(u16::from(b'.')) {
            if !self.next().is_some_and(is_digit) {
                return Err(
                    self.error(Token::Illegal, Some(Template::UnterminatedFractionalNumber))
                );
            }
            self.skip_digits();
        }

        if matches!(self.current(), Some(unit) if unit == u16::from(b'e') || unit == u16::from(b'E'))
        {
            let mut unit = self.next();
            if unit == Some(u16::from(b'-')) || unit == Some(u16::from(b'+')) {
                unit = self.next();
            }
            if !unit.is_some_and(is_digit) {
                return Err(self.error(Token::Illegal, Some(Template::ExponentPartMissingNumber)));
            }
            self.skip_digits();
        }

        // Only ASCII digits, signs, a point and an exponent were scanned.
        let text: String = self.source[start..self.cursor]
            .iter()
            .map(|&unit| char::from(unit as u8))
            .collect();
        // A literal too large for a double is Infinity, as in JavaScript.
        Ok(Json::Number(text.parse::<f64>().unwrap_or(f64::NAN)))
    }

    fn skip_digits(&mut self) {
        while self.current().is_some_and(is_digit) {
            self.cursor += 1;
        }
    }

    /// Reads a string whose opening quote is already consumed.
    fn string(&mut self) -> Result<String, JsonSyntaxError> {
        let mut units: Vec<u16> = Vec::new();
        loop {
            let Some(unit) = self.current() else {
                return Err(self.error(Token::Illegal, Some(Template::UnterminatedString)));
            };
            if unit == u16::from(b'"') {
                self.cursor += 1;
                return Ok(String::from_utf16_lossy(&units));
            }
            if unit == u16::from(b'\\') {
                let escaped = match self.next() {
                    None => return Err(self.error(Token::EndOfString, None)),
                    Some(escaped) => escaped,
                };
                let Ok(escaped) = u8::try_from(escaped) else {
                    return Err(self.error(Token::Illegal, None));
                };
                match escaped {
                    b'"' | b'/' | b'\\' => units.push(u16::from(escaped)),
                    b'b' => units.push(0x08),
                    b'f' => units.push(0x0C),
                    b'n' => units.push(0x0A),
                    b'r' => units.push(0x0D),
                    b't' => units.push(0x09),
                    b'u' => {
                        let mut value: u16 = 0;
                        for _ in 0..4 {
                            let digit = self
                                .next()
                                .and_then(|unit| u8::try_from(unit).ok())
                                .and_then(|unit| char::from(unit).to_digit(16));
                            let Some(digit) = digit else {
                                return Err(
                                    self.error(Token::Illegal, Some(Template::BadUnicodeEscape))
                                );
                            };
                            // Four hexadecimal digits fit sixteen bits.
                            value = value * 16 + digit as u16;
                        }
                        units.push(value);
                    }
                    _ => {
                        return Err(self.error(Token::Illegal, Some(Template::BadEscapedCharacter)));
                    }
                }
                self.cursor += 1;
                continue;
            }
            if unit < 0x20 {
                return Err(self.error(Token::Illegal, Some(Template::BadControlCharacter)));
            }
            units.push(unit);
            self.cursor += 1;
        }
    }

    /// The error for the token at the cursor.
    fn error(&self, token: Token, template: Option<Template>) -> JsonSyntaxError {
        let position = self.cursor;
        let message = match (template, token) {
            (Some(template), _) => self.located(template.text(), position),
            (None, Token::EndOfString) => "Unexpected end of JSON input".to_owned(),
            (None, Token::Number) => self.located("Unexpected number in JSON", position),
            (None, Token::String) => self.located("Unexpected string in JSON", position),
            (None, _) => self.unexpected_token(position),
        };
        JsonSyntaxError { message }
    }

    fn located(&self, text: &str, position: usize) -> String {
        // Only \r and \n end a line in JSON, and \r\n is one line break.
        let mut line = 1;
        let mut line_start = 0;
        let before = &self.source[..position.min(self.source.len())];
        let mut index = 0;
        while index < before.len() {
            if before[index] == u16::from(b'\r') && before.get(index + 1) == Some(&u16::from(b'\n'))
            {
                index += 1;
            }
            if before[index] == u16::from(b'\r') || before[index] == u16::from(b'\n') {
                line += 1;
                line_start = index + 1;
            }
            index += 1;
        }
        let column = 1 + position.saturating_sub(line_start);
        format!("{text} at position {position} (line {line} column {column})")
    }

    fn unexpected_token(&self, position: usize) -> String {
        let source = String::from_utf16_lossy(self.source);
        // What a value that is not JSON text turns into when it is given to JSON.parse.
        if matches!(
            source.as_str(),
            "[object Object]" | "NaN" | "Infinity" | "undefined"
        ) {
            return format!("\"{source}\" is not valid JSON");
        }

        let token = self
            .source
            .get(position)
            .map(|&unit| String::from_utf16_lossy(&[unit]))
            .unwrap_or_default();
        let length = self.source.len();
        if length < MAX_CONTEXT_CHARACTERS * 2 + 1 {
            return format!("Unexpected token '{token}', \"{source}\" is not valid JSON");
        }
        let quote = |from: usize, to: usize| String::from_utf16_lossy(&self.source[from..to]);
        if position < MAX_CONTEXT_CHARACTERS {
            let context = quote(0, position + MAX_CONTEXT_CHARACTERS);
            format!("Unexpected token '{token}', \"{context}\"... is not valid JSON")
        } else if position < length - MAX_CONTEXT_CHARACTERS {
            let context = quote(
                position - MAX_CONTEXT_CHARACTERS,
                position + MAX_CONTEXT_CHARACTERS,
            );
            format!("Unexpected token '{token}', ...\"{context}\"... is not valid JSON")
        } else {
            let context = quote(position - MAX_CONTEXT_CHARACTERS, length);
            format!("Unexpected token '{token}', ...\"{context}\" is not valid JSON")
        }
    }
}

fn is_digit(unit: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&unit)
}

/// Collects the members of an object in the order JavaScript keeps them:
/// array indices first, in ascending order, then the other keys as they first
/// appeared. A key that comes again replaces the value and keeps its place.
#[derive(Default)]
pub(crate) struct ObjectBuilder {
    indices: BTreeMap<u32, (String, Json)>,
    named: Vec<(String, Json)>,
    /// Where each named key is, once there are too many to look through: a
    /// megabyte of JSON can hold a hundred thousand of them.
    positions: Option<HashMap<String, usize>>,
}

impl ObjectBuilder {
    /// Up to this many keys, comparing them all is quicker than hashing.
    const SEARCHED: usize = 16;

    pub(crate) fn insert(&mut self, key: String, value: Json) {
        if let Some(index) = array_index(&key) {
            self.indices.insert(index, (key, value));
            return;
        }
        let existing = match &self.positions {
            Some(positions) => positions.get(&key).copied(),
            None => self.named.iter().position(|(name, _)| *name == key),
        };
        if let Some(position) = existing {
            self.named[position].1 = value;
            return;
        }
        if self.positions.is_none() && self.named.len() == Self::SEARCHED {
            let positions = self.named.iter().enumerate();
            self.positions = Some(
                positions
                    .map(|(at, (name, _))| (name.clone(), at))
                    .collect(),
            );
        }
        if let Some(positions) = &mut self.positions {
            positions.insert(key.clone(), self.named.len());
        }
        self.named.push((key, value));
    }

    pub(crate) fn finish(self) -> Vec<(String, Json)> {
        self.indices.into_values().chain(self.named).collect()
    }
}

/// An array index is the canonical decimal form of an integer below 2^32 - 1.
pub(crate) fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|&index| index != u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numbers by their bits and strings behind a marker, as the recorder
    /// wrote the reference values.
    fn canonical(value: &Json) -> serde_json::Value {
        match value {
            Json::Null => serde_json::Value::Null,
            Json::Bool(value) => serde_json::Value::Bool(*value),
            Json::Number(value) => serde_json::Value::String(format!("#{:016x}", value.to_bits())),
            Json::String(value) => serde_json::Value::String(format!("${value}")),
            Json::Array(items) => serde_json::Value::Array(items.iter().map(canonical).collect()),
            Json::Object(members) => serde_json::Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect(),
            ),
        }
    }

    fn assert_corpus(text: &str) {
        let corpus: serde_json::Value = serde_json::from_str(text).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        assert!(cases.len() > 1000);
        let mut failures = Vec::new();
        for case in cases {
            let input = case[0].as_str().unwrap();
            let expected = case[2].as_str().unwrap();
            let actual = match parse(input) {
                Ok(value) => (0, canonical(&value).to_string()),
                Err(error) => (1, error.message().to_owned()),
            };
            if (actual.0, actual.1.as_str()) != (case[1].as_i64().unwrap(), expected) {
                failures.push(format!(
                    "input {input:?}\n  node: {} {expected:?}\n  rust: {} {:?}",
                    case[1], actual.0, actual.1
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} cases differ from Node:\n{}",
            failures.len(),
            cases.len(),
            failures[..failures.len().min(25)].join("\n")
        );
    }

    #[test]
    fn parses_and_refuses_what_json_parse_does_with_the_same_messages() {
        assert_corpus(include_str!("../tests/fixtures/json_parse.json"));
    }

    /// A larger corpus written by the same recorder (`gen_json_corpus.mjs big <file>`).
    #[test]
    #[ignore = "needs MCP_AUTH_JSON_CORPUS=<file written by the recorder>"]
    fn agrees_with_json_parse_on_an_extended_corpus() {
        let path = std::env::var("MCP_AUTH_JSON_CORPUS").unwrap();
        assert_corpus(&std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn refuses_nesting_no_document_has() {
        let deep = "[".repeat(MAX_DEPTH + 2);
        assert_eq!(
            parse(&deep).unwrap_err().message(),
            "JSON is nested more than 512 levels deep"
        );
        let allowed = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&allowed).is_ok());
    }

    #[test]
    fn keeps_the_order_of_javascript_for_many_members() {
        let mut text = String::from("{");
        for number in (0..60_000).rev() {
            text.push_str(&format!("\"k{number}\":{number},\"{number}\":0,"));
        }
        text.push_str("\"k59999\":-1}");
        let Json::Object(members) = parse(&text).unwrap() else {
            panic!("not an object");
        };
        assert_eq!(members.len(), 120_000);
        // Array indices ascending, then the names as first written.
        assert_eq!(members[0].0, "0");
        assert_eq!(members[59_999].0, "59999");
        assert_eq!(members[60_000], ("k59999".to_owned(), Json::Number(-1.0)));
        assert_eq!(members[119_999].0, "k0");
    }

    #[test]
    fn decodes_a_body_as_response_text_does() {
        assert_eq!(decode_body(b"\xEF\xBB\xBF{}"), "{}");
        assert_eq!(decode_body(b"a\xFFb"), "a\u{FFFD}b");
    }

    #[test]
    fn writes_numbers_as_json_stringify_does() {
        assert_eq!(number_to_value(3600.0).to_string(), "3600");
        assert_eq!(number_to_value(-0.0).to_string(), "0");
        assert_eq!(number_to_value(3.5).to_string(), "3.5");
        assert_eq!(number_to_value(f64::INFINITY).to_string(), "null");
    }
}
