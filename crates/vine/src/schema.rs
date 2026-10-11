//! Schemas: the types `vine.string()`, `vine.object()` and the others
//! return, and the rules each of them brings.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::field::{FieldContext, ParseContext};
use crate::helpers::{self, UrlOptions};
use crate::js::{self, JsRegex};
use crate::messages::default_message;
use crate::rule::Rule;

pub(crate) type ParseFn = dyn Fn(Option<Value>, &ParseContext<'_>) -> Option<Value> + Send + Sync;
pub(crate) type TransformFn = dyn Fn(Value, &FieldContext<'_>) -> Value + Send + Sync;

/// The `parse` callback of a schema: what `schema.options.parse` holds.
#[derive(Clone)]
pub struct Parser(pub(crate) Arc<ParseFn>);

impl Parser {
    /// Run the callback. `None` stands for `undefined`, in and out.
    pub fn call(&self, value: Option<Value>, context: &ParseContext<'_>) -> Option<Value> {
        (self.0)(value, context)
    }
}

impl fmt::Debug for Parser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Parser")
    }
}

/// The check of the type itself, which runs before the rules and decides
/// whether they run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataType {
    String,
    Number { strict: bool },
    Date,
}

/// What a literal type says of itself in a JSON Schema beyond its rules.
#[derive(Debug, Clone)]
pub(crate) enum Describes {
    Rules,
    Any,
    Literal(Value),
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    Literal {
        data_type: Option<DataType>,
        describes: Describes,
    },
    Object {
        properties: Vec<(String, Schema)>,
        allow_unknown: bool,
    },
    Array(Box<Schema>),
    Record(Box<Schema>),
}

/// Any schema, whatever its type: what an object takes for a property, an
/// array for its items, and [`Vine::create`](crate::Vine::create) for the
/// root. Every schema type converts into it.
///
/// It keeps the modifiers all types share. To keep adding the rules of one
/// type (`max_length`, `min`, ...), stay with that type: they all return
/// themselves from every method.
#[derive(Clone)]
pub struct Schema {
    pub(crate) kind: Kind,
    pub(crate) bail: bool,
    pub(crate) allow_null: bool,
    pub(crate) is_optional: bool,
    pub(crate) parse: Option<Parser>,
    pub(crate) validations: Vec<Rule>,
    /// How many of the rules were added before `optional()`. Only those are
    /// described in the JSON Schema, as in Vine.
    pub(crate) described: Option<usize>,
    pub(crate) transform: Option<Arc<TransformFn>>,
}

impl fmt::Debug for Schema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Schema")
            .field("kind", &self.kind)
            .field("bail", &self.bail)
            .field("allow_null", &self.allow_null)
            .field("is_optional", &self.is_optional)
            .field("parse", &self.parse)
            .field("validations", &self.validations)
            .field("transform", &self.transform.is_some())
            .finish()
    }
}

/// The operators of `requiredWhen(otherField, operator, expectedValue)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `'='`
    Eq,
    /// `'!='`
    NotEq,
    /// `'in'`
    In,
    /// `'notIn'`
    NotIn,
    /// `'>'`
    Gt,
    /// `'<'`
    Lt,
    /// `'>='`
    Gte,
    /// `'<='`
    Lte,
}

impl Operator {
    fn holds(self, value: Option<&Value>, expected: &Value) -> bool {
        let within = || match (value, expected) {
            (Some(value), Value::Array(list)) => js::includes(list, value),
            _ => false,
        };
        let order = || js::compare(value, Some(expected));
        match self {
            Self::Eq => js::strict_equals_opt(value, Some(expected)),
            Self::NotEq => !js::strict_equals_opt(value, Some(expected)),
            Self::In => within(),
            Self::NotIn => !within(),
            Self::Gt => order() == Some(Ordering::Greater),
            Self::Lt => order() == Some(Ordering::Less),
            Self::Gte => matches!(order(), Some(Ordering::Greater | Ordering::Equal)),
            Self::Lte => matches!(order(), Some(Ordering::Less | Ordering::Equal)),
        }
    }
}

