//! The part of [VineJS](https://vinejs.dev) 4.4 that MyMCPs validates its
//! input with, for `serde_json::Value`.
//!
//! The TypeScript app describes every input it accepts as a Vine schema:
//! the forms of its pages, route parameters, OAuth requests, the arguments
//! agents call tools with, the JSON other services answer with. This crate
//! lets those schemas be written again almost line for line, and answers as
//! Vine does: the same values pass, the same output comes back, and the
//! same errors are reported in the same order with the same messages.
//! `tests/differential.rs` holds it to that, against what the real Vine
//! answers for some eighteen thousand inputs.
//!
//! ```
//! use std::sync::LazyLock;
//!
//! use mymcps_vine as vine;
//! use serde_json::json;
//!
//! // export const recordIdParamsValidator = vine.create({
//! //   id: vine.number().withoutDecimals().positive(),
//! // })
//! static RECORD_ID_PARAMS: LazyLock<vine::Validator> = LazyLock::new(|| {
//!     vine::global().create(vine::object! {
//!         "id" => vine::number().without_decimals().positive(),
//!     })
//! });
//!
//! // const [, route] = await recordIdParamsValidator.tryValidate(params)
//! let route = RECORD_ID_PARAMS.try_validate(&json!({ "id": "42", "other": true }));
//! assert_eq!(route.unwrap(), json!({ "id": 42 }));
//!
//! let error = RECORD_ID_PARAMS.validate(&json!({ "id": "4.2" })).unwrap_err();
//! assert_eq!(error.messages[0].message, "The id field must be an integer");
//! assert_eq!(error.messages[0].rule, "withoutDecimals");
//! assert_eq!(error.messages[0].field, "id");
//! ```
//!
//! # From TypeScript to Rust
//!
//! Import the crate as `vine`: `use mymcps_vine as vine;`.
//!
//! | TypeScript | Rust |
//! |---|---|
//! | **Instances** | |
//! | `vine` (the default export, after `start/validator.ts`) | [`vine::global()`](global): empty strings are read as `null` |
//! | `new Vine()` | [`Vine::new()`]: values are read as they are |
//! | `vine.convertEmptyStringsToNull = true` | [`.convert_empty_strings_to_null(true)`](Vine::convert_empty_strings_to_null) |
//! | `vine.messagesProvider = provider` | [`.messages_provider(provider)`](Vine::messages_provider) |
//! | `vine.create({ a: ..., b: ... })` | `vine.create(vine::object! { "a" => ..., "b" => ... })` |
//! | `vine.create(schema)` | [`vine.create(schema)`](Vine::create) |
//! | `vine.withMetaData<T>().create(schema)` | `vine.create(schema)`: see [Metadata](#metadata) |
//! | `validator.messagesProvider = provider` | [`validator.messages_provider(provider)`](Validator::messages_provider), chained on `create` |
//! | **Validating** | |
//! | `await validator.validate(data)` | [`validator.validate(&data)?`](Validator::validate) |
//! | `const [error, output] = await validator.tryValidate(data)` | `match validator.try_validate(&data) { Ok(output) => ..., Err(error) => ... }` |
//! | `validator.validate(data, { meta })` | [`validator.validate_with(&data, &meta)`](Validator::validate_with) |
//! | `Infer<typeof validator>` | a `Deserialize` type, and [`validator.validate_as::<T>(&data)`](Validator::validate_as) |
//! | `validator.tryValidate(undefined)` | `validator.validate(None::<&Value>)`, or any `Option<&Value>`: see [Values](#values) |
//! | `validator.toJSONSchema()` | [`validator.to_json_schema()`](Validator::to_json_schema) |
//! | `error.messages[0].message` | `error.messages[0].message` ([`ValidationError`], [`FieldError`]) |
//! | `new errors.E_VALIDATION_ERROR([{ field, message, rule }])` | [`ValidationError::single(field, rule, message)`](ValidationError::single), [`merge`](ValidationError::merge), [`push`](ValidationError::push) |
//! | `error instanceof errors.E_VALIDATION_ERROR` | the `Err` of `validate` is always one |
//! | **Types** | |
//! | `vine.string()` | [`vine::string()`](string) |
//! | `vine.number()`, `vine.number({ strict: true })` | [`vine::number()`](number), `vine::number().strict()` |
//! | `vine.boolean()`, `vine.boolean({ strict: true })` | [`vine::boolean()`](boolean), `vine::boolean().strict()` |
//! | `vine.enum(['a', 'b'])` | [`vine::enum_(["a", "b"])`](enum_) |
//! | `vine.literal('code')` | [`vine::literal("code")`](literal) |
//! | `vine.any()` | [`vine::any()`](any) |
//! | `vine.date({ formats: ['iso8601'] })` | [`vine::date_iso8601()`](date_iso8601): the output is an ISO string |
//! | `vine.object({ a: ... })` | [`vine::object! { "a" => ... }`](object!), or [`vine::object(list)`](object()) |
//! | `{ ...period(), limit: rows() }` | `vine::object! { ..period(), "limit" => rows() }`, with `period()` returning [`vine::properties! { ... }`](properties!) |
//! | `vine.array(schema)` | [`vine::array(schema)`](array()) |
//! | `vine.record(schema)` | [`vine::record(schema)`](record) |
//! | `class VineArgument extends BaseLiteralType`, `new VineArgument(rule)` | [`vine::custom([rule])`](custom), a [`VineCustom`] |
//! | a schema of any type (`SchemaTypes`) | [`Schema`], which every type converts `.into()` |
//! | **Modifiers, on every type** | |
//! | `.optional()` | `.optional()` |
//! | `.nullable()` | `.nullable()` |
//! | `.parse((value, context) => ...)` | `.parse(\|value, context\| ...)`: [`Schema::parse`], [`ParseContext`] |
//! | `schema.options.parse` | [`schema.parser()`](Schema::parser) |
//! | `.transform((value, field) => ...)` | `.transform(\|value, field\| ...)`, on the types that hold one value |
//! | `.use(rule())` | `.use_rule(rule())` |
//! | `.bail(false)` | `.bail(false)` |
//! | `.requiredWhen('transport', '=', 'http')` | `.required_when("transport", Operator::Eq, "http")`: [`Operator`] |
//! | `.requiredWhen((field) => ...)` | `.required_when_fn(\|field\| ...)` |
//! | `.requiredIfExists`, `.requiredIfAnyExists`, `.requiredIfMissing`, `.requiredIfAnyMissing` | `.required_if_exists([...])`, `.required_if_any_exists([...])`, `.required_if_missing([...])`, `.required_if_any_missing([...])` |
//! | **Strings** ([`VineString`]) | |
//! | `.trim()` | `.trim()` |
//! | `.minLength(1)`, `.maxLength(120)`, `.fixedLength(2)` | `.min_length(1)`, `.max_length(120)`, `.fixed_length(2)` |
//! | `.regex(/^[0-9a-f]{64}$/)` | `.regex(vine::js::regex(r"^[0-9a-f]{64}$", "").expect("static regex"))`: see [Regular expressions](#regular-expressions) |
//! | `.email()` | `.email()` |
//! | `.url()`, `.url({ require_tld: false })` | `.url()`, `.url_with(UrlOptions { require_tld: false, ..UrlOptions::default() })` |
//! | `.confirmed({ confirmationField: 'passwordConfirmation' })`, `.confirmed()` | `.confirmed("passwordConfirmation")`, `.confirmed(None)` |
//! | `.startsWith('/authorize?')`, `.endsWith(s)`, `.in([...])` | `.starts_with("/authorize?")`, `.ends_with(s)`, `.in_([...])` |
//! | `.unique({ table, column })` (Lucid) | `.use_rule(vine::lucid::unique())`, and the query made by the caller: see [Checks that wait](#checks-that-wait) |
//! | **Numbers** ([`VineNumber`]) | |
//! | `.min(1)`, `.max(365)`, `.range([1, 20])` | `.min(1)`, `.max(365)`, `.range([1, 20])` |
//! | `.positive()`, `.withoutDecimals()` | `.positive()`, `.without_decimals()` |
//! | `.in([10, 25, 50, 100])` | `.in_([10, 25, 50, 100])` |
//! | **Arrays and objects** | |
//! | `.minLength(1)`, `.maxLength(50)`, `.fixedLength(1)` | `.min_length(1)`, `.max_length(50)`, `.fixed_length(1)` ([`VineArray`]) |
//! | `.allowUnknownProperties()` | `.allow_unknown_properties()` ([`VineObject`]) |
//! | **Rules** | |
//! | `vine.createRule((value, options, field) => { ... })` then `rule(options)` | a function taking the options and returning [`vine::rule(move \|value, field\| { ... })`](rule()) |
//! | `vine.createRule(fn, { implicit: true })` | `vine::rule(...).implicit()` |
//! | `vine.createRule(fn, { toJSONSchema: (schema, options) => ... })` | `vine::rule(...).json_schema(move \|schema\| ...)` |
//! | `field.report(message, rule, field)` | [`field.report(message, rule)`](FieldContext::report) |
//! | `field.report(message, rule, field, args)` | [`field.report_with(message, rule, json!({ ... }))`](FieldContext::report_with) |
//! | `field.mutate(value, field)` | [`field.mutate(value)`](FieldContext::mutate) |
//! | `field.name`, `field.wildCardPath`, `field.getFieldPath()` | `field.name()`, `field.wildcard_path()`, `field.path()` |
//! | `field.isValid`, `field.isDefined`, `field.isArrayMember` | `field.is_valid()`, `field.is_defined()`, `field.is_array_member()` |
//! | `value === null`, `value === undefined` (implicit rules) | `field.is_null()`, `field.is_undefined()` |
//! | `field.parent`, `field.parent[other]`, `field.data` | `field.parent()`, [`field.parent_get(other)`](FieldContext::parent_get), `field.data()` |
//! | `field.meta` | [`field.meta::<T>()`](FieldContext::meta) |
//! | `vine.helpers.getNestedValue(key, field)` | `field.nested_value(key)` |
//! | **Messages** | |
//! | `new SimpleMessagesProvider(messages, fields)` | [`SimpleMessagesProvider::new(messages).with_fields(fields)`](SimpleMessagesProvider) |
//! | `class X extends SimpleMessagesProvider { getMessage(...) }` | a type holding a `SimpleMessagesProvider`, with [`MessagesProvider`] implemented for it |
//! | `{ getMessage: (message, rule, field, args) => ... }` | a closure `\|message: &str, rule: &str, field: FieldRef<'_>, args: Option<&Map<String, Value>>\| -> String` |
//! | `{ ...field, name }` | [`field.with_name(name)`](FieldRef::with_name) |
//! | **Around Vine** | |
//! | `convertEmptyStringsToNull: true` in `config/bodyparser.ts` | [`vine::empty_strings_to_null(&mut body)`](empty_strings_to_null) |
//! | `value.trim()`, `value.length`, `Number(value)`, `String(value)`, `a === b` | [`js::trim`], [`js::utf16_len`], [`js::to_number`], [`js::to_string`], [`js::strict_equals`] |
//!
//! Every method returns the schema it was called on, of the same type, so a
//! chain reads as in TypeScript whatever its order. Schemas are plain
//! values: cloning one is cheap, and where the TypeScript reuses a constant
//! (`headerName.optional()`), the Rust clones it (`header_name.clone().optional()`).
//!
//! Not ported, because no validator of the app uses them: unions
//! (`vine.union`, `vine.unionOfTypes`, `vine.group`), tuples,
//! `vine.accepted()`, `vine.nativeFile()`, `toCamelCase()`, the formats of
//! `vine.date()` other than `iso8601` and its comparison rules, and the
//! string, number, array and record rules that are not in the table.
//!
//! # Values
//!
//! JavaScript has two ways to say that a value is not there, and Vine tells
//! them apart: a field that is `nullable()` and not `optional()` takes
//! `null` and refuses `undefined`. `serde_json::Value` only has `null`, so
//! wherever a value may be missing this crate says `Option<Value>` or
//! `Option<&Value>`, where `None` is `undefined` and `Some(Value::Null)` is
//! `null`:
//!
//! - a validator takes `&Value`, `Option<&Value>` or `&Option<Value>`;
//! - a `parse` callback takes and returns `Option<Value>`;
//! - `field.parent_get(key)` and `field.data()` return `Option<&Value>`.
//!
//! A rule receives a `&Value`. Only an implicit rule can receive a value
//! that is not there, and it asks `field.is_undefined()` or
//! `field.is_null()` which of the two it is.
//!
//! The output is what Vine returns once written as JSON: the properties the
//! schema names, in the order it names them; nothing for a property that
//! was left out; numbers as numbers, whole ones without a fraction (`"5"`
//! read by `vine::number()` is `5`, never `5.0`). An output of `undefined`,
//! which only an optional root gives, is `Value::Null` from
//! [`Validator::validate`] and `None` from [`Validator::validate_opt`].
//!
//! `NaN` and the infinities cannot be written in a `serde_json::Value`: the
//! cases Vine has for them have no counterpart here.
//!
//! # Empty strings
//!
//! The TypeScript app turns empty strings into `null` at two places, and a
//! port of an HTTP handler needs both:
//!
//! 1. the body parser, for every string of a JSON or form body that is
//!    exactly `""`, at any depth: [`empty_strings_to_null`];
//! 2. Vine, when `convertEmptyStringsToNull` is on, for every field a
//!    schema names whose value is a string made of whitespace only. This
//!    also applies to what a rule mutates a value to. [`global()`] has it
//!    on, [`Vine::new()`] has it off.
//!
//! Validators that judge a value as it was sent (OAuth parameters, tool
//! arguments, stored JSON) are created by an instance that has it off:
//! `const verbatim = new Vine()` and `toolVine` in TypeScript,
//! `static VERBATIM: LazyLock<Vine> = LazyLock::new(Vine::new);` here.
//!
//! # Rules
//!
//! For each field Vine reads the value (through `parse` when there is
//! one), reports `required` when it is missing and the schema is not
//! optional, checks its type, then runs its rules in the order they were
//! added. With `bail` on, which is the default, the rules stop at the
//! first that reports: one error per field. The other fields are validated
//! all the same, so `error.messages[0]` is about the first field of the
//! schema that is wrong, which is what agents are told.
//!
//! A rule is skipped when the value is `null` or missing, unless it is
//! implicit. See [`Rule`] for how one is written.
//!
//! # Metadata
//!
//! `validator.validate(data, { meta })` makes `meta` available to rules as
//! `field.meta` and to `parse` callbacks as `context.meta`. Here it is a
//! `&dyn Any`: [`Validator::validate_with`] takes a reference to any value
//! and a rule gets it back with `field.meta::<T>()`, which is `None` when
//! no metadata was given or when it is not a `T`.
//!
//! It is not a `serde_json::Value` because the metadata of the app is the
//! context a tool runs with: a struct that holds the account's addresses
//! next to credentials and clients. Passing a reference costs nothing,
//! keeps the secrets out of a JSON tree, and gives rules typed fields. A
//! caller that has JSON can still pass a `&Value` and read it back with
//! `field.meta::<Value>()`.
//!
//! `T` is the type the reference points to. Passing `&context` when
//! `context` is itself a `&Context` hands the rules a `&&Context`, which
//! is not a `Context`.
//!
//! # Checks that wait
//!
//! Rules are synchronous. The one asynchronous rule of the app is Lucid's
//! `unique`, which queries the users table. It is declared with
//! [`lucid::unique()`], which holds its place among the rules, and the
//! caller makes the query between [`Validator::start`] and
//! [`Run::finish`]: see [`Rule::deferred`]. The errors come out in the
//! order Vine reports them, the one of the query among the others.
//!
//! For a check that has no place in a schema, such as the ones
//! `mcps_controller.ts` makes after validation, build the error by hand:
//! [`ValidationError::single`], [`ValidationError::push`],
//! [`ValidationError::merge`].
//!
//! # Regular expressions
//!
//! `.regex()` takes a [`Pattern`]. Make it from the JavaScript expression
//! with [`js::regex(source, flags)`](js::regex), which translates it for
//! the `regex` crate with JavaScript's meaning: `\d` is `[0-9]` and not
//! every digit of Unicode, `\w` is `[A-Za-z0-9_]`, `\s` is JavaScript's
//! whitespace, `/i` does not fold `K` (the Kelvin sign) into `k`. An
//! identifier checked with `/^\d{1,19}$/` ends up in a request path: write
//! rules ported from TypeScript with [`js::regex`] too, not with
//! `Regex::new`.
//!
//! The `regex` crate has no lookahead, lookbehind or backreferences:
//! [`js::regex`] returns an error for them, and the check is then written
//! by hand with [`Pattern::from_fn`]. `/^(?!__).+?__/s` in
//! `app/validators/gateway.ts` is the one such expression.
//!
//! # JSON Schema
//!
//! [`Validator::to_json_schema`] gives what Vine's `toJSONSchema()` gives:
//! `type`, `properties`, `required` and `additionalProperties` for an
//! object, `items` for an array, and what each rule adds (`minLength`,
//! `maximum`, `enum`, `pattern`, ...). A custom rule adds its part with
//! [`Rule::json_schema`]. As in Vine, the rules added after `.optional()`
//! are not described, and a property is required unless it is optional or
//! nullable.
//!
//! # An HTML form: `app/validators/user.ts`
//!
//! ```
//! use std::sync::LazyLock;
//!
//! use mymcps_vine as vine;
//! use serde::Deserialize;
//! use serde_json::json;
//! use vine::{Rule, Validator, VineString};
//!
//! # fn is_valid_five_field_cron(expression: &str) -> bool { expression.split(' ').count() == 5 }
//! // const email = () => vine.string().email().maxLength(254)
//! // const password = () => vine.string().minLength(8).maxLength(32)
//! fn email() -> VineString {
//!     vine::string().email().max_length(254)
//! }
//!
//! fn password() -> VineString {
//!     vine::string().min_length(8).max_length(32)
//! }
//!
//! // const fiveFieldCron = vine.createRule((value, _options, field) => {
//! //   if (typeof value !== 'string') return
//! //   if (!isValidFiveFieldCron(value)) {
//! //     field.report('The {{ field }} field must be a valid 5-field cron expression', 'cron', field)
//! //   }
//! // })
//! fn five_field_cron() -> Rule {
//!     vine::rule(|value, field| {
//!         let Some(expression) = value.as_str() else { return };
//!         if !is_valid_five_field_cron(expression) {
//!             field.report("The {{ field }} field must be a valid 5-field cron expression", "cron");
//!         }
//!     })
//! }
//!
//! // export const updateMcpLoggingValidator = vine.create({ ... })
//! static UPDATE_MCP_LOGGING: LazyLock<Validator> = LazyLock::new(|| {
//!     vine::global().create(vine::object! {
//!         "gatewayToolMode" => vine::enum_(["eager", "lazy"]),
//!         "mcpLogLevel" => vine::enum_(["off", "metadata", "arguments", "responses"]),
//!         "mcpLogRetentionDays" => vine::number().without_decimals().min(1).max(365),
//!         // Switch submits "on" when checked; omitted when unchecked.
//!         "mcpAutoUpdateEnabled" => vine::boolean().optional(),
//!         "mcpAutoUpdateCron" => vine::string().trim().max_length(64).optional().use_rule(five_field_cron()),
//!     })
//! });
//!
//! // export const onboardingValidator = vine.create({ ... })
//! static ONBOARDING: LazyLock<Validator> = LazyLock::new(|| {
//!     vine::global().create(vine::object! {
//!         "fullName" => vine::string().trim().min_length(1).max_length(120),
//!         // email().unique({ table: 'users', column: 'email' })
//!         "email" => email().use_rule(vine::lucid::unique()),
//!         "password" => password().confirmed("passwordConfirmation"),
//!         "passwordConfirmation" => vine::string(),
//!     })
//! });
//!
//! // type Payload = Infer<typeof updateMcpLoggingValidator>
//! #[derive(Debug, Deserialize)]
//! #[serde(rename_all = "camelCase")]
//! struct McpLogging {
//!     gateway_tool_mode: String,
//!     mcp_log_retention_days: u32,
//!     mcp_auto_update_enabled: Option<bool>,
//!     mcp_auto_update_cron: Option<String>,
//! }
//!
//! // const payload = await request.validateUsing(updateMcpLoggingValidator)
//! let mut body = json!({
//!     "gatewayToolMode": "lazy",
//!     "mcpLogLevel": "metadata",
//!     "mcpLogRetentionDays": "30",
//!     "mcpAutoUpdateEnabled": "on",
//!     "mcpAutoUpdateCron": "",
//! });
//! vine::empty_strings_to_null(&mut body);
//! let payload: McpLogging = UPDATE_MCP_LOGGING.validate_as(&body).unwrap();
//! assert_eq!(payload.gateway_tool_mode, "lazy");
//! assert_eq!(payload.mcp_log_retention_days, 30);
//! assert_eq!(payload.mcp_auto_update_enabled, Some(true));
//! assert_eq!(payload.mcp_auto_update_cron, None);
//!
//! // A form that is wrong: one message per field, in the order of the schema.
//! let body = json!({
//!     "fullName": "  ",
//!     "email": "ada@example.com",
//!     "password": "short",
//!     "passwordConfirmation": "short",
//! });
//! let mut run = ONBOARDING.start(&body);
//! for check in run.take_checks() {
//!     // `SELECT 1 FROM users WHERE email = ?` with `check.value`, awaited.
//!     let taken = check.rule == "database.unique" && check.value == "ada@example.com";
//!     if taken {
//!         run.reject(&check);
//!     }
//! }
//! let error = run.finish().unwrap_err();
//! let messages: Vec<(&str, &str)> =
//!     error.messages.iter().map(|one| (one.field.as_str(), one.message.as_str())).collect();
//! assert_eq!(
//!     messages,
//!     [
//!         ("fullName", "The fullName field must be defined"),
//!         ("email", "The email has already been taken"),
//!         ("password", "The password field must have at least 8 characters"),
//!     ]
//! );
//! ```
//!
//! # Tool arguments, with metadata: `app/validators/builtin_tools.ts`
//!
//! The tools of built-in MCPs have a Vine of their own (`toolVine`), a
//! messages provider that names the argument (`ArgumentMessages`), and a
//! type made of rules (`VineArgument`). `toolInput` passes the context of
//! the call as metadata.
//!
//! ```
//! use std::any::Any;
//! use std::sync::LazyLock;
//!
//! use mymcps_vine as vine;
//! use serde_json::{Map, Value, json};
//! use vine::{FieldRef, MessagesProvider, Rule, SimpleMessagesProvider, Validator, Vine, VineCustom};
//!
//! // class ArgumentMessages extends SimpleMessagesProvider {
//! //   getMessage(message, rule, field, args) {
//! //     const [argument] = field.wildCardPath.split('.')
//! //     const choices = args?.choices
//! //     return super.getMessage(message, rule, { ...field, name: argument || 'arguments' },
//! //       Array.isArray(choices) ? { ...args, choices: choices.join(', ') } : args)
//! //   }
//! // }
//! struct ArgumentMessages(SimpleMessagesProvider);
//!
//! impl MessagesProvider for ArgumentMessages {
//!     fn get_message(
//!         &self,
//!         message: &str,
//!         rule: &str,
//!         field: FieldRef<'_>,
//!         args: Option<&Map<String, Value>>,
//!     ) -> String {
//!         let argument = field.wildcard_path.split('.').next().unwrap_or_default();
//!         let field = field.with_name(if argument.is_empty() { "arguments" } else { argument });
//!         match args.and_then(|args| args.get("choices")).and_then(Value::as_array) {
//!             Some(choices) => {
//!                 let choices: Vec<String> = choices.iter().map(vine::js::to_string).collect();
//!                 let mut args = args.cloned().unwrap_or_default();
//!                 args.insert("choices".to_owned(), Value::String(choices.join(", ")));
//!                 self.0.get_message(message, rule, field, Some(&args))
//!             }
//!             None => self.0.get_message(message, rule, field, args),
//!         }
//!     }
//! }
//!
//! // export const toolVine = new Vine()
//! // toolVine.messagesProvider = new ArgumentMessages({ ... })
//! static TOOL_VINE: LazyLock<Vine> = LazyLock::new(|| {
//!     Vine::new().messages_provider(ArgumentMessages(SimpleMessagesProvider::new([
//!         ("required", "{{ field }} is required"),
//!         ("object", "{{ field }} must be an object"),
//!         ("boolean", "{{ field }} must be true or false"),
//!         ("enum", "{{ field }} must be one of: {{ choices }}"),
//!     ])))
//! });
//!
//! // export function blankAsMissing(value: unknown) {
//! //   return value === '' ? undefined : value
//! // }
//! fn blank_as_missing(value: Option<Value>) -> Option<Value> {
//!     value.filter(|value| value.as_str() != Some(""))
//! }
//!
//! // const textRule = toolVine.createRule<{ max: number }>(
//! //   (value, { max }, field) => {
//! //     if (typeof value !== 'string' || value.length > max) {
//! //       field.report('{{ field }} must be text of at most {{ max }} characters', 'text', field, { max })
//! //     }
//! //   },
//! //   { toJSONSchema: (schema, { max }) => { Object.assign(schema, { type: 'string', maxLength: max }) } }
//! // )
//! fn text_rule(max: usize) -> Rule {
//!     vine::rule(move |value, field| {
//!         if !value.as_str().is_some_and(|text| vine::js::utf16_len(text) <= max) {
//!             field.report_with(
//!                 "{{ field }} must be text of at most {{ max }} characters",
//!                 "text",
//!                 json!({ "max": max }),
//!             );
//!         }
//!     })
//!     .json_schema(move |schema| {
//!         schema.insert("type".to_owned(), json!("string"));
//!         schema.insert("maxLength".to_owned(), json!(max));
//!     })
//! }
//!
//! // export function text(max: number) {
//! //   return new VineArgument<string>(textRule({ max }))
//! // }
//! fn text(max: usize) -> VineCustom {
//!     vine::custom([text_rule(max)])
//! }
//!
//! // export function trimmedText(max: number) {
//! //   return text(max).parse((value) =>
//! //     typeof value === 'string' && value.length <= max ? value.trim() || undefined : value)
//! // }
//! fn trimmed_text(max: usize) -> VineCustom {
//!     text(max).parse(move |value, _| match value {
//!         Some(Value::String(text)) if vine::js::utf16_len(&text) <= max => {
//!             let trimmed = vine::js::trim(&text);
//!             (!trimmed.is_empty()).then(|| Value::String(trimmed.to_owned()))
//!         }
//!         other => other,
//!     })
//! }
//!
//! /// The sign-in a mail tool runs for: `BuiltinPasswordContext`.
//! struct MailContext {
//!     username: String,
//!     aliases: Vec<String>,
//! }
//!
//! // const ownAddressRule = toolVine.createRule((value, _options, field) => {
//! //   const { username, aliases } = field.meta as Senders
//! //   const allowed = [username, ...aliases]
//! //   const saved = allowed.find((candidate) => candidate.toLowerCase() === value.toLowerCase())
//! //   if (!saved) {
//! //     field.report('{{ field }} must be one of the sender addresses allowed for this MCP: {{ allowed }}',
//! //       'ownAddress', field, { allowed: allowed.join(', ') })
//! //     return
//! //   }
//! //   field.mutate(saved, field)
//! // })
//! fn own_address_rule() -> Rule {
//!     vine::rule(|value, field| {
//!         let allowed: Vec<&str> = field.meta::<MailContext>().map_or_else(Vec::new, |context| {
//!             std::iter::once(&context.username).chain(&context.aliases).map(String::as_str).collect()
//!         });
//!         let written = value.as_str().unwrap_or_default().to_lowercase();
//!         match allowed.iter().find(|candidate| candidate.to_lowercase() == written) {
//!             Some(saved) => field.mutate(*saved),
//!             None => field.report_with(
//!                 "{{ field }} must be one of the sender addresses allowed for this MCP: {{ allowed }}",
//!                 "ownAddress",
//!                 json!({ "allowed": allowed.join(", ") }),
//!             ),
//!         }
//!     })
//! }
//!
//! // export const compositionValidator = toolVine.withMetaData<Senders>().create({ ... })
//! static COMPOSITION: LazyLock<Validator> = LazyLock::new(|| {
//!     TOOL_VINE.create(vine::object! {
//!         "subject" => trimmed_text(255),
//!         "from" => trimmed_text(254).use_rule(own_address_rule()).optional(),
//!         "text" => text(100_000).parse(|value, _| blank_as_missing(value)),
//!         "format" => vine::enum_(["plain", "html"]).parse(|value, _| blank_as_missing(value)).optional(),
//!     })
//! });
//!
//! // export async function toolInput(validator, args, context) {
//! //   const [error, input] = await validator.tryValidate(args, { meta: context })
//! //   if (error) throw new BuiltinToolError(error.messages[0].message)
//! //   return input
//! // }
//! fn tool_input(validator: &Validator, args: &Value, context: &dyn Any) -> Result<Value, String> {
//!     validator.validate_with(args, context).map_err(|error| error.messages[0].message.clone())
//! }
//!
//! let context = MailContext {
//!     username: "Ada@icloud.com".to_owned(),
//!     aliases: vec!["ada@example.com".to_owned()],
//! };
//!
//! let args = json!({ "subject": " Hello ", "from": "ADA@ICLOUD.COM", "text": "", "extra": 1 });
//! assert_eq!(tool_input(&COMPOSITION, &args, &context), Err("text is required".to_owned()));
//!
//! let args = json!({ "subject": " Hello ", "from": "ADA@ICLOUD.COM", "text": " Hi. ", "format": "" });
//! assert_eq!(
//!     tool_input(&COMPOSITION, &args, &context),
//!     Ok(json!({ "subject": "Hello", "from": "Ada@icloud.com", "text": " Hi. " }))
//! );
//!
//! let args = json!({ "subject": "Hello", "from": "eve@example.com", "text": "Hi.", "format": "rtf" });
//! assert_eq!(
//!     tool_input(&COMPOSITION, &args, &context),
//!     Err("from must be one of the sender addresses allowed for this MCP: Ada@icloud.com, ada@example.com".to_owned())
//! );
//! let args = json!({ "subject": "Hello", "text": "Hi.", "format": "rtf" });
//! assert_eq!(tool_input(&COMPOSITION, &args, &context), Err("format must be one of: plain, html".to_owned()));
//! assert_eq!(tool_input(&COMPOSITION, &json!([]), &context), Err("arguments must be an object".to_owned()));
//!
//! // What the tool advertises can be compared with what its validator enforces.
//! assert_eq!(
//!     COMPOSITION.to_json_schema(),
//!     &json!({
//!         "type": "object",
//!         "properties": {
//!             "subject": { "type": "string", "maxLength": 255 },
//!             "from": { "type": "string", "maxLength": 254 },
//!             "text": { "type": "string", "maxLength": 100000 },
//!             "format": { "enum": ["plain", "html"] },
//!         },
//!         "required": ["subject", "text"],
//!         "additionalProperties": false,
//!     })
//! );
//! ```
//!
//! # Where the port differs from Vine
//!
//! - Dates: the server's time zone is taken to be UTC, and the strings only
//!   V8's fallback date parser understands are refused. See
//!   [`date_iso8601`].
//! - Regular expressions without the `u` flag match whole characters where
//!   JavaScript matches UTF-16 code units. See [`JsRegex`].
//! - Two objects or two arrays are never `===`, since values read from JSON
//!   are never the same instance: an `enum`, a `literal` or a
//!   `requiredWhen` comparing against one does not match it, as in Vine
//!   for values that come from a request.
//! - A rule cannot throw. Where a rule of Vine would throw a `TypeError` on
//!   a value of the wrong type, the rule here does nothing.

mod date;
mod error;
mod field;
pub mod helpers;
pub mod js;
mod js_regex;
mod messages;
mod rule;
mod schema;
mod validator;

pub use error::{Error, FieldError, ValidationError};
pub use field::{FieldContext, ParseContext, PendingCheck};
pub use helpers::UrlOptions;
pub use js_regex::{JsRegex, JsRegexError};
pub use messages::{
    DEFAULT_FIELDS, DEFAULT_MESSAGES, FieldRef, MessagesProvider, SimpleMessagesProvider,
    interpolate,
};
pub use rule::{Rule, rule};
pub use schema::{
    Operator, Parser, Pattern, Schema, VineAny, VineArray, VineBoolean, VineCustom, VineDate,
    VineEnum, VineLiteral, VineNumber, VineObject, VineRecord, VineString, any, array, boolean,
    custom, date_iso8601, enum_, literal, number, object, record, string,
};
pub use validator::{Input, Run, Validator, Vine, empty_strings_to_null, global, lucid};
