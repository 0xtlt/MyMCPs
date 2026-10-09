//! Three validator files of the app, ported with the library to show what a
//! port looks like: `app/validators/user.ts` (HTML forms),
//! `app/validators/gateway.ts` (arguments of the lazy gateway's own tools)
//! and the rules of `app/validators/builtin_tools.ts` (arguments of the
//! tools of built-in MCPs). The real ports belong to the crates that own
//! those features; these stay here, with the cases of the TypeScript tests
//! that cover them.

use mymcps_vine as vine;
use serde_json::{Value, json};

/// `app/validators/user.ts`
mod user {
    use std::sync::LazyLock;

    use mymcps_vine as vine;
    use vine::{Rule, Validator, VineString};

    /// Stands for `isValidFiveFieldCron` of `#services/mcp_auto_update_cron`.
    fn is_valid_five_field_cron(expression: &str) -> bool {
        expression.split(' ').count() == 5
    }

    fn email() -> VineString {
        vine::string().email().max_length(254)
    }

    fn password() -> VineString {
        vine::string().min_length(8).max_length(32)
    }

    fn five_field_cron() -> Rule {
        vine::rule(|value, field| {
            let Some(expression) = value.as_str() else {
                return;
            };
            if !is_valid_five_field_cron(expression) {
                field.report(
                    "The {{ field }} field must be a valid 5-field cron expression",
                    "cron",
                );
            }
        })
    }

    /// Change the signed-in user's email address. The caller checks that no
    /// other user has the address: see `changes_an_email_address`.
    pub static UPDATE_EMAIL: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(vine::object! {
            "email" => email().use_rule(vine::lucid::unique()),
            "currentPassword" => vine::string().min_length(1),
        })
    });

    /// Change the signed-in user's password.
    pub static UPDATE_PASSWORD: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(vine::object! {
            "currentPassword" => vine::string().min_length(1),
            "newPassword" => password().confirmed("passwordConfirmation"),
            "passwordConfirmation" => vine::string(),
        })
    });

    pub static UPDATE_MCP_LOGGING: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(vine::object! {
            "gatewayToolMode" => vine::enum_(["eager", "lazy"]),
            "mcpLogLevel" => vine::enum_(["off", "metadata", "arguments", "responses"]),
            "mcpLogRetentionDays" => vine::number().without_decimals().min(1).max(365),
            // Switch submits "on" when checked; omitted when unchecked.
            "mcpAutoUpdateEnabled" => vine::boolean().optional(),
            "mcpAutoUpdateCron" => vine::string().trim().max_length(64).optional().use_rule(five_field_cron()),
        })
    });

    /// First-run onboarding: create the instance admin.
    pub static ONBOARDING: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(vine::object! {
            "fullName" => vine::string().trim().min_length(1).max_length(120),
            "email" => email().use_rule(vine::lucid::unique()),
            "password" => password().confirmed("passwordConfirmation"),
            "passwordConfirmation" => vine::string(),
        })
    });

    /// Login credentials.
    pub static LOGIN: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(vine::object! {
            "email" => email(),
            "password" => vine::string().min_length(1),
        })
    });
}

/// `app/validators/gateway.ts`
mod gateway {
    use std::collections::HashMap;
    use std::sync::LazyLock;

    use mymcps_vine as vine;
    use serde_json::{Map, Value, json};
    use vine::{FieldRef, MessagesProvider, Rule, Validator, VineString};

    /// An agent that gets an argument wrong reads one sentence saying what
    /// that argument must be, whichever rule refused it.
    fn argument_messages<const N: usize>(
        messages: [(&'static str, &'static str); N],
    ) -> impl MessagesProvider {
        let messages = HashMap::from(messages);
        move |default_message: &str,
              _rule: &str,
              field: FieldRef<'_>,
              _args: Option<&Map<String, Value>>| {
            messages
                .get(field.wildcard_path)
                .copied()
                .unwrap_or(default_message)
                .to_owned()
        }
    }

    /// Vine reads null as a value left out. In the arguments of a tool call
    /// it is a value the agent sent, and it is not a valid one.
    fn not_null() -> Rule {
        vine::rule(|_, field| {
            if field.is_null() {
                field.report("The {{ field }} field must not be null", "notNull");
            }
        })
        .implicit()
    }

    fn mcp_slug_argument() -> VineString {
        vine::string().trim().min_length(1).max_length(120)
    }

    /// The `X-MyMCPs-Tool-Mode` request header. Case and surrounding
    /// whitespace are ignored, and a blank header leaves the choice to the
    /// instance.
    pub static GATEWAY_TOOL_MODE: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(
            vine::enum_(["eager", "lazy"])
                .parse(|value, _| match value {
                    Some(Value::String(mode)) => {
                        let mode = vine::js::trim(&mode).to_lowercase();
                        (!mode.is_empty()).then_some(Value::String(mode))
                    }
                    other => other,
                })
                .optional(),
        )
    });

    /// `/^(?!__).+?__/s`: a separator with something before it. The regex
    /// crate has no lookahead, so the check is written out.
    fn has_slug_and_separator(name: &str) -> bool {
        let first = name.chars().next().map_or(0, char::len_utf8);
        !name.starts_with("__") && name[first..].contains("__")
    }

    /// A tool name of the eager gateway, `<slug>__<tool>`, split at its
    /// first separator.
    pub static NAMESPACED_TOOL: LazyLock<Validator> = LazyLock::new(|| {
        vine::global().create(
            vine::string()
                .regex(vine::Pattern::from_fn(
                    "^(?!__).+?__",
                    has_slug_and_separator,
                ))
                .transform(|name, _| {
                    let name = name.as_str().unwrap_or_default();
                    let separator = name.find("__").unwrap_or(0);
                    json!({ "slug": name[..separator], "toolName": name[separator + 2..] })
                }),
        )
    });

    /// Arguments of the lazy gateway's `tool_search`.
    pub static TOOL_SEARCH: LazyLock<Validator> = LazyLock::new(|| {
        vine::global()
            .create(vine::object! {
                "mcp" => mcp_slug_argument(),
                "query" => vine::string().trim().min_length(1).max_length(200),
                "limit" => vine::number()
                    .strict()
                    .parse(|value, _| Some(value.unwrap_or_else(|| json!(10))))
                    .without_decimals()
                    .range([1, 20]),
            })
            .messages_provider(argument_messages([
                (
                    "mcp",
                    "mcp must be a non-empty MCP slug of at most 120 characters",
                ),
                (
                    "query",
                    "query must be non-empty and at most 200 characters",
                ),
                ("limit", "limit must be an integer between 1 and 20"),
            ]))
    });

    /// Arguments of the lazy gateway's `call_tool`.
    pub static CALL_TOOL: LazyLock<Validator> = LazyLock::new(|| {
        vine::global()
            .create(vine::object! {
                "mcp" => mcp_slug_argument(),
                "tool" => vine::string().trim().min_length(1).max_length(128),
                "arguments" => vine::object! {}.use_rule(not_null()).optional(),
            })
            .messages_provider(argument_messages([
                (
                    "mcp",
                    "mcp must be a non-empty MCP slug of at most 120 characters",
                ),
                (
                    "tool",
                    "tool must be a non-empty upstream tool name of at most 128 characters",
                ),
                ("arguments", "arguments must be an object when provided"),
            ]))
    });
}

