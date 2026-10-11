//! The Access tokens page: the gateway URL, the list of tokens and OAuth
//! connections, and its dialogs (create, edit, install).

use chrono::{DateTime, Utc};
use maud::{Markup, html};
use mymcps_core::Timestamp;
use mymcps_core::models::{AccessToken, Mcp, ScopeMode, TokenSource};
use serde_json::Value;

use crate::forms::FormState;
use crate::install_config::{McpClient, McpInstallAuthMode, split_at_token};
use crate::validators::access_token::EXPIRY_VALIDATOR;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};
use crate::views::{grouped, time_minute};

/// How many tokens one page of the list shows.
pub const ROWS_PER_PAGE: usize = 25;

/// What a snippet shows where its token goes, until one is typed.
const TOKEN_PLACEHOLDER: &str = "<YOUR_ACCESS_TOKEN>";
/// What a snippet shows in place of the gateway URL while `APP_URL` is not set.
const GATEWAY_PLACEHOLDER: &str = "<YOUR_GATEWAY_URL>";

const DELETE_NOTE: &str =
    "This cannot be undone. Existing activity logs will keep their token names and identifiers.";

/// Which tokens the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Active,
    /// Expired and revoked: the tokens that can be deleted.
    Inactive,
}

impl StatusFilter {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("active") => Self::Active,
            Some("inactive") => Self::Inactive,
            _ => Self::All,
        }
    }

    fn key(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Active => Some("active"),
            Self::Inactive => Some("inactive"),
        }
    }

    pub fn shows(self, token: &AccessToken) -> bool {
        match self {
            Self::All => true,
            Self::Active => token.is_active(),
            Self::Inactive => !token.is_active(),
        }
    }
}

/// The part of the list a request asks for. Its query string travels with
/// every link and form of the page, so that an action comes back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListView {
    pub status: StatusFilter,
    /// From 1.
    pub page: usize,
}

impl ListView {
    /// `status=inactive&page=2`, without what is the default.
    pub fn query(&self) -> String {
        let mut pairs = Vec::new();
        if let Some(status) = self.status.key() {
            pairs.push(format!("status={status}"));
        }
        if self.page > 1 {
            pairs.push(format!("page={}", self.page));
        }
        pairs.join("&")
    }

    /// `path` with the view's query string after it.
    pub fn url(&self, path: &str) -> String {
        let query = self.query();
        let separator = if path.contains('?') { '&' } else { '?' };
        if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}{separator}{query}")
        }
    }

    fn with(&self, status: StatusFilter, page: usize) -> String {
        ListView { status, page }.url("/tokens")
    }
}

/// A token of the list with the MCPs it is limited to.
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub token: AccessToken,
    pub mcp_ids: Vec<i64>,
}

impl TokenRow {
    fn is_manual(&self) -> bool {
        self.token.source == TokenSource::Manual
    }

    /// An OAuth connection can be revoked until it is, a manual token while it works.
    pub fn can_revoke(&self) -> bool {
        if self.is_manual() {
            self.token.is_usable()
        } else {
            !self.token.is_revoked()
        }
    }

    pub fn can_delete(&self) -> bool {
        !self.token.is_active()
    }

    /// Only manual tokens that are not revoked: the secret is never rotated.
    pub fn can_edit(&self) -> bool {
        self.is_manual() && !self.token.is_revoked()
    }

    /// The refresh-token expiry of an OAuth connection, the expiry of a manual token.
    fn display_expires_at(&self) -> Option<Timestamp> {
        if self.is_manual() {
            self.token.expires_at
        } else {
            self.token.oauth_refresh_expires_at
        }
    }

    fn scope_label(&self) -> String {
        if self.token.scope_mode == ScopeMode::All {
            return "All MCPs".to_string();
        }
        let count = self.mcp_ids.len();
        format!("{count} MCP{}", if count == 1 { "" } else { "s" })
    }

    fn source_label(&self) -> &'static str {
        if self.is_manual() { "Manual" } else { "OAuth" }
    }

    fn identifier(&self) -> String {
        format!("{}…", self.token.token_prefix)
    }
}

/// How many tokens each segment of the toolbar holds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub all: usize,
    pub active: usize,
    pub inactive: usize,
}

/// The dialog a page is drawn with, when its URL names one.
pub enum OpenDialog<'a> {
    None,
    Create,
    Install,
    Edit(&'a TokenRow),
}

