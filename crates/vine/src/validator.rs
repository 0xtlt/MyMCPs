//! Running a schema over data: the `Vine` instances that make validators,
//! and the validators themselves.
//!
//! Vine compiles a schema to a JavaScript function. The steps below are
//! that function's, in its order: read the value, check it is there, check
//! its type, run its rules, then write it out or go through its children.

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, LazyLock, OnceLock};

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::date;
use crate::error::{Error, FieldError, ValidationError};
use crate::field::{Env, FieldContext, Location, Name, ParseContext, PendingCheck, Presence, Sink};
use crate::helpers;
use crate::js;
use crate::messages::{FieldRef, MessagesProvider, SimpleMessagesProvider, default_message};
use crate::rule::{Rule, RuleKind};
use crate::schema::{DataType, Kind, Parser, Schema, TransformFn};

static NULL: Value = Value::Null;

/// A Vine instance: the settings its validators are created with. The
/// schema types do not belong to an instance, only `create` does.
///
/// The TypeScript app has three kinds of them:
///
/// - `vine`, the default export, which `start/validator.ts` configures to
///   read empty strings as `null`: [`global()`];
/// - `new Vine()`, which leaves values as they are: [`Vine::new`];
/// - `toolVine`, a `new Vine()` with its own messages provider:
///   `Vine::new().messages_provider(...)`.
#[derive(Clone)]
pub struct Vine {
    convert_empty_strings_to_null: bool,
    messages_provider: Arc<dyn MessagesProvider>,
}

impl fmt::Debug for Vine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Vine")
            .field(
                "convert_empty_strings_to_null",
                &self.convert_empty_strings_to_null,
            )
            .finish_non_exhaustive()
    }
}

impl Default for Vine {
    fn default() -> Self {
        Self::new()
    }
}

impl Vine {
    /// `new Vine()`: values are validated as they are sent, and errors read
    /// as Vine's default messages.
    pub fn new() -> Self {
        Self {
            convert_empty_strings_to_null: false,
            messages_provider: Arc::new(SimpleMessagesProvider::defaults()),
        }
    }

    /// `vine.convertEmptyStringsToNull = state`: read a string that is
    /// empty, or made of whitespace only, as `null`. HTML forms send an
    /// empty string for a field left blank.
    #[must_use]
    pub fn convert_empty_strings_to_null(mut self, state: bool) -> Self {
        self.convert_empty_strings_to_null = state;
        self
    }

    /// `vine.messagesProvider = provider`.
    #[must_use]
    pub fn messages_provider(mut self, provider: impl MessagesProvider + 'static) -> Self {
        self.messages_provider = Arc::new(provider);
        self
    }

    /// `vine.create(schema)` and `vine.create({ ...properties })`: a
    /// validator for this schema, with the settings the instance has now.
    /// `vine.withMetaData<T>().create(schema)` is the same call: what the
    /// metadata is only matters to the rules that read it.
    pub fn create(&self, schema: impl Into<Schema>) -> Validator {
        let schema = schema.into();
        Validator {
            root: Node::compile(&schema, ""),
            schema,
            convert_empty_strings_to_null: self.convert_empty_strings_to_null,
            messages_provider: Arc::clone(&self.messages_provider),
            json_schema: OnceLock::new(),
        }
    }
}

/// The app's default instance, as `start/validator.ts` configures it: a
/// string that is empty or made of whitespace only is read as `null`.
pub fn global() -> &'static Vine {
    static GLOBAL: LazyLock<Vine> =
        LazyLock::new(|| Vine::new().convert_empty_strings_to_null(true));
    &GLOBAL
}

/// The data given to a validator. `None` stands for `undefined`, which is
/// what a header that was not sent or a property that is missing is in
/// TypeScript.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a>(Option<&'a Value>);

impl Input<'_> {
    /// `undefined`.
    pub fn undefined() -> Self {
        Self(None)
    }
}

impl<'a> From<&'a Value> for Input<'a> {
    fn from(value: &'a Value) -> Self {
        Self(Some(value))
    }
}

impl<'a> From<Option<&'a Value>> for Input<'a> {
    fn from(value: Option<&'a Value>) -> Self {
        Self(value)
    }
}

