//! The form that creates or edits an MCP, and the two dialogs around it.
//! (`inertia/components/mcp_form_fields.tsx`, `oauth_callback_paste.tsx`,
//! and the dialogs of `inertia/pages/mcps/index.tsx`)

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use maud::{Markup, html};
use mymcps_core::models::{McpAuthType, McpStatus, McpTransport};
use mymcps_vine::ValidationError;
use serde_json::{Map, Value};
use url::Url;

use crate::mcp_templates::McpTemplate;
use crate::validators::mcp::McpListQuery;
use crate::views::icon::icon;
use crate::views::mcps::builtin::{BuiltinFields, builtin_choices, builtin_fields};
use crate::views::mcps::{McpView, PublicApp, builtin_provider_name, builtin_setup_guide, href};
use crate::views::shell::PageContext;

/// The most environment variables an npm MCP takes.
const MAX_ENVIRONMENT_VARIABLES: usize = 50;

/// One row of the environment editor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvRow {
    pub name: String,
    /// What was typed. A saved value is never put here.
    pub value: String,
}

/// What the fields of the form hold. Secrets are only ever what the person
/// typed in the request being answered: a saved one is never read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormValues {
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub http_url: String,
    pub npm_package: String,
    pub npm_version: String,
    pub npm_args: String,
    pub npm_env: Vec<EnvRow>,
    pub builtin_key: String,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub builtin_username: String,
    pub builtin_password: String,
    pub builtin_aliases: String,
    pub builtin_permissions: Vec<String>,
    /// What a built-in MCP needs beyond its sign-in.
    pub builtin_settings: BTreeMap<String, String>,
    pub builtin_write_enabled: bool,
    pub auth_type: McpAuthType,
    pub auth_bearer: String,
    pub auth_header_name: String,
    pub auth_header_value: String,
    pub enabled: bool,
}

impl Default for FormValues {
    /// A new form: HTTP, automatic authentication, enabled.
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            transport: McpTransport::Http,
            http_url: String::new(),
            npm_package: String::new(),
            npm_version: String::new(),
            npm_args: String::new(),
            npm_env: Vec::new(),
            builtin_key: String::new(),
            oauth_client_id: String::new(),
            oauth_client_secret: String::new(),
            builtin_username: String::new(),
            builtin_password: String::new(),
            builtin_aliases: String::new(),
            builtin_permissions: Vec::new(),
            builtin_settings: BTreeMap::new(),
            builtin_write_enabled: false,
            auth_type: McpAuthType::Auto,
            auth_bearer: String::new(),
            auth_header_name: String::new(),
            auth_header_value: String::new(),
            enabled: true,
        }
    }
}

fn text(input: &Map<String, Value>, field: &str) -> String {
    match input.get(field) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// What a checkbox of the form sent.
fn checked(input: &Map<String, Value>, field: &str) -> bool {
    match input.get(field) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => matches!(text.as_str(), "on" | "true" | "1"),
        Some(Value::Number(number)) => number.as_i64() == Some(1),
        _ => false,
    }
}

impl FormValues {
    /// The form a template of the gallery prefills.
    pub fn from_template(template: &McpTemplate) -> Self {
        let values = &template.values;
        Self {
            name: values.name.to_string(),
            description: values.description.to_string(),
            transport: values.transport,
            http_url: values.http_url.unwrap_or_default().to_string(),
            npm_package: values.npm_package.unwrap_or_default().to_string(),
            npm_version: values.npm_version.unwrap_or_default().to_string(),
            npm_args: values.npm_args.unwrap_or_default().to_string(),
            npm_env: values
                .npm_env
                .iter()
                .map(|name| EnvRow {
                    name: name.to_string(),
                    value: String::new(),
                })
                .collect(),
            builtin_key: values.builtin_key.unwrap_or_default().to_string(),
            builtin_permissions: values
                .builtin_permissions
                .iter()
                .map(|permission| permission.to_string())
                .collect(),
            auth_type: values.auth_type,
            ..Self::default()
        }
    }

    /// The form of a saved MCP. Its secrets stay where they are.
    pub fn from_mcp(mcp: &McpView) -> Self {
        Self {
            name: mcp.name.clone(),
            description: mcp.description.clone().unwrap_or_default(),
            transport: mcp.transport,
            http_url: mcp.http_url.clone().unwrap_or_default(),
            npm_package: mcp.npm_package.clone().unwrap_or_default(),
            npm_version: mcp.npm_version.clone().unwrap_or_default(),
            npm_args: mcp.npm_args.clone(),
            npm_env: mcp
                .npm_env
                .iter()
                .map(|name| EnvRow {
                    name: name.clone(),
                    value: String::new(),
                })
                .collect(),
            builtin_key: mcp.builtin_key.clone().unwrap_or_default(),
            oauth_client_id: mcp.oauth_client_id.clone().unwrap_or_default(),
            builtin_username: mcp.builtin_username.clone().unwrap_or_default(),
            builtin_aliases: mcp.builtin_aliases.join(", "),
            builtin_permissions: mcp.builtin_permissions.clone(),
            builtin_settings: mcp.builtin_settings.clone(),
            builtin_write_enabled: mcp.builtin_write_enabled,
            auth_type: mcp.auth_type,
            auth_header_name: mcp.auth_header_name.clone().unwrap_or_default(),
            enabled: mcp.enabled,
            ..Self::default()
        }
    }