/// `app/validators/builtin_tools.ts`, without the two rules that read dates
/// with Luxon.
mod builtin_tools {
    use std::any::Any;
    use std::sync::LazyLock;

    use mymcps_vine as vine;
    use serde_json::{Map, Value, json};
    use vine::{
        FieldRef, JsRegex, MessagesProvider, Rule, SimpleMessagesProvider, Validator, Vine,
        VineBoolean, VineCustom, VineEnum,
    };

    /// Says what is wrong in one sentence the agent can act on, naming the
    /// argument. An item of a list is named after its list: `uids`, not `2`.
    struct ArgumentMessages(SimpleMessagesProvider);

    impl MessagesProvider for ArgumentMessages {
        fn get_message(
            &self,
            message: &str,
            rule: &str,
            field: FieldRef<'_>,
            args: Option<&Map<String, Value>>,
        ) -> String {
            let argument = field.wildcard_path.split('.').next().unwrap_or_default();
            let field = field.with_name(if argument.is_empty() {
                "arguments"
            } else {
                argument
            });
            match args
                .and_then(|args| args.get("choices"))
                .and_then(Value::as_array)
            {
                Some(choices) => {
                    let choices: Vec<String> = choices.iter().map(vine::js::to_string).collect();
                    let mut args = args.cloned().unwrap_or_default();
                    args.insert("choices".to_owned(), Value::String(choices.join(", ")));
                    self.0.get_message(message, rule, field, Some(&args))
                }
                None => self.0.get_message(message, rule, field, args),
            }
        }
    }

    /// Arguments are JSON written by an agent, not the fields of a form, so
    /// they get a Vine of their own: the one the pages use turns an empty
    /// string into null, and here an empty description is how a description
    /// gets cleared.
    pub static TOOL_VINE: LazyLock<Vine> = LazyLock::new(|| {
        // The rules of Vine's own types that the schemas use. The others below bring their sentence.
        Vine::new().messages_provider(ArgumentMessages(SimpleMessagesProvider::new([
            ("required", "{{ field }} is required"),
            ("object", "{{ field }} must be an object"),
            ("boolean", "{{ field }} must be true or false"),
            ("enum", "{{ field }} must be one of: {{ choices }}"),
        ])))
    });

    pub fn is_blank(value: Option<&Value>) -> bool {
        match value {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.is_empty(),
            Some(_) => false,
        }
    }

    /// `onlyWith` of `app/validators/builtin_icloud_mail.ts`: an argument
    /// that says more about `other`, and is not looked at without it.
    pub fn only_with(other: &'static str, schema: impl Into<vine::Schema>) -> vine::Schema {
        let schema = schema.into();
        let parse = schema.parser();
        schema.parse(move |value, context| {
            if is_blank(context.parent_get(other)) {
                return None;
            }
            match &parse {
                Some(parse) => parse.call(value, context),
                None => value,
            }
        })
    }

    /// An empty string is one of the ways agents leave an argument out.
    pub fn blank_as_missing(value: Option<Value>) -> Option<Value> {
        value.filter(|value| value.as_str() != Some(""))
    }

    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

    fn integer_rule(min: f64, max: Option<f64>) -> Rule {
        static QUOTED: LazyLock<JsRegex> =
            LazyLock::new(|| vine::js::regex(r"^-?\d+$", "").expect("static regex"));
        vine::rule(move |value, field| {
            // Agents often quote large identifiers.
            let parsed = match value {
                Value::String(text) if QUOTED.test(vine::js::trim(text)) => {
                    Some(vine::js::string_to_number(text))
                }
                other => vine::js::as_f64(other),
            };
            let in_range = |number: &f64| {
                vine::js::is_safe_integer(*number)
                    && *number >= min
                    && *number <= max.unwrap_or(MAX_SAFE_INTEGER)
            };
            let Some(number) = parsed.filter(in_range) else {
                match max {
                    None => field.report_with(
                        "{{ field }} must be an integer of at least {{ min }}",
                        "integer",
                        json!({ "min": vine::js::number(min) }),
                    ),
                    Some(max) => field.report_with(
                        "{{ field }} must be an integer between {{ min }} and {{ max }}",
                        "integer",
                        json!({ "min": vine::js::number(min), "max": vine::js::number(max) }),
                    ),
                }
                return;
            };
            field.mutate(vine::js::number(number));
        })
        .json_schema(move |schema| {
            schema.insert("type".to_owned(), json!("integer"));
            schema.insert("minimum".to_owned(), vine::js::number(min));
            if let Some(max) = max {
                schema.insert("maximum".to_owned(), vine::js::number(max));
            }
        })
    }

    /// A whole number, also when it is quoted.
    pub fn integer(min: impl Into<f64>, max: impl Into<Option<f64>>) -> VineCustom {
        vine::custom([integer_rule(min.into(), max.into())])
            .parse(|value, _| blank_as_missing(value))
    }

    fn number_rule(min: f64, max: f64) -> Rule {
        vine::rule(move |value, field| {
            let parsed = match value {
                Value::String(text) if !vine::js::trim(text).is_empty() => {
                    Some(vine::js::string_to_number(text))
                }
                other => vine::js::as_f64(other),
            };
            match parsed.filter(|number| number.is_finite() && *number >= min && *number <= max) {
                Some(number) => field.mutate(vine::js::number(number)),
                None => field.report_with(
                    "{{ field }} must be a number between {{ min }} and {{ max }}",
                    "number",
                    json!({ "min": vine::js::number(min), "max": vine::js::number(max) }),
                ),
            }
        })
        .json_schema(move |schema| {
            schema.insert("type".to_owned(), json!("number"));
            schema.insert("minimum".to_owned(), vine::js::number(min));
            schema.insert("maximum".to_owned(), vine::js::number(max));
        })
    }

    /// A number, also when it is quoted.
    pub fn number(min: impl Into<f64>, max: impl Into<f64>) -> VineCustom {
        vine::custom([number_rule(min.into(), max.into())])
            .parse(|value, _| blank_as_missing(value))
    }

    /// JSON booleans, and the same two words quoted. Vine's own conversion
    /// would also take 1 and "on".
    pub fn boolean() -> VineBoolean {
        vine::boolean()
            .strict()
            .parse(|value, _| match value.as_ref().and_then(Value::as_str) {
                Some("true") => Some(Value::Bool(true)),
                Some("false") => Some(Value::Bool(false)),
                _ => blank_as_missing(value),
            })
    }

    pub fn choice<const N: usize>(values: [&str; N]) -> VineEnum {
        vine::enum_(values).parse(|value, _| blank_as_missing(value))
    }