impl<'a> From<&'a Option<Value>> for Input<'a> {
    fn from(value: &'a Option<Value>) -> Self {
        Self(value.as_ref())
    }
}

/// A schema placed in its tree, ready to run.
struct Node {
    wildcard_path: String,
    bail: bool,
    allow_null: bool,
    is_optional: bool,
    parse: Option<Parser>,
    validations: Vec<Rule>,
    transform: Option<Arc<TransformFn>>,
    kind: NodeKind,
}

enum NodeKind {
    Literal(Option<DataType>),
    Object {
        properties: Vec<(String, Node)>,
        allow_unknown: bool,
    },
    Array(Box<Node>),
    Record(Box<Node>),
}

impl Node {
    fn compile(schema: &Schema, wildcard_path: &str) -> Self {
        let under = |name: &str| {
            if wildcard_path.is_empty() {
                name.to_owned()
            } else {
                format!("{wildcard_path}.{name}")
            }
        };
        let kind = match &schema.kind {
            Kind::Literal { data_type, .. } => NodeKind::Literal(*data_type),
            Kind::Object {
                properties,
                allow_unknown,
            } => NodeKind::Object {
                properties: properties
                    .iter()
                    .map(|(name, property)| (name.clone(), Self::compile(property, &under(name))))
                    .collect(),
                allow_unknown: *allow_unknown,
            },
            Kind::Array(each) => NodeKind::Array(Box::new(Self::compile(each, &under("*")))),
            Kind::Record(each) => NodeKind::Record(Box::new(Self::compile(each, &under("*")))),
        };
        Self {
            wildcard_path: wildcard_path.to_owned(),
            bail: schema.bail,
            allow_null: schema.allow_null,
            is_optional: schema.is_optional,
            parse: schema.parse.clone(),
            validations: schema.validations.clone(),
            transform: schema.transform.clone(),
            kind,
        }
    }
}

/// `defineValue`: with the conversion on, a string that trims to nothing is
/// `null`. This also applies to what a rule mutates the value to.
fn define(
    value: Option<Cow<'_, Value>>,
    convert_empty_strings_to_null: bool,
) -> Option<Cow<'_, Value>> {
    match value.as_deref() {
        Some(Value::String(text)) if convert_empty_strings_to_null && js::trim(text).is_empty() => {
            Some(Cow::Owned(Value::Null))
        }
        _ => value,
    }
}

/// The check of a literal's own type. It is asked even when the value is
/// missing, and answers whether the rules may run.
fn check_data_type<'v>(
    data_type: DataType,
    value: &mut Option<Cow<'v, Value>>,
    field: &mut FieldContext<'_>,
) -> bool {
    if !field.is_defined() {
        return false;
    }
    let Some(current) = value.as_deref() else {
        return false;
    };
    match data_type {
        DataType::String => {
            if current.is_string() {
                return true;
            }
            field.report(default_message("string"), "string");
            false
        }
        DataType::Number { strict } => {
            let number = if strict {
                js::as_f64(current)
            } else {
                Some(helpers::as_number(current))
            };
            match number.filter(|number| number.is_finite()) {
                Some(number) => {
                    *value = Some(Cow::Owned(js::number(number)));
                    true
                }
                None => {
                    field.report(default_message("number"), "number");
                    false
                }
            }
        }
        DataType::Date => match date::parse(current) {
            Some(time) => {
                *value = Some(Cow::Owned(Value::String(date::to_iso_string(time))));
                true
            }
            None => {
                field.report(default_message("date"), "date");
                false
            }
        },
    }
}