/// `requiredWhen`: an implicit rule that asks for the field when the
/// checker says so.
fn required_when_rule(checker: impl Fn(&FieldContext<'_>) -> bool + Send + Sync + 'static) -> Rule {
    Rule::new(move |_, field| {
        if !field.is_defined() && checker(field) {
            field.report(default_message("required"), "required");
        }
    })
    .implicit()
}

fn names(fields: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    fields.into_iter().map(Into::into).collect()
}

impl Schema {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            bail: true,
            allow_null: false,
            is_optional: false,
            parse: None,
            validations: Vec::new(),
            described: None,
            transform: None,
        }
    }

    fn literal(data_type: Option<DataType>, describes: Describes, rules: Vec<Rule>) -> Self {
        let mut schema = Self::new(Kind::Literal {
            data_type,
            describes,
        });
        schema.validations = rules;
        schema
    }

    /// `.parse(callback)`: change the value before anything looks at it.
    /// The callback receives the value as it was sent, where `None` stands
    /// for `undefined`, and returns the value to validate. A second call
    /// replaces the first.
    #[must_use]
    pub fn parse(
        mut self,
        callback: impl Fn(Option<Value>, &ParseContext<'_>) -> Option<Value> + Send + Sync + 'static,
    ) -> Self {
        self.parse = Some(Parser(Arc::new(callback)));
        self
    }

    /// `schema.options.parse`: the callback set by [`parse`](Self::parse).
    pub fn parser(&self) -> Option<Parser> {
        self.parse.clone()
    }

    /// `.use(rule)`: add a rule after the ones the schema has.
    #[must_use]
    pub fn use_rule(mut self, rule: Rule) -> Self {
        self.validations.push(rule);
        self
    }

    /// `.bail(state)`: with `false`, keep running the rules of the field
    /// after one of them reported.
    #[must_use]
    pub fn bail(mut self, state: bool) -> Self {
        self.bail = state;
        self
    }

    /// `.optional()`: the field may be left out or `null`. Either way it is
    /// then absent from the output.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.is_optional = true;
        self.described.get_or_insert(self.validations.len());
        self
    }

    /// `.nullable()`: the field may be `null`, and is `null` in the output.
    #[must_use]
    pub fn nullable(mut self) -> Self {
        self.allow_null = true;
        self
    }

    /// `.requiredWhen(otherField, operator, expectedValue)`: required when
    /// the sibling `other_field` compares that way to `expected`. A name
    /// with dots is a path from the root of the data.
    #[must_use]
    pub fn required_when(
        self,
        other_field: &str,
        operator: Operator,
        expected: impl Into<Value>,
    ) -> Self {
        let other_field = other_field.to_owned();
        let expected = expected.into();
        self.required_when_fn(move |field| {
            operator.holds(field.nested_value(&other_field), &expected)
        })
    }

    /// `.requiredWhen((field) => boolean)`.
    #[must_use]
    pub fn required_when_fn(
        self,
        checker: impl Fn(&FieldContext<'_>) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.use_rule(required_when_rule(checker))
    }

    /// `.requiredIfExists(fields)`: required when all of `fields` are
    /// neither `null` nor left out.
    #[must_use]
    pub fn required_if_exists(self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let fields = names(fields);
        self.required_when_fn(move |field| {
            fields
                .iter()
                .all(|other| helpers::exists(field.nested_value(other)))
        })
    }

    /// `.requiredIfAnyExists(fields)`.
    #[must_use]
    pub fn required_if_any_exists(
        self,
        fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let fields = names(fields);
        self.required_when_fn(move |field| {
            fields
                .iter()
                .any(|other| helpers::exists(field.nested_value(other)))
        })
    }

    /// `.requiredIfMissing(fields)`: required when all of `fields` are
    /// `null` or left out.
    #[must_use]
    pub fn required_if_missing(self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let fields = names(fields);
        self.required_when_fn(move |field| {
            fields
                .iter()
                .all(|other| helpers::is_missing(field.nested_value(other)))
        })
    }

    /// `.requiredIfAnyMissing(fields)`.
    #[must_use]
    pub fn required_if_any_missing(
        self,
        fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let fields = names(fields);
        self.required_when_fn(move |field| {
            fields
                .iter()
                .any(|other| helpers::is_missing(field.nested_value(other)))
        })
    }

    /// `schema.toJSONSchema()`: the schema as a JSON Schema, made of what
    /// its type and each of its rules say of themselves.
    pub fn to_json_schema(&self) -> Value {
        Value::Object(self.describe())
    }

    fn describe(&self) -> Map<String, Value> {
        let mut schema = Map::new();
        let is_literal = matches!(self.kind, Kind::Literal { .. });
        match &self.kind {
            Kind::Literal {
                data_type,
                describes,
            } => {
                if matches!(describes, Describes::Any) {
                    let types = ["string", "number", "boolean", "array", "object"];
                    let any: Vec<Value> =
                        types.iter().map(|name| json!({ "type": name })).collect();
                    schema.insert("anyOf".to_owned(), Value::Array(any));
                }
                match data_type {
                    Some(DataType::String) => {
                        schema.insert("type".to_owned(), json!("string"));
                    }
                    Some(DataType::Number { .. }) => {
                        schema.insert("type".to_owned(), json!("number"));
                    }
                    Some(DataType::Date) | None => {}
                }
            }
            Kind::Object {
                properties,
                allow_unknown,
            } => {
                let mut described = Map::new();
                let mut required = Vec::new();
                for (name, property) in properties {
                    described.insert(name.clone(), property.to_json_schema());
                    if !property.is_optional && !property.allow_null {
                        required.push(Value::String(name.clone()));
                    }
                }
                schema.insert("type".to_owned(), json!("object"));
                schema.insert("properties".to_owned(), Value::Object(described));
                schema.insert("required".to_owned(), Value::Array(required));
                schema.insert(
                    "additionalProperties".to_owned(),
                    Value::Bool(*allow_unknown),
                );
            }
            Kind::Array(each) => {
                schema.insert("type".to_owned(), json!("array"));
                schema.insert("items".to_owned(), each.to_json_schema());
            }
            Kind::Record(each) => {
                schema.insert("type".to_owned(), json!("object"));
                schema.insert("additionalProperties".to_owned(), each.to_json_schema());
            }
        }

        let described = self.described.unwrap_or(self.validations.len());
        for rule in &self.validations[..described.min(self.validations.len())] {
            if let Some(describe) = &rule.json_schema {
                describe(&mut schema);
            }
        }
        if let Kind::Literal {
            describes: Describes::Literal(value),
            ..
        } = &self.kind
        {
            let name = match value {
                Value::String(_) => Some("string"),
                Value::Bool(_) => Some("boolean"),
                Value::Number(_) => Some("number"),
                _ => None,
            };
            if let Some(name) = name {
                schema.insert("type".to_owned(), json!(name));
                schema.insert("enum".to_owned(), json!([value]));
            }
        }
        if self.allow_null {
            with_null(schema, is_literal)
        } else {
            schema
        }
    }
}