    fn text_rule(max: usize) -> Rule {
        vine::rule(move |value, field| {
            if !value
                .as_str()
                .is_some_and(|text| vine::js::utf16_len(text) <= max)
            {
                field.report_with(
                    "{{ field }} must be text of at most {{ max }} characters",
                    "text",
                    json!({ "max": max }),
                );
            }
        })
        .json_schema(move |schema| {
            schema.insert("type".to_owned(), json!("string"));
            schema.insert("maxLength".to_owned(), json!(max));
        })
    }

    /// Text as written: an empty one is a value.
    pub fn text(max: usize) -> VineCustom {
        vine::custom([text_rule(max)])
    }

    /// Text without the spaces around it. Left out when nothing remains.
    pub fn trimmed_text(max: usize) -> VineCustom {
        // Text that is too long is left for the rule to refuse.
        text(max).parse(move |value, _| match value {
            Some(Value::String(text)) if vine::js::utf16_len(&text) <= max => {
                let trimmed = vine::js::trim(&text);
                (!trimmed.is_empty()).then(|| Value::String(trimmed.to_owned()))
            }
            other => other,
        })
    }

    fn single_line_rule() -> Rule {
        vine::rule(|value, field| {
            // `/\p{Cc}/u`
            if value
                .as_str()
                .is_some_and(|text| text.chars().any(char::is_control))
            {
                field.report("{{ field }} must be a single line of text", "line");
            }
        })
    }

    /// One line of text: it ends up in a mail header or an IMAP command.
    pub fn line(max: usize) -> VineCustom {
        trimmed_text(max).use_rule(single_line_rule())
    }

    fn pattern_rule(expression: JsRegex, hint: &str) -> Rule {
        let hint = hint.to_owned();
        vine::rule(move |value, field| {
            let written = match value {
                Value::Number(_) => Some(vine::js::to_string(value)),
                Value::String(text) => Some(text.clone()),
                _ => None,
            };
            match written.map(|written| vine::js::trim(&written).to_owned()) {
                Some(written) if expression.test(&written) => field.mutate(written),
                _ => field.report_with(
                    "{{ field }} must be {{ hint }}",
                    "pattern",
                    json!({ "hint": hint }),
                ),
            }
        })
        .json_schema(|schema| {
            schema.insert("type".to_owned(), json!("string"));
        })
    }

    /// Identifiers end up in request paths, so only an exact pattern match is accepted.
    pub fn pattern(expression: &str, hint: &str) -> VineCustom {
        let expression = vine::js::regex(expression, "").expect("static regex");
        vine::custom([pattern_rule(expression, hint)]).parse(|value, _| blank_as_missing(value))
    }

    /// How many items a list may have. `sentence` is what the agent reads
    /// when it has fewer or more, since each list names its items its own way.
    pub fn list_length(min: Option<usize>, max: usize, sentence: &str) -> Rule {
        let sentence = sentence.to_owned();
        vine::rule(move |value, field| {
            let length = value.as_array().map_or(0, Vec::len);
            if length < min.unwrap_or(0) || length > max {
                field.report(&sentence, "listLength");
            }
        })
        .json_schema(move |schema| {
            if let Some(min) = min {
                schema.insert("minItems".to_owned(), json!(min));
            }
            schema.insert("maxItems".to_owned(), json!(max));
        })
    }

    /// For the tools that take no arguments: whatever they are passed is ignored.
    pub static NO_ARGUMENTS: LazyLock<Validator> =
        LazyLock::new(|| TOOL_VINE.create(vine::object! {}));

    /// `BuiltinToolError` of `#services/builtin/definition`.
    #[derive(Debug, PartialEq)]
    pub struct BuiltinToolError(pub String);

    /// `toolInput` of `#services/builtin/tool_input`: check what an agent
    /// passed to a tool. It is told about one argument at a time: the first
    /// that is wrong, in the order the schema lists them. `context` is the
    /// validation's metadata, for the rules that depend on the account the
    /// tool runs for.
    pub fn tool_input(
        validator: &Validator,
        args: Option<&Value>,
        context: &dyn Any,
    ) -> Result<Value, BuiltinToolError> {
        validator.validate_with(args, context).map_err(|error| {
            BuiltinToolError(
                error
                    .messages
                    .first()
                    .map(|first| first.message.clone())
                    .unwrap_or_default(),
            )
        })
    }
}

// ---------------------------------------------------------------------------
// user.ts
// ---------------------------------------------------------------------------

fn messages(error: &vine::ValidationError) -> Vec<(&str, &str, &str)> {
    error
        .messages
        .iter()
        .map(|one| (one.field.as_str(), one.rule.as_str(), one.message.as_str()))
        .collect()
}

#[test]
fn signs_in_with_an_email_and_a_password() {
    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Login {
        email: String,
        password: String,
    }

    let login: Login = user::LOGIN
        .validate_as(
            &json!({ "email": "ada@example.com", "password": " secret ", "remember": "on" }),
        )
        .unwrap();
    assert_eq!(
        login,
        Login {
            email: "ada@example.com".to_owned(),
            password: " secret ".to_owned()
        }
    );

    // A form sends an empty string for a field left blank.
    let error = user::LOGIN
        .validate(&json!({ "email": "", "password": "" }))
        .unwrap_err();
    assert_eq!(
        messages(&error),
        [
            ("email", "required", "The email field must be defined"),
            ("password", "required", "The password field must be defined"),
        ]
    );
    let error = user::LOGIN
        .validate(&json!({ "email": "ada", "password": "x" }))
        .unwrap_err();
    assert_eq!(
        messages(&error),
        [(
            "email",
            "email",
            "The email field must be a valid email address"
        )]
    );
    assert_eq!(vine::ValidationError::STATUS, 422);
}

#[test]
fn changes_a_password_that_is_confirmed() {
    let valid = json!({
        "currentPassword": "old",
        "newPassword": "newsecret1",
        "passwordConfirmation": "newsecret1",
    });
    assert_eq!(user::UPDATE_PASSWORD.validate(&valid).unwrap(), valid);

    let error = user::UPDATE_PASSWORD
        .validate(&json!({
            "currentPassword": "old",
            "newPassword": "newsecret1",
            "passwordConfirmation": "different",
        }))
        .unwrap_err();
    assert_eq!(
        messages(&error),
        [(
            "passwordConfirmation",
            "confirmed",
            "The newPassword field and passwordConfirmation field must be the same"
        )]
    );
    assert_eq!(
        error.messages[0].meta,
        json!({ "otherField": "passwordConfirmation", "originalField": "newPassword" })
            .as_object()
            .cloned()
    );

    let error = user::UPDATE_PASSWORD
        .validate(&json!({ "currentPassword": "old", "newPassword": "short", "passwordConfirmation": "short" }))
        .unwrap_err();
    assert_eq!(
        messages(&error),
        [(
            "newPassword",
            "minLength",
            "The newPassword field must have at least 8 characters"
        )]
    );
}