/// Validate one node. Returns what is written to the output for it, or
/// `None` when nothing is: the field was left out, or is not valid.
fn exec(
    node: &Node,
    raw: Option<&Value>,
    location: &Location<'_>,
    parent: Option<&Value>,
    env: Env<'_>,
    sink: &mut Sink,
) -> Option<Value> {
    let convert = env.convert_empty_strings_to_null;
    let parsed = match &node.parse {
        Some(parser) => {
            let context = ParseContext {
                data: env.data,
                parent,
                meta: env.meta,
            };
            parser.call(raw.cloned(), &context).map(Cow::Owned)
        }
        None => raw.map(Cow::Borrowed),
    };
    let mut value = define(parsed, convert);
    let mut field = FieldContext {
        sink,
        env,
        location,
        parent,
        is_valid: true,
        presence: Presence::of(value.as_deref()),
        mutation: None,
    };

    // A field that is not optional must be there; one that is nullable may be `null`.
    if !node.is_optional {
        let missing = if node.allow_null {
            field.is_undefined()
        } else {
            !field.is_defined()
        };
        if missing {
            field.report(default_message("required"), "required");
        }
    }

    let (has_type, valid_type) = match &node.kind {
        NodeKind::Literal(None) => (false, false),
        NodeKind::Literal(Some(data_type)) => {
            (true, check_data_type(*data_type, &mut value, &mut field))
        }
        NodeKind::Object { .. } | NodeKind::Record(_) => {
            let valid = field.is_defined() && {
                let is_object = value.as_deref().is_some_and(Value::is_object);
                if !is_object {
                    field.report(default_message("object"), "object");
                }
                is_object
            };
            (true, valid)
        }
        NodeKind::Array(_) => {
            let valid = field.is_defined() && {
                let is_array = value.as_deref().is_some_and(Value::is_array);
                if !is_array {
                    field.report(default_message("array"), "array");
                }
                is_array
            };
            (true, valid)
        }
    };

    for rule in &node.validations {
        if node.bail && !field.is_valid {
            break;
        }
        // A rule that is not implicit only sees a value of the right type,
        // or, for a type without a check of its own, a value that is there.
        let exists = if has_type {
            valid_type
        } else {
            field.is_defined()
        };
        if !rule.implicit && !exists {
            continue;
        }
        match &rule.kind {
            RuleKind::Sync(run) => {
                run(value.as_deref().unwrap_or(&NULL), &mut field);
                if let Some(mutated) = field.mutation.take() {
                    value = define(Some(Cow::Owned(mutated)), convert);
                    field.presence = Presence::of(value.as_deref());
                }
            }
            RuleKind::Deferred { rule, message } => {
                if field.is_valid {
                    let check = PendingCheck {
                        rule: rule.to_string(),
                        field: location.path(),
                        value: value.as_deref().cloned().unwrap_or(Value::Null),
                        message: message.to_string(),
                        name: location.name().into_owned(),
                        wildcard_path: location.wildcard_path.to_owned(),
                        index: location.index(),
                        position: field.sink.errors.len(),
                    };
                    field.sink.checks.push(check);
                }
            }
        }
    }

    let is_null = field.is_null();
    let null_output = (node.allow_null && is_null).then_some(Value::Null);
    match &node.kind {
        NodeKind::Literal(_) => {
            let output = if field.is_defined() && field.is_valid {
                // An object kept as it is still lists its keys as JavaScript does.
                value.map(|value| match value.into_owned() {
                    compound @ (Value::Array(_) | Value::Object(_)) => {
                        js::order_keys_deep(compound)
                    }
                    plain => plain,
                })
            } else {
                null_output
            }?;
            Some(match &node.transform {
                Some(transform) => transform(output, &field),
                None => output,
            })
        }
        NodeKind::Object {
            properties,
            allow_unknown,
        } => {
            if !valid_type {
                return null_output;
            }
            if node.bail && !field.is_valid {
                return None;
            }
            let container = value.as_deref();
            let Some(Value::Object(object)) = container else {
                return None;
            };
            let mut output = Map::new();
            for (name, property) in properties {
                let location = Location {
                    name: Name::Key(name),
                    wildcard_path: &property.wildcard_path,
                    parent: Some(location),
                    is_array_member: false,
                };
                let written = exec(
                    property,
                    object.get(name),
                    &location,
                    container,
                    env,
                    field.sink,
                );
                if let Some(written) = written {
                    output.insert(name.clone(), written);
                }
            }
            if *allow_unknown {
                for key in js::own_keys(object) {
                    if !properties.iter().any(|(name, _)| name == key)
                        && let Some(unknown) = object.get(key)
                    {
                        output.insert(key.clone(), js::order_keys_deep(unknown.clone()));
                    }
                }
            }
            Some(Value::Object(js::order_keys(output)))
        }
        NodeKind::Array(each) => {
            if !valid_type {
                return null_output;
            }
            if node.bail && !field.is_valid {
                return None;
            }
            let container = value.as_deref();
            let Some(Value::Array(items)) = container else {
                return None;
            };
            let mut output = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let location = Location {
                    name: Name::Index(index),
                    wildcard_path: &each.wildcard_path,
                    parent: Some(location),
                    is_array_member: true,
                };
                if let Some(written) = exec(each, Some(item), &location, container, env, field.sink)
                {
                    // An item that is not written leaves a hole, which
                    // reads `null` once there is an item after it.
                    output.resize(index, Value::Null);
                    output.push(written);
                }
            }
            Some(Value::Array(output))
        }
        NodeKind::Record(each) => {
            if !valid_type {
                return null_output;
            }
            if node.bail && !field.is_valid {
                return None;
            }
            let container = value.as_deref();
            let Some(Value::Object(object)) = container else {
                return None;
            };
            let mut output = Map::new();
            for key in js::own_keys(object) {
                let location = Location {
                    name: Name::Key(key),
                    wildcard_path: &each.wildcard_path,
                    parent: Some(location),
                    is_array_member: false,
                };
                if let Some(written) =
                    exec(each, object.get(key), &location, container, env, field.sink)
                {
                    output.insert(key.clone(), written);
                }
            }
            Some(Value::Object(output))
        }
    }
}

