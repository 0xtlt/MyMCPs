//! Error messages: Vine's defaults, and the providers that turn a rule's
//! message template into the sentence a person or an agent reads.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::js;

/// Vine's default messages, by rule name (`src/defaults.ts`). The table is
/// complete on purpose: a custom rule reported under one of these names gets
/// the message here instead of its own, as it does in Vine.
pub const DEFAULT_MESSAGES: &[(&str, &str)] = &[
    ("required", "The {{ field }} field must be defined"),
    ("string", "The {{ field }} field must be a string"),
    (
        "email",
        "The {{ field }} field must be a valid email address",
    ),
    (
        "mobile",
        "The {{ field }} field must be a valid mobile phone number",
    ),
    (
        "creditCard",
        "The {{ field }} field must be a valid {{ providersList }} card number",
    ),
    (
        "passport",
        "The {{ field }} field must be a valid passport number",
    ),
    (
        "postalCode",
        "The {{ field }} field must be a valid postal code",
    ),
    ("regex", "The {{ field }} field format is invalid"),
    (
        "ascii",
        "The {{ field }} field must only contain ASCII characters",
    ),
    ("iban", "The {{ field }} field must be a valid IBAN number"),
    ("jwt", "The {{ field }} field must be a valid JWT token"),
    (
        "coordinates",
        "The {{ field }} field must contain latitude and longitude coordinates",
    ),
    ("url", "The {{ field }} field must be a valid URL"),
    ("activeUrl", "The {{ field }} field must be a valid URL"),
    ("alpha", "The {{ field }} field must contain only letters"),
    (
        "alphaNumeric",
        "The {{ field }} field must contain only letters and numbers",
    ),
    (
        "minLength",
        "The {{ field }} field must have at least {{ min }} characters",
    ),
    (
        "maxLength",
        "The {{ field }} field must not be greater than {{ max }} characters",
    ),
    (
        "fixedLength",
        "The {{ field }} field must be {{ size }} characters long",
    ),
    (
        "confirmed",
        "The {{ originalField }} field and {{ otherField }} field must be the same",
    ),
    (
        "endsWith",
        "The {{ field }} field must end with {{ substring }}",
    ),
    (
        "startsWith",
        "The {{ field }} field must start with {{ substring }}",
    ),
    (
        "sameAs",
        "The {{ field }} field and {{ otherField }} field must be the same",
    ),
    (
        "notSameAs",
        "The {{ field }} field and {{ otherField }} field must be different",
    ),
    ("in", "The selected {{ field }} is invalid"),
    ("notIn", "The selected {{ field }} is invalid"),
    (
        "ipAddress",
        "The {{ field }} field must be a valid IP address",
    ),
    ("vat", "The {{ field }} field must be a valid VAT number"),
    ("uuid", "The {{ field }} field must be a valid UUID"),
    ("ulid", "The {{ field }} field must be a valid ULID"),
    (
        "hexCode",
        "The {{ field }} field must be a valid hex color code",
    ),
    ("boolean", "The value must be a boolean"),
    ("number", "The {{ field }} field must be a number"),
    (
        "number.in",
        "The selected {{ field }} is not in {{ values }}",
    ),
    ("min", "The {{ field }} field must be at least {{ min }}"),
    (
        "max",
        "The {{ field }} field must not be greater than {{ max }}",
    ),
    (
        "range",
        "The {{ field }} field must be between {{ min }} and {{ max }}",
    ),
    ("positive", "The {{ field }} field must be positive"),
    ("negative", "The {{ field }} field must be negative"),
    (
        "nonNegative",
        "The {{ field }} field must be positive or zero",
    ),
    (
        "nonPositive",
        "The {{ field }} field must be negative or zero",
    ),
    (
        "decimal",
        "The {{ field }} field must have {{ digits }} decimal places",
    ),
    (
        "withoutDecimals",
        "The {{ field }} field must be an integer",
    ),
    ("accepted", "The {{ field }} field must be accepted"),
    ("enum", "The selected {{ field }} is invalid"),
    (
        "literal",
        "The {{ field }} field must be {{ expectedValue }}",
    ),
    ("object", "The {{ field }} field must be an object"),
    ("array", "The {{ field }} field must be an array"),
    (
        "array.minLength",
        "The {{ field }} field must have at least {{ min }} items",
    ),
    (
        "array.maxLength",
        "The {{ field }} field must not have more than {{ max }} items",
    ),
    (
        "array.fixedLength",
        "The {{ field }} field must contain {{ size }} items",
    ),
    ("notEmpty", "The {{ field }} field must not be empty"),
    ("distinct", "The {{ field }} field has duplicate values"),
    ("record", "The {{ field }} field must be an object"),
    (
        "record.minLength",
        "The {{ field }} field must have at least {{ min }} items",
    ),
    (
        "record.maxLength",
        "The {{ field }} field must not have more than {{ max }} items",
    ),
    (
        "record.fixedLength",
        "The {{ field }} field must contain {{ size }} items",
    ),
    ("tuple", "The {{ field }} field must be an array"),
    ("union", "Invalid value provided for {{ field }} field"),
    ("unionGroup", "Invalid value provided for {{ field }} field"),
    (
        "unionOfTypes",
        "Invalid value provided for {{ field }} field",
    ),
    ("date", "The {{ field }} field must be a datetime value"),
    (
        "date.equals",
        "The {{ field }} field must be a date equal to {{ expectedValue }}",
    ),
    (
        "date.after",
        "The {{ field }} field must be a date after {{ expectedValue }}",
    ),
    (
        "date.before",
        "The {{ field }} field must be a date before {{ expectedValue }}",
    ),
    (
        "date.afterOrEqual",
        "The {{ field }} field must be a date after or equal to {{ expectedValue }}",
    ),
    (
        "date.beforeOrEqual",
        "The {{ field }} field must be a date before or equal to {{ expectedValue }}",
    ),
    (
        "date.sameAs",
        "The {{ field }} field and {{ otherField }} field must be the same",
    ),
    (
        "date.notSameAs",
        "The {{ field }} field and {{ otherField }} field must be different",
    ),
    (
        "date.afterField",
        "The {{ field }} field must be a date after {{ otherField }}",
    ),
    (
        "date.afterOrSameAs",
        "The {{ field }} field must be a date after or same as {{ otherField }}",
    ),
    (
        "date.beforeField",
        "The {{ field }} field must be a date before {{ otherField }}",
    ),
    (
        "date.beforeOrSameAs",
        "The {{ field }} field must be a date before or same as {{ otherField }}",
    ),
    ("date.weekend", "The {{ field }} field is not a weekend"),
    ("date.weekday", "The {{ field }} field is not a weekday"),
    ("nativeFile", "The {{ field }} field must be a valid file"),
    (
        "nativeFile.minSize",
        "The {{ field }} field must be at least {{ min }} bytes in size",
    ),
    (
        "nativeFile.maxSize",
        "The {{ field }} field must not exceed {{ max }} bytes in size",
    ),
    (
        "nativeFile.mimeTypes",
        "The {{ field }} mime type is invalid",
    ),
];