#[test]
fn saves_the_logging_settings_a_form_sends() {
    #[derive(Debug, PartialEq, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Settings {
        gateway_tool_mode: String,
        mcp_log_level: String,
        mcp_log_retention_days: u32,
        mcp_auto_update_enabled: Option<bool>,
        mcp_auto_update_cron: Option<String>,
    }

    let settings: Settings = user::UPDATE_MCP_LOGGING
        .validate_as(&json!({
            "gatewayToolMode": "lazy",
            "mcpLogLevel": "metadata",
            "mcpLogRetentionDays": "30",
            "mcpAutoUpdateEnabled": "on",
            "mcpAutoUpdateCron": " 0 3 * * * ",
        }))
        .unwrap();
    assert_eq!(
        settings,
        Settings {
            gateway_tool_mode: "lazy".to_owned(),
            mcp_log_level: "metadata".to_owned(),
            mcp_log_retention_days: 30,
            mcp_auto_update_enabled: Some(true),
            mcp_auto_update_cron: Some("0 3 * * *".to_owned()),
        }
    );

    // An unchecked switch is not sent, and a blank expression counts as none.
    let output = user::UPDATE_MCP_LOGGING
        .validate(&json!({
            "gatewayToolMode": "eager",
            "mcpLogLevel": "off",
            "mcpLogRetentionDays": 365,
            "mcpAutoUpdateCron": "",
        }))
        .unwrap();
    assert_eq!(
        output.to_string(),
        r#"{"gatewayToolMode":"eager","mcpLogLevel":"off","mcpLogRetentionDays":365}"#
    );

    let error = user::UPDATE_MCP_LOGGING
        .validate(&json!({
            "gatewayToolMode": "sometimes",
            "mcpLogLevel": "off",
            "mcpLogRetentionDays": "366",
            "mcpAutoUpdateEnabled": "yes",
            "mcpAutoUpdateCron": "every day",
        }))
        .unwrap_err();
    assert_eq!(
        messages(&error),
        [
            (
                "gatewayToolMode",
                "enum",
                "The selected gatewayToolMode is invalid"
            ),
            (
                "mcpLogRetentionDays",
                "max",
                "The mcpLogRetentionDays field must not be greater than 365"
            ),
            (
                "mcpAutoUpdateEnabled",
                "boolean",
                "The value must be a boolean"
            ),
            (
                "mcpAutoUpdateCron",
                "cron",
                "The mcpAutoUpdateCron field must be a valid 5-field cron expression"
            ),
        ]
    );
}

/// What the controller does around `UPDATE_EMAIL` and `ONBOARDING`, where
/// Lucid's `unique` queried the users table from inside the validator.
fn validate_with_unique_email(
    validator: &vine::Validator,
    body: &Value,
    taken: &[&str],
) -> Result<Value, vine::ValidationError> {
    let mut run = validator.start(body);
    for check in run.take_checks() {
        assert_eq!(
            (check.rule.as_str(), check.field.as_str()),
            ("database.unique", "email")
        );
        // Here: `SELECT 1 FROM users WHERE email = ?`, awaited.
        if check
            .value
            .as_str()
            .is_some_and(|email| taken.contains(&email))
        {
            run.reject(&check);
        }
    }
    run.finish()
}

#[test]
fn changes_an_email_address() {
    let body = json!({ "email": "ada@example.com", "currentPassword": "secret" });
    assert_eq!(
        validate_with_unique_email(&user::UPDATE_EMAIL, &body, &[]).unwrap(),
        body
    );

    let error =
        validate_with_unique_email(&user::UPDATE_EMAIL, &body, &["ada@example.com"]).unwrap_err();
    assert_eq!(
        messages(&error),
        [(
            "email",
            "database.unique",
            "The email has already been taken"
        )]
    );

    // A check nobody answers for does not pass.
    let error = user::UPDATE_EMAIL.validate(&body).unwrap_err();
    assert_eq!(
        messages(&error),
        [(
            "email",
            "database.unique",
            "The email has already been taken"
        )]
    );

    // The query is not made for an address the other rules refused.
    let malformed = json!({ "email": "ada", "currentPassword": "secret" });
    assert!(user::UPDATE_EMAIL.start(&malformed).checks().is_empty());
}

#[test]
fn creates_the_first_account() {
    let body = json!({
        "fullName": " Ada Lovelace ",
        "email": "ada@example.com",
        "password": "short",
        "passwordConfirmation": "other",
    });
    // Everything wrong is reported at once, in the order of the form.
    let error =
        validate_with_unique_email(&user::ONBOARDING, &body, &["ada@example.com"]).unwrap_err();
    assert_eq!(
        messages(&error),
        [
            (
                "email",
                "database.unique",
                "The email has already been taken"
            ),
            (
                "password",
                "minLength",
                "The password field must have at least 8 characters"
            ),
        ]
    );

    let body = json!({
        "fullName": " Ada Lovelace ",
        "email": "ada@example.com",
        "password": "longenough",
        "passwordConfirmation": "longenough",
    });
    let output = validate_with_unique_email(&user::ONBOARDING, &body, &[]).unwrap();
    assert_eq!(output["fullName"], "Ada Lovelace");
}

// ---------------------------------------------------------------------------
// gateway.ts, with the cases of tests/unit/vine_gateway_validators.spec.ts
// ---------------------------------------------------------------------------

const MCP_MESSAGE: &str = "mcp must be a non-empty MCP slug of at most 120 characters";
const QUERY_MESSAGE: &str = "query must be non-empty and at most 200 characters";
const LIMIT_MESSAGE: &str = "limit must be an integer between 1 and 20";
const TOOL_MESSAGE: &str = "tool must be a non-empty upstream tool name of at most 128 characters";
const ARGUMENTS_MESSAGE: &str = "arguments must be an object when provided";

/// Values that stand where a string is expected without being one. The
/// first is `undefined`.
fn not_strings() -> Vec<Option<Value>> {
    let mut values = vec![None];
    values.extend(
        [
            json!(null),
            json!(0),
            json!(1),
            json!(true),
            json!(["issues"]),
            json!([]),
            json!({}),
            json!({ "slug": "issues" }),
        ]
        .map(Some),
    );
    values
}

/// `{ ...base, [key]: value }`, where `None` leaves the key out.
fn with(base: Value, key: &str, value: Option<Value>) -> Value {
    let mut object = base;
    if let (Some(object), Some(value)) = (object.as_object_mut(), value) {
        object.insert(key.to_owned(), value);
    }
    object
}

/// `parseToolSearchInput` and `parseCallToolInput` of `#services/gateway_lazy_tools`.
fn first_message(validator: &vine::Validator, args: Option<Value>) -> Result<Value, String> {
    validator
        .try_validate(&args.unwrap_or_else(|| json!({})))
        .map_err(|error| error.messages[0].message.clone())
}

