//! The Settings page: the account of the signed-in person and, for an
//! administrator, the settings of the instance.

use maud::{Markup, html};
use mymcps_core::models::{DEFAULT_MCP_AUTO_UPDATE_CRON, InstanceSetting, McpLogLevel, User};

use crate::forms::FormState;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};

/// What the Settings page shows.
pub struct SettingsPage<'a> {
    pub user: &'a User,
    /// The settings of the instance. `None` for a member, who is not shown them.
    pub instance: Option<&'a InstanceSetting>,
    /// Each form with what was refused when it was posted without the
    /// page's script. At most one of them has errors.
    pub email_form: &'a FormState,
    pub password_form: &'a FormState,
    pub instance_form: &'a FormState,
}

/// The message of a field and the attributes that tie it to its control.
struct FieldError<'a> {
    message: Option<&'a str>,
    id: String,
}

impl<'a> FieldError<'a> {
    fn of(form: &'a FormState, name: &str, field_id: &str) -> Self {
        Self {
            message: form.error(name),
            id: format!("{field_id}-error"),
        }
    }

    fn invalid(&self) -> Option<&'static str> {
        self.message.map(|_| "true")
    }

    /// `aria-describedby`: the error, then the help line of the field.
    fn described_by(&self, help_id: Option<&str>) -> Option<String> {
        match (self.message, help_id) {
            (Some(_), Some(help_id)) => Some(format!("{} {help_id}", self.id)),
            (Some(_), None) => Some(self.id.clone()),
            (None, help_id) => help_id.map(str::to_string),
        }
    }

    fn line(&self) -> Markup {
        html! {
            @if let Some(message) = self.message { p class="field__error" id=(self.id) { (message) } }
        }
    }
}

