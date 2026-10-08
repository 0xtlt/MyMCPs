//! Validation rules: what `vine.createRule` makes.

use std::fmt;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::field::FieldContext;

pub(crate) type RuleFn = dyn Fn(&Value, &mut FieldContext<'_>) + Send + Sync;
pub(crate) type JsonSchemaFn = dyn Fn(&mut Map<String, Value>) + Send + Sync;

#[derive(Clone)]
pub(crate) enum RuleKind {
    Sync(Arc<RuleFn>),
    Deferred { rule: Arc<str>, message: Arc<str> },
}

/// A validation rule, ready to be added to a schema with `.use_rule(rule)`.
///
/// In TypeScript a rule is made in two steps: `vine.createRule(validator)`
/// returns a function, and calling it with options returns the rule. Here a
/// function that takes the options and returns `vine::rule(move |value,
/// field| ...)` does both, the closure capturing its options:
///
/// ```
/// use mymcps_vine as vine;
/// use serde_json::json;
///
/// // const textRule = toolVine.createRule<{ max: number }>((value, { max }, field) => { ... },
/// //   { toJSONSchema: (schema, { max }) => Object.assign(schema, { type: 'string', maxLength: max }) })
/// fn text_rule(max: usize) -> vine::Rule {
///     vine::rule(move |value, field| {
///         if !value.as_str().is_some_and(|text| vine::js::utf16_len(text) <= max) {
///             field.report_with(
///                 "{{ field }} must be text of at most {{ max }} characters",
///                 "text",
///                 json!({ "max": max }),
///             );
///         }
///     })
///     .json_schema(move |schema| {
///         schema.insert("type".to_owned(), json!("string"));
///         schema.insert("maxLength".to_owned(), json!(max));
///     })
/// }
/// # let _ = text_rule(10);
/// ```
///
/// A rule runs when the value is defined (neither `null` nor left out),
/// unless it is [`implicit`](Self::implicit). When a field has several
/// rules, they run in the order they were added and stop at the first that
/// reports, unless the schema turned `bail` off.
#[derive(Clone)]
pub struct Rule {
    pub(crate) kind: RuleKind,
    pub(crate) implicit: bool,
    pub(crate) json_schema: Option<Arc<JsonSchemaFn>>,
}

impl fmt::Debug for Rule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.kind {
            RuleKind::Sync(_) => "sync",
            RuleKind::Deferred { .. } => "deferred",
        };
        formatter
            .debug_struct("Rule")
            .field("kind", &kind)
            .field("implicit", &self.implicit)
            .field("json_schema", &self.json_schema.is_some())
            .finish()
    }
}

impl Rule {
    /// `vine.createRule(validator)`. The validator receives the value and
    /// the field. For a value that was left out, which only an implicit rule
    /// can see, it receives `Value::Null` and `field.is_undefined()` is true.
    pub fn new(validator: impl Fn(&Value, &mut FieldContext<'_>) + Send + Sync + 'static) -> Self {
        Self {
            kind: RuleKind::Sync(Arc::new(validator)),
            implicit: false,
            json_schema: None,
        }
    }

    /// `{ implicit: true }`: run the rule also when the value is `null` or
    /// left out, as a rule that decides whether a field is required must.
    #[must_use]
    pub fn implicit(mut self) -> Self {
        self.implicit = true;
        self
    }

    /// `{ toJSONSchema }`: how the rule reads in the JSON Schema of its
    /// field. The callback edits the schema of the field in place.
    #[must_use]
    pub fn json_schema(
        mut self,
        describe: impl Fn(&mut Map<String, Value>) + Send + Sync + 'static,
    ) -> Self {
        self.json_schema = Some(Arc::new(describe));
        self
    }

    /// A check the validator cannot make itself because it needs to wait for
    /// something, such as a query: the rules Lucid adds to Vine (`unique`,
    /// `exists`) are of that kind. The rule holds the place of the check
    /// among the rules of its field; the caller makes the check and says how
    /// it went:
    ///
    /// ```
    /// use mymcps_vine as vine;
    /// use serde_json::json;
    ///
    /// let validator = vine::global().create(vine::object! {
    ///     "email" => vine::string().email().use_rule(vine::lucid::unique()),
    ///     "password" => vine::string().min_length(8),
    /// });
    ///
    /// let mut run = validator.start(&json!({ "email": "ada@example.com", "password": "short" }));
    /// for check in run.take_checks() {
    ///     // Here: `SELECT 1 FROM users WHERE email = ?` with `check.value`.
    ///     let taken = check.field == "email";
    ///     if taken {
    ///         run.reject(&check);
    ///     }
    /// }
    /// let error = run.finish().unwrap_err();
    /// // In the order Vine reports them: the email first, then the password.
    /// assert_eq!(error.messages[0].message, "The email has already been taken");
    /// assert_eq!(error.messages[1].rule, "minLength");
    /// ```
    ///
    /// The check is only pending when the rules before it accepted the
    /// value. Add it last: the rules after it have already run by the time
    /// the caller answers.
    ///
    /// [`Validator::validate`](crate::Validator::validate) cannot wait for an
    /// answer, so it counts every pending check as failed. A validator with a
    /// deferred rule is run with [`Validator::start`](crate::Validator::start).
    pub fn deferred(rule: &str, message: &str) -> Self {
        Self {
            kind: RuleKind::Deferred {
                rule: Arc::from(rule),
                message: Arc::from(message),
            },
            implicit: false,
            json_schema: None,
        }
    }
}

/// `vine.createRule(validator)`. See [`Rule`].
pub fn rule(validator: impl Fn(&Value, &mut FieldContext<'_>) + Send + Sync + 'static) -> Rule {
    Rule::new(validator)
}