#[test]
fn reads_eager_and_lazy_whatever_their_case_and_surrounding_whitespace() {
    for (header, mode) in [
        ("eager", "eager"),
        ("lazy", "lazy"),
        ("LAZY", "lazy"),
        ("Eager", "eager"),
        (" LaZy ", "lazy"),
        ("\tlazy\n", "lazy"),
    ] {
        assert_eq!(
            gateway::GATEWAY_TOOL_MODE
                .validate_opt(&json!(header))
                .unwrap(),
            Some(json!(mode))
        );
    }
}

#[test]
fn leaves_the_mode_to_the_instance_when_the_header_is_absent_or_blank() {
    for header in [None, Some(json!("")), Some(json!(" ")), Some(json!(" \t "))] {
        assert_eq!(
            gateway::GATEWAY_TOOL_MODE.validate_opt(&header).unwrap(),
            None
        );
        // Without the distinction, `undefined` reads `null`.
        assert_eq!(
            gateway::GATEWAY_TOOL_MODE.validate(&header).unwrap(),
            Value::Null
        );
    }
}

#[test]
fn refuses_a_header_that_names_no_mode_the_gateway_has() {
    for header in [
        "sometimes",
        "lazyy",
        "laz",
        "lazy, eager",
        "eager lazy",
        "\"lazy\"",
        "0",
    ] {
        assert!(
            gateway::GATEWAY_TOOL_MODE.validate(&json!(header)).is_err(),
            "{header}"
        );
    }
    for header in [json!(0), json!(1), json!(true), json!(["lazy"]), json!({})] {
        assert!(
            gateway::GATEWAY_TOOL_MODE.validate(&header).is_err(),
            "{header}"
        );
    }
}

#[test]
fn splits_a_name_at_its_first_separator() {
    let long = "x".repeat(4000);
    for (name, slug, tool_name) in [
        ("weather__get_forecast", "weather", "get_forecast"),
        ("weather__get__forecast", "weather", "get__forecast"),
        ("weather___get_forecast", "weather", "_get_forecast"),
        ("_weather__get_forecast", "_weather", "get_forecast"),
        ("weather__", "weather", ""),
        (" __tool", " ", "tool"),
        ("Not A Slug__tool", "Not A Slug", "tool"),
        ("multi\nline__tool\nname", "multi\nline", "tool\nname"),
        (format!("{long}__tool").as_str(), long.as_str(), "tool"),
    ] {
        assert_eq!(
            gateway::NAMESPACED_TOOL.validate(&json!(name)).unwrap(),
            json!({ "slug": slug, "toolName": tool_name })
        );
    }
}

#[test]
fn refuses_a_name_without_a_separator_or_without_a_slug_before_it() {
    for name in [
        "",
        " ",
        "_",
        "__",
        "___",
        "__tool",
        "___tool",
        "__weather__tool",
        "weather",
        "weather_tool",
        "weather_",
    ] {
        assert!(
            gateway::NAMESPACED_TOOL.validate(&json!(name)).is_err(),
            "{name:?}"
        );
    }
    for name in not_strings() {
        assert!(
            gateway::NAMESPACED_TOOL.validate(&name).is_err(),
            "{name:?}"
        );
    }
}

#[test]
fn trims_the_slug_and_the_query_and_defaults_the_limit_to_10() {
    assert_eq!(
        first_message(
            &gateway::TOOL_SEARCH,
            Some(json!({ "mcp": " issues ", "query": " create issue " }))
        ),
        Ok(json!({ "mcp": "issues", "query": "create issue", "limit": 10 }))
    );
    assert_eq!(
        first_message(
            &gateway::TOOL_SEARCH,
            Some(json!({ "mcp": "issues", "query": "q", "limit": 20, "extra": true }))
        ),
        Ok(json!({ "mcp": "issues", "query": "q", "limit": 20 }))
    );
    // The bounds apply to what is left after trimming.
    let padded = json!({ "mcp": format!(" {} ", "x".repeat(120)), "query": format!(" {} ", "q".repeat(200)) });
    assert!(first_message(&gateway::TOOL_SEARCH, Some(padded)).is_ok());
}

#[test]
fn tells_the_agent_what_the_slug_and_the_query_must_be() {
    assert_eq!(
        first_message(&gateway::TOOL_SEARCH, None),
        Err(MCP_MESSAGE.to_owned())
    );
    assert_eq!(
        first_message(&gateway::TOOL_SEARCH, Some(json!({}))),
        Err(MCP_MESSAGE.to_owned())
    );
    let long = "x".repeat(121);
    let mut slugs: Vec<Option<Value>> = [
        "",
        " ",
        " \n\t",
        long.as_str(),
        format!(" {long} ").as_str(),
    ]
    .map(|slug| Some(json!(slug)))
    .to_vec();
    slugs.extend(not_strings());
    for mcp in slugs {
        let args = with(json!({ "query": "issue" }), "mcp", mcp.clone());
        assert_eq!(
            first_message(&gateway::TOOL_SEARCH, Some(args)),
            Err(MCP_MESSAGE.to_owned()),
            "{mcp:?}"
        );
    }

    let long = "q".repeat(201);
    let mut queries: Vec<Option<Value>> = ["", " ", long.as_str(), format!(" {long} ").as_str()]
        .map(|query| Some(json!(query)))
        .to_vec();
    queries.extend(not_strings());
    for query in queries {
        let args = with(json!({ "mcp": "issues" }), "query", query.clone());
        assert_eq!(
            first_message(&gateway::TOOL_SEARCH, Some(args)),
            Err(QUERY_MESSAGE.to_owned()),
            "{query:?}"
        );
    }
}

#[test]
fn takes_an_integer_from_1_to_20_as_limit_and_nothing_that_only_looks_like_one() {
    for limit in [json!(1), json!(2), json!(10), json!(20), json!(5.0)] {
        let output = first_message(
            &gateway::TOOL_SEARCH,
            Some(json!({ "mcp": "issues", "query": "q", "limit": limit })),
        )
        .unwrap();
        assert_eq!(output["limit"].as_f64(), limit.as_f64());
        assert!(
            output["limit"].is_u64(),
            "{limit} is written without a fraction"
        );
    }
    // NaN and the infinities of the TypeScript test cannot be written in JSON.
    for limit in [
        json!(0),
        json!(-1),
        json!(21),
        json!(1.5),
        json!(1e21),
        json!("5"),
        json!(""),
        json!(" "),
        json!(null),
        json!(true),
        json!(false),
        json!([5]),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            first_message(
                &gateway::TOOL_SEARCH,
                Some(json!({ "mcp": "issues", "query": "q", "limit": limit }))
            ),
            Err(LIMIT_MESSAGE.to_owned()),
            "{limit}"
        );
    }
}

#[test]
fn reports_the_first_wrong_argument_only() {
    let search = |mcp: &str, query: &str| {
        first_message(
            &gateway::TOOL_SEARCH,
            Some(json!({ "mcp": mcp, "query": query, "limit": 0 })),
        )
    };
    assert_eq!(search("", ""), Err(MCP_MESSAGE.to_owned()));
    assert_eq!(search("issues", ""), Err(QUERY_MESSAGE.to_owned()));
    assert_eq!(search("issues", "q"), Err(LIMIT_MESSAGE.to_owned()));
}

