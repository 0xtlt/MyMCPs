//! What a rule, a `parse` callback or a `transform` callback is told about
//! the field it runs on: Vine's `FieldContext`.

use std::any::Any;
use std::borrow::Cow;

use serde_json::{Map, Value};

use crate::error::FieldError;
use crate::messages::{FieldRef, MessagesProvider};

/// What a field is called in its parent.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Name<'a> {
    Root,
    Key(&'a str),
    Index(usize),
}

/// Where a field stands in the data.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Location<'a> {
    pub(crate) name: Name<'a>,
    pub(crate) wildcard_path: &'a str,
    pub(crate) parent: Option<&'a Location<'a>>,
    pub(crate) is_array_member: bool,
}

impl Location<'_> {
    pub(crate) fn name(&self) -> Cow<'_, str> {
        match self.name {
            Name::Root => Cow::Borrowed(""),
            Name::Key(key) => Cow::Borrowed(key),
            Name::Index(index) => Cow::Owned(index.to_string()),
        }
    }

    pub(crate) fn index(&self) -> Option<usize> {
        match self.name {
            Name::Index(index) => Some(index),
            _ => None,
        }
    }

    /// `getFieldPath()`: the names from the root down, joined by dots. The
    /// root has an empty path and its fields are not prefixed by it.
    pub(crate) fn path(&self) -> String {
        match self.parent {
            None => String::new(),
            Some(parent) if parent.parent.is_none() => self.name().into_owned(),
            Some(parent) => format!("{}.{}", parent.path(), self.name()),
        }
    }
}

/// What stays the same for every field of one validation.
#[derive(Clone, Copy)]
pub(crate) struct Env<'a> {
    pub(crate) messages: &'a dyn MessagesProvider,
    pub(crate) meta: Option<&'a dyn Any>,
    pub(crate) data: Option<&'a Value>,
    pub(crate) convert_empty_strings_to_null: bool,
}

/// A check a validator leaves to its caller. See [`crate::Rule::deferred`].
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCheck {
    /// The rule name the check fails under, such as `database.unique`.
    pub rule: String,
    /// The path of the field to check.
    pub field: String,
    /// The value of the field, as the rules before the check left it.
    pub value: Value,
    pub(crate) message: String,
    pub(crate) name: String,
    pub(crate) wildcard_path: String,
    pub(crate) index: Option<usize>,
    /// How many errors were reported before the check was reached.
    pub(crate) position: usize,
}

/// Where errors and pending checks are collected.
#[derive(Debug, Default)]
pub(crate) struct Sink {
    pub(crate) errors: Vec<FieldError>,
    pub(crate) checks: Vec<PendingCheck>,
}

/// `null` and `undefined` are both "not defined" for Vine, and a few rules
/// still tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Undefined,
    Null,
    Defined,
}

impl Presence {
    pub(crate) fn of(value: Option<&Value>) -> Self {
        match value {
            None => Self::Undefined,
            Some(Value::Null) => Self::Null,
            Some(_) => Self::Defined,
        }
    }
}

fn property<'a>(container: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match container? {
        Value::Object(object) => object.get(key),
        Value::Array(items) => {
            let index: usize = key.parse().ok()?;
            (index.to_string() == key)
                .then(|| items.get(index))
                .flatten()
        }
        _ => None,
    }
}

/// `dlv(data, 'a.b.c')`: stops at the first value that is not truthy.
fn delve<'a>(data: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    let mut current = data;
    for key in path.split('.') {
        let truthy = match current? {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
            Value::String(text) => !text.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        };
        current = if truthy { property(current, key) } else { None };
    }
    current
}

/// The field a rule runs on.
///
/// The value itself is the first argument of the rule. Everything else Vine
/// puts on `field` is here: where the field is, what surrounds it, and the
/// two things a rule does, [`report`](Self::report) and
/// [`mutate`](Self::mutate).
pub struct FieldContext<'a> {
    pub(crate) sink: &'a mut Sink,
    pub(crate) env: Env<'a>,
    pub(crate) location: &'a Location<'a>,
    pub(crate) parent: Option<&'a Value>,
    pub(crate) is_valid: bool,
    pub(crate) presence: Presence,
    pub(crate) mutation: Option<Value>,
}