    /// The form as it was submitted, to draw it again when it is refused:
    /// nothing has to be typed twice.
    pub fn from_input(input: &Map<String, Value>) -> Self {
        let npm_env = match input.get("npmEnv") {
            Some(Value::Array(rows)) => rows
                .iter()
                .filter_map(Value::as_object)
                .map(|row| EnvRow {
                    name: text(row, "name"),
                    value: text(row, "value"),
                })
                .collect(),
            _ => Vec::new(),
        };
        let builtin_permissions = match input.get("builtinPermissions") {
            Some(Value::String(permission)) => vec![permission.clone()],
            Some(Value::Array(permissions)) => permissions
                .iter()
                .filter_map(|permission| permission.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let builtin_settings = match input.get("builtinSettings") {
            Some(Value::Object(settings)) => settings
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect(),
            _ => BTreeMap::new(),
        };
        Self {
            name: text(input, "name"),
            description: text(input, "description"),
            transport: McpTransport::parse(&text(input, "transport")).unwrap_or_default(),
            http_url: text(input, "httpUrl"),
            npm_package: text(input, "npmPackage"),
            npm_version: text(input, "npmVersion"),
            npm_args: text(input, "npmArgs"),
            npm_env,
            builtin_key: text(input, "builtinKey"),
            oauth_client_id: text(input, "oauthClientId"),
            oauth_client_secret: text(input, "oauthClientSecret"),
            builtin_username: text(input, "builtinUsername"),
            builtin_password: text(input, "builtinPassword"),
            builtin_aliases: text(input, "builtinAliases"),
            builtin_permissions,
            builtin_settings,
            builtin_write_enabled: checked(input, "builtinWriteEnabled"),
            auth_type: McpAuthType::parse(&text(input, "authType")).unwrap_or_default(),
            auth_bearer: text(input, "authBearer"),
            auth_header_name: text(input, "authHeaderName"),
            auth_header_value: text(input, "authHeaderValue"),
            enabled: checked(input, "enabled"),
        }
    }
}

/// The state the form is drawn in: its values, and what was refused.
#[derive(Debug, Clone, Default)]
pub struct McpForm {
    pub values: FormValues,
    /// The first message of each field that was refused, in field order.
    errors: Vec<(String, String)>,
    /// The fields whose message a control has shown. The others are shown
    /// above the form, so that no refusal goes unsaid.
    shown: RefCell<HashSet<String>>,
}

impl McpForm {
    pub fn new(values: FormValues) -> Self {
        Self {
            values,
            ..Self::default()
        }
    }

    /// A form sent back with what is wrong with it.
    pub fn refused(values: FormValues, error: &ValidationError) -> Self {
        let mut errors: Vec<(String, String)> = Vec::new();
        for refusal in &error.messages {
            if !errors.iter().any(|(field, _)| *field == refusal.field) {
                errors.push((refusal.field.clone(), refusal.message.clone()));
            }
        }
        Self {
            values,
            errors,
            shown: RefCell::default(),
        }
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// The message of a field, for the control that shows it.
    pub fn error(&self, field: &str) -> Option<&str> {
        let message = self
            .errors
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, message)| message.as_str())?;
        self.shown.borrow_mut().insert(field.to_string());
        Some(message)
    }

    pub fn first_error(&self) -> Option<&str> {
        self.errors.first().map(|(_, message)| message.as_str())
    }

    /// What no control has shown yet, by field.
    fn unshown_errors(&self) -> Vec<(&str, &str)> {
        let shown = self.shown.borrow();
        self.errors
            .iter()
            .filter(|(field, _)| !shown.contains(field))
            .map(|(field, message)| (field.as_str(), message.as_str()))
            .collect()
    }
}

fn url_origin(value: &str) -> Option<String> {
    Url::parse(value.trim())
        .ok()
        .map(|url| url.origin().ascii_serialization())
}

/// Whether saving these values keeps the saved token, header value and
/// environment values. The server drops them when the MCP moves to another
/// origin, transport or package, so the form must not offer to keep them.
pub fn keeps_saved_credentials(saved: &McpView, values: &FormValues) -> bool {
    if saved.transport != values.transport {
        return false;
    }
    match values.transport {
        McpTransport::Npm => {
            saved.npm_package.as_deref().unwrap_or_default() == values.npm_package.trim()
        }
        McpTransport::Http => {
            let saved_origin = saved.http_url.as_deref().and_then(url_origin);
            let next_origin = url_origin(&values.http_url);
            // A URL still being typed is not a decision yet.
            saved_origin.is_none() || next_origin.is_none() || saved_origin == next_origin
        }
        McpTransport::Builtin => true,
    }
}

/// The saved credentials of the MCP being edited, as the form talks about them.
struct Saved<'a> {
    mcp: &'a McpView,
    /// Whether the form, as it is drawn, keeps them.
    kept: bool,
    /// The same question as a condition of the page script, so the answer
    /// follows what is typed. `None` when it cannot be written as one.
    condition: Option<String>,
}