/// A password field of a dialog, with its show/hide button. Its value is
/// never rendered.
fn password_field(
    form: &FormState,
    id: &str,
    name: &str,
    label: &str,
    autocomplete: &str,
    autofocus: bool,
) -> Markup {
    let error = FieldError::of(form, name, id);
    html! {
        div class="field" {
            label class="field__label" for=(id) { (label) }
            div class="input-group" {
                input class="input-group__control" id=(id) name=(name) type="password" autocomplete=(autocomplete) required
                    autofocus[autofocus] aria-invalid=[error.invalid()] aria-describedby=[error.described_by(None)];
                button type="button" class="input-group__action" data-password-toggle=(format!("#{id}")) aria-pressed="false"
                    aria-label="Show password" data-label-pressed="Hide password" { (icon("eye")) (icon("eye-off")) }
            }
            (error.line())
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

fn dialog_footer(submit: &str) -> Markup {
    html! {
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" { (submit) }
        }
    }
}

/// The form of the "Change email" dialog: what its `[data-fragment]` holds,
/// and what a refused submission of the page's script gets back.
pub fn email_form(context: &PageContext, user: &User, form: &FormState) -> Markup {
    // A fresh form starts on the first field; in a refused one, the first
    // invalid field takes the focus by itself.
    let fresh = !form.has_errors();
    let email = if fresh {
        user.email.clone()
    } else {
        form.old("email")
    };
    let error = FieldError::of(form, "email", "new-email");
    html! {
        form method="post" action="/settings/email?_method=PATCH" data-async {
            (context.csrf_field())
            (dialog_header("change-email-title", "Change email", "Confirm the change with your current password"))
            div class="dialog__body" {
                div class="field" {
                    label class="field__label" for="new-email" { "New email" }
                    input class="input" id="new-email" name="email" type="email" autocomplete="email" value=(email) required
                        autofocus[fresh] aria-invalid=[error.invalid()] aria-describedby=[error.described_by(None)];
                    (error.line())
                }
                (password_field(form, "email-current-password", "currentPassword", "Current password", "current-password", false))
            }
            (dialog_footer("Save email"))
        }
    }
}

/// The form of the "Change password" dialog.
pub fn password_form(context: &PageContext, form: &FormState) -> Markup {
    let fresh = !form.has_errors();
    html! {
        form method="post" action="/settings/password?_method=PATCH" data-async {
            (context.csrf_field())
            (dialog_header("change-password-title", "Change password", "Use between 8 and 32 characters"))
            div class="dialog__body" {
                (password_field(form, "current-password", "currentPassword", "Current password", "current-password", fresh))
                (password_field(form, "new-password", "newPassword", "New password", "new-password", false))
                (password_field(form, "new-password-confirmation", "passwordConfirmation", "Confirm new password", "new-password", false))
            }
            (dialog_footer("Save password"))
        }
    }
}

/// A dialog whose form the script sends and swaps in place.
fn form_dialog(id: &str, title_id: &str, open: bool, form: Markup) -> Markup {
    html! {
        dialog class="dialog dialog--sm" id=(id) aria-labelledby=(title_id) data-dialog-reset data-open[open] {
            div data-fragment { (form) }
        }
    }
}

fn detail_row(label: &str, value: &str, action: Option<(&str, &str)>) -> Markup {
    html! {
        div class="detail-row" {
            div class="detail-row__text" {
                span class="detail-row__label" { (label) }
                span class="detail-row__value" { (value) }
            }
            @if let Some((label, dialog)) = action {
                button type="button" class="button button--secondary" data-dialog-open=(dialog) { (label) }
            }
        }
    }
}

fn account_section(user: &User) -> Markup {
    let name = user
        .full_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or("Not set");
    html! {
        section class="section" aria-labelledby="account-title" {
            div class="section__intro" {
                h2 class="section__title" id="account-title" { "My account" }
                p class="section__description" { "Update the credentials used to sign in to MyMCPs." }
            }
            div class="card" {
                (detail_row("Name", name, None))
                (detail_row("Email", &user.email, Some(("Change email", "#change-email"))))
                (detail_row(
                    "Password",
                    "Change your password whenever you need to secure your account.",
                    Some(("Change password", "#change-password")),
                ))
            }
        }
    }
}

/// What the instance form shows in its controls: the stored settings, or
/// what was typed when the form was refused.
struct InstanceValues {
    tool_mode: String,
    log_level: String,
    retention_days: String,
    auto_update: bool,
    cron: String,
}

impl InstanceValues {
    fn of(settings: &InstanceSetting, form: &FormState) -> Self {
        if form.has_errors() {
            return Self {
                tool_mode: form.old("gatewayToolMode"),
                log_level: form.old("mcpLogLevel"),
                retention_days: form.old("mcpLogRetentionDays"),
                // A switch submits "on" when checked, and nothing when unchecked.
                auto_update: matches!(
                    form.old("mcpAutoUpdateEnabled").as_str(),
                    "on" | "true" | "1"
                ),
                cron: form.old("mcpAutoUpdateCron"),
            };
        }
        let cron = if settings.mcp_auto_update_cron.is_empty() {
            DEFAULT_MCP_AUTO_UPDATE_CRON
        } else {
            &settings.mcp_auto_update_cron
        };
        Self {
            tool_mode: settings.gateway_tool_mode.as_str().to_string(),
            log_level: settings.mcp_log_level.as_str().to_string(),
            retention_days: settings.mcp_log_retention_days.to_string(),
            auto_update: settings.mcp_auto_update_enabled,
            cron: cron.to_string(),
        }
    }
}

fn log_level_label(level: McpLogLevel) -> &'static str {
    match level {
        McpLogLevel::Off => "Off",
        McpLogLevel::Metadata => "Metadata",
        McpLogLevel::Arguments => "Metadata + arguments",
        McpLogLevel::Responses => "Metadata + arguments + responses",
    }
}

fn tool_mode_card(
    value: &str,
    title: &str,
    description: &str,
    current: &str,
    autofocus: bool,
) -> Markup {
    html! {
        label class="option-card" {
            input type="radio" class="radio" name="gatewayToolMode" value=(value) checked[current == value] autofocus[autofocus];
            span class="option-card__text" {
                span class="option-card__title" { (title) }
                span class="option-card__description" { (description) }
            }
        }
    }
}

/// Administrators only: a member does not get this section.
fn instance_section(context: &PageContext, settings: &InstanceSetting, form: &FormState) -> Markup {
    let values = InstanceValues::of(settings, form);
    let tool_mode = FieldError::of(form, "gatewayToolMode", "tool-mode");
    let log_level = FieldError::of(form, "mcpLogLevel", "log-level");
    let retention = FieldError::of(form, "mcpLogRetentionDays", "log-retention");
    let auto_update = FieldError::of(form, "mcpAutoUpdateEnabled", "auto-update");
    let cron = FieldError::of(form, "mcpAutoUpdateCron", "auto-update-cron");
    // After a refusal, the first field to correct takes the focus.
    let first_invalid = [
        "gatewayToolMode",
        "mcpLogLevel",
        "mcpLogRetentionDays",
        "mcpAutoUpdateEnabled",
        "mcpAutoUpdateCron",
    ]
    .into_iter()
    .find(|name| form.error(name).is_some());
    let focused = |name: &str| first_invalid == Some(name);
    html! {
        section class="section" aria-labelledby="instance-title" {
            div class="section__intro" {
                h2 class="section__title" id="instance-title" { "My Instance " span class="badge badge--info badge--no-dot" { "Admin only" } }
                p class="section__description" { "Configure settings that apply to everyone using this MyMCPs instance." }
            }
            form class="card" method="post" action="/settings/mcp-logging?_method=PATCH" {
                (context.csrf_field())
                div class="card__body gap-500" {
                    @if form.has_errors() {
                        div class="banner banner--critical" role="alert" {
                            (icon("circle-x"))
                            div class="banner__content" {
                                p class="banner__title" { "The instance settings were not saved" }
                                p {
                                    @if form.error_count() == 1 { "Check the field marked below, then save again." }
                                    @else { "Check the " (form.error_count()) " fields marked below, then save again." }
                                }
                            }
                        }
                    }

                    fieldset class="field-group" aria-describedby=[tool_mode.described_by(None)] {
                        legend class="field-group__label" { "Default MCP tool mode" }
                        p class="field-group__help" { "Used when a client does not send X-MyMCPs-Tool-Mode. A request header always overrides this default." }
                        div class="option-cards" {
                            (tool_mode_card("eager", "Eager", "Expose every allowed upstream tool in tools/list.", &values.tool_mode, focused("gatewayToolMode")))
                            (tool_mode_card("lazy", "Lazy", "Expose only list_mcps, tool_search, and call_tool until tools are requested.", &values.tool_mode, false))
                        }
                        (tool_mode.line())
                    }

                    div class="banner banner--warning" role="status" {
                        (icon("triangle-alert"))
                        div class="banner__content" {
                            p class="banner__title" { "Arguments and responses can contain sensitive data" }
                            p { "Argument and response capture stores exact MCP JSON without redaction. Tool responses may contain secrets, personal data, or large payloads." }
                        }
                    }

                    div class="grid grid--2" {
                        div class="field" {
                            label class="field__label" for="log-level" { "Call logging level" }
                            div class="select" {
                                select class="input" id="log-level" name="mcpLogLevel" autofocus[focused("mcpLogLevel")] aria-invalid=[log_level.invalid()]
                                    aria-describedby=[log_level.described_by(Some("log-level-help"))] {
                                    @for level in McpLogLevel::ALL {
                                        option value=(level.as_str()) selected[values.log_level == level.as_str()] { (log_level_label(*level)) }
                                    }
                                }
                                (icon("chevron-down"))
                            }
                            (log_level.line())
                            p class="field__help" id="log-level-help" { "Changes apply to future tool calls only." }
                        }
                        div class="field" {
                            label class="field__label" for="log-retention" { "Log retention" }
                            // A whole number of days, 1 to 365. The unit follows the value inside the field.
                            label class="input-group" {
                                input class="input-group__control" id="log-retention" name="mcpLogRetentionDays" type="number" inputmode="numeric"
                                    min="1" max="365" step="1" value=(values.retention_days) autofocus[focused("mcpLogRetentionDays")] aria-invalid=[retention.invalid()]
                                    aria-describedby=[retention.described_by(Some("log-retention-help"))];
                                span class="input-group__unit" { "days" }
                            }
                            (retention.line())
                            p class="field__help" id="log-retention-help" { "Records older than this are deleted." }
                        }
                    }

                    label class="setting-row" {
                        span class="setting-row__text" {
                            span class="setting-row__title" { "Auto-update Deno npm MCPs" }
                            span class="setting-row__description" { "Reloads the Deno cache for npm MCPs that already track latest. Pinned versions are never changed." }
                        }
                        input type="checkbox" class="switch" role="switch" name="mcpAutoUpdateEnabled" checked[values.auto_update]
                            autofocus[focused("mcpAutoUpdateEnabled")] aria-invalid=[auto_update.invalid()] aria-describedby=[auto_update.described_by(None)];
                    }
                    (auto_update.line())

                    div class="grid grid--2" {
                        div class="field" {
                            label class="field__label" for="auto-update-cron" { "Auto-update schedule" }
                            input class="input" id="auto-update-cron" name="mcpAutoUpdateCron" type="text" value=(values.cron)
                                placeholder=(DEFAULT_MCP_AUTO_UPDATE_CRON) autocomplete="off" spellcheck="false" autofocus[focused("mcpAutoUpdateCron")] aria-invalid=[cron.invalid()]
                                aria-describedby=[cron.described_by(Some("auto-update-cron-help"))];
                            (cron.line())
                            p class="field__help" id="auto-update-cron-help" { "5-field cron in UTC. Default is every day at 02:00." }
                        }
                    }
                }
                footer class="card__footer" {
                    button type="submit" class="button button--primary" { "Save instance settings" }
                }
            }
        }
    }
}

pub fn settings_page(context: &PageContext, settings: &SettingsPage) -> Markup {
    // What an open dialog already says under its field is not said again in a toast.
    let reopened = [settings.email_form, settings.password_form]
        .into_iter()
        .find(|form| form.has_errors());
    let mut context = context.clone();
    if reopened.is_some_and(|form| context.flash_error.as_deref() == form.first_error()) {
        context.flash_error = None;
    }
    let context = &context;

    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Settings" }
                p class="page-header__subtitle" { "Manage your account and this MyMCPs instance." }
            }
        }
        (account_section(settings.user))
        @if let Some(instance) = settings.instance {
            (instance_section(context, instance, settings.instance_form))
        }
    };
    let overlays = html! {
        (form_dialog(
            "change-email",
            "change-email-title",
            settings.email_form.has_errors(),
            email_form(context, settings.user, settings.email_form),
        ))
        (form_dialog(
            "change-password",
            "change-password-title",
            settings.password_form.has_errors(),
            password_form(context, settings.password_form),
        ))
    };
    app_page(context, "Settings", content, overlays)
}