#[test]
fn tells_the_agent_which_of_the_slug_the_tool_and_the_arguments_is_wrong() {
    assert_eq!(
        first_message(
            &gateway::CALL_TOOL,
            Some(json!({ "mcp": " issues ", "tool": " create_issue " }))
        ),
        Ok(json!({ "mcp": "issues", "tool": "create_issue" }))
    );
    assert_eq!(
        first_message(&gateway::CALL_TOOL, None),
        Err(MCP_MESSAGE.to_owned())
    );
    assert_eq!(
        first_message(&gateway::CALL_TOOL, Some(json!({ "mcp": "issues" }))),
        Err(TOOL_MESSAGE.to_owned())
    );
    let long = "t".repeat(129);
    let mut tools: Vec<Option<Value>> = ["", " ", long.as_str(), format!(" {long} ").as_str()]
        .map(|tool| Some(json!(tool)))
        .to_vec();
    tools.extend(not_strings());
    for tool in tools {
        let args = with(json!({ "mcp": "issues" }), "tool", tool.clone());
        assert_eq!(
            first_message(&gateway::CALL_TOOL, Some(args)),
            Err(TOOL_MESSAGE.to_owned()),
            "{tool:?}"
        );
    }

    // Null is an argument the agent sent, unlike an argument left out.
    for sent in [
        json!(null),
        json!([]),
        json!([{}]),
        json!("text"),
        json!(""),
        json!(" "),
        json!(0),
        json!(1),
        json!(true),
        json!(false),
    ] {
        assert_eq!(
            first_message(
                &gateway::CALL_TOOL,
                Some(json!({ "mcp": "issues", "tool": "create_issue", "arguments": sent }))
            ),
            Err(ARGUMENTS_MESSAGE.to_owned()),
            "{sent}"
        );
    }
    assert_eq!(
        first_message(
            &gateway::CALL_TOOL,
            Some(json!({ "mcp": "", "tool": "", "arguments": [] }))
        ),
        Err(MCP_MESSAGE.to_owned())
    );
    assert_eq!(
        first_message(
            &gateway::CALL_TOOL,
            Some(json!({ "mcp": "issues", "tool": "", "arguments": [] }))
        ),
        Err(TOOL_MESSAGE.to_owned())
    );
}

// ---------------------------------------------------------------------------
// builtin_tools.ts, with the cases of tests/unit/vine_builtin_tools.spec.ts
// ---------------------------------------------------------------------------

use builtin_tools::{
    BuiltinToolError, NO_ARGUMENTS, TOOL_VINE, blank_as_missing, boolean, choice, integer, line,
    list_length, number, pattern, text, tool_input, trimmed_text,
};

/// What a tool gets for the argument `x`.
#[derive(Debug, PartialEq)]
enum Argument {
    Value(Value),
    Missing,
    /// The sentence the agent reads instead.
    Refused(String),
}

fn refused(sentence: &str) -> Argument {
    Argument::Refused(sentence.to_owned())
}

fn argument(schema: impl Into<vine::Schema>) -> impl Fn(Option<Value>) -> Argument {
    let validator = TOOL_VINE.create(vine::object! { "x" => schema });
    move |value| {
        let args = with(json!({}), "x", value);
        match tool_input(&validator, Some(&args), &()) {
            Ok(input) => input
                .get("x")
                .cloned()
                .map_or(Argument::Missing, Argument::Value),
            Err(BuiltinToolError(sentence)) => Argument::Refused(sentence),
        }
    }
}

fn given(value: Value) -> Option<Value> {
    Some(value)
}

#[test]
fn takes_a_whole_number_quoted_or_not_and_nothing_that_only_reads_like_one() {
    let per_page = argument(integer(1, 100.0).optional());
    let sentence = "x must be an integer between 1 and 100";

    for (value, expected) in [
        (json!(5), 5),
        (json!("5"), 5),
        (json!(" 7 "), 7),
        (json!("007"), 7),
        (json!(100), 100),
        (json!(1.0), 1),
    ] {
        let read = per_page(given(value.clone()));
        assert_eq!(read, Argument::Value(json!(expected)), "{value}");
        // A whole number, whichever way it was written.
        assert!(
            matches!(read, Argument::Value(number) if number.is_u64()),
            "{value}"
        );
    }
    for value in [None, given(json!(null)), given(json!(""))] {
        assert_eq!(per_page(value.clone()), Argument::Missing, "{value:?}");
    }
    // Vine's own number type would read every one of these as a number.
    for value in [
        json!(0),
        json!(101),
        json!(1.5),
        json!("1.0"),
        json!("1e1"),
        json!("0x10"),
        json!("+5"),
        json!(" "),
        json!(true),
        json!([5]),
        json!({}),
        json!("５"),
    ] {
        assert_eq!(per_page(given(value.clone())), refused(sentence), "{value}");
    }
}

#[test]
fn names_the_bounds_of_a_whole_number_and_stays_within_the_safe_integers() {
    let id = argument(integer(1, None));

    assert_eq!(
        id(given(json!("15000000001"))),
        Argument::Value(json!(15_000_000_001_u64))
    );
    assert_eq!(
        id(given(json!(9_007_199_254_740_991_u64))),
        Argument::Value(json!(9_007_199_254_740_991_u64))
    );
    for value in [
        json!(0),
        json!(-1),
        json!("abc"),
        json!("12/../../athlete"),
        json!(9_007_199_254_740_992_u64),
        json!("9007199254740993"),
    ] {
        assert_eq!(
            id(given(value.clone())),
            refused("x must be an integer of at least 1"),
            "{value}"
        );
    }
    assert_eq!(id(None), refused("x is required"));
    assert_eq!(id(given(json!(""))), refused("x is required"));

    let category = argument(integer(0, 5.0));
    assert_eq!(
        category(given(json!(6))),
        refused("x must be an integer between 0 and 5")
    );
}

#[test]
fn takes_a_number_quoted_or_not_within_its_bounds() {
    let latitude = argument(number(-90, 90).optional());
    let sentence = "x must be a number between -90 and 90";

    for (value, expected) in [
        (json!(45.5), json!(45.5)),
        (json!("45.5"), json!(45.5)),
        (json!(" -90 "), json!(-90)),
        (json!("1e1"), json!(10)),
        (json!(0), json!(0)),
    ] {
        assert_eq!(
            latitude(given(value.clone())),
            Argument::Value(expected),
            "{value}"
        );
    }
    assert_eq!(latitude(given(json!(""))), Argument::Missing);
    for value in [
        json!(120),
        json!(-90.01),
        json!(" "),
        json!("north"),
        json!("Infinity"),
        json!(true),
        json!(false),
        json!([45]),
        json!({}),
    ] {
        assert_eq!(latitude(given(value.clone())), refused(sentence), "{value}");
    }

    let distance = argument(number(0, 10_000_000));
    assert_eq!(
        distance(given(json!(-5))),
        refused("x must be a number between 0 and 10000000")
    );
    assert_eq!(distance(given(json!(null))), refused("x is required"));
}