impl<'a> Saved<'a> {
    fn of(mcp: &'a McpView, values: &FormValues) -> Self {
        let condition = match mcp.transport {
            McpTransport::Http => Some(match mcp.http_url.as_deref().and_then(url_origin) {
                Some(origin) => format!("transport=http&httpUrl:origin={origin}"),
                None => "transport=http".to_string(),
            }),
            McpTransport::Npm => mcp
                .npm_package
                .as_deref()
                .filter(|package| is_condition_value(package))
                .map(|package| format!("transport=npm&npmPackage={package}")),
            McpTransport::Builtin => None,
        };
        Self {
            mcp,
            kept: keeps_saved_credentials(mcp, values),
            condition,
        }
    }
}

/// Whether a value can be compared by a condition of the page script, which
/// splits on these characters.
fn is_condition_value(value: &str) -> bool {
    !value.is_empty() && !value.contains(['&', '|']) && value.trim() == value
}

pub(super) fn optional_mark() -> Markup {
    html! { span class="field__optional" { "Optional" } }
}

fn banner_icon(tone: &str) -> &'static str {
    match tone {
        "warning" => "triangle-alert",
        "critical" => "circle-x",
        "success" => "circle-check",
        _ => "info",
    }
}

/// A banner of a dialog. `action` goes at its end.
pub(super) fn banner(tone: &str, title: &str, body: Markup, action: Markup) -> Markup {
    html! {
        div class=(format!("banner banner--{tone}")) role=(if tone == "critical" { "alert" } else { "status" }) {
            (icon(banner_icon(tone)))
            div class="banner__content" {
                p class="banner__title" { (title) }
                (body)
            }
            (action)
        }
    }
}

pub(super) fn choice(
    name: &str,
    value: Option<&str>,
    checked: bool,
    label: &str,
    description: Option<&str>,
) -> Markup {
    html! {
        label class="choice" {
            input type="checkbox" class="checkbox" name=(name) value=[value] checked[checked];
            span class="choice__text" {
                span class="choice__label" { (label) }
                @if let Some(description) = description { span class="choice__description" { (description) } }
            }
        }
    }
}

fn option_card(name: &str, value: &str, checked: bool, title: &str, description: &str) -> Markup {
    html! {
        label class="option-card" {
            input type="radio" class="radio" name=(name) value=(value) checked[checked];
            span class="option-card__text" {
                span class="option-card__title" { (title) }
                span class="option-card__description" { (description) }
            }
        }
    }
}

/// A text field of the form: its label, its control, then its error and its help line.
pub(super) struct Field<'a> {
    id: String,
    name: String,
    label: Markup,
    kind: &'static str,
    value: &'a str,
    placeholder: Option<&'a str>,
    help: Option<Markup>,
    /// Conditions of the page script that show or hide the help line.
    help_show_when: Option<String>,
    help_hide_when: Option<String>,
    help_hidden: bool,
    error: Option<&'a str>,
    inputmode: Option<&'static str>,
    verbatim: bool,
    autofocus: bool,
    required: bool,
    optional_when: Option<String>,
    /// A condition of the page script the whole field is shown under, and
    /// whether it holds as the form is drawn.
    show_when: Option<(&'static str, bool)>,
}

impl<'a> Field<'a> {
    pub(super) fn new(id: String, name: &str, label: Markup) -> Self {
        Self {
            id,
            name: name.to_string(),
            label,
            kind: "text",
            value: "",
            placeholder: None,
            help: None,
            help_show_when: None,
            help_hide_when: None,
            help_hidden: false,
            error: None,
            inputmode: None,
            verbatim: false,
            autofocus: false,
            required: false,
            optional_when: None,
            show_when: None,
        }
    }

    pub(super) fn password(mut self) -> Self {
        self.kind = "password";
        self
    }

    pub(super) fn value(mut self, value: &'a str) -> Self {
        self.value = value;
        self
    }

    pub(super) fn placeholder(self, placeholder: &'a str) -> Self {
        self.placeholder_opt(Some(placeholder))
    }

    pub(super) fn placeholder_opt(mut self, placeholder: Option<&'a str>) -> Self {
        self.placeholder = placeholder;
        self
    }