/// `VineValidator`: a schema compiled once, to validate any number of
/// values from any number of threads.
///
/// ```
/// use std::sync::LazyLock;
///
/// use mymcps_vine as vine;
/// use serde_json::json;
///
/// static LOGIN: LazyLock<vine::Validator> = LazyLock::new(|| {
///     vine::global().create(vine::object! {
///         "email" => vine::string().email().max_length(254),
///         "password" => vine::string().min_length(1),
///     })
/// });
///
/// let error = LOGIN.validate(&json!({ "email": "ada@example.com" })).unwrap_err();
/// assert_eq!(error.messages[0].message, "The password field must be defined");
/// ```
pub struct Validator {
    schema: Schema,
    root: Node,
    convert_empty_strings_to_null: bool,
    messages_provider: Arc<dyn MessagesProvider>,
    json_schema: OnceLock<Value>,
}

impl fmt::Debug for Validator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Validator")
            .field("schema", &self.schema)
            .field(
                "convert_empty_strings_to_null",
                &self.convert_empty_strings_to_null,
            )
            .finish_non_exhaustive()
    }
}

impl Validator {
    /// `validator.messagesProvider = provider`: the messages of this
    /// validator, instead of those of the instance that created it.
    #[must_use]
    pub fn messages_provider(mut self, provider: impl MessagesProvider + 'static) -> Self {
        self.messages_provider = Arc::new(provider);
        self
    }

    fn run(&self, data: Option<&Value>, meta: Option<&dyn Any>) -> (Option<Value>, Sink) {
        let env = Env {
            messages: self.messages_provider.as_ref(),
            meta,
            data,
            convert_empty_strings_to_null: self.convert_empty_strings_to_null,
        };
        let root = Location {
            name: Name::Root,
            wildcard_path: "",
            parent: None,
            is_array_member: false,
        };
        let mut sink = Sink::default();
        // The parent of the root is the data itself.
        let output = exec(&self.root, data, &root, data, env, &mut sink);
        (output, sink)
    }