#[test]
fn takes_true_and_false_quoted_or_not_and_no_other_way_to_say_them() {
    let flag = argument(boolean().optional());

    assert_eq!(flag(given(json!(true))), Argument::Value(json!(true)));
    assert_eq!(flag(given(json!("true"))), Argument::Value(json!(true)));
    assert_eq!(flag(given(json!(false))), Argument::Value(json!(false)));
    assert_eq!(flag(given(json!("false"))), Argument::Value(json!(false)));
    assert_eq!(flag(given(json!(""))), Argument::Missing);
    assert_eq!(flag(given(json!(null))), Argument::Missing);
    // Vine's own boolean type would take all of these.
    for value in [
        json!(1),
        json!(0),
        json!("1"),
        json!("0"),
        json!("on"),
        json!("off"),
        json!("TRUE"),
        json!(" true "),
    ] {
        assert_eq!(
            flag(given(value.clone())),
            refused("x must be true or false"),
            "{value}"
        );
    }
}

#[test]
fn lists_the_choices_of_an_argument_that_has_a_few() {
    let activity_type = argument(choice(["riding", "running"]).optional());

    assert_eq!(
        activity_type(given(json!("running"))),
        Argument::Value(json!("running"))
    );
    assert_eq!(activity_type(given(json!(""))), Argument::Missing);
    for value in [
        json!("Riding"),
        json!(" riding"),
        json!("walking"),
        json!(1),
        json!(["riding"]),
    ] {
        assert_eq!(
            activity_type(given(value.clone())),
            refused("x must be one of: riding, running"),
            "{value}"
        );
    }
}

#[test]
fn keeps_text_as_written_an_empty_one_included() {
    let description = argument(text(10).optional());

    for written in ["", "  two  ", "a\nb", "xxxxxxxxxx"] {
        assert_eq!(
            description(given(json!(written))),
            Argument::Value(json!(written))
        );
    }
    assert_eq!(description(given(json!(null))), Argument::Missing);
    for value in [
        json!("xxxxxxxxxxx"),
        json!(5),
        json!(true),
        json!(["a"]),
        json!({}),
    ] {
        assert_eq!(
            description(given(value.clone())),
            refused("x must be text of at most 10 characters"),
            "{value}"
        );
    }

    let body = argument(text(10).parse(|value, _| blank_as_missing(value)));
    assert_eq!(body(given(json!(""))), refused("x is required"));
    assert_eq!(body(given(json!(" "))), Argument::Value(json!(" ")));
}

#[test]
fn trims_text_and_counts_what_is_left_empty_as_left_out() {
    let name = argument(trimmed_text(10));

    assert_eq!(
        name(given(json!(" Evening "))),
        Argument::Value(json!("Evening"))
    );
    assert_eq!(name(given(json!("a\nb"))), Argument::Value(json!("a\nb")));
    assert_eq!(name(given(json!("   "))), refused("x is required"));
    assert_eq!(name(given(json!(""))), refused("x is required"));
    // The limit is on what was written, spaces included.
    assert_eq!(
        name(given(json!(format!(" {}", "x".repeat(9))))),
        Argument::Value(json!("x".repeat(9)))
    );
    assert_eq!(
        name(given(json!(format!(" {}", "x".repeat(10))))),
        refused("x must be text of at most 10 characters")
    );
    assert_eq!(
        name(given(json!(5))),
        refused("x must be text of at most 10 characters")
    );
}

#[test]
fn refuses_control_characters_in_a_single_line_of_text() {
    let mailbox = argument(line(10).optional());

    assert_eq!(
        mailbox(given(json!(" Archive "))),
        Argument::Value(json!("Archive"))
    );
    assert_eq!(
        mailbox(given(json!("Boîte"))),
        Argument::Value(json!("Boîte"))
    );
    assert_eq!(mailbox(given(json!(" \n "))), Argument::Missing);
    for value in [
        "a\nb",
        "a\rb",
        "a\tb",
        "a\u{0000}b",
        "a\u{007f}b",
        "a\u{0085}b",
    ] {
        assert_eq!(
            mailbox(given(json!(value))),
            refused("x must be a single line of text"),
            "{value:?}"
        );
    }
    assert_eq!(
        mailbox(given(json!("x".repeat(11)))),
        refused("x must be text of at most 10 characters")
    );
    assert_eq!(
        argument(line(10))(given(json!("  "))),
        refused("x is required")
    );
}

#[test]
fn takes_an_identifier_only_when_it_matches_its_pattern_in_full() {
    let gear = argument(pattern(
        r"^[bg]\d{1,20}$",
        "a gear identifier such as b1234567",
    ));
    let sentence = "x must be a gear identifier such as b1234567";

    assert_eq!(
        gear(given(json!("b1234567"))),
        Argument::Value(json!("b1234567"))
    );
    assert_eq!(gear(given(json!(" g1 "))), Argument::Value(json!("g1")));
    for value in [
        json!("../athlete"),
        json!("b12?x=1"),
        json!("B12"),
        json!("b"),
        json!(" "),
        json!(12),
        json!(true),
        json!(["b1"]),
    ] {
        assert_eq!(gear(given(value.clone())), refused(sentence), "{value}");
    }
    assert_eq!(gear(given(json!(""))), refused("x is required"));

    // An identifier made of digits may come as a number.
    let part = argument(pattern(r"^\d{1,3}(\.\d{1,3}){0,9}$", "a part"));
    assert_eq!(part(given(json!(2))), Argument::Value(json!("2")));
    assert_eq!(part(given(json!(1.2))), Argument::Value(json!("1.2")));
    assert_eq!(part(given(json!(1000))), refused("x must be a part"));
}

fn agent_validator() -> vine::Validator {
    TOOL_VINE.create(vine::object! {
        "first" => integer(1, None),
        "second" => boolean().optional(),
        "ids" => vine::array(integer(1, 9.0))
            .use_rule(list_length(Some(1), 3, "{{ field }} must be a list of 1 to 3 ids"))
            .optional(),
    })
}

/// The sentence the agent reads, or `None` when the arguments are fine.
fn refusal(args: Option<Value>) -> Option<String> {
    tool_input(&agent_validator(), args.as_ref(), &())
        .err()
        .map(|BuiltinToolError(sentence)| sentence)
}

#[test]
fn reports_one_argument_at_a_time_in_the_order_the_schema_lists_them() {
    assert_eq!(
        refusal(given(json!({ "second": "maybe", "ids": "x" }))).as_deref(),
        Some("first is required")
    );
    assert_eq!(
        refusal(given(json!({ "first": 0, "second": "maybe" }))).as_deref(),
        Some("first must be an integer of at least 1")
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "second": "maybe", "ids": [] }))).as_deref(),
        Some("second must be true or false")
    );
    assert_eq!(refusal(given(json!({ "first": 1 }))), None);
}