pub struct TokensPage<'a> {
    pub view: ListView,
    /// The tokens of this page of the view.
    pub rows: &'a [TokenRow],
    /// How many tokens the view holds, on all its pages.
    pub total: usize,
    pub counts: Counts,
    /// Every expired or revoked token, for "Delete all".
    pub deletable_ids: &'a [i64],
    pub mcps: &'a [Mcp],
    /// The address MCP clients connect to, when `APP_URL` can be published.
    pub gateway_url: Option<&'a str>,
    /// The token the previous request created, shown this once.
    pub created_plaintext: Option<&'a str>,
    /// With what its creation flashed for the banner.
    pub created_message: Option<&'a str>,
    pub open: OpenDialog<'a>,
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn status_badge(token: &AccessToken) -> Markup {
    html! {
        @if token.is_revoked() {
            span class="badge" { "Revoked" }
        } @else if token.is_active() {
            span class="badge badge--success" { "Active" }
        } @else {
            span class="badge badge--warning" { "Expired" }
        }
    }
}

fn banner(tone: &str, icon_name: &str, title: &str, text: Option<&str>) -> Markup {
    let class = if tone.is_empty() {
        "banner".to_string()
    } else {
        format!("banner banner--{tone}")
    };
    html! {
        div class=(class) role="status" {
            (icon(icon_name))
            div class="banner__content" {
                p class="banner__title" { (title) }
                @if let Some(text) = text { p { (text) } }
            }
        }
    }
}

// --------------------------------------------------------------------- forms

/// What the fields of the create and edit forms hold.
struct TokenFormValues {
    name: String,
    selected: bool,
    mcp_ids: Vec<String>,
    /// The expiry as an instant, when there is one that can be read.
    expires_at: Option<DateTime<Utc>>,
}

impl TokenFormValues {
    fn of_token(row: &TokenRow) -> Self {
        Self {
            name: row.token.name.clone(),
            selected: row.token.scope_mode == ScopeMode::Selected,
            mcp_ids: row.mcp_ids.iter().map(i64::to_string).collect(),
            expires_at: row
                .token
                .expires_at
                .map(|expires_at| expires_at.as_datetime()),
        }
    }

    /// What a refused form had in it.
    fn of_submission(form: &FormState) -> Self {
        let text = |value: &Value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        let mcp_ids = match form.old_value("mcpIds") {
            Some(Value::Array(ids)) => ids.iter().filter_map(text).collect(),
            Some(single) => text(single).into_iter().collect(),
            None => Vec::new(),
        };
        Self {
            name: form.old("name"),
            selected: form.old("scopeMode") == "selected",
            mcp_ids,
            expires_at: form
                .old_value("expiresAt")
                .and_then(|value| EXPIRY_VALIDATOR.validate_as::<DateTime<Utc>>(value).ok()),
        }
    }
}

/// The error of the MCP list: of the list itself, or of one of its members.
fn mcp_ids_error(form: &FormState, submitted: usize) -> Option<&str> {
    form.error("mcpIds")
        .or_else(|| (0..submitted).find_map(|index| form.error(&format!("mcpIds.{index}"))))
}

struct TokenForm<'a> {
    /// Prefix of the ids of the form: `create-token` or `edit-token`.
    id: &'a str,
    action: String,
    title: String,
    subtitle: String,
    submit: &'a str,
}

