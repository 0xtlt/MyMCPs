//! What a failed validation returns: Vine's `ValidationError`, with the
//! messages its `SimpleErrorReporter` collects.

use serde::Serialize;
use serde::ser::SerializeMap;
use serde_json::{Map, Value};

/// One entry of `error.messages`.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldError {
    /// The sentence to show.
    pub message: String,
    /// The name of the rule that refused the value.
    pub rule: String,
    /// The path of the field from the root, such as `tools.0.name`. Empty
    /// for the root itself.
    pub field: String,
    /// The position of the field in its array, when it is the item of one.
    pub index: Option<usize>,
    /// What the rule reported along with its message, such as `{ "min": 8 }`.
    pub meta: Option<Map<String, Value>>,
}

impl FieldError {
    pub fn new(
        field: impl Into<String>,
        rule: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            rule: rule.into(),
            field: field.into(),
            index: None,
            meta: None,
        }
    }

    /// The item of an array that is the root of the data has its index for a
    /// path, and Vine writes that path as a number.
    fn field_is_index(&self) -> bool {
        self.index
            .is_some_and(|index| index.to_string() == self.field)
    }
}

/// Written as Vine writes it: `message`, `rule`, `field`, then `meta` and
/// `index` when there are any.
impl Serialize for FieldError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("message", &self.message)?;
        map.serialize_entry("rule", &self.rule)?;
        match self.index {
            Some(index) if self.field_is_index() => map.serialize_entry("field", &index)?,
            _ => map.serialize_entry("field", &self.field)?,
        }
        if let Some(meta) = &self.meta {
            map.serialize_entry("meta", meta)?;
        }
        if let Some(index) = self.index {
            map.serialize_entry("index", &index)?;
        }
        map.end()
    }
}

/// `errors.E_VALIDATION_ERROR`: the data did not pass its validator.
///
/// `messages` lists what is wrong in the order the schema lists its fields,
/// one entry per field unless the field turned `bail` off.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("Validation failure")]
pub struct ValidationError {
    pub messages: Vec<FieldError>,
}

impl ValidationError {
    /// The HTTP status of a validation failure.
    pub const STATUS: u16 = 422;
    /// The code of Vine's error.
    pub const CODE: &'static str = "E_VALIDATION_ERROR";

    /// `new errors.E_VALIDATION_ERROR(messages)`.
    pub fn new(messages: Vec<FieldError>) -> Self {
        Self { messages }
    }

    /// An error about one field, for a check made outside of a validator:
    /// `new errors.E_VALIDATION_ERROR([{ field, rule, message }])`.
    pub fn single(
        field: impl Into<String>,
        rule: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(vec![FieldError::new(field, rule, message)])
    }

    /// Add one more message, after the others.
    pub fn push(&mut self, error: FieldError) {
        self.messages.push(error);
    }

    /// Add the messages of another error, after the others.
    pub fn merge(&mut self, other: ValidationError) {
        self.messages.extend(other.messages);
    }

    /// `error.messages[0]`: the first thing that is wrong.
    pub fn first(&self) -> Option<&FieldError> {
        self.messages.first()
    }

    /// The messages about one field, by its path.
    pub fn for_field<'a>(&'a self, field: &'a str) -> impl Iterator<Item = &'a FieldError> {
        self.messages
            .iter()
            .filter(move |error| error.field == field)
    }

    /// Whether any message is about this field.
    pub fn has_field(&self, field: &str) -> bool {
        self.for_field(field).next().is_some()
    }

    /// `error.messages` as JSON.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(&self.messages).unwrap_or(Value::Null)
    }
}

/// What [`Validator::validate_as`](crate::Validator::validate_as) returns
/// when it has no value to give.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The data did not pass the validator.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The data passed, and the validated output does not fit the Rust type
    /// asked for: the type and the schema disagree, which is a bug.
    #[error("validated output does not fit the requested type: {0}")]
    Output(#[from] serde_json::Error),
}

impl Error {
    /// The validation error, when that is what this is.
    pub fn into_validation(self) -> Option<ValidationError> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Output(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serializes_messages_as_vine_does() {
        let mut item = FieldError::new("0", "string", "The 0 field must be a string");
        item.index = Some(0);
        let mut nested = FieldError::new("ids.1", "min", "too small");
        nested.index = Some(1);
        nested.meta = json!({ "min": 1 }).as_object().cloned();
        let error = ValidationError::new(vec![item, nested]);
        assert_eq!(
            serde_json::to_string(&error.messages).unwrap(),
            r#"[{"message":"The 0 field must be a string","rule":"string","field":0,"index":0},{"message":"too small","rule":"min","field":"ids.1","meta":{"min":1},"index":1}]"#
        );
    }

    #[test]
    fn collects_errors_found_outside_of_a_validator() {
        let mut error = ValidationError::single("npmEnv.0.name", "npmEnvironment", "Reserved");
        error.merge(ValidationError::single("email", "database.unique", "Taken"));
        assert_eq!(error.messages.len(), 2);
        assert!(error.has_field("email"));
        assert_eq!(
            error.first().map(|first| first.rule.as_str()),
            Some("npmEnvironment")
        );
        assert_eq!(error.to_string(), "Validation failure");
    }
}