#[test]
fn names_an_item_of_a_list_after_the_list() {
    let list = "ids must be a list of 1 to 3 ids";
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [] }))).as_deref(),
        Some(list)
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, 2, 3, 4] }))).as_deref(),
        Some(list)
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, "x"] }))).as_deref(),
        Some("ids must be an integer between 1 and 9")
    );
    assert_eq!(
        refusal(given(json!({ "first": 1, "ids": [1, null] }))).as_deref(),
        Some("ids is required")
    );
    assert_eq!(
        tool_input(
            &agent_validator(),
            Some(&json!({ "first": 1, "ids": ["2", 3] })),
            &()
        ),
        Ok(json!({ "first": 1, "ids": [2, 3] }))
    );
}

#[test]
fn drops_the_arguments_a_tool_does_not_know_and_refuses_what_is_not_a_set_of_them() {
    assert_eq!(
        tool_input(
            &agent_validator(),
            Some(&json!({ "first": "1", "extra": true, "constructor": 1 })),
            &()
        ),
        Ok(json!({ "first": 1 }))
    );
    assert_eq!(
        tool_input(&NO_ARGUMENTS, Some(&json!({ "anything": 1 })), &()),
        Ok(json!({}))
    );
    assert_eq!(
        refusal(given(json!(null))).as_deref(),
        Some("arguments is required")
    );
    assert_eq!(refusal(None).as_deref(), Some("arguments is required"));
    assert_eq!(
        refusal(given(json!([1]))).as_deref(),
        Some("arguments must be an object")
    );
    assert_eq!(
        refusal(given(json!("first"))).as_deref(),
        Some("arguments must be an object")
    );
}

#[test]
fn leaves_an_empty_string_alone_unlike_the_vine_of_the_pages() {
    // start/validator.ts turns empty strings into null for HTML forms.
    let form = vine::global().create(vine::object! { "description" => vine::string().optional() });
    let tool = TOOL_VINE.create(vine::object! { "description" => text(100).optional() });
    assert_eq!(
        form.validate(&json!({ "description": "" })).unwrap(),
        json!({})
    );
    assert_eq!(
        tool_input(&tool, Some(&json!({ "description": "" })), &()),
        Ok(json!({ "description": "" }))
    );
}

#[test]
fn says_how_each_argument_reads_in_a_json_schema() {
    let described = TOOL_VINE.create(vine::object! {
        "id" => integer(1, None),
        "per_page" => integer(1, 100.0).optional(),
        "weight" => number(20, 400),
        "starred" => boolean().optional(),
        "kind" => choice(["riding", "running"]).optional(),
        "name" => trimmed_text(255),
        "mailbox" => line(255).optional(),
        "gear_id" => pattern("^b$", "b").optional(),
        "ids" => vine::array(integer(1, None)).use_rule(list_length(Some(1), 3, "")).optional(),
    });

    assert_eq!(
        described.to_json_schema(),
        &json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "minimum": 1 },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100 },
                "weight": { "type": "number", "minimum": 20, "maximum": 400 },
                "starred": { "type": "boolean" },
                "kind": { "enum": ["riding", "running"] },
                "name": { "type": "string", "maxLength": 255 },
                "mailbox": { "type": "string", "maxLength": 255 },
                "gear_id": { "type": "string" },
                "ids": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 1 },
                    "minItems": 1,
                    "maxItems": 3,
                },
            },
            "required": ["id", "weight", "name"],
            "additionalProperties": false,
        })
    );
}

#[test]
fn looks_at_an_argument_only_with_the_one_it_says_more_about() {
    let validator = TOOL_VINE.create(vine::object! {
        "reply_to_uid" => integer(1, None).optional(),
        "reply_to_mailbox" => builtin_tools::only_with("reply_to_uid", line(10)).optional(),
        "reply_all" => builtin_tools::only_with("reply_to_uid", boolean()).optional(),
    });
    let input = |args: Value| tool_input(&validator, Some(&args), &());

    // Without the message replied to, whatever the other two hold is ignored.
    assert_eq!(
        input(json!({ "reply_to_mailbox": "far too long", "reply_all": "perhaps" })),
        Ok(json!({}))
    );
    assert_eq!(
        input(json!({ "reply_to_uid": "7", "reply_to_mailbox": " Sent ", "reply_all": "true" })),
        Ok(json!({ "reply_to_uid": 7, "reply_to_mailbox": "Sent", "reply_all": true }))
    );
    assert_eq!(
        input(json!({ "reply_to_uid": 7, "reply_to_mailbox": "far too long" })),
        Err(BuiltinToolError(
            "reply_to_mailbox must be text of at most 10 characters".to_owned()
        ))
    );
    assert_eq!(
        input(json!({ "reply_to_uid": 7, "reply_all": "perhaps" })),
        Err(BuiltinToolError(
            "reply_all must be true or false".to_owned()
        ))
    );
}

/// The context of a call, as a tool receives it.
struct Context {
    greeting: &'static str,
    known: Vec<&'static str>,
}

#[test]
fn passes_the_context_of_the_call_to_the_rules_that_depend_on_it() {
    let known_rule = vine::rule(|value, field| {
        let known = field.meta::<Context>().is_some_and(|context| {
            value
                .as_str()
                .is_some_and(|name| context.known.contains(&name))
        });
        if !known {
            field.report("{{ field }} must be someone we know", "known");
        }
    });
    let validator =
        TOOL_VINE.create(vine::object! { "name" => trimmed_text(20).use_rule(known_rule) });
    let context = Context {
        greeting: "Hi",
        known: vec!["Ada"],
    };

    let input = tool_input(&validator, Some(&json!({ "name": " Ada " })), &context).unwrap();
    assert_eq!(input, json!({ "name": "Ada" }));
    assert_eq!(
        format!("{} {}", context.greeting, input["name"].as_str().unwrap()),
        "Hi Ada"
    );
    assert_eq!(
        tool_input(&validator, Some(&json!({ "name": "Bob" })), &context),
        Err(BuiltinToolError("name must be someone we know".to_owned()))
    );
}

#[test]
fn shares_validators_between_threads() {
    fn assert_shareable<T: Send + Sync>() {}
    assert_shareable::<vine::Validator>();
    assert_shareable::<vine::Vine>();
    assert_shareable::<vine::Schema>();
    assert_shareable::<vine::Rule>();
    assert_shareable::<vine::ValidationError>();

    let handles: Vec<_> = (0..8)
        .map(|index| {
            std::thread::spawn(move || {
                let output = user::LOGIN
                    .validate(
                        &json!({ "email": format!("user{index}@example.com"), "password": "x" }),
                    )
                    .unwrap();
                assert_eq!(output["email"], format!("user{index}@example.com"));
                assert!(
                    gateway::CALL_TOOL
                        .validate(&json!({ "mcp": index }))
                        .is_err()
                );
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}