    pub(super) fn help(mut self, help: Markup) -> Self {
        self.help = Some(help);
        self
    }

    /// A help line the page script hides while `condition` holds.
    fn help_unless(mut self, help: Markup, condition: Option<&str>, hidden: bool) -> Self {
        self.help = Some(help);
        self.help_hide_when = condition.map(str::to_string);
        self.help_hidden = hidden;
        self
    }

    /// A help line the page script shows while `condition` holds.
    fn help_while(mut self, help: Markup, condition: Option<&str>, hidden: bool) -> Self {
        self.help = Some(help);
        self.help_show_when = condition.map(str::to_string);
        self.help_hidden = hidden;
        self
    }

    pub(super) fn error(mut self, error: Option<&'a str>) -> Self {
        self.error = error;
        self
    }

    pub(super) fn inputmode(mut self, inputmode: &'static str) -> Self {
        self.inputmode = Some(inputmode);
        self
    }

    /// An identifier, typed as it is: no capital added, no spelling checked.
    pub(super) fn verbatim(mut self) -> Self {
        self.verbatim = true;
        self
    }

    fn autofocus(mut self, autofocus: bool) -> Self {
        self.autofocus = autofocus;
        self
    }

    /// Required, unless `optional_when` holds for the page script.
    fn required(mut self, required: bool, optional_when: Option<&str>) -> Self {
        self.required = required;
        self.optional_when = optional_when.map(str::to_string);
        self
    }

    /// Shown only while `condition` holds for the page script.
    fn show_when(mut self, condition: &'static str, holds: bool) -> Self {
        self.show_when = Some((condition, holds));
        self
    }

    pub(super) fn render(self) -> Markup {
        let error_id = format!("{}-error", self.id);
        let help_id = format!("{}-help", self.id);
        // The error is named first: it is what has to be read.
        let described_by = match (self.error.is_some(), self.help.is_some()) {
            (true, true) => Some(format!("{error_id} {help_id}")),
            (true, false) => Some(error_id.clone()),
            (false, true) => Some(help_id.clone()),
            (false, false) => None,
        };
        html! {
            div class="field" data-show-when=[self.show_when.map(|(condition, _)| condition)]
                hidden[self.show_when.is_some_and(|(_, holds)| !holds)] {
                label class="field__label" for=(self.id) { (self.label) }
                input class="input" id=(self.id) name=(self.name) type=(self.kind) value=[(!self.value.is_empty()).then_some(self.value)]
                    inputmode=[self.inputmode] placeholder=[self.placeholder] autocomplete="off"
                    autocapitalize=[self.verbatim.then_some("off")] spellcheck=[self.verbatim.then_some("false")]
                    autofocus[self.autofocus] required[self.required] data-optional-when=[self.optional_when]
                    aria-invalid=[self.error.map(|_| "true")] aria-describedby=[described_by];
                @if let Some(message) = self.error { p class="field__error" id=(error_id) { (message) } }
                @if let Some(help) = self.help {
                    p class="field__help" id=(help_id) data-show-when=[self.help_show_when] data-hide-when=[self.help_hide_when]
                        hidden[self.help_hidden] { (help) }
                }
            }
        }
    }
}

/// The label of a secret the MCP may already hold: it says so while the
/// form keeps the saved one.
fn secret_label(label: &str, saved: Option<&Saved>, has_saved: bool) -> Markup {
    let Some(saved) = saved.filter(|_| has_saved) else {
        return html! { (label) };
    };
    html! {
        (label)
        @match &saved.condition {
            Some(condition) => {
                span data-show-when=(condition) hidden[!saved.kept] { " (leave blank to keep)" }
                " "
                span class="field__optional" data-show-when=(condition) hidden[!saved.kept] { "Optional" }
            }
            None => {
                @if saved.kept { " (leave blank to keep) " (optional_mark()) }
            }
        }
    }
}

/// A secret field of the authentication. `moved` is what it says once the
/// form points the MCP at another server.
fn secret_field<'a>(
    field: Field<'a>,
    saved: Option<&Saved>,
    has_saved: bool,
    moved: &str,
) -> Field<'a> {
    match saved.filter(|_| has_saved) {
        Some(saved) => match &saved.condition {
            Some(condition) => field.help_unless(html! { (moved) }, Some(condition), saved.kept),
            None if saved.kept => field,
            None => field.help(html! { (moved) }),
        },
        None => field,
    }
}