    fn begin<'v>(&'v self, data: Option<&Value>, meta: Option<&dyn Any>) -> Run<'v> {
        let (output, sink) = self.run(data, meta);
        Run {
            validator: self,
            output,
            errors: sink.errors,
            checks: sink.checks,
            rejected: Vec::new(),
        }
    }

    /// `validator.validate(data)` and `validator.tryValidate(data)`: the
    /// validated output, or what is wrong with the data. `Err(error)` is
    /// the `[error, null]` of `tryValidate` and `Ok(output)` its
    /// `[null, output]`.
    ///
    /// The output is what Vine returns: the properties the schema names, in
    /// the order it names them, with every value as its rules left it. An
    /// output of `undefined`, which an optional root gives for missing
    /// data, is `Value::Null` here; [`validate_opt`](Self::validate_opt)
    /// tells the two apart.
    pub fn validate<'a>(&self, data: impl Into<Input<'a>>) -> Result<Value, ValidationError> {
        self.begin(data.into().0, None).finish()
    }

    /// `validator.validate(data, { meta })`: the same, with metadata for
    /// the rules that depend on who or what the data is validated for. A
    /// rule reads it back with `field.meta::<T>()`, where `T` is the type
    /// `meta` points to: pass `&context`, not `&&context`.
    pub fn validate_with<'a>(
        &self,
        data: impl Into<Input<'a>>,
        meta: &dyn Any,
    ) -> Result<Value, ValidationError> {
        self.begin(data.into().0, Some(meta)).finish()
    }

    /// Like [`validate`](Self::validate), with `None` for an output of
    /// `undefined`.
    pub fn validate_opt<'a>(
        &self,
        data: impl Into<Input<'a>>,
    ) -> Result<Option<Value>, ValidationError> {
        self.begin(data.into().0, None).finish_opt()
    }

    /// Like [`validate_with`](Self::validate_with), with `None` for an
    /// output of `undefined`.
    pub fn validate_opt_with<'a>(
        &self,
        data: impl Into<Input<'a>>,
        meta: &dyn Any,
    ) -> Result<Option<Value>, ValidationError> {
        self.begin(data.into().0, Some(meta)).finish_opt()
    }

    /// `validator.tryValidate(data)`. The same as
    /// [`validate`](Self::validate): a `Result` is what the pair
    /// `[error, output]` is in Rust.
    pub fn try_validate<'a>(&self, data: impl Into<Input<'a>>) -> Result<Value, ValidationError> {
        self.validate(data)
    }

    /// `validator.tryValidate(data, { meta })`. The same as
    /// [`validate_with`](Self::validate_with).
    pub fn try_validate_with<'a>(
        &self,
        data: impl Into<Input<'a>>,
        meta: &dyn Any,
    ) -> Result<Value, ValidationError> {
        self.validate_with(data, meta)
    }

    /// Validate, then read the output as a `T`: the `Infer<typeof
    /// validator>` of TypeScript, written by hand as a `Deserialize` type.
    pub fn validate_as<'a, T: DeserializeOwned>(
        &self,
        data: impl Into<Input<'a>>,
    ) -> Result<T, Error> {
        Ok(serde_json::from_value(self.validate(data)?)?)
    }

    /// [`validate_as`](Self::validate_as) with metadata.
    pub fn validate_as_with<'a, T: DeserializeOwned>(
        &self,
        data: impl Into<Input<'a>>,
        meta: &dyn Any,
    ) -> Result<T, Error> {
        Ok(serde_json::from_value(self.validate_with(data, meta)?)?)
    }

    /// Validate up to the checks the schema leaves to its caller. See
    /// [`Rule::deferred`].
    pub fn start<'a>(&self, data: impl Into<Input<'a>>) -> Run<'_> {
        self.begin(data.into().0, None)
    }

    /// [`start`](Self::start) with metadata.
    pub fn start_with<'a>(&self, data: impl Into<Input<'a>>, meta: &dyn Any) -> Run<'_> {
        self.begin(data.into().0, Some(meta))
    }

    /// `validator.toJSONSchema()`: what the validator accepts, as a JSON
    /// Schema. Each type and each rule says its part; a custom rule says
    /// its own with [`Rule::json_schema`].
    pub fn to_json_schema(&self) -> &Value {
        self.json_schema
            .get_or_init(|| self.schema.to_json_schema())
    }
}

/// A validation that waits for its caller to make the checks the schema
/// leaves to it. See [`Rule::deferred`].
#[derive(Debug)]
pub struct Run<'v> {
    validator: &'v Validator,
    output: Option<Value>,
    errors: Vec<FieldError>,
    checks: Vec<PendingCheck>,
    rejected: Vec<(usize, FieldError)>,
}