/// Vine's default field names: the root of the data is called `data`.
pub const DEFAULT_FIELDS: &[(&str, &str)] = &[("", "data")];

/// The default message of a rule.
pub(crate) fn default_message(rule: &str) -> &'static str {
    DEFAULT_MESSAGES
        .iter()
        .find(|(name, _)| *name == rule)
        .map_or("", |(_, message)| message)
}

/// What a messages provider is told about the field an error is on: the part
/// of Vine's `FieldContext` that messages are made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRef<'a> {
    /// `field.name`: the key of the field in its parent, the index of an
    /// item in its array, or the empty string for the root.
    pub name: &'a str,
    /// `field.getFieldPath()`: the path from the root, such as `tools.0.name`.
    pub path: &'a str,
    /// `field.wildCardPath`: the path with `*` for every index and record
    /// key, such as `tools.*.name`.
    pub wildcard_path: &'a str,
    /// `field.isArrayMember`.
    pub is_array_member: bool,
}

impl<'a> FieldRef<'a> {
    /// `{ ...field, name }`: the same field under another name.
    pub fn with_name(self, name: &'a str) -> Self {
        Self { name, ..self }
    }
}

/// `MessagesProviderContact`: makes the message of an error.
///
/// `message` is the template the rule reported, `args` what it reported
/// along with it (`{ min: 8 }` for `minLength(8)`).
pub trait MessagesProvider: Send + Sync {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String;
}

impl<F> MessagesProvider for F
where
    F: Fn(&str, &str, FieldRef<'_>, Option<&Map<String, Value>>) -> String + Send + Sync,
{
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        self(message, rule, field, args)
    }
}

impl MessagesProvider for Arc<dyn MessagesProvider> {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        self.as_ref().get_message(message, rule, field, args)
    }
}

/// `SimpleMessagesProvider`: looks a message up by `<field path>.<rule>`,
/// then `<wildcard path>.<rule>`, then `<rule>`, and falls back to the
/// template the rule reported. `{{ field }}` and the rule's arguments are
/// interpolated in whichever is found.
#[derive(Debug, Clone, Default)]
pub struct SimpleMessagesProvider {
    messages: HashMap<String, String>,
    fields: HashMap<String, String>,
}

impl SimpleMessagesProvider {
    /// `new SimpleMessagesProvider(messages)`: messages by `<rule>`,
    /// `<wildcard path>.<rule>` or `<field path>.<rule>`, and no field names.
    pub fn new<I, K, V>(messages: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            messages: messages
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            fields: HashMap::new(),
        }
    }

    /// The second argument of `new SimpleMessagesProvider(messages, fields)`:
    /// the names fields go by in messages, by field path or field name.
    #[must_use]
    pub fn with_fields<I, K, V>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.fields = fields
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    /// The provider of a new `Vine`: the default messages and field names.
    pub fn defaults() -> Self {
        Self::new(DEFAULT_MESSAGES.iter().copied()).with_fields(DEFAULT_FIELDS.iter().copied())
    }

    fn message(&self, key: &str) -> Option<&str> {
        self.messages
            .get(key)
            .map(String::as_str)
            .filter(|message| !message.is_empty())
    }

    fn field_name(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .filter(|name| !name.is_empty())
    }
}