/// One row of the environment editor. `index` is its place in the form, or
/// `__i__` in the template the page script copies.
fn env_row(
    prefix: &str,
    index: &str,
    row: &EnvRow,
    saved: Option<&Saved>,
    form: Option<(&McpForm, usize)>,
) -> Markup {
    let name_id = format!("{prefix}-env-{index}-name");
    let value_id = format!("{prefix}-env-{index}-value");
    let name_field = format!("npmEnv[{index}][name]");
    let (name_error, value_error) = match form {
        Some((form, position)) => (
            form.error(&format!("npmEnv.{position}.name")),
            form.error(&format!("npmEnv.{position}.value")),
        ),
        None => (None, None),
    };

    // A variable saved under this name keeps its value when the field is
    // left blank, as long as the MCP still runs the package it was entered for.
    let saved = saved.filter(|saved| saved.mcp.npm_env.contains(&row.name));
    let named = saved
        .filter(|_| is_condition_value(&row.name))
        .map(|_| format!("{name_field}={}", row.name));
    let package = saved.and_then(|saved| {
        saved
            .mcp
            .npm_package
            .as_deref()
            .filter(|package| is_condition_value(package))
    });
    let (keeps, again) = match (&named, package) {
        (Some(named), Some(package)) => (
            Some(format!("npmPackage={package}&{named}")),
            Some(format!("npmPackage!={package}")),
        ),
        _ => (None, None),
    };
    let kept = saved.is_some_and(|saved| saved.kept);

    let value_label = html! {
        "Value"
        @if saved.is_some() {
            @match &keeps {
                Some(keeps) => { " " span class="field__optional" data-show-when=(keeps) hidden[!kept] { "Optional" } }
                None => { @if kept { " " (optional_mark()) } }
            }
        }
    };
    let mut value = Field::new(value_id, &format!("npmEnv[{index}][value]"), value_label)
        .password()
        .value(&row.value)
        .error(value_error)
        .required(!kept, keeps.as_deref());
    if saved.is_some() {
        value = match (&named, &again, &keeps) {
            (Some(named), Some(again), Some(_)) => value.help_while(
                html! {
                    span data-hide-when=(again) hidden[!kept] { "Leave blank to keep the saved value" }
                    span data-show-when=(again) hidden[kept] { "Enter the value again for the new package" }
                },
                Some(named),
                false,
            ),
            _ => value.help(html! {
                @if kept { "Leave blank to keep the saved value" } @else { "Enter the value again for the new package" }
            }),
        };
    }

    let remove = if row.name.is_empty() {
        "Remove environment variable".to_string()
    } else {
        format!("Remove {}", row.name)
    };
    html! {
        div class="repeat-row" data-repeat-row {
            (Field::new(name_id, &name_field, html! { "Name" })
                .value(&row.name)
                .placeholder("API_KEY")
                .verbatim()
                .error(name_error)
                .render())
            (value.render())
            button type="button" class="button" data-repeat-remove aria-label=(remove) { "Remove" }
        }
    }
}

/// Transport, its fields and Authentication: what a custom MCP chooses.
fn custom_fields(prefix: &str, form: &McpForm, saved: Option<&Saved>) -> Markup {
    let values = &form.values;
    let http = values.transport == McpTransport::Http;
    let cached = saved.and_then(|saved| saved.mcp.npm_cached_version.as_deref());
    html! {
        fieldset class="field-group" {
            legend class="field-group__label" { "Transport" }
            div class="option-cards" {
                (option_card("transport", "http", http, "HTTP", "Remote Streamable HTTP URL"))
                (option_card("transport", "npm", !http, "npm package", "Runs in a Deno sandbox"))
            }
        }

        (Field::new(format!("{prefix}-http-url"), "httpUrl", html! { "HTTP URL" })
            .value(&values.http_url)
            .placeholder("https://example.com/mcp")
            .inputmode("url")
            .verbatim()
            .error(form.error("httpUrl"))
            .show_when("transport=http", http)
            .render())

        div class="stack" data-show-when="transport=npm" hidden[http] {
            (Field::new(format!("{prefix}-npm-package"), "npmPackage", html! { "npm package" })
                .value(&values.npm_package)
                .placeholder("@modelcontextprotocol/server-everything")
                .verbatim()
                .error(form.error("npmPackage"))
                .render())
            div class="grid grid--2" {
                @let version = Field::new(format!("{prefix}-npm-version"), "npmVersion", html! { "Version " (optional_mark()) })
                    .value(&values.npm_version)
                    .placeholder("latest")
                    .verbatim()
                    .error(form.error("npmVersion"));
                @match cached {
                    Some(cached) => { (version.help(html! { "Cached in Deno: " (cached) }).render()) }
                    None => { (version.render()) }
                }
                (Field::new(format!("{prefix}-npm-args"), "npmArgs", html! { "Extra args " (optional_mark()) })
                    .value(&values.npm_args)
                    .verbatim()
                    .error(form.error("npmArgs"))
                    .render())
            }
            fieldset class="field-group" data-repeat data-repeat-min="0" data-repeat-max=(MAX_ENVIRONMENT_VARIABLES) {
                legend class="field-group__label" { "Environment variables" }
                p class="field-group__help" { "Values are encrypted and only provided to this MCP process." }
                div class="repeat-list" data-repeat-list {
                    @for (index, row) in values.npm_env.iter().enumerate() {
                        (env_row(prefix, &index.to_string(), row, saved, Some((form, index))))
                    }
                }
                template data-repeat-template { (env_row(prefix, "__i__", &EnvRow::default(), None, None)) }
                div class="cluster" {
                    button type="button" class="button button--secondary" data-repeat-add { (icon("plus")) "Add variable" }
                }
            }
        }

        fieldset class="field-group" {
            legend class="field-group__label" { "Authentication" }
            div class="option-cards" {
                (option_card("authType", "auto", values.auth_type == McpAuthType::Auto, "Auto", "No credentials, or OAuth detected automatically"))
                (option_card("authType", "bearer", values.auth_type == McpAuthType::Bearer, "Bearer token", "Send a static token"))
                (option_card("authType", "header", values.auth_type == McpAuthType::Header, "Custom header", "Send a named header"))
            }
        }

        @let has_bearer = saved.is_some_and(|saved| saved.mcp.has_auth_bearer);
        (secret_field(
            Field::new(format!("{prefix}-auth-bearer"), "authBearer", secret_label("Bearer token", saved, has_bearer))
                .password()
                .value(&values.auth_bearer)
                .error(form.error("authBearer"))
                .show_when("authType=bearer", values.auth_type == McpAuthType::Bearer),
            saved,
            has_bearer,
            "The saved token is not sent to a different server. Enter the token for this one.",
        ).render())

        @let has_header_value = saved.is_some_and(|saved| saved.mcp.has_auth_header_value);
        div class="grid grid--2" data-show-when="authType=header" hidden[values.auth_type != McpAuthType::Header] {
            (Field::new(format!("{prefix}-auth-header-name"), "authHeaderName", html! { "Header name" })
                .value(&values.auth_header_name)
                .verbatim()
                .error(form.error("authHeaderName"))
                .render())
            (secret_field(
                Field::new(format!("{prefix}-auth-header-value"), "authHeaderValue", secret_label("Header value", saved, has_header_value))
                    .password()
                    .value(&values.auth_header_value)
                    .error(form.error("authHeaderValue")),
                saved,
                has_header_value,
                "The saved value is not sent to a different server. Enter the value for this one.",
            ).render())
        }
    }
}