impl Run<'_> {
    /// The checks left to make. Each belongs to a field whose other rules
    /// passed.
    pub fn checks(&self) -> &[PendingCheck] {
        &self.checks
    }

    /// Take the checks left to make. The caller then answers for them: the
    /// ones it does not [`reject`](Self::reject) have passed.
    pub fn take_checks(&mut self) -> Vec<PendingCheck> {
        std::mem::take(&mut self.checks)
    }

    /// The check failed: report its error, at the place the rule has among
    /// the others.
    pub fn reject(&mut self, check: &PendingCheck) {
        let field = FieldRef {
            name: &check.name,
            path: &check.field,
            wildcard_path: &check.wildcard_path,
            is_array_member: check.index.is_some(),
        };
        let message =
            self.validator
                .messages_provider
                .get_message(&check.message, &check.rule, field, None);
        let error = FieldError {
            message,
            rule: check.rule.clone(),
            field: check.field.clone(),
            index: check.index,
            meta: None,
        };
        self.rejected.push((check.position, error));
    }

    /// The validated output, or everything that is wrong with the data. A
    /// check that was not taken has nobody to answer for it, and counts as
    /// failed.
    pub fn finish(self) -> Result<Value, ValidationError> {
        self.finish_opt()
            .map(|output| output.unwrap_or(Value::Null))
    }

    /// [`finish`](Self::finish), with `None` for an output of `undefined`.
    pub fn finish_opt(mut self) -> Result<Option<Value>, ValidationError> {
        for check in self.take_checks() {
            self.reject(&check);
        }
        if self.errors.is_empty() && self.rejected.is_empty() {
            return Ok(self.output);
        }
        self.rejected.sort_by_key(|(position, _)| *position);
        let mut rejected = self.rejected.into_iter().peekable();
        let mut messages = Vec::with_capacity(self.errors.len() + rejected.len());
        for (position, error) in self.errors.into_iter().enumerate() {
            while let Some((_, failed)) = rejected.next_if(|(at, _)| *at <= position) {
                messages.push(failed);
            }
            messages.push(error);
        }
        messages.extend(rejected.map(|(_, failed)| failed));
        Err(ValidationError::new(messages))
    }

    /// [`finish`](Self::finish), then read the output as a `T`.
    pub fn finish_as<T: DeserializeOwned>(self) -> Result<T, Error> {
        Ok(serde_json::from_value(self.finish()?)?)
    }
}

/// Lucid's rules for Vine. They query the database, so they are
/// [deferred](Rule::deferred): the validator holds their place and the
/// caller runs the query.
pub mod lucid {
    use crate::rule::Rule;

    /// `.unique({ table, column })`: fails as `database.unique`, with
    /// Lucid's message.
    pub fn unique() -> Rule {
        Rule::deferred("database.unique", "The {{ field }} has already been taken")
    }

    /// `.exists({ table, column })`: fails as `database.exists`, with
    /// Lucid's message.
    pub fn exists() -> Rule {
        Rule::deferred("database.exists", "The selected {{ field }} is invalid")
    }
}

