//! What a form shows after it was refused: the message under each field and
//! what the person had typed. A page form (sign in, onboarding) is answered
//! with a redirect back to its page, so both travel in the session as flash
//! data, as the Node app's `flashValidationErrors` did.

use mymcps_vine::ValidationError;
use serde_json::{Map, Value};

use crate::session::Session;

const ERRORS_KEY: &str = "errors";
const OLD_KEY: &str = "old";

/// The state a form is drawn in.
#[derive(Debug, Clone, Default)]
pub struct FormState {
    /// The first message of each field that was refused, in field order.
    errors: Vec<(String, String)>,
    /// What was submitted, without anything that holds a password.
    old: Map<String, Value>,
}

/// The refusal inside a validator's error. The other kind, an output the
/// handler's own type cannot read, means validator and type disagree: a bug.
pub fn refusal(error: mymcps_vine::Error) -> Result<ValidationError, crate::error::AppError> {
    match error {
        mymcps_vine::Error::Validation(error) => Ok(error),
        mymcps_vine::Error::Output(error) => Err(crate::error::AppError::internal(error)),
    }
}

/// Passwords are never sent back to the browser, nor kept in its session.
fn holds_a_password(field: &str) -> bool {
    field.to_ascii_lowercase().contains("password")
}

impl FormState {
    pub fn new(error: &ValidationError, input: &Map<String, Value>) -> Self {
        let mut errors: Vec<(String, String)> = Vec::new();
        for message in &error.messages {
            if !errors.iter().any(|(field, _)| *field == message.field) {
                errors.push((message.field.clone(), message.message.clone()));
            }
        }
        let old = input
            .iter()
            .filter(|(field, _)| !holds_a_password(field))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self { errors, old }
    }

    /// A refusal that is not about one field, such as wrong credentials.
    pub fn with_error(field: &str, message: &str, input: &Map<String, Value>) -> Self {
        Self::new(&ValidationError::single(field, "invalid", message), input)
    }

    /// Keep the state for the page the browser is sent back to.
    pub fn flash(&self, session: &Session) {
        let errors: Map<String, Value> = self
            .errors
            .iter()
            .map(|(field, message)| (field.clone(), Value::String(message.clone())))
            .collect();
        session.flash(ERRORS_KEY, errors);
        session.flash(OLD_KEY, &self.old);
    }

    /// The state the previous request flashed, or a blank form.
    pub fn from_session(session: &Session) -> Self {
        let errors = match session.flashed(ERRORS_KEY) {
            Some(Value::Object(errors)) => errors
                .into_iter()
                .filter_map(|(field, message)| Some((field, message.as_str()?.to_string())))
                .collect(),
            _ => Vec::new(),
        };
        let old = match session.flashed(OLD_KEY) {
            Some(Value::Object(old)) => old,
            _ => Map::new(),
        };
        Self { errors, old }
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, message)| message.as_str())
    }

    /// The first message of the form, for its banner.
    pub fn first_error(&self) -> Option<&str> {
        self.errors.first().map(|(_, message)| message.as_str())
    }

    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// What was typed in a field, to put back in it.
    pub fn old(&self, field: &str) -> String {
        match self.old.get(field) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            Some(Value::Bool(flag)) => flag.to_string(),
            _ => String::new(),
        }
    }

    pub fn old_value(&self, field: &str) -> Option<&Value> {
        self.old.get(field)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_errors_and_input_for_the_next_page_but_never_a_password() {
        let mut error = ValidationError::single(
            "email",
            "email",
            "The email field must be a valid email address",
        );
        error.push(mymcps_vine::FieldError::new(
            "email",
            "maxLength",
            "second message of the same field",
        ));
        error.push(mymcps_vine::FieldError::new(
            "password",
            "minLength",
            "The password field must have at least 8 characters",
        ));
        let input = json!({ "email": "nope", "fullName": "Ada", "password": "secret", "passwordConfirmation": "secret", "currentPassword": "old" });
        let state = FormState::new(&error, input.as_object().unwrap());

        let session = Session::default();
        state.flash(&session);
        let next = Session::from_values(session.snapshot());
        let shown = FormState::from_session(&next);

        assert!(shown.has_errors());
        assert_eq!(shown.error_count(), 2);
        assert_eq!(
            shown.error("email"),
            Some("The email field must be a valid email address")
        );
        assert_eq!(
            shown.first_error(),
            Some("The email field must be a valid email address")
        );
        assert_eq!(shown.error("fullName"), None);
        assert_eq!(shown.old("email"), "nope");
        assert_eq!(shown.old("fullName"), "Ada");
        for secret in ["password", "passwordConfirmation", "currentPassword"] {
            assert_eq!(shown.old(secret), "");
        }
        assert!(
            !serde_json::to_string(&session.snapshot())
                .unwrap()
                .contains("secret")
        );

        assert!(!FormState::from_session(&Session::default()).has_errors());
    }
}