/// Name and description, which every MCP has.
fn identity_fields(prefix: &str, form: &McpForm, autofocus: bool) -> Markup {
    html! {
        div class="grid grid--2" {
            (Field::new(format!("{prefix}-name"), "name", html! { "Name" })
                .value(&form.values.name)
                .autofocus(autofocus)
                .error(form.error("name"))
                .render())
            (Field::new(format!("{prefix}-description"), "description", html! { "Description " (optional_mark()) })
                .value(&form.values.description)
                .error(form.error("description"))
                .render())
        }
    }
}

fn enabled_field(form: &McpForm) -> Markup {
    choice(
        "enabled",
        None,
        form.values.enabled,
        "Enabled",
        Some("Disabled MCPs are excluded from the gateway"),
    )
}

/// What is wrong with the form that none of its fields has said.
fn form_errors(form: &McpForm) -> Markup {
    html! {
        @for (field, message) in form.unshown_errors() {
            @let title = if field == "npmEnv" { "Environment variables error" } else { "This MCP could not be saved" };
            (banner("critical", title, html! { p { (message) } }, html! {}))
        }
    }
}

fn dialog_header(title_id: &str, title: &str, subtitle: &str) -> Markup {
    html! {
        div class="dialog__header" {
            div class="dialog__heading" {
                h2 class="dialog__title" id=(title_id) { (title) }
                p class="dialog__subtitle" { (subtitle) }
            }
            button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
        }
    }
}

/// The dialog that adds an MCP, from a template or from nothing.
pub struct CreateDialog<'a> {
    /// The template the form started from. `None` for a custom MCP.
    pub template: Option<&'static McpTemplate>,
    pub form: &'a McpForm,
    pub public_app: Option<&'a PublicApp>,
    /// The list behind the dialog, which "Back to templates" keeps.
    pub query: &'a McpListQuery,
}

