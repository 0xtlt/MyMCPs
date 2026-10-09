//! How a tool gets its arguments checked: the port of
//! `app/services/builtin/tool_input.ts`.
//!
//! In TypeScript `builtinTool({ ... })` wraps `run` and `describe` so that
//! they only ever see arguments that passed the tool's validator. Here
//! [`BuiltinTool::new`](crate::BuiltinTool::new) does the wrapping, for any
//! [`ToolInput`]; this module makes a validator of `mymcps-vine` one, so
//! that a tool is declared as it was:
//!
//! ```
//! use std::sync::{Arc, LazyLock};
//!
//! use mymcps_builtin::arguments::{TOOL_VINE, integer, trimmed_text};
//! use mymcps_builtin::{BuiltinTool, ToolInput};
//! use mymcps_vine as vine;
//! use serde::Deserialize;
//! use serde_json::json;
//!
//! struct Context {
//!     greeting: &'static str,
//! }
//!
//! static GREET_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
//!     TOOL_VINE.create(vine::object! {
//!         "name" => trimmed_text(20),
//!         "times" => integer(1..=3).optional(),
//!     })
//! });
//!
//! #[derive(Deserialize)]
//! struct Greet {
//!     name: String,
//!     times: Option<usize>,
//! }
//!
//! // builtinTool({
//! //   name: 'greet',
//! //   description: 'Greets.',
//! //   inputSchema: { type: 'object', properties: { name: { type: 'string' } } },
//! //   input: greetValidator,
//! //   run: async ({ name, times = 1 }, context) =>
//! //     Array.from({ length: times }, () => `${context.greeting} ${name}`),
//! // })
//! let tool = BuiltinTool::new(
//!     "greet",
//!     "Greets.",
//!     json!({ "type": "object", "properties": { "name": { "type": "string" } } }),
//!     &GREET_VALIDATOR,
//!     |input: Greet, context: Arc<Context>| async move {
//!         let greeting = format!("{} {}", context.greeting, input.name);
//!         Ok(json!(vec![greeting; input.times.unwrap_or(1)]))
//!     },
//! );
//!
//! let arguments = json!({ "name": "  Ada ", "times": "2", "other": 1 });
//! let context = Arc::new(Context { greeting: "Hi" });
//! let greetings = futures::executor::block_on(tool.run(arguments.as_object().cloned().unwrap(), context));
//! assert_eq!(greetings.unwrap(), json!(["Hi Ada", "Hi Ada"]));
//! assert_eq!(tool.input.json_schema()["required"], json!(["name"]));
//! ```
//!
//! The context of the call is the metadata of the validation, as it is in
//! `toolInput(validator, args, context)`: a rule reads it with
//! `field.meta::<Context>()`.

use std::any::Any;
use std::sync::{Arc, LazyLock};

use mymcps_vine::{Input, ValidationError, Validator};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::definition::ToolInput;
use crate::error::{BuiltinError, BuiltinResult};

/// The sentence the agent reads: what is wrong with the first argument that
/// is, in the order the schema lists them.
fn first_message(error: ValidationError) -> String {
    error
        .messages
        .into_iter()
        .next()
        .map(|first| first.message)
        .unwrap_or_default()
}

/// A validator of `mymcps-vine` as the `input` of a tool. The context the
/// tool runs with is the metadata of the validation.
impl<C: Any + Send + Sync> ToolInput<C> for Validator {
    fn validate(&self, arguments: Value, context: &C) -> Result<Value, String> {
        self.validate_with(&arguments, context)
            .map_err(first_message)
    }

    fn json_schema(&self) -> Value {
        self.to_json_schema().clone()
    }
}

/// A validator kept in a `static`, which is where the validators of a
/// provider live: `&LIST_ACTIVITIES_VALIDATOR` is the `input` of a tool.
impl<C: Any + Send + Sync> ToolInput<C> for LazyLock<Validator> {
    fn validate(&self, arguments: Value, context: &C) -> Result<Value, String> {
        ToolInput::<C>::validate(&**self, arguments, context)
    }

    fn json_schema(&self) -> Value {
        ToolInput::<C>::json_schema(&**self)
    }
}

impl<C, T: ToolInput<C> + ?Sized> ToolInput<C> for &'static T {
    fn validate(&self, arguments: Value, context: &C) -> Result<Value, String> {
        (**self).validate(arguments, context)
    }

    fn json_schema(&self) -> Value {
        (**self).json_schema()
    }
}

impl<C, T: ToolInput<C> + ?Sized> ToolInput<C> for Arc<T> {
    fn validate(&self, arguments: Value, context: &C) -> Result<Value, String> {
        (**self).validate(arguments, context)
    }

    fn json_schema(&self) -> Value {
        (**self).json_schema()
    }
}

/// The validated input as the type the caller reads it as. A validator
/// that lets through what that type cannot read disagrees with it: a bug,
/// not a mistake of the agent.
fn read<T: DeserializeOwned>(validated: Result<Value, ValidationError>) -> BuiltinResult<T> {
    let input = validated.map_err(|error| BuiltinError::Tool(first_message(error)))?;
    serde_json::from_value(input).map_err(|error| {
        BuiltinError::internal(format!(
            "a validator accepted input its reader cannot read: {error}"
        ))
    })
}

/// `toolInput(validator, args)`: check what an agent passed to a tool, or
/// what a tool put in a link it handed out. It is told about one argument
/// at a time: the first that is wrong, in the order the schema lists them,
/// as a [`BuiltinError::Tool`].
///
/// `arguments` is a `&Value`, or an `Option<&Value>` where `None` stands
/// for `undefined`. The input comes back as `T`, the type that stands for
/// `Infer<typeof validator>`; ask for a `Value` to get it as it is.
pub fn tool_input<'a, T: DeserializeOwned>(
    validator: &Validator,
    arguments: impl Into<Input<'a>>,
) -> BuiltinResult<T> {
    read(validator.validate(arguments))
}

/// `toolInput(validator, args, context)`: the same, where `context` is the
/// validation's metadata, for the rules that depend on the account the tool
/// runs for. They read it with `field.meta::<C>()`, `C` being the type
/// `context` points to.
pub fn tool_input_with<'a, T: DeserializeOwned, C: Any>(
    validator: &Validator,
    arguments: impl Into<Input<'a>>,
    context: &C,
) -> BuiltinResult<T> {
    read(validator.validate_with(arguments, context))
}