/// What `.nullable()` does to a JSON Schema.
fn with_null(mut schema: Map<String, Value>, is_literal: bool) -> Map<String, Value> {
    let null = json!({ "type": "null" });
    if let Some(Value::Array(any)) = schema.get_mut("anyOf") {
        any.push(null);
        return schema;
    }
    if is_literal && schema.contains_key("enum") {
        let mut wrapped = Map::new();
        wrapped.insert(
            "anyOf".to_owned(),
            Value::Array(vec![Value::Object(schema), null]),
        );
        return wrapped;
    }
    match schema.get_mut("type") {
        None => {
            schema.insert("type".to_owned(), json!("null"));
        }
        Some(Value::Array(types)) => types.push(json!("null")),
        Some(name) => *name = json!([name.clone(), "null"]),
    }
    schema
}

/// What `vine.string().regex(...)` takes: a regular expression, or any
/// other way of saying whether a string has the right format.
#[derive(Clone)]
pub struct Pattern {
    test: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    source: String,
}

impl Pattern {
    /// A check written by hand, for the expressions the `regex` crate cannot
    /// run, such as one with a lookahead. `source` is the JavaScript
    /// expression it stands for, which the JSON Schema of the field gives as
    /// its `pattern`.
    pub fn from_fn(
        source: impl Into<String>,
        test: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            test: Arc::new(test),
            source: source.into(),
        }
    }

    /// `expression.test(text)`.
    pub fn test(&self, text: &str) -> bool {
        (self.test)(text)
    }

    /// `expression.source`.
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl fmt::Debug for Pattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Pattern")
            .field(&self.source)
            .finish()
    }
}

impl From<JsRegex> for Pattern {
    fn from(expression: JsRegex) -> Self {
        let source = expression.source().to_owned();
        Self {
            test: Arc::new(move |text| expression.test(text)),
            source,
        }
    }
}

impl From<&JsRegex> for Pattern {
    fn from(expression: &JsRegex) -> Self {
        expression.clone().into()
    }
}

/// An expression of the `regex` crate, taken as it is. Mind what `\d`, `\w`
/// and `\s` mean there: see [`JsRegex`].
impl From<Regex> for Pattern {
    fn from(expression: Regex) -> Self {
        let source = expression.as_str().to_owned();
        Self {
            test: Arc::new(move |text| expression.is_match(text)),
            source,
        }
    }
}

fn set(schema: &mut Map<String, Value>, key: &str, value: Value) {
    schema.insert(key.to_owned(), value);
}