/// What the body parser does before validation, with
/// `convertEmptyStringsToNull: true` in `config/bodyparser.ts`: every
/// string of a JSON or form body that is exactly empty becomes `null`, at
/// any depth.
///
/// This is not Vine's conversion, which also counts whitespace as empty and
/// applies to the fields a schema names: a request body goes through both.
/// Like the parser's JSON reviver, this leaves alone a body that is itself
/// a string and a value under an empty key.
pub fn empty_strings_to_null(body: &mut Value) {
    fn walk(value: &mut Value, is_root: bool) {
        match value {
            Value::String(text) if text.is_empty() && !is_root => *value = Value::Null,
            Value::Array(items) => items.iter_mut().for_each(|item| walk(item, false)),
            Value::Object(object) => {
                for (key, property) in object.iter_mut() {
                    // The reviver returns the value of an empty key as it is.
                    let kept = key.is_empty() && property.is_string();
                    if !kept {
                        walk(property, false);
                    }
                }
            }
            _ => {}
        }
    }
    walk(body, true);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::{array, date_iso8601, number, object, string};

    #[test]
    fn tells_undefined_from_null_at_the_root() {
        let optional = Vine::new().create(string().optional());
        assert_eq!(optional.validate_opt(Input::undefined()).unwrap(), None);
        assert_eq!(optional.validate_opt(&Value::Null).unwrap(), None);
        assert_eq!(optional.validate(None::<&Value>).unwrap(), Value::Null);

        let nullable = Vine::new().create(string().nullable());
        assert_eq!(
            nullable.validate_opt(&Value::Null).unwrap(),
            Some(Value::Null)
        );
        assert_eq!(
            nullable.validate_opt(&None).unwrap_err().messages[0].rule,
            "required"
        );
        assert_eq!(nullable.validate(&Some(json!("x"))).unwrap(), json!("x"));
    }

    #[test]
    fn reads_the_output_as_a_type() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Page {
            page: u32,
            ids: Vec<i64>,
        }

        let validator = global().create(object([
            ("page", Schema::from(number().without_decimals().positive())),
            ("ids", array(number()).into()),
        ]));
        let page: Page = validator
            .validate_as(&json!({ "page": "2", "ids": ["-1", 3.0] }))
            .unwrap();
        assert_eq!(
            page,
            Page {
                page: 2,
                ids: vec![-1, 3]
            }
        );

        let refused = validator
            .validate_as::<Page>(&json!({ "page": 0, "ids": [] }))
            .unwrap_err();
        let error = refused.into_validation().unwrap();
        assert_eq!(error.messages[0].message, "The page field must be positive");

        // The data passes and the type asked for does not match the schema.
        let mismatch = validator.validate_as::<Vec<String>>(&json!({ "page": 1, "ids": [] }));
        assert!(matches!(mismatch, Err(Error::Output(_))));
    }

    #[test]
    fn places_a_rejected_check_where_its_rule_stands() {
        let validator = Vine::new().create(object([
            ("first", Schema::from(string())),
            ("email", string().use_rule(lucid::unique()).into()),
            ("ids", array(number().use_rule(lucid::exists())).into()),
            ("last", string().into()),
        ]));
        let data = json!({ "first": 1, "email": "a@b.co", "ids": [1, "x", 3], "last": 2 });

        let mut run = validator.start(&data);
        let fields: Vec<&str> = run
            .checks()
            .iter()
            .map(|check| check.field.as_str())
            .collect();
        assert_eq!(fields, ["email", "ids.0", "ids.2"]);
        let checks = run.take_checks();
        assert_eq!(checks[2].value, json!(3));
        run.reject(&checks[2]);
        run.reject(&checks[0]);
        let error = run.finish().unwrap_err();
        let reported: Vec<(&str, &str)> = error
            .messages
            .iter()
            .map(|one| (one.field.as_str(), one.rule.as_str()))
            .collect();
        assert_eq!(
            reported,
            [
                ("first", "string"),
                ("email", "database.unique"),
                ("ids.1", "number"),
                ("ids.2", "database.exists"),
                ("last", "string"),
            ]
        );
        assert_eq!(error.messages[3].message, "The selected 2 is invalid");
        assert_eq!(error.messages[3].index, Some(2));

        // Taken and not rejected: the checks passed.
        let mut run =
            validator.start(&json!({ "first": "a", "email": "a@b.co", "ids": [], "last": "z" }));
        assert_eq!(run.take_checks().len(), 1);
        assert_eq!(run.finish().unwrap()["email"], "a@b.co");
    }

    #[test]
    fn refuses_the_strings_only_v8_reads_as_dates() {
        let validator = Vine::new().create(date_iso8601());
        // Day.js parses with the format "iso8601" first, of which `s` is the
        // only token: `i<seconds>o8601` is today at that second for Vine.
        // V8 then guesses at whatever is not ISO 8601.
        for text in [
            "i5o8601",
            "5",
            "October 1, 2026",
            "Thu, 01 Oct 2026 12:00:00 GMT",
            "2026-10-01 12:00:00Z",
        ] {
            let error = validator.validate(&json!(text)).unwrap_err();
            assert_eq!(
                error.messages[0].message, "The data field must be a datetime value",
                "{text}"
            );
        }
        assert_eq!(
            validator.validate(&json!("2026-10-01T12:00:00Z")).unwrap(),
            "2026-10-01T12:00:00.000Z"
        );
    }

    #[test]
    fn nulls_empty_strings_as_the_body_parser_does() {
        let mut body = json!({ "name": "", "note": " ", "list": ["", { "deep": "" }], "": "" });
        empty_strings_to_null(&mut body);
        assert_eq!(
            body,
            json!({ "name": null, "note": " ", "list": [null, { "deep": null }], "": "" })
        );
    }
}