impl MessagesProvider for SimpleMessagesProvider {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        let field_name = self
            .field_name(field.path)
            .or_else(|| self.field_name(field.name))
            .unwrap_or(field.name);
        let template = self
            .message(&format!("{}.{rule}", field.path))
            .or_else(|| self.message(&format!("{}.{rule}", field.wildcard_path)))
            .or_else(|| self.message(rule))
            .unwrap_or(message);
        interpolate(template, field_name, args)
    }
}

/// Replace every `{{ key }}` of a template. `field` is the name of the
/// field, any other key is looked up in `args`, where `a.b` reaches into an
/// object. What is not found reads `undefined`, as in Vine.
pub fn interpolate(template: &str, field: &str, args: Option<&Map<String, Value>>) -> String {
    if !template.contains("{{") {
        return template.to_owned();
    }
    let is_line_end = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        // The key is what stands before the first `}}` of the same line.
        let close = after
            .find("}}")
            .filter(|close| !after[..*close].contains(is_line_end));
        let Some(close) = close else {
            out.push_str(&rest[..open + 2]);
            rest = after;
            continue;
        };
        // A backslash right before the braces goes with them.
        let before = &rest[..open];
        out.push_str(before.strip_suffix('\\').unwrap_or(before));
        out.push_str(&resolve(&after[..close], field, args));
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

fn resolve(key: &str, field: &str, args: Option<&Map<String, Value>>) -> String {
    let mut tokens = js::trim(key).split('.');
    let first = tokens.next().unwrap_or("");
    // `{ field: fieldName, ...args }`: an argument called `field` wins.
    let mut current: Option<&Value> = args.and_then(|args| args.get(first));
    if current.is_none() && first == "field" {
        return match tokens.next() {
            None => field.to_owned(),
            Some(_) => "undefined".to_owned(),
        };
    }
    let mut tokens = tokens.peekable();
    while let Some(token) = tokens.next() {
        current = match current {
            Some(Value::Object(object)) => object.get(token),
            // The length of an array is a property of its own.
            Some(Value::Array(items)) if token == "length" => {
                return match tokens.peek() {
                    None => items.len().to_string(),
                    Some(_) => "undefined".to_owned(),
                };
            }
            Some(Value::Array(items)) => match token.parse::<usize>() {
                Ok(index) if index.to_string() == token => items.get(index),
                _ => None,
            },
            _ => return "undefined".to_owned(),
        };
    }
    current.map_or_else(|| "undefined".to_owned(), js::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn interpolates_the_field_and_the_arguments() {
        let range =
            args(json!({ "min": 1, "max": 20.5, "choices": ["a", "b"], "o": { "k": true } }));
        let text = interpolate(
            "{{ field }}: {{min}}-{{ max }} [{{ choices }}] {{ o.k }} {{ o.x }} {{ nope }}",
            "limit",
            Some(&range),
        );
        assert_eq!(text, "limit: 1-20.5 [a,b] true undefined undefined");
    }

    #[test]
    fn leaves_what_is_not_a_placeholder() {
        assert_eq!(interpolate("no braces", "x", None), "no braces");
        assert_eq!(interpolate("open {{ only", "x", None), "open {{ only");
        assert_eq!(interpolate("a \\{{ field }} b", "x", None), "a x b");
        assert_eq!(
            interpolate("{{ a\n }} {{ field }}", "x", None),
            "{{ a\n }} x"
        );
    }

    #[test]
    fn looks_a_message_up_from_the_most_to_the_least_specific() {
        let provider = SimpleMessagesProvider::new([
            ("required", "{{ field }} is required"),
            ("tools.*.name.required", "every tool needs a name"),
            ("tools.0.name.required", "the first tool needs a name"),
        ])
        .with_fields([("mcp", "MCP slug")]);
        let field = |path, wildcard_path| FieldRef {
            name: "name",
            path,
            wildcard_path,
            is_array_member: false,
        };
        let message = |field| provider.get_message("fallback", "required", field, None);
        assert_eq!(
            message(field("tools.0.name", "tools.*.name")),
            "the first tool needs a name"
        );
        assert_eq!(
            message(field("tools.1.name", "tools.*.name")),
            "every tool needs a name"
        );
        assert_eq!(message(field("name", "name")), "name is required");
        assert_eq!(
            message(field("mcp", "mcp").with_name("mcp")),
            "MCP slug is required"
        );
        assert_eq!(
            provider.get_message("raw {{ field }}", "other", field("a", "a"), None),
            "raw name"
        );
    }
}