fn token_form(
    context: &PageContext,
    shape: &TokenForm<'_>,
    values: &TokenFormValues,
    form: &FormState,
    mcps: &[Mcp],
) -> Markup {
    let id = shape.id;
    let name_error = form.error("name");
    let name_error_id = format!("{id}-name-error");
    let scope_error = form.error("scopeMode");
    let mcps_error = mcp_ids_error(form, values.mcp_ids.len());
    let mcps_error_id = format!("{id}-mcps-error");
    let expires_error = form.error("expiresAt");
    let expires_help_id = format!("{id}-expires-help");
    let expires_error_id = format!("{id}-expires-error");
    let expires_described_by = match expires_error {
        Some(_) => format!("{expires_help_id} {expires_error_id}"),
        None => expires_help_id.clone(),
    };
    // The field shows UTC until the page script rewrites it in local time.
    let expires_value = values
        .expires_at
        .map(|expires_at| expires_at.format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default();
    let expires_utc = values
        .expires_at
        .map(|expires_at| Timestamp::from(expires_at).to_iso())
        .unwrap_or_default();

    html! {
        form method="post" action=(shape.action) data-async {
            (context.csrf_field())
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id=(format!("{id}-title")) { (shape.title) }
                    p class="dialog__subtitle" { (shape.subtitle) }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="field" {
                    label class="field__label" for=(format!("{id}-name")) { "Name" }
                    input class="input" id=(format!("{id}-name")) name="name" type="text" value=(values.name)
                        placeholder="Cursor agent" required maxlength="120" autocomplete="off" autofocus
                        aria-invalid=[name_error.map(|_| "true")] aria-describedby=[name_error.map(|_| name_error_id.as_str())];
                    @if let Some(message) = name_error { p class="field__error" id=(name_error_id) { (message) } }
                }

                fieldset class="field-group" {
                    legend class="field-group__label" { "MCP access" }
                    p class="field-group__help" { "All includes every enabled MCP, including ones added later." }
                    label class="choice" {
                        input type="radio" class="radio" name="scopeMode" value="all" checked[!values.selected];
                        span class="choice__label" { "All MCPs" }
                    }
                    label class="choice" {
                        input type="radio" class="radio" name="scopeMode" value="selected" checked[values.selected];
                        span class="choice__label" { "Selected MCPs" }
                    }
                    @if let Some(message) = scope_error { p class="field__error" { (message) } }
                }

                fieldset class="field-group" data-show-when="scopeMode=selected" hidden[!values.selected]
                    aria-describedby=[mcps_error.map(|_| mcps_error_id.as_str())] {
                    legend class="field-group__label" { "MCPs" }
                    @if mcps.is_empty() {
                        (banner("warning", "triangle-alert", "Add an MCP first", None))
                    } @else {
                        div class="grid grid--2" {
                            @for mcp in mcps {
                                label class="choice" {
                                    input type="checkbox" class="checkbox" name="mcpIds[]" value=(mcp.id)
                                        checked[values.mcp_ids.contains(&mcp.id.to_string())];
                                    span class="choice__text" {
                                        span class="choice__label" { (mcp.name) }
                                        span class="choice__description" {
                                            (mcp.slug) @if !mcp.enabled { " (disabled)" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    @if let Some(message) = mcps_error { p class="field__error" id=(mcps_error_id) { (message) } }
                }

                div class="field" {
                    label class="field__label" for=(format!("{id}-expires")) {
                        "Expires at " span class="field__optional" { "Optional" }
                    }
                    input class="input" id=(format!("{id}-expires")) name="expiresAt" type="datetime-local"
                        value=(expires_value) data-utc=(expires_utc) aria-describedby=(expires_described_by)
                        aria-invalid=[expires_error.map(|_| "true")];
                    p class="field__help" id=(expires_help_id) {
                        "Leave empty for no expiration. Time is interpreted in your local timezone."
                    }
                    @if let Some(message) = expires_error { p class="field__error" id=(expires_error_id) { (message) } }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { (shape.submit) }
            }
        }
    }
}

/// The form of the create dialog: what its `[data-fragment]` holds. `form`
/// carries what a refused submission had in it, and why it was refused.
pub fn create_form(context: &PageContext, form: &FormState, mcps: &[Mcp]) -> Markup {
    let values = TokenFormValues::of_submission(form);
    token_form(
        context,
        &TokenForm {
            id: "create-token",
            action: "/tokens".to_string(),
            title: "Create token".to_string(),
            subtitle: "Issue an identifier for the /mcp gateway".to_string(),
            submit: "Create token",
        },
        &values,
        form,
        mcps,
    )
}

/// The form of the edit dialog, for the token as it is stored, or for what
/// a refused submission had in it when `form` holds errors.
pub fn edit_form(
    context: &PageContext,
    view: &ListView,
    row: &TokenRow,
    form: &FormState,
    mcps: &[Mcp],
) -> Markup {
    let values = if form.has_errors() {
        TokenFormValues::of_submission(form)
    } else {
        TokenFormValues::of_token(row)
    };
    token_form(
        context,
        &TokenForm {
            id: "edit-token",
            action: view.url(&format!("/tokens/{}?_method=PUT", row.token.id)),
            title: format!("Edit {}", row.token.name),
            subtitle: row.identifier(),
            submit: "Save changes",
        },
        &values,
        form,
        mcps,
    )
}

// ------------------------------------------------------------ install dialog

/// What the install dialog starts on.
struct InstallState<'a> {
    gateway_url: Option<&'a str>,
    auth: McpInstallAuthMode,
    /// The token just created, or nothing.
    token: &'a str,
}

fn install_snippet(
    state: &InstallState<'_>,
    client: McpClient,
    auth: McpInstallAuthMode,
    lazy: bool,
) -> Markup {
    let id = format!(
        "install-{}-{}{}",
        client.key(),
        auth.key(),
        if lazy { "-lazy" } else { "" }
    );
    let condition = format!(
        "auth={}&lazy={}",
        auth.key(),
        if lazy { "on" } else { "off" }
    );
    let (config, parts) = split_at_token(
        client,
        state.gateway_url.unwrap_or(GATEWAY_PLACEHOLDER),
        lazy,
        auth,
    );
    // Lazy tool mode starts off.
    let shown = auth == state.auth && !lazy;
    let format = client.token_format();

    html! {
        div class="code-block" data-show-when=(condition) hidden[!shown] {
            div class="code-block__header" {
                span class="code-block__title" { (config.title) }
                // Nothing to copy without a gateway URL, nor before a token is pasted.
                @if state.gateway_url.is_some() {
                    @if auth == McpInstallAuthMode::Token {
                        button type="button" class="icon-button" data-copy-target=(format!("#{id}"))
                            data-show-when="token!=" aria-label="Copy" hidden[state.token.is_empty()] {
                            (icon("copy")) (icon("check"))
                        }
                    } @else {
                        button type="button" class="icon-button" data-copy-target=(format!("#{id}")) aria-label="Copy" {
                            (icon("copy")) (icon("check"))
                        }
                    }
                }
            }
            pre class="code-block__body" id=(id) {
                @match parts {
                    Some((before, after)) => {
                        (before)
                        span data-bind="token" data-bind-format=(format.key()) data-bind-empty=(TOKEN_PLACEHOLDER) {
                            @if state.token.is_empty() { (TOKEN_PLACEHOLDER) } @else { (format.escape(state.token)) }
                        }
                        (after)
                    }
                    None => { (config.code) }
                }
            }
        }
    }
}

fn install_panel(state: &InstallState<'_>, client: McpClient, selected: bool) -> Markup {
    let (config, _) = split_at_token(client, "", false, McpInstallAuthMode::Oauth);
    let waits_for_token = state.auth == McpInstallAuthMode::Token && state.token.is_empty();
    html! {
        div class="stack" role="tabpanel" id=(format!("install-panel-{}", client.key()))
            aria-labelledby=(format!("install-tab-{}", client.key())) hidden[!selected] {
            @for auth in [McpInstallAuthMode::Oauth, McpInstallAuthMode::Token] {
                @for lazy in [false, true] {
                    (install_snippet(state, client, auth, lazy))
                }
            }
            @if state.gateway_url.is_none() {
                (banner("", "info", "Configure APP_URL before copying this configuration.", None))
            } @else {
                div class="banner" role="status" data-show-when="auth=token&token=" hidden[!waits_for_token] {
                    (icon("info"))
                    div class="banner__content" { p class="banner__title" { "Paste an access token to enable copying." } }
                }
            }
            ol class="steps" {
                li class="step" { (config.restart_instruction) }
                li class="step" { (config.verify_instruction) }
            }
        }
    }
}

/// The install dialog. Everything happens in the browser: its form is never
/// sent, so a pasted token never leaves the page.
fn install_dialog(page: &TokensPage<'_>, list_url: &str) -> Markup {
    let token = page.created_plaintext.unwrap_or_default();
    let state = InstallState {
        gateway_url: page.gateway_url,
        // A new token is what the person came to install; without a gateway
        // URL, OAuth cannot work.
        auth: if !token.is_empty() || page.gateway_url.is_none() {
            McpInstallAuthMode::Token
        } else {
            McpInstallAuthMode::Oauth
        },
        token,
    };
    let open = !token.is_empty() || matches!(page.open, OpenDialog::Install);
    let token_mode = state.auth == McpInstallAuthMode::Token;
    let shown = !token.is_empty();
    let default_client = McpClient::Claude;

    html! {
        dialog class="dialog dialog--lg" id="install" aria-labelledby="install-title" data-open[open]
            data-dialog-reset data-dialog-return=(list_url) {
            form method="dialog" data-bind-scope {
                // The default button of the form: disabled, so that Enter in the token field submits nothing.
                button type="submit" disabled hidden {}
                div class="dialog__header" {
                    div class="dialog__heading" {
                        h2 class="dialog__title" id="install-title" { "Install MyMCPs" }
                        p class="dialog__subtitle" { "Connect this gateway to your MCP client" }
                    }
                    button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
                }
                div class="dialog__body" {
                    @if state.gateway_url.is_some() {
                        div class="segmented segmented--block" role="radiogroup" aria-label="Authentication" {
                            label class="segment" {
                                input type="radio" class="visually-hidden" name="auth" value="oauth" checked[!token_mode];
                                "OAuth (recommended)"
                            }
                            label class="segment" {
                                input type="radio" class="visually-hidden" name="auth" value="token" checked[token_mode];
                                "Access token"
                            }
                        }
                    } @else {
                        input type="hidden" name="auth" value="token";
                        (banner("warning", "triangle-alert", "OAuth is unavailable",
                            Some("Set APP_URL to this instance's public HTTPS origin to enable OAuth installation.")))
                    }

                    div class="banner" role="status" data-show-when="auth=oauth" hidden[token_mode] {
                        (icon("info"))
                        div class="banner__content" {
                            p class="banner__title" { "No token to copy" }
                            p { "Add the gateway URL to your MCP client. The client will open this instance in your browser so you can sign in and approve the connection." }
                        }
                    }

                    div class="stack gap-300" data-show-when="auth=token" hidden[!token_mode] {
                        (banner("warning", "triangle-alert", "Your token will be stored in plaintext",
                            Some("These quick-install configurations include the access token directly. Keep the configuration private and revoke the token immediately if it is exposed.")))
                        div class="field" {
                            label class="field__label" for="install-token" { "Access token" }
                            div class="input-group" {
                                input class="input-group__control" id="install-token" name="token"
                                    type=(if shown { "text" } else { "password" }) value=[shown.then_some(token)]
                                    placeholder="Paste your MyMCPs access token" autocomplete="off" autocapitalize="off"
                                    spellcheck="false" data-bind-source="token" aria-describedby="install-token-help";
                                @if shown {
                                    button type="button" class="input-group__action" data-password-toggle="#install-token"
                                        aria-pressed="true" aria-label="Hide access token" data-label="Show access token"
                                        data-label-pressed="Hide access token" { (icon("eye")) (icon("eye-off")) }
                                } @else {
                                    button type="button" class="input-group__action" data-password-toggle="#install-token"
                                        aria-pressed="false" aria-label="Show access token"
                                        data-label-pressed="Hide access token" { (icon("eye")) (icon("eye-off")) }
                                }
                            }
                            p class="field__help" id="install-token-help" {
                                "This value stays in this browser tab and is cleared when you close the modal."
                            }
                        }
                    }

                    label class="setting-row" {
                        span class="setting-row__text" {
                            span class="setting-row__title" { "Enable lazy tool mode" }
                            span class="setting-row__description" {
                                "Adds X-MyMCPs-Tool-Mode: lazy so clients discover tools on demand."
                            }
                        }
                        input type="checkbox" class="switch" role="switch" name="lazy";
                    }

                    div class="tabs stack" data-tabs {
                        div class="tabs__list" role="tablist" aria-label="MCP client" {
                            @for client in McpClient::ALL {
                                @let selected = client == default_client;
                                button type="button" class="tab" role="tab" id=(format!("install-tab-{}", client.key()))
                                    aria-controls=(format!("install-panel-{}", client.key()))
                                    aria-selected=(if selected { "true" } else { "false" })
                                    tabindex=[(!selected).then_some("-1")] { (client.label()) }
                            }
                        }
                        @for client in McpClient::ALL {
                            (install_panel(&state, client, client == default_client))
                        }
                    }
                }
                div class="dialog__footer" {
                    button type="button" class="button button--secondary" data-dialog-close { "Close" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------- list

fn delete_message(count: usize) -> String {
    format!(
        "Permanently delete {} token{}? {DELETE_NOTE}",
        grouped(count),
        plural(count)
    )
}

/// The form behind "Revoke" or "Delete" of a row. The row menu of the
/// mobile list submits the same form, so each request exists once in the page.
fn row_form(context: &PageContext, view: &ListView, row: &TokenRow) -> Markup {
    let token = &row.token;
    html! {
        @if row.can_delete() {
            form id=(format!("delete-token-{}", token.id)) method="post" action=(view.url("/tokens?_method=DELETE"))
                data-confirm=(delete_message(1)) data-confirm-title="Delete tokens?" data-confirm-label="Delete tokens"
                data-confirm-tone="critical" {
                (context.csrf_field())
                input type="hidden" name="ids[]" value=(token.id);
                button type="submit" class="button" { "Delete" }
            }
        } @else if row.can_revoke() {
            @let message = if row.is_manual() {
                format!("Agents that use {} lose access to the gateway immediately.", token.name)
            } else {
                "Revoking this OAuth connection stops both its access and refresh tokens.".to_string()
            };
            form id=(format!("revoke-token-{}", token.id)) method="post"
                action=(view.url(&format!("/tokens/{}/revoke", token.id)))
                data-confirm=(message) data-confirm-title=(format!("Revoke {}?", token.name))
                data-confirm-label="Revoke" data-confirm-tone="critical" {
                (context.csrf_field())
                button type="submit" class="button" { "Revoke" }
            }
        }
    }
}

fn edit_url(view: &ListView, row: &TokenRow) -> String {
    view.url(&format!("/tokens/{}/edit", row.token.id))
}

fn table_row(context: &PageContext, view: &ListView, row: &TokenRow) -> Markup {
    let token = &row.token;
    html! {
        tr {
            @if view.status == StatusFilter::Inactive {
                td {
                    div class="cell-media" {
                        input type="checkbox" class="checkbox" name="ids[]" value=(token.id) form="delete-selected"
                            data-select-item aria-label=(format!("Select {} for deletion", token.name));
                        span class="cell-title" { (token.name) }
                    }
                }
            } @else {
                td class="cell-strong" { (token.name) }
            }
            td {
                @if row.is_manual() {
                    span class="badge badge--no-dot" { "Manual" }
                } @else {
                    span class="badge badge--info badge--no-dot" { "OAuth" }
                }
            }
            td class="cell-code" { (row.identifier()) }
            td { (row.scope_label()) }
            @match row.display_expires_at() {
                Some(expires_at) => td { (time_minute(expires_at)) },
                None => td class="cell-tertiary" { "No expiry" },
            }
            td class="cell-secondary" {
                @match token.last_used_at {
                    Some(last_used_at) => (time_minute(last_used_at)),
                    None => "Never",
                }
            }
            td { (status_badge(token)) }
            td class="cell-end" {
                span class="row-actions" {
                    @if row.can_edit() {
                        a class="button" href=(edit_url(view, row)) data-dialog-open="#edit-token" data-dialog-fetch { "Edit" }
                    }
                    (row_form(context, view, row))
                }
            }
        }
    }
}

fn mobile_row(view: &ListView, row: &TokenRow) -> Markup {
    let token = &row.token;
    let menu_id = format!("token-{}-menu", token.id);
    let has_actions = row.can_edit() || row.can_delete() || row.can_revoke();
    html! {
        div class="list-row" {
            span class="list-row__text" {
                span class="cluster" {
                    span class="list-row__title" { (token.name) }
                    (status_badge(token))
                }
                span class="list-row__subtitle" {
                    (row.source_label()) " · " (row.identifier()) " · " (row.scope_label())
                }
                span class="list-row__subtitle" {
                    @match row.display_expires_at() {
                        Some(expires_at) => { "Expires " (crate::views::date(expires_at)) },
                        None => "No expiry",
                    }
                }
                span class="list-row__subtitle" {
                    @match token.last_used_at {
                        Some(last_used_at) => { "Last used " (crate::views::time(last_used_at)) },
                        None => "Never used",
                    }
                }
            }
            @if has_actions {
                button type="button" class="icon-button" popovertarget=(menu_id)
                    aria-label=(format!("Actions for {}", token.name)) { (icon("ellipsis")) }
                div class="menu" id=(menu_id) popover role="menu" {
                    @if row.can_edit() {
                        a class="menu__item" role="menuitem" href=(edit_url(view, row)) data-dialog-open="#edit-token"
                            data-dialog-fetch { (icon("pencil")) "Edit" }
                    }
                    @if row.can_delete() {
                        button type="submit" class="menu__item menu__item--critical" role="menuitem"
                            form=(format!("delete-token-{}", token.id)) { (icon("trash")) "Delete" }
                    } @else if row.can_revoke() {
                        button type="submit" class="menu__item" role="menuitem"
                            form=(format!("revoke-token-{}", token.id)) { (icon("ban")) "Revoke" }
                    }
                }
            }
        }
    }
}

fn pagination(page: &TokensPage<'_>) -> Markup {
    let view = &page.view;
    let first = if page.total == 0 {
        0
    } else {
        (view.page - 1) * ROWS_PER_PAGE + 1
    };
    let last = (view.page * ROWS_PER_PAGE).min(page.total);
    let has_previous = view.page > 1;
    let has_next = last < page.total;
    html! {
        nav class="pagination" aria-label="Pagination" {
            span class="pagination__range" { (grouped(first)) "–" (grouped(last)) " of " (grouped(page.total)) }
            span class="pagination__buttons" {
                @if has_previous {
                    a class="icon-button icon-button--secondary" href=(view.with(view.status, view.page - 1))
                        aria-label="Previous page" { (icon("chevron-left")) }
                } @else {
                    a class="icon-button icon-button--secondary" aria-disabled="true" aria-label="Previous page" {
                        (icon("chevron-left"))
                    }
                }
                @if has_next {
                    a class="icon-button icon-button--secondary" href=(view.with(view.status, view.page + 1))
                        aria-label="Next page" { (icon("chevron-right")) }
                } @else {
                    a class="icon-button icon-button--secondary" aria-disabled="true" aria-label="Next page" {
                        (icon("chevron-right"))
                    }
                }
            }
        }
    }
}

fn toolbar(context: &PageContext, page: &TokensPage<'_>) -> Markup {
    let view = &page.view;
    let counts = &page.counts;
    let deletable = page.deletable_ids.len();
    let selecting = view.status == StatusFilter::Inactive;
    let segment = |status: StatusFilter, label: &str, count: usize| {
        html! {
            a class="segment" href=(view.with(status, 1)) aria-current=[(view.status == status).then_some("true")] {
                (label) " " (grouped(count))
            }
        }
    };
    html! {
        div class="toolbar" {
            nav class="segmented" aria-label="Token status" {
                (segment(StatusFilter::All, "All", counts.all))
                (segment(StatusFilter::Active, "Active", counts.active))
                (segment(StatusFilter::Inactive, "Expired & revoked", counts.inactive))
            }
            @if deletable > 0 {
                p class="toolbar__note push-end" data-select-empty=[selecting.then_some("#tokens-table")] {
                    (grouped(deletable)) " expired or revoked"
                }
                @if selecting {
                    form class="cluster push-end" id="delete-selected" method="post" action=(view.url("/tokens?_method=DELETE"))
                        data-select-bar="#tokens-table"
                        data-confirm=(format!("Permanently delete the selected tokens? {DELETE_NOTE}"))
                        data-confirm-title="Delete tokens?" data-confirm-label="Delete tokens" data-confirm-tone="critical" hidden {
                        (context.csrf_field())
                        span class="toolbar__note" { span data-select-count="#tokens-table" { "0" } " selected" }
                        button type="submit" class="button" { "Delete selected" }
                    }
                }
                form method="post" action=(view.url("/tokens?_method=DELETE")) data-confirm=(delete_message(deletable))
                    data-confirm-title="Delete all expired and revoked tokens?" data-confirm-label="Delete all"
                    data-confirm-tone="critical" {
                    (context.csrf_field())
                    @for id in page.deletable_ids { input type="hidden" name="ids[]" value=(id); }
                    button type="submit" class="button" { (icon("trash")) "Delete all" }
                }
            }
        }
    }
}

fn token_list(context: &PageContext, page: &TokensPage<'_>) -> Markup {
    let view = &page.view;
    if page.counts.all == 0 {
        // No token at all: no toolbar, and the empty state takes the place of the table.
        return html! {
            div class="card" {
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("key-round")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No tokens yet" }
                        p class="empty-state__description" { "Create a token so agents can call the gateway." }
                    }
                }
            }
        };
    }

    let selecting = view.status == StatusFilter::Inactive;
    let several_pages = page.total > ROWS_PER_PAGE;
    html! {
        (toolbar(context, page))

        @if page.rows.is_empty() {
            div class="card" {
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("key-round")) }
                    div class="empty-state__text" {
                        @if view.status == StatusFilter::Active {
                            p class="empty-state__title" { "No active tokens" }
                            p class="empty-state__description" { "Create a token so agents can call the gateway." }
                        } @else {
                            p class="empty-state__title" { "No expired or revoked tokens" }
                            p class="empty-state__description" { "Tokens that expire or are revoked can be deleted from here." }
                        }
                    }
                    a class="button button--secondary" href="/tokens" { "Show all tokens" }
                }
            }
        } @else {
            div class="card hide-mobile" {
                div class="table-scroll" {
                    table class="table" id="tokens-table" {
                        thead { tr {
                            @if selecting {
                                th scope="col" {
                                    div class="cell-media" {
                                        input type="checkbox" class="checkbox" data-select-all="#tokens-table" aria-label="Select all";
                                        span { "Name" }
                                    }
                                }
                            } @else {
                                th scope="col" { "Name" }
                            }
                            th scope="col" class="col-80" { "Type" }
                            th scope="col" class="col-124" { "Identifier" }
                            th scope="col" class="col-72" { "Scope" }
                            th scope="col" class="col-124" { "Expires" }
                            th scope="col" class="col-124" { "Last used" }
                            th scope="col" class="col-84" { "Status" }
                            th scope="col" class="col-124 cell-end" { "Actions" }
                        } }
                        tbody { @for row in page.rows { (table_row(context, view, row)) } }
                    }
                }
                div class="table-footer" {
                    p class="table-footer__note" { (ROWS_PER_PAGE) " rows per page" }
                    (pagination(page))
                }
            }

            // Under 768px: the same tokens as a list.
            section class="card hide-desktop" aria-label="Access tokens" {
                @for row in page.rows { (mobile_row(view, row)) }
                @if several_pages { div class="table-footer" { (pagination(page)) } }
            }
        }
    }
}

fn gateway_card(page: &TokensPage<'_>) -> Markup {
    html! {
        section class="card" aria-labelledby="gateway-url-title" {
            div class="card__body" {
                div class="heading" {
                    h2 class="heading__title" id="gateway-url-title" { "Gateway URL" }
                    p class="heading__description" {
                        "MCP clients can sign in with OAuth. Manual integrations can still send Authorization: Bearer <token> on every request."
                    }
                }
                div class="cluster" {
                    @match page.gateway_url {
                        Some(gateway_url) => div class="copy-field" {
                            span class="copy-field__value" id="gateway-url" { (gateway_url) }
                            button type="button" class="icon-button" data-copy-target="#gateway-url" aria-label="Copy gateway URL" {
                                (icon("copy")) (icon("check"))
                            }
                        },
                        None => div class="copy-field" {
                            span class="copy-field__value" { "Configure APP_URL to reveal the gateway URL." }
                            button type="button" class="icon-button" disabled title="Set APP_URL to enable public links"
                                aria-label="Copy gateway URL" { (icon("copy")) }
                        },
                    }
                    a class="button button--secondary" href=(page.view.url("/tokens/install")) data-dialog-open="#install" {
                        (icon("download")) "Install MCP"
                    }
                }
                (banner("", "info", "Reduce tool-definition overhead", Some("Optional: configure your MCP client to send X-MyMCPs-Tool-Mode: lazy. The gateway then exposes list_mcps, tool_search, and call_tool instead of loading every upstream tool definition.")))
            }
        }
    }
}

/// `GET /tokens`, and the same page with one of its dialogs open.
pub fn tokens_page(context: &PageContext, page: &TokensPage<'_>) -> Markup {
    let list_url = page.view.url("/tokens");
    let blank = FormState::default();
    let content = html! {
        // Shown once, on the page that follows the creation: the plaintext
        // exists nowhere else afterwards. A banner, not a toast, so that the
        // sentence stays with the token.
        @if let Some(plaintext) = page.created_plaintext {
            div class="banner banner--success" role="status" {
                (icon("circle-check"))
                div class="banner__content gap-200" {
                    p class="banner__title" {
                        (page.created_message.unwrap_or("Access token created"))
                    }
                    div class="cluster" {
                        div class="copy-field copy-field--wrap" {
                            span class="copy-field__value" id="created-token" { (plaintext) }
                            button type="button" class="icon-button" data-copy-target="#created-token"
                                aria-label="Copy the new access token to the clipboard" { (icon("copy")) (icon("check")) }
                        }
                    }
                }
            }
        }

        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Access tokens" }
                p class="page-header__subtitle" {
                    "Manual access tokens and OAuth connections used by MCP clients. Revoking an OAuth connection stops both its access and refresh tokens."
                }
            }
            div class="page-header__actions" {
                a class="button button--primary" href=(page.view.url("/tokens/new")) data-dialog-open="#create-token" {
                    (icon("plus")) "Create token"
                }
            }
        }

        (gateway_card(page))
        (token_list(context, page))
    };

    let overlays = html! {
        dialog class="dialog dialog--sm" id="create-token" aria-labelledby="create-token-title"
            data-open[matches!(page.open, OpenDialog::Create)] data-dialog-reset data-dialog-return=(list_url) {
            div data-fragment { (create_form(context, &blank, page.mcps)) }
        }
        // Filled by "Edit" (`GET /tokens/{id}/edit`), or drawn open when the URL names a token.
        dialog class="dialog dialog--sm" id="edit-token" aria-labelledby="edit-token-title"
            data-open[matches!(page.open, OpenDialog::Edit(_))] data-dialog-return=(list_url) {
            div data-fragment {
                @if let OpenDialog::Edit(row) = &page.open { (edit_form(context, &page.view, row, &blank, page.mcps)) }
            }
        }
        (install_dialog(page, &list_url))
    };

    app_page(context, "Access tokens", content, overlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        for (number, text) in [
            (0, "0"),
            (7, "7"),
            (999, "999"),
            (1000, "1,000"),
            (8412, "8,412"),
            (1234567, "1,234,567"),
        ] {
            assert_eq!(grouped(number), text);
        }
    }

    #[test]
    fn a_view_names_itself_in_links() {
        let all = ListView {
            status: StatusFilter::All,
            page: 1,
        };
        assert_eq!(all.url("/tokens"), "/tokens");
        assert_eq!(all.url("/tokens?_method=DELETE"), "/tokens?_method=DELETE");

        let inactive = ListView {
            status: StatusFilter::Inactive,
            page: 3,
        };
        assert_eq!(inactive.query(), "status=inactive&page=3");
        assert_eq!(
            inactive.url("/tokens/4/edit"),
            "/tokens/4/edit?status=inactive&page=3"
        );
        assert_eq!(
            inactive.url("/tokens?_method=DELETE"),
            "/tokens?_method=DELETE&status=inactive&page=3"
        );
        assert_eq!(StatusFilter::parse(Some("active")), StatusFilter::Active);
        assert_eq!(StatusFilter::parse(Some("other")), StatusFilter::All);
        assert_eq!(StatusFilter::parse(None), StatusFilter::All);
    }
}