/// The content of the create dialog: what its `[data-fragment]` holds, and
/// what a refused submission of the page's script gets back.
pub fn create_fragment(context: &PageContext, dialog: &CreateDialog) -> Markup {
    let form = dialog.form;
    let values = &form.values;
    let builtin = values.transport == McpTransport::Builtin;
    let title = match dialog.template {
        Some(template) => format!("Set up {}", template.name),
        None => "Set up a custom MCP".to_string(),
    };
    let subtitle = if builtin {
        builtin_setup_guide(Some(&values.builtin_key))
            .map_or("Runs inside MyMCPs", |guide| guide.requirement)
    } else if dialog.template.is_some() {
        "Review the prefilled settings, then add this MCP"
    } else {
        "Register an HTTP or npm upstream server"
    };

    // After a refusal, the first field to correct takes the focus.
    let fields = html! {
        (identity_fields("new-mcp", form, !form.has_errors()))
        @if builtin {
            (builtin_fields(&BuiltinFields {
                prefix: "new-mcp",
                form,
                has_saved_client_secret: false,
                has_saved_password: false,
                is_connected: false,
                public_app: dialog.public_app,
            }))
        } @else {
            (custom_fields("new-mcp", form, None))
        }
        (enabled_field(form))
    };

    html! {
        (dialog_header("new-mcp-title", &title, subtitle))
        form class="dialog__body" id="new-mcp-form" method="post" action="/mcps" data-async {
            (context.csrf_field())
            // Names the dialog again when the form comes back refused. It is not saved.
            @if let Some(template) = dialog.template { input type="hidden" name="template" value=(template.id); }
            @if builtin { (builtin_choices(&values.builtin_key)) }
            (form_errors(form))
            div class="cluster" {
                a class="button button--secondary" href=(href("/mcps/new", dialog.query, &[])) { (icon("arrow-left")) "Back to templates" }
            }
            (fields)
        }
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="new-mcp-form" { "Add MCP" }
        }
    }
}

/// The dialog that edits a saved MCP.
pub struct EditDialog<'a> {
    pub mcp: &'a McpView,
    pub form: &'a McpForm,
    pub public_app: Option<&'a PublicApp>,
}

impl EditDialog<'_> {
    /// Whether the dialog opens on the reason the last connection failed.
    pub fn shows_last_error(&self) -> bool {
        !self.mcp.oauth_required
            && !self.mcp.awaits_write_authorization()
            && self.last_error().is_some()
    }

    fn last_error(&self) -> Option<&str> {
        self.mcp
            .last_error
            .as_deref()
            .filter(|error| !error.is_empty())
    }
}

fn status_class(status: McpStatus) -> &'static str {
    match status {
        McpStatus::Ready => "status status--success push-end",
        McpStatus::Draft => "status status--warning push-end",
        McpStatus::Error => "status status--critical push-end",
    }
}

/// The link that sends the browser to the provider to approve access. A
/// provider that only redirects to a loopback address is opened in another
/// tab, so that this page stays open to receive the pasted callback.
fn oauth_link(
    context: &PageContext,
    mcp: &McpView,
    label: &str,
    class: &str,
    reveals: bool,
) -> Markup {
    if !context.app_url_configured {
        return html! {
            a class=(class) aria-disabled="true" title="Set APP_URL to connect with OAuth" { (label) }
        };
    }
    let start = format!("/mcps/{}/oauth/start", mcp.id);
    html! {
        @if mcp.oauth_pasted_callback {
            a class=(class) href=(start) target="_blank" rel="noopener" data-reveal=[reveals.then_some("#edit-mcp-authorize")] { (label) }
        } @else {
            a class=(class) href=(start) { (label) }
        }
    }
}

/// Finishes OAuth for providers that only redirect to a loopback address.
/// The field has no name: it is not part of the edit form, and Enter in it
/// finishes connecting instead of saving the MCP.
fn oauth_callback_paste() -> Markup {
    html! {
        div class="field" data-oauth-paste {
            label class="field__label" for="edit-mcp-callback" { "Callback address" }
            div class="cluster cluster--nowrap" {
                input class="input grow" id="edit-mcp-callback" type="text" inputmode="url" placeholder="http://localhost:…/callback?code=…&state=…"
                    autocomplete="off" spellcheck="false" aria-describedby="edit-mcp-callback-help";
                button type="button" class="button button--secondary" data-oauth-paste-submit { "Finish connecting" }
            }
            p class="field__error" role="alert" data-oauth-paste-error hidden { "Paste the full localhost address, including ?code=…" }
            p class="field__help" id="edit-mcp-callback-help" {
                "After you approve access, the provider's tab lands on a localhost address that fails to load. Copy it from the address bar and paste it here."
            }
        }
    }
}

fn authorization_banner(context: &PageContext, mcp: &McpView) -> Markup {
    let description = if mcp.transport == McpTransport::Builtin {
        format!(
            "Connect opens {} so you can approve access with your account.",
            builtin_provider_name(mcp.builtin_key.as_deref())
        )
    } else if mcp.oauth_pasted_callback {
        "This MCP requires OAuth. Connect opens the provider in a new tab; paste the address it ends on below.".to_string()
    } else {
        "This MCP requires OAuth. Connect your account to finish setup.".to_string()
    };
    let label = if mcp.has_oauth_access_token {
        "Re-authorize"
    } else {
        "Connect"
    };
    banner(
        "warning",
        "Authorization required",
        html! { p { (description) } },
        oauth_link(context, mcp, label, "button button--secondary", false),
    )
}