/// A rule over the text of a string field.
fn text_rule(check: impl Fn(&str, &mut FieldContext<'_>) + Send + Sync + 'static) -> Rule {
    Rule::new(move |value, field| {
        if let Value::String(text) = value {
            check(text, field);
        }
    })
}

/// A rule over the number of a number field.
fn number_rule(check: impl Fn(f64, &mut FieldContext<'_>) + Send + Sync + 'static) -> Rule {
    Rule::new(move |value, field| {
        if let Some(number) = js::as_f64(value) {
            check(number, field);
        }
    })
}

fn boolean_rule(strict: bool) -> Rule {
    Rule::new(move |value, field| {
        let flag = if strict {
            value.as_bool()
        } else {
            helpers::as_boolean(value)
        };
        match flag {
            Some(flag) => field.mutate(flag),
            None => field.report(default_message("boolean"), "boolean"),
        }
    })
    .json_schema(move |schema| {
        if strict {
            set(schema, "type", json!("boolean"));
        } else {
            set(
                schema,
                "enum",
                json!(["1", 1, "true", true, "on", "0", 0, "false", false]),
            );
        }
    })
}

fn enum_rule(choices: Vec<Value>) -> Rule {
    let described = choices.clone();
    Rule::new(move |value, field| {
        if !js::includes(&choices, value) {
            field.report_with(
                default_message("enum"),
                "enum",
                json!({ "choices": choices }),
            );
        }
    })
    .json_schema(move |schema| set(schema, "enum", Value::Array(described.clone())))
}

/// `helpers.compareValues`: the value is read as the type of the one
/// expected before the two are compared.
fn literal_rule(expected: Value) -> Rule {
    Rule::new(move |value, field| {
        let casted = match &expected {
            Value::Bool(_) => helpers::as_boolean(value).map_or(Value::Null, Value::Bool),
            Value::Number(_) => js::number(helpers::as_number(value)),
            _ => value.clone(),
        };
        if js::strict_equals(&casted, &expected) {
            field.mutate(casted);
        } else {
            field.report_with(
                default_message("literal"),
                "literal",
                json!({ "expectedValue": expected }),
            );
        }
    })
}

/// Generates, for a schema type, the methods every schema has.
macro_rules! common_methods {
    ($type:ident) => {
        impl $type {
            /// `.parse(callback)`. See [`Schema::parse`].
            #[must_use]
            pub fn parse(
                self,
                callback: impl Fn(Option<Value>, &ParseContext<'_>) -> Option<Value>
                + Send
                + Sync
                + 'static,
            ) -> Self {
                Self(self.0.parse(callback))
            }

            /// `schema.options.parse`. See [`Schema::parser`].
            pub fn parser(&self) -> Option<Parser> {
                self.0.parser()
            }

            /// `.use(rule)`. See [`Schema::use_rule`].
            #[must_use]
            pub fn use_rule(self, rule: Rule) -> Self {
                Self(self.0.use_rule(rule))
            }

            /// `.bail(state)`. See [`Schema::bail`].
            #[must_use]
            pub fn bail(self, state: bool) -> Self {
                Self(self.0.bail(state))
            }

            /// `.optional()`. See [`Schema::optional`].
            #[must_use]
            pub fn optional(self) -> Self {
                Self(self.0.optional())
            }

            /// `.nullable()`. See [`Schema::nullable`].
            #[must_use]
            pub fn nullable(self) -> Self {
                Self(self.0.nullable())
            }

            /// `.requiredWhen(otherField, operator, expectedValue)`. See
            /// [`Schema::required_when`].
            #[must_use]
            pub fn required_when(
                self,
                other_field: &str,
                operator: Operator,
                expected: impl Into<Value>,
            ) -> Self {
                Self(self.0.required_when(other_field, operator, expected))
            }

            /// `.requiredWhen((field) => boolean)`.
            #[must_use]
            pub fn required_when_fn(
                self,
                checker: impl Fn(&FieldContext<'_>) -> bool + Send + Sync + 'static,
            ) -> Self {
                Self(self.0.required_when_fn(checker))
            }

            /// `.requiredIfExists(fields)`. See [`Schema::required_if_exists`].
            #[must_use]
            pub fn required_if_exists(
                self,
                fields: impl IntoIterator<Item = impl Into<String>>,
            ) -> Self {
                Self(self.0.required_if_exists(fields))
            }

            /// `.requiredIfAnyExists(fields)`.
            #[must_use]
            pub fn required_if_any_exists(
                self,
                fields: impl IntoIterator<Item = impl Into<String>>,
            ) -> Self {
                Self(self.0.required_if_any_exists(fields))
            }

            /// `.requiredIfMissing(fields)`. See [`Schema::required_if_missing`].
            #[must_use]
            pub fn required_if_missing(
                self,
                fields: impl IntoIterator<Item = impl Into<String>>,
            ) -> Self {
                Self(self.0.required_if_missing(fields))
            }

            /// `.requiredIfAnyMissing(fields)`.
            #[must_use]
            pub fn required_if_any_missing(
                self,
                fields: impl IntoIterator<Item = impl Into<String>>,
            ) -> Self {
                Self(self.0.required_if_any_missing(fields))
            }

            /// `schema.toJSONSchema()`. See [`Schema::to_json_schema`].
            pub fn to_json_schema(&self) -> Value {
                self.0.to_json_schema()
            }
        }

        impl From<$type> for Schema {
            fn from(schema: $type) -> Schema {
                schema.0
            }
        }
    };
}

/// Generates the methods of the types that hold one value, on top of the
/// common ones.
macro_rules! literal_methods {
    ($type:ident) => {
        common_methods!($type);

        impl $type {
            /// `.transform(callback)`: change the value once it is valid,
            /// for the output only. When the schema is nullable the callback
            /// also receives `null`.
            #[must_use]
            pub fn transform(
                mut self,
                callback: impl Fn(Value, &FieldContext<'_>) -> Value + Send + Sync + 'static,
            ) -> Self {
                self.0.transform = Some(Arc::new(callback));
                self
            }
        }
    };
}

/// `vine.string()`.
#[derive(Debug, Clone)]
pub struct VineString(Schema);
literal_methods!(VineString);

/// `vine.string()`: the value must be a string. With the conversion of
/// empty strings on, one made of whitespace only counts as `null`.
pub fn string() -> VineString {
    VineString(Schema::literal(
        Some(DataType::String),
        Describes::Rules,
        Vec::new(),
    ))
}

impl VineString {
    /// `.trim()`: remove the whitespace around the string, as
    /// `String.prototype.trim` does.
    #[must_use]
    pub fn trim(self) -> Self {
        self.use_rule(text_rule(|text, field| {
            if field.is_valid() {
                field.mutate(js::trim(text));
            }
        }))
    }

    /// `.minLength(min)`, in UTF-16 code units as `String.prototype.length`.
    #[must_use]
    pub fn min_length(self, min: usize) -> Self {
        let rule = text_rule(move |text, field| {
            if js::utf16_len(text) < min {
                field.report_with(
                    default_message("minLength"),
                    "minLength",
                    json!({ "min": min }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "minLength", json!(min))))
    }

    /// `.maxLength(max)`, in UTF-16 code units.
    #[must_use]
    pub fn max_length(self, max: usize) -> Self {
        let rule = text_rule(move |text, field| {
            if js::utf16_len(text) > max {
                field.report_with(
                    default_message("maxLength"),
                    "maxLength",
                    json!({ "max": max }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "maxLength", json!(max))))
    }

    /// `.fixedLength(size)`, in UTF-16 code units.
    #[must_use]
    pub fn fixed_length(self, size: usize) -> Self {
        let rule = text_rule(move |text, field| {
            if js::utf16_len(text) != size {
                field.report_with(
                    default_message("fixedLength"),
                    "fixedLength",
                    json!({ "size": size }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| {
            set(schema, "minLength", json!(size));
            set(schema, "maxLength", json!(size));
        }))
    }

    /// `.regex(expression)`. Pass a [`JsRegex`] to keep what the
    /// JavaScript expression means, or a [`Pattern::from_fn`] for what the
    /// `regex` crate cannot run.
    #[must_use]
    pub fn regex(self, expression: impl Into<Pattern>) -> Self {
        let expression = expression.into();
        let source = expression.source().to_owned();
        let rule = text_rule(move |text, field| {
            if !expression.test(text) {
                field.report(default_message("regex"), "regex");
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "pattern", json!(source))))
    }

    /// `.url()`: what validator.js takes for a URL, with its default options.
    #[must_use]
    pub fn url(self) -> Self {
        self.url_with(UrlOptions::default())
    }

    /// `.url(options)`, such as `.url({ require_tld: false })`.
    #[must_use]
    pub fn url_with(self, options: UrlOptions) -> Self {
        let rule = text_rule(move |text, field| {
            if !helpers::is_url(text, &options) {
                field.report(default_message("url"), "url");
            }
        });
        self.use_rule(rule.json_schema(|schema| set(schema, "format", json!("uri"))))
    }

    /// `.email()`: what validator.js takes for an email address, with its
    /// default options.
    #[must_use]
    pub fn email(self) -> Self {
        let rule = text_rule(|text, field| {
            if !helpers::is_email(text) {
                field.report(default_message("email"), "email");
            }
        });
        self.use_rule(rule.json_schema(|schema| set(schema, "format", json!("email"))))
    }

    /// `.confirmed()` and `.confirmed({ confirmationField })`: the sibling
    /// that confirms the field must hold the same value. Without a name the
    /// sibling is `<field>_confirmation`. The error is reported on the
    /// sibling.
    #[must_use]
    pub fn confirmed<'a>(self, confirmation_field: impl Into<Option<&'a str>>) -> Self {
        let confirmation_field = confirmation_field.into().map(str::to_owned);
        self.use_rule(Rule::new(move |value, field| {
            let other = match &confirmation_field {
                Some(other) => other.clone(),
                None => format!("{}_confirmation", field.name()),
            };
            if !js::strict_equals_opt(field.parent_get(&other), Some(value)) {
                let args = json!({ "otherField": other, "originalField": field.name() });
                let Value::Object(args) = args else { return };
                field.report_on_sibling(&other, default_message("confirmed"), "confirmed", args);
            }
        }))
    }

    /// `.startsWith(substring)`.
    #[must_use]
    pub fn starts_with(self, substring: &str) -> Self {
        let substring = substring.to_owned();
        self.use_rule(text_rule(move |text, field| {
            if !text.starts_with(&substring) {
                field.report_with(
                    default_message("startsWith"),
                    "startsWith",
                    json!({ "substring": substring }),
                );
            }
        }))
    }

    /// `.endsWith(substring)`.
    #[must_use]
    pub fn ends_with(self, substring: &str) -> Self {
        let substring = substring.to_owned();
        self.use_rule(text_rule(move |text, field| {
            if !text.ends_with(&substring) {
                field.report_with(
                    default_message("endsWith"),
                    "endsWith",
                    json!({ "substring": substring }),
                );
            }
        }))
    }

    /// `.in(choices)`: the string must be one of `choices`.
    #[must_use]
    pub fn in_(self, choices: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let choices: Vec<String> = names(choices);
        self.use_rule(text_rule(move |text, field| {
            if !choices.iter().any(|choice| choice == text) {
                field.report_with(default_message("in"), "in", json!({ "choices": choices }));
            }
        }))
    }
}

/// `vine.number()`.
#[derive(Debug, Clone)]
pub struct VineNumber(Schema);
literal_methods!(VineNumber);

/// `vine.number()`: a number, or what `Number(value)` reads as one: a
/// numeric string, a boolean, and more. The output is the number.
/// [`strict`](VineNumber::strict) takes numbers only.
pub fn number() -> VineNumber {
    VineNumber(Schema::literal(
        Some(DataType::Number { strict: false }),
        Describes::Rules,
        Vec::new(),
    ))
}

impl VineNumber {
    /// `vine.number({ strict: true })`: the value must be a JSON number.
    #[must_use]
    pub fn strict(mut self) -> Self {
        if let Kind::Literal { data_type, .. } = &mut self.0.kind {
            *data_type = Some(DataType::Number { strict: true });
        }
        self
    }

    /// `.min(value)`.
    #[must_use]
    pub fn min(self, min: impl Into<f64>) -> Self {
        let min = min.into();
        let rule = number_rule(move |number, field| {
            if number < min {
                field.report_with(
                    default_message("min"),
                    "min",
                    json!({ "min": js::number(min) }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "minimum", js::number(min))))
    }

    /// `.max(value)`.
    #[must_use]
    pub fn max(self, max: impl Into<f64>) -> Self {
        let max = max.into();
        let rule = number_rule(move |number, field| {
            if number > max {
                field.report_with(
                    default_message("max"),
                    "max",
                    json!({ "max": js::number(max) }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "maximum", js::number(max))))
    }

    /// `.range([min, max])`.
    #[must_use]
    pub fn range<N: Into<f64>>(self, bounds: [N; 2]) -> Self {
        let [min, max] = bounds.map(Into::into);
        let rule = number_rule(move |number, field| {
            if number < min || number > max {
                field.report_with(
                    default_message("range"),
                    "range",
                    json!({ "min": js::number(min), "max": js::number(max) }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| {
            set(schema, "minimum", js::number(min));
            set(schema, "maximum", js::number(max));
        }))
    }

    /// `.positive()`: above zero.
    #[must_use]
    pub fn positive(self) -> Self {
        let rule = number_rule(|number, field| {
            if number <= 0.0 {
                field.report(default_message("positive"), "positive");
            }
        });
        self.use_rule(rule.json_schema(|schema| set(schema, "minimum", json!(0))))
    }

    /// `.withoutDecimals()`: a whole number.
    #[must_use]
    pub fn without_decimals(self) -> Self {
        let rule = number_rule(|number, field| {
            if !js::is_integer(number) {
                field.report(default_message("withoutDecimals"), "withoutDecimals");
            }
        });
        self.use_rule(rule.json_schema(|schema| set(schema, "type", json!("integer"))))
    }

    /// `.in(values)`: the number must be one of `values`.
    #[must_use]
    pub fn in_(self, values: impl IntoIterator<Item = impl Into<f64>>) -> Self {
        let values: Vec<f64> = values.into_iter().map(Into::into).collect();
        let list: Vec<Value> = values.iter().copied().map(js::number).collect();
        let described = list.clone();
        let rule = number_rule(move |number, field| {
            if !values.contains(&number) {
                // Vine reports the message of `number.in` under the rule name `in`.
                field.report_with(
                    default_message("number.in"),
                    "in",
                    json!({ "values": list }),
                );
            }
        });
        self.use_rule(
            rule.json_schema(move |schema| set(schema, "enum", Value::Array(described.clone()))),
        )
    }
}

/// `vine.boolean()`.
#[derive(Debug, Clone)]
pub struct VineBoolean(Schema);
literal_methods!(VineBoolean);

/// `vine.boolean()`: `true`, `1`, `"1"`, `"true"` and `"on"` are true;
/// `false`, `0`, `"0"` and `"false"` are false. The output is the boolean.
/// [`strict`](VineBoolean::strict) takes booleans only.
pub fn boolean() -> VineBoolean {
    VineBoolean(Schema::literal(
        None,
        Describes::Rules,
        vec![boolean_rule(false)],
    ))
}

impl VineBoolean {
    /// `vine.boolean({ strict: true })`: the value must be a JSON boolean.
    #[must_use]
    pub fn strict(mut self) -> Self {
        // The check of the type is the first rule of the schema.
        if let Some(first) = self.0.validations.first_mut() {
            *first = boolean_rule(true);
        }
        self
    }
}

/// `vine.enum(values)`.
#[derive(Debug, Clone)]
pub struct VineEnum(Schema);
literal_methods!(VineEnum);

/// `vine.enum(values)`: the value must be one of `values`, compared with
/// `===`.
pub fn enum_(values: impl IntoIterator<Item = impl Into<Value>>) -> VineEnum {
    let choices: Vec<Value> = values.into_iter().map(Into::into).collect();
    VineEnum(Schema::literal(
        None,
        Describes::Rules,
        vec![enum_rule(choices)],
    ))
}

/// `vine.literal(value)`.
#[derive(Debug, Clone)]
pub struct VineLiteral(Schema);
literal_methods!(VineLiteral);

/// `vine.literal(value)`: the value must be `value`. When that is a boolean
/// or a number, the value is first read as one, as `vine.boolean()` and
/// `vine.number()` would.
pub fn literal(value: impl Into<Value>) -> VineLiteral {
    let value = value.into();
    VineLiteral(Schema::literal(
        None,
        Describes::Literal(value.clone()),
        vec![literal_rule(value)],
    ))
}

/// `vine.any()`.
#[derive(Debug, Clone)]
pub struct VineAny(Schema);
literal_methods!(VineAny);

/// `vine.any()`: any value but `null` and `undefined`, kept as it is.
pub fn any() -> VineAny {
    VineAny(Schema::literal(None, Describes::Any, Vec::new()))
}

/// `vine.date({ formats: ['iso8601'] })`.
#[derive(Debug, Clone)]
pub struct VineDate(Schema);
literal_methods!(VineDate);

/// `vine.date({ formats: ['iso8601'] })`: an ISO 8601 date, or date and
/// time, or a number of milliseconds since the epoch.
///
/// Vine hands the value to Day.js, and Day.js hands a string it does not
/// know to `new Date(string)`. What they read is read here the same way,
/// quirks included: `2026-10-01` and `2026-10-01T12:00` are dates, a time
/// without an offset is in the server's time zone, a month or a day out of
/// range rolls over as in `Date`. Two bounds:
///
/// - the server's time zone is taken to be UTC, which is what the Docker
///   image runs in;
/// - of `new Date(string)`, only the ECMAScript date time format is read.
///   The other shapes V8 guesses at, such as `October 1, 2026`, an RFC 2822
///   date or the digit `5`, are refused.
///
/// The output is the instant as `Date.prototype.toISOString()` writes it,
/// such as `2026-10-01T10:00:00.000Z`, where the TypeScript app got a Luxon
/// `DateTime` through `VineDate.transform` in `start/validator.ts`. A
/// `chrono::DateTime<Utc>` deserializes from it for any year from 0000 to
/// 9999.
pub fn date_iso8601() -> VineDate {
    VineDate(Schema::literal(
        Some(DataType::Date),
        Describes::Rules,
        Vec::new(),
    ))
}

/// A type made of rules alone: what a class extending `BaseLiteralType`
/// without a data type validator is in TypeScript, such as `VineArgument`.
#[derive(Debug, Clone)]
pub struct VineCustom(Schema);
literal_methods!(VineCustom);

/// `new VineArgument(...validations)`: a type that is whatever its rules
/// accept. The rules run on any value that is neither `null` nor left out,
/// so the first of them has to check the type.
pub fn custom(rules: impl IntoIterator<Item = Rule>) -> VineCustom {
    VineCustom(Schema::literal(
        None,
        Describes::Rules,
        rules.into_iter().collect(),
    ))
}

/// `vine.object(properties)`.
#[derive(Debug, Clone)]
pub struct VineObject(Schema);
common_methods!(VineObject);

/// `vine.object(properties)`: an object with these properties, validated
/// in this order. The other properties are left out of the output. See
/// [`object!`](crate::object!) for properties of different types.
pub fn object<K, S>(properties: impl IntoIterator<Item = (K, S)>) -> VineObject
where
    K: Into<String>,
    S: Into<Schema>,
{
    let mut list: Vec<(String, Schema)> = Vec::new();
    for (name, schema) in properties {
        let (name, schema) = (name.into(), schema.into());
        // As in an object literal: a name written twice keeps its first
        // place and its last schema.
        match list.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => entry.1 = schema,
            None => list.push((name, schema)),
        }
    }
    // And as in any JavaScript object, the names that are array indexes
    // come first: this is the order Vine validates the properties in.
    let mut indexed = Map::new();
    for (position, (name, _)) in list.iter().enumerate() {
        indexed.insert(name.clone(), json!(position));
    }
    let order: Vec<usize> = js::own_keys(&indexed)
        .into_iter()
        .filter_map(|name| indexed.get(name).and_then(Value::as_u64))
        .filter_map(|position| usize::try_from(position).ok())
        .collect();
    let mut slots: Vec<Option<(String, Schema)>> = list.into_iter().map(Some).collect();
    let properties = order
        .into_iter()
        .filter_map(|position| slots.get_mut(position)?.take())
        .collect();
    VineObject(Schema::new(Kind::Object {
        properties,
        allow_unknown: false,
    }))
}

impl VineObject {
    /// `.allowUnknownProperties()`: copy the properties the schema does not
    /// name to the output, as they are.
    #[must_use]
    pub fn allow_unknown_properties(mut self) -> Self {
        if let Kind::Object { allow_unknown, .. } = &mut self.0.kind {
            *allow_unknown = true;
        }
        self
    }
}

/// The length of the array a rule runs on.
fn length_rule(check: impl Fn(usize, &mut FieldContext<'_>) + Send + Sync + 'static) -> Rule {
    Rule::new(move |value, field| {
        if let Value::Array(items) = value {
            check(items.len(), field);
        }
    })
}

/// `vine.array(schema)`.
#[derive(Debug, Clone)]
pub struct VineArray(Schema);
common_methods!(VineArray);

/// `vine.array(schema)`: an array whose items each pass `schema`. The rules
/// of the array run before its items are looked at.
pub fn array(schema: impl Into<Schema>) -> VineArray {
    VineArray(Schema::new(Kind::Array(Box::new(schema.into()))))
}

impl VineArray {
    /// `.minLength(min)`.
    #[must_use]
    pub fn min_length(self, min: usize) -> Self {
        let rule = length_rule(move |length, field| {
            if length < min {
                field.report_with(
                    default_message("array.minLength"),
                    "array.minLength",
                    json!({ "min": min }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "minItems", json!(min))))
    }

    /// `.maxLength(max)`.
    #[must_use]
    pub fn max_length(self, max: usize) -> Self {
        let rule = length_rule(move |length, field| {
            if length > max {
                field.report_with(
                    default_message("array.maxLength"),
                    "array.maxLength",
                    json!({ "max": max }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| set(schema, "maxItems", json!(max))))
    }

    /// `.fixedLength(size)`.
    #[must_use]
    pub fn fixed_length(self, size: usize) -> Self {
        let rule = length_rule(move |length, field| {
            if length != size {
                field.report_with(
                    default_message("array.fixedLength"),
                    "array.fixedLength",
                    json!({ "size": size }),
                );
            }
        });
        self.use_rule(rule.json_schema(move |schema| {
            set(schema, "minItems", json!(size));
            set(schema, "maxItems", json!(size));
        }))
    }
}

/// `vine.record(schema)`.
#[derive(Debug, Clone)]
pub struct VineRecord(Schema);
common_methods!(VineRecord);

/// `vine.record(schema)`: an object with any keys, whose values each pass
/// `schema`.
pub fn record(schema: impl Into<Schema>) -> VineRecord {
    VineRecord(Schema::new(Kind::Record(Box::new(schema.into()))))
}

/// The properties of an object, as a list: `"name" => schema` for a
/// property and `..expression` for the properties another list holds, the
/// way `...period()` spreads them in TypeScript.
///
/// ```
/// use mymcps_vine as vine;
///
/// fn pagination() -> Vec<(String, vine::Schema)> {
///     vine::properties! {
///         "page" => vine::number().optional(),
///         "per_page" => vine::number().optional(),
///     }
/// }
///
/// let schema = vine::object! { "id" => vine::number(), ..pagination() };
/// # let _ = schema;
/// ```
#[macro_export]
macro_rules! properties {
    (@list $list:ident;) => {};
    (@list $list:ident; ..$spread:expr $(, $($rest:tt)*)?) => {
        $list.extend($spread);
        $crate::properties!(@list $list; $($($rest)*)?);
    };
    (@list $list:ident; $name:expr => $schema:expr $(, $($rest:tt)*)?) => {
        $list.extend(::std::iter::once((
            ::std::string::String::from($name),
            ::std::convert::Into::<$crate::Schema>::into($schema),
        )));
        $crate::properties!(@list $list; $($($rest)*)?);
    };
    ($($body:tt)*) => {{
        #[allow(unused_mut)]
        let mut list: ::std::vec::Vec<(::std::string::String, $crate::Schema)> =
            ::std::vec::Vec::new();
        $crate::properties!(@list list; $($body)*);
        list
    }};
}

/// `vine.object({ ... })` for properties of different types. See
/// [`properties!`](crate::properties!) for what goes between the braces.
#[macro_export]
macro_rules! object {
    ($($body:tt)*) => {
        $crate::object($crate::properties!($($body)*))
    };
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::validator::Vine;

    fn period() -> Vec<(String, Schema)> {
        crate::properties! {
            "start_date" => string().optional(),
            "end_date" => string().optional(),
        }
    }

    #[test]
    fn spreads_properties_as_an_object_literal_does() {
        let schema = crate::object! {
            "customer_id" => number(),
            ..period(),
            "limit" => number().optional(),
            // Written twice: the first place, the last schema.
            "start_date" => number(),
        };
        let described = schema.to_json_schema();
        let names: Vec<&String> = described["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(names, ["customer_id", "start_date", "end_date", "limit"]);
        assert_eq!(
            described["properties"]["start_date"],
            json!({ "type": "number" })
        );
        assert_eq!(described["required"], json!(["customer_id", "start_date"]));
    }

    #[test]
    fn requires_a_field_when_a_callback_says_so() {
        let validator = Vine::new().create(crate::object! {
            "kind" => string(),
            "url" => string().optional().required_when_fn(|field| {
                field.parent_get("kind").and_then(Value::as_str) == Some("http")
            }),
        });
        assert!(validator.validate(&json!({ "kind": "npm" })).is_ok());
        let error = validator.validate(&json!({ "kind": "http" })).unwrap_err();
        assert_eq!(error.messages[0].message, "The url field must be defined");
        assert_eq!(error.messages[0].rule, "required");
    }

    #[test]
    fn takes_a_pattern_from_the_regex_crate_or_from_a_function() {
        let hex = Regex::new("^[0-9a-f]+$").unwrap();
        let validator = Vine::new().create(string().regex(hex));
        assert!(validator.validate(&json!("c0ffee")).is_ok());
        assert_eq!(
            validator.validate(&json!("coffee")).unwrap_err().messages[0].message,
            "The data field format is invalid"
        );
        assert_eq!(
            validator.to_json_schema(),
            &json!({ "type": "string", "pattern": "^[0-9a-f]+$" })
        );

        let even = Pattern::from_fn("^(..)*$", |text| text.len() % 2 == 0);
        let validator = Vine::new().create(string().regex(even));
        assert!(validator.validate(&json!("ab")).is_ok());
        assert!(validator.validate(&json!("abc")).is_err());
    }
}