impl<'a> FieldContext<'a> {
    /// `field.name`: the key of the field in its parent, the index of an
    /// item in its array, or the empty string for the root.
    pub fn name(&self) -> Cow<'_, str> {
        self.location.name()
    }

    /// `field.wildCardPath`: the path with `*` for every index and record key.
    pub fn wildcard_path(&self) -> &'a str {
        self.location.wildcard_path
    }

    /// `field.getFieldPath()`: the path from the root, such as `tools.0.name`.
    pub fn path(&self) -> String {
        self.location.path()
    }

    /// `field.isArrayMember`.
    pub fn is_array_member(&self) -> bool {
        self.location.is_array_member
    }

    /// `field.isValid`: no rule has refused the value so far.
    pub fn is_valid(&self) -> bool {
        self.is_valid
    }

    /// `field.isDefined`: the value is neither `null` nor `undefined`.
    pub fn is_defined(&self) -> bool {
        self.presence == Presence::Defined
    }

    /// The value is `null`. Only an implicit rule can see one.
    pub fn is_null(&self) -> bool {
        self.presence == Presence::Null
    }

    /// The value is `undefined`: it was left out. Only an implicit rule can
    /// see one, and it receives `Value::Null` for it.
    pub fn is_undefined(&self) -> bool {
        self.presence == Presence::Undefined
    }

    /// `field.parent`: the object or array the field is in, as it was
    /// received (after its own `parse`), not as it is validated. For the
    /// root it is the data itself. `None` stands for `undefined`.
    pub fn parent(&self) -> Option<&'a Value> {
        self.parent
    }

    /// `field.parent[key]`. `None` stands for `undefined`.
    pub fn parent_get(&self, key: &str) -> Option<&'a Value> {
        property(self.parent, key)
    }

    /// `field.data`: everything being validated, as it was received.
    pub fn data(&self) -> Option<&'a Value> {
        self.env.data
    }

    /// `helpers.getNestedValue(key, field)`: the sibling called `key`, or,
    /// when `key` has dots, the value at that path from the root.
    pub fn nested_value(&self, key: &str) -> Option<&'a Value> {
        if key.contains('.') {
            delve(self.env.data, key)
        } else {
            self.parent_get(key)
        }
    }

    /// `field.meta`: what the caller passed to `validate_with`, when it is a
    /// `T`.
    pub fn meta<T: Any>(&self) -> Option<&'a T> {
        self.env.meta.and_then(|meta| meta.downcast_ref::<T>())
    }

    /// `field.report(message, rule, field)`: refuse the value. The message
    /// goes through the messages provider, which fills `{{ field }}` in.
    pub fn report(&mut self, message: &str, rule: &str) {
        self.report_args(message, rule, None);
    }

    /// `field.report(message, rule, field, args)`: refuse the value, with
    /// arguments for the placeholders of the message. `args` is a JSON
    /// object, such as `json!({ "min": 1, "max": 9 })`; it ends up in the
    /// `meta` of the error.
    pub fn report_with(&mut self, message: &str, rule: &str, args: Value) {
        let args = match args {
            Value::Object(args) => Some(args),
            _ => None,
        };
        self.report_args(message, rule, args);
    }

    fn report_args(&mut self, message: &str, rule: &str, args: Option<Map<String, Value>>) {
        self.is_valid = false;
        let path = self.location.path();
        let name = self.location.name();
        let field = FieldRef {
            name: &name,
            path: &path,
            wildcard_path: self.location.wildcard_path,
            is_array_member: self.location.is_array_member,
        };
        let message = self
            .env
            .messages
            .get_message(message, rule, field, args.as_ref());
        self.sink.errors.push(FieldError {
            message,
            rule: rule.to_owned(),
            field: path,
            index: self.location.index(),
            meta: args,
        });
    }

    /// Report an error on the sibling `other`, the way `confirmed` blames
    /// the confirmation field. This field stays valid, as it does in Vine.
    pub(crate) fn report_on_sibling(
        &mut self,
        other: &str,
        message: &str,
        rule: &str,
        args: Map<String, Value>,
    ) {
        let name = self.location.name();
        let path = self.location.path().replacen(name.as_ref(), other, 1);
        let wildcard_path = self
            .location
            .wildcard_path
            .replacen(name.as_ref(), other, 1);
        let field = FieldRef {
            name: other,
            path: &path,
            wildcard_path: &wildcard_path,
            is_array_member: self.location.is_array_member,
        };
        let message = self
            .env
            .messages
            .get_message(message, rule, field, Some(&args));
        self.sink.errors.push(FieldError {
            message,
            rule: rule.to_owned(),
            field: path,
            index: None,
            meta: Some(args),
        });
    }

    /// `field.mutate(value, field)`: replace the value for the rules that
    /// follow and for the output.
    pub fn mutate(&mut self, value: impl Into<Value>) {
        self.mutation = Some(value.into());
    }
}

/// The second argument of a `parse` callback: `{ data, meta, parent }`.
#[derive(Clone, Copy)]
pub struct ParseContext<'a> {
    pub(crate) data: Option<&'a Value>,
    pub(crate) parent: Option<&'a Value>,
    pub(crate) meta: Option<&'a dyn Any>,
}

impl<'a> ParseContext<'a> {
    /// `context.data`: everything being validated, as it was received.
    pub fn data(&self) -> Option<&'a Value> {
        self.data
    }

    /// `context.parent`: the object or array the field is in.
    pub fn parent(&self) -> Option<&'a Value> {
        self.parent
    }

    /// `context.parent[key]`. `None` stands for `undefined`.
    pub fn parent_get(&self, key: &str) -> Option<&'a Value> {
        property(self.parent, key)
    }

    /// `context.meta`, when it is a `T`.
    pub fn meta<T: Any>(&self) -> Option<&'a T> {
        self.meta.and_then(|meta| meta.downcast_ref::<T>())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn builds_paths_without_a_prefix_for_the_root() {
        let root = Location {
            name: Name::Root,
            wildcard_path: "",
            parent: None,
            is_array_member: false,
        };
        let tools = Location {
            name: Name::Key("tools"),
            parent: Some(&root),
            ..root
        };
        let item = Location {
            name: Name::Index(2),
            parent: Some(&tools),
            ..root
        };
        let name = Location {
            name: Name::Key("name"),
            parent: Some(&item),
            ..root
        };
        assert_eq!(root.path(), "");
        assert_eq!(tools.path(), "tools");
        assert_eq!(item.path(), "tools.2");
        assert_eq!(name.path(), "tools.2.name");
    }

    #[test]
    fn reaches_into_the_data_by_a_dotted_path() {
        let data = json!({ "a": { "b": [10, { "c": "x" }] }, "zero": 0 });
        assert_eq!(delve(Some(&data), "a.b.1.c"), Some(&json!("x")));
        assert_eq!(delve(Some(&data), "a.b.0"), Some(&json!(10)));
        assert_eq!(delve(Some(&data), "zero.x"), None);
        assert_eq!(delve(Some(&data), "a.missing.c"), None);
    }
}