/// What the dialog opens on: the first of these that applies.
fn edit_banner(context: &PageContext, dialog: &EditDialog) -> Markup {
    let mcp = dialog.mcp;
    if mcp.oauth_required {
        return html! {
            (authorization_banner(context, mcp))
            @if mcp.oauth_pasted_callback { (oauth_callback_paste()) }
        };
    }
    if mcp.awaits_write_authorization() {
        return banner(
            "warning",
            "Write access not granted yet",
            html! {
                p {
                    "Select Re-authorize and keep the write permissions checked on "
                    (builtin_provider_name(mcp.builtin_key.as_deref()))
                    ". Until then, only the read tools are available."
                }
            },
            oauth_link(
                context,
                mcp,
                "Re-authorize",
                "button button--secondary",
                false,
            ),
        );
    }
    match dialog.last_error() {
        Some(error) => banner(
            "critical",
            "Last connection error",
            html! { p { (error) } },
            html! {
                @if mcp.can_reauthorize() {
                    (oauth_link(context, mcp, "Re-authorize", "button button--secondary", mcp.oauth_pasted_callback))
                }
            },
        ),
        None => html! {},
    }
}

/// The content of the edit dialog: what its `[data-fragment]` holds, what
/// the list fetches to open it, and what a refused save gets back.
pub fn edit_fragment(context: &PageContext, dialog: &EditDialog) -> Markup {
    let mcp = dialog.mcp;
    let form = dialog.form;
    let builtin = form.values.transport == McpTransport::Builtin;
    let saved = Saved::of(mcp, &form.values);
    // Shown by a banner of its own unless one of the banners above already offers it.
    let banner_reauthorizes = mcp.awaits_write_authorization() || dialog.shows_last_error();
    let reauthorize = mcp.can_reauthorize() && !banner_reauthorizes;

    let fields = html! {
        (identity_fields("edit-mcp", form, false))
        @if builtin {
            (builtin_fields(&BuiltinFields {
                prefix: "edit-mcp",
                form,
                has_saved_client_secret: mcp.has_oauth_client_secret,
                has_saved_password: mcp.has_builtin_password,
                is_connected: mcp.is_connected(),
                public_app: dialog.public_app,
            }))
        } @else {
            (custom_fields("edit-mcp", form, Some(&saved)))
        }
        (enabled_field(form))
    };

    html! {
        (dialog_header("edit-mcp-title", &format!("Edit {}", mcp.name), &mcp.slug))
        form class="dialog__body" id="edit-mcp-form" method="post" action=(format!("/mcps/{}?_method=PUT", mcp.id)) data-async {
            (context.csrf_field())
            @if builtin { (builtin_choices(&form.values.builtin_key)) }
            (form_errors(form))
            (edit_banner(context, dialog))
            // Authorizing again opens the provider in another tab too: what
            // receives the pasted callback waits here until then.
            @if mcp.oauth_pasted_callback && mcp.can_reauthorize() {
                div class="stack" id="edit-mcp-authorize" hidden {
                    (authorization_banner(context, mcp))
                    (oauth_callback_paste())
                }
            }
            // The list has no row menu under 768px: its other actions show here.
            div class="cluster" {
                button type="submit" class="button button--secondary" form="edit-mcp-probe" { (icon("refresh-cw")) "Test connection" }
                @if mcp.tracks_latest() {
                    button type="submit" class="button button--secondary hide-desktop" form="edit-mcp-update" { (icon("download")) "Update MCP" }
                }
                a class="button button--secondary hide-desktop" href=(format!("/mcps/{}/tools", mcp.id))
                    title="Choose which tools run on their own and which wait for a person" { "Tool approvals" }
                @if reauthorize {
                    // A pasted callback needs this dialog, so the row menu sends here for it.
                    @let class = if mcp.oauth_pasted_callback { "button button--secondary" } else { "button button--secondary hide-desktop" };
                    (oauth_link(context, mcp, "Re-authorize", class, true))
                }
                span class=(status_class(mcp.status)) { (mcp.status.as_str()) }
            }
            (fields)
        }
        div class="dialog__footer" {
            button type="submit" class="button button--critical dialog__footer-start" form="edit-mcp-delete" { "Delete" }
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="edit-mcp-form" { "Save changes" }
        }
        // The other requests of the dialog are forms of their own: their
        // buttons name them, so Enter in a field always saves.
        form id="edit-mcp-probe" method="post" action=(format!("/mcps/{}/probe", mcp.id)) data-async hidden { (context.csrf_field()) }
        @if mcp.tracks_latest() {
            form id="edit-mcp-update" method="post" action=(format!("/mcps/{}/update", mcp.id)) data-async hidden { (context.csrf_field()) }
        }
        form id="edit-mcp-delete" method="post" action=(format!("/mcps/{}?_method=DELETE", mcp.id))
            data-confirm=(format!("Delete {}?", mcp.name)) data-confirm-label="Delete" data-confirm-tone="critical" hidden {
            (context.csrf_field())
        }
    }
}
