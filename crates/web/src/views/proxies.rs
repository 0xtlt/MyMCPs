//! The Proxies page: the Tailscale nodes a team runs, and the dialogs that
//! add and change one. A proxy is a device of a tailnet that MyMCPs starts
//! itself; an HTTP MCP picks it in its Network field.

use std::net::IpAddr;

use maud::{Markup, html};
use mymcps_core::models::MAX_PROXIES_PER_TEAM;
use mymcps_vine::ValidationError;
use serde_json::{Map, Value};
use url::Url;

use crate::proxy_nodes::{Availability, NodeState, NodeStatus};
use crate::validators::proxy::DEFAULT_NODE_NAME;
use crate::views::grouped;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};
use crate::views::teams::card_header;

/// The id of the edit dialog, which the page script names when it asks for
/// the dialog's content alone.
pub const EDIT_DIALOG_ID: &str = "edit-proxy";
/// The id of the dialog that adds a proxy.
pub const CREATE_DIALOG_ID: &str = "add-proxy";

/// Where an admin of a tailnet creates auth keys.
const KEYS_URL: &str = "https://login.tailscale.com/admin/settings/keys";

/// The field a refusal that belongs to no field of the form is filed under.
pub const FORM_ERROR_FIELD: &str = "proxy";

/// What a proxy is, as far as the page shows it. It never holds the auth
/// key, decrypted or not: only whether one is saved.
#[derive(Debug, Clone)]
pub struct ProxyView {
    pub id: i64,
    pub name: String,
    pub hostname: String,
    pub has_key: bool,
    /// What the node was last seen doing.
    pub status: NodeStatus,
    /// How many MCPs go through it.
    pub used_by: usize,
}

/// A login address Tailscale gave, when it is one a browser may be sent to.
fn web_address(address: &str) -> Option<&str> {
    Url::parse(address)
        .ok()
        .filter(|url| matches!(url.scheme(), "https" | "http"))
        .map(|_| address)
}

impl ProxyView {
    /// The link that signs the node in to Tailscale, once Tailscale gave one.
    pub fn login_url(&self) -> Option<&str> {
        match &self.status.state {
            NodeState::NeedsLogin {
                auth_url: Some(address),
            } => web_address(address),
            _ => None,
        }
    }

    /// `Tailscale · tailnet`, or `Tailscale` while the tailnet is not known.
    fn network_label(&self) -> String {
        match self
            .status
            .tailnet
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            Some(tailnet) => format!("Tailscale · {tailnet}"),
            None => "Tailscale".to_string(),
        }
    }

    /// The first Tailscale address: IPv4 when the node has one.
    fn first_address(&self) -> Option<IpAddr> {
        self.status
            .addresses
            .iter()
            .copied()
            .find(IpAddr::is_ipv4)
            .or_else(|| self.status.addresses.first().copied())
    }

    /// The name in the tailnet and the address under it, as the list writes
    /// them. `None` while the node has neither.
    fn address_lines(&self) -> Option<(String, Option<String>)> {
        let address = self.first_address().map(|address| address.to_string());
        match (
            self.status.dns_name.clone().filter(|name| !name.is_empty()),
            address,
        ) {
            (Some(name), address) => Some((name, address)),
            (None, Some(address)) => Some((address, None)),
            (None, None) => None,
        }
    }

    /// The subtitle of a row of the mobile list, as a sentence:
    /// `Tailscale · tailnet · 2 MCPs`, or `Tailscale · not used`.
    fn list_subtitle(&self) -> String {
        let usage = match self.used_by {
            0 => "not used".to_string(),
            _ => self.used_by_label(),
        };
        format!("{} · {usage}", self.network_label())
    }

    fn used_by_label(&self) -> String {
        match self.used_by {
            0 => "None".to_string(),
            1 => "1 MCP".to_string(),
            count => format!("{} MCPs", grouped(count)),
        }
    }
}

/// How a node reads in the list.
pub fn state_label(state: &NodeState) -> &'static str {
    match state {
        NodeState::Connected => "Connected",
        NodeState::NeedsLogin { .. } => "Needs login",
        NodeState::NeedsApproval => "Waiting for approval",
        NodeState::Starting => "Starting",
        NodeState::Failed { .. } => "Error",
        NodeState::Stopped => "Stopped",
    }
}

/// The tone of a state: what needs the person is a warning, what is wrong
/// is critical.
pub fn state_tone(state: &NodeState) -> &'static str {
    match state {
        NodeState::Connected => "success",
        NodeState::NeedsLogin { .. } | NodeState::NeedsApproval => "warning",
        NodeState::Failed { .. } => "critical",
        NodeState::Starting | NodeState::Stopped => "neutral",
    }
}

/// What the fields of the form hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormValues {
    pub name: String,
    pub hostname: String,
    /// Only what was typed in the request being answered: a saved key is never read back.
    pub auth_key: String,
}

impl FormValues {
    /// A new proxy: its node takes the default name.
    pub fn blank() -> Self {
        Self {
            name: String::new(),
            hostname: DEFAULT_NODE_NAME.to_string(),
            auth_key: String::new(),
        }
    }

    /// The form of a saved proxy. Its key stays where it is.
    pub fn of_proxy(proxy: &ProxyView) -> Self {
        Self {
            name: proxy.name.clone(),
            hostname: proxy.hostname.clone(),
            auth_key: String::new(),
        }
    }

    /// The form as it was submitted, to draw it again when it is refused.
    pub fn of_input(input: &Map<String, Value>) -> Self {
        let text = |field: &str| match input.get(field) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        Self {
            name: text("name"),
            hostname: text("tailscaleHostname"),
            auth_key: text("tailscaleAuthKey"),
        }
    }
}

/// The state the form is drawn in: its values, and what was refused.
#[derive(Debug, Clone)]
pub struct ProxyForm {
    pub values: FormValues,
    /// The first message of each field that was refused, in field order.
    errors: Vec<(String, String)>,
}

impl ProxyForm {
    pub fn new(values: FormValues) -> Self {
        Self {
            values,
            errors: Vec::new(),
        }
    }

    /// A form sent back with what is wrong with its fields.
    pub fn refused(values: FormValues, error: &ValidationError) -> Self {
        let mut errors: Vec<(String, String)> = Vec::new();
        for refusal in &error.messages {
            if !errors.iter().any(|(field, _)| *field == refusal.field) {
                errors.push((refusal.field.clone(), refusal.message.clone()));
            }
        }
        Self { values, errors }
    }

    /// A form sent back with a refusal that is about none of its fields.
    pub fn refused_as_a_whole(values: FormValues, message: impl Into<String>) -> Self {
        Self {
            values,
            errors: vec![(FORM_ERROR_FIELD.to_string(), message.into())],
        }
    }

    fn error(&self, field: &str) -> Option<&str> {
        self.errors
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, message)| message.as_str())
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn first_error(&self) -> Option<&str> {
        self.errors.first().map(|(_, message)| message.as_str())
    }
}

/// The dialog that edits a saved proxy.
pub struct EditDialog<'a> {
    pub proxy: &'a ProxyView,
    pub form: &'a ProxyForm,
}

/// What the Proxies page shows.
pub struct ProxiesPage<'a> {
    /// The proxies of the team, by name.
    pub rows: &'a [ProxyView],
    pub availability: &'a Availability,
    /// Why a proxy cannot be added now, when it cannot.
    pub creation_refusal: Option<&'a str>,
    /// The form of the dialog that adds a proxy.
    pub create_form: &'a ProxyForm,
    /// The dialog of a saved proxy, when the page opens on one.
    pub editing: Option<&'a EditDialog<'a>>,
}

// ------------------------------------------------------------------------------- banners

fn banner(tone: &str, title: &str, body: Markup, action: Markup) -> Markup {
    let icon_name = match tone {
        "warning" => "triangle-alert",
        "critical" => "circle-x",
        _ => "info",
    };
    // Information is the banner with no tone.
    let class = if tone.is_empty() {
        "banner".to_string()
    } else {
        format!("banner banner--{tone}")
    };
    html! {
        div class=(class) role=(if tone == "critical" { "alert" } else { "status" }) {
            (icon(icon_name))
            div class="banner__content" {
                p class="banner__title" { (title) }
                (body)
            }
            (action)
        }
    }
}

/// Whether Tailscale proxies can run on this server, when they cannot.
fn availability_banner(availability: &Availability) -> Markup {
    match availability {
        Availability::Available { .. } => html! {},
        Availability::Unavailable { reason } => banner(
            "warning",
            "Tailscale proxies cannot start",
            html! { p { (reason) } },
            html! {},
        ),
        Availability::Disabled => banner(
            "",
            "Tailscale proxies are off",
            html! { p { "A super admin turns them on with TAILSCALE_MAX_PROXIES." } },
            html! {},
        ),
    }
}

fn login_button(address: &str) -> Markup {
    html! {
        a class="button button--secondary" href=(address) target="_blank" rel="noopener noreferrer" {
            (icon("external-link")) "Log in to Tailscale"
        }
    }
}

fn reconnect_form(context: &PageContext, proxy: &ProxyView) -> Markup {
    html! {
        form method="post" action=(format!("/proxies/{}/reconnect", proxy.id)) {
            (context.csrf_field())
            button type="submit" class="button button--secondary" { (icon("refresh-cw")) "Reconnect" }
        }
    }
}

/// The action of a banner that sends the person to the edit dialog: the
/// dialog when the script runs, the edit page without it.
fn edit_button(proxy: &ProxyView) -> Markup {
    html! {
        a class="button button--secondary" href=(edit_href(proxy)) data-dialog-open="#edit-proxy" data-dialog-fetch { "Edit" }
    }
}

/// What a proxy asks of the person, one banner for each, above the list.
fn attention(context: &PageContext, rows: &[ProxyView]) -> Markup {
    html! {
        @for proxy in rows {
            @match &proxy.status.state {
                NodeState::NeedsLogin { .. } => {
                    @if let Some(address) = proxy.login_url() {
                        (banner(
                            "warning",
                            &format!("{} needs a Tailscale login", proxy.name),
                            html! { p { "Sign in to Tailscale to add this proxy to your tailnet. You can also edit it and paste an auth key." } },
                            login_button(address),
                        ))
                    } @else {
                        (banner(
                            "warning",
                            &format!("{} needs a Tailscale login", proxy.name),
                            html! {
                                p { "Tailscale has not given a login link yet. Paste an auth key in the proxy to log it in, or wait for the link." }
                                @if let Some(issue) = proxy.status.health.first() { p { (issue) } }
                            },
                            edit_button(proxy),
                        ))
                    }
                }
                NodeState::NeedsApproval => {
                    (banner(
                        "warning",
                        &format!("{} is waiting for approval", proxy.name),
                        html! { p { "Approve the device under Machines in the Tailscale admin console." } },
                        html! {},
                    ))
                }
                NodeState::Failed { error } => {
                    (banner(
                        "critical",
                        &format!("{} could not connect", proxy.name),
                        html! { p { (error) } },
                        reconnect_form(context, proxy),
                    ))
                }
                NodeState::Starting | NodeState::Connected | NodeState::Stopped => {}
            }
        }
    }
}

// ------------------------------------------------------------------------------ the list

fn edit_href(proxy: &ProxyView) -> String {
    format!("/proxies/{}/edit", proxy.id)
}

fn row_menu(context: &PageContext, proxy: &ProxyView) -> Markup {
    html! {
        div class="menu" id=(format!("proxy-{}-menu", proxy.id)) popover role="menu" {
            @if let Some(address) = proxy.login_url() {
                a class="menu__item" role="menuitem" href=(address) target="_blank" rel="noopener noreferrer" {
                    (icon("external-link")) "Log in to Tailscale"
                }
            }
            form method="post" action=(format!("/proxies/{}/reconnect", proxy.id)) {
                (context.csrf_field())
                button type="submit" class="menu__item" role="menuitem" { (icon("refresh-cw")) "Reconnect" }
            }
            div class="menu__separator" {}
            form method="post" action=(format!("/proxies/{}?_method=DELETE", proxy.id))
                data-confirm=(format!("Delete {}?", proxy.name)) data-confirm-label="Delete" data-confirm-tone="critical" {
                (context.csrf_field())
                button type="submit" class="menu__item menu__item--critical" role="menuitem" { (icon("trash")) "Delete" }
            }
        }
    }
}

fn table_row(context: &PageContext, proxy: &ProxyView) -> Markup {
    let state = &proxy.status.state;
    let tone = state_tone(state);
    html! {
        tr {
            td {
                span class=(if tone == "neutral" { "status".to_string() } else { format!("status status--{tone}") }) {
                    (state_label(state))
                }
            }
            td {
                div class="cell-media" {
                    span class="tile" { (icon("cable")) }
                    div class="cell-stack" {
                        span class="cell-title" { (proxy.name) }
                        span class="cell-sub" { (proxy.network_label()) }
                    }
                }
            }
            @match proxy.address_lines() {
                Some((name, address)) => {
                    td {
                        div class="cell-stack" {
                            span class="cell-code" { (name) }
                            @if let Some(address) = address { span class="cell-sub cell-sub--code" { (address) } }
                        }
                    }
                }
                None => { td class="cell-tertiary" { "—" } }
            }
            td class="cell-secondary" { (proxy.used_by_label()) }
            td class="cell-end" {
                span class="row-actions" {
                    a class="button" id=(format!("proxy-{}-edit", proxy.id)) href=(edit_href(proxy))
                        data-dialog-open="#edit-proxy" data-dialog-fetch { "Edit" }
                    button type="button" class="icon-button" popovertarget=(format!("proxy-{}-menu", proxy.id))
                        aria-label=(format!("More actions for {}", proxy.name)) { (icon("ellipsis")) }
                }
                (row_menu(context, proxy))
            }
        }
    }
}

/// A row of the list shown under 768px: it opens the edit dialog, which
/// holds the actions the table has in its row menu.
fn list_row(proxy: &ProxyView) -> Markup {
    let state = &proxy.status.state;
    let tone = state_tone(state);
    html! {
        a class="list-row" href=(edit_href(proxy)) data-dialog-open="#edit-proxy" data-dialog-fetch {
            span class="tile" { (icon("cable")) }
            span class="list-row__text" {
                span class="list-row__title" { (proxy.name) }
                span class="list-row__subtitle" { (proxy.list_subtitle()) }
            }
            span class=(if tone == "neutral" { "badge".to_string() } else { format!("badge badge--{tone}") }) { (state_label(state)) }
        }
    }
}

fn summary(rows: &[ProxyView]) -> String {
    format!(
        "{} of {} proxies in this team",
        rows.len(),
        MAX_PROXIES_PER_TEAM
    )
}

/// The "Add proxy" button: it opens the dialog, or says it cannot.
fn add_button(refusal: Option<&str>, secondary: bool, hide_on_mobile: bool) -> Markup {
    let class = format!(
        "button {}{}",
        if secondary {
            "button--secondary"
        } else {
            "button--primary"
        },
        if hide_on_mobile { " hide-mobile" } else { "" }
    );
    html! {
        @if refusal.is_some() {
            button type="button" class=(class) aria-disabled="true" aria-describedby="add-proxy-reason" { (icon("plus")) "Add proxy" }
        } @else {
            button type="button" class=(class) data-dialog-open=(format!("#{CREATE_DIALOG_ID}")) { (icon("plus")) "Add proxy" }
        }
    }
}

fn empty_state(refusal: Option<&str>) -> Markup {
    html! {
        div class="card" {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("cable")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No proxies yet" }
                    p class="empty-state__description" { "Add a Tailscale proxy to reach MCPs that only listen on your tailnet." }
                }
                // Under 768px the header action sits right above the card: the second button would repeat it.
                (add_button(refusal, true, true))
            }
        }
    }
}

fn proxies_list(context: &PageContext, rows: &[ProxyView]) -> Markup {
    html! {
        section class="card hide-mobile" aria-labelledby="proxies-title" {
            (card_header("proxies-title", "Tailscale proxies", &summary(rows)))
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr {
                            th scope="col" class="col-136" { "Status" }
                            th scope="col" { "Name" }
                            th scope="col" { "Address" }
                            th scope="col" class="col-96" { "Used by" }
                            th scope="col" class="col-120 cell-end" { "Actions" }
                        }
                    }
                    tbody {
                        @for proxy in rows { (table_row(context, proxy)) }
                    }
                }
            }
        }

        section class="card hide-desktop" aria-labelledby="proxies-list-title" {
            (card_header("proxies-list-title", "Tailscale proxies", &summary(rows)))
            @for proxy in rows { (list_row(proxy)) }
        }
    }
}

/// The switch that follows the nodes as they connect.
fn live_switch() -> Markup {
    html! {
        div class="toolbar" {
            div class="live push-end" {
                span class="status-dot status-dot--success" {}
                span id="live-label" { "Live · refreshes every 5 s" }
                button type="button" class="switch" aria-pressed="false" aria-labelledby="live-label"
                    data-live-refresh="#proxies-data" data-live-interval="5000" {}
            }
        }
    }
}

// ----------------------------------------------------------------------------- the dialogs

/// A text field of the form: its label, its control, then its error and its help line.
struct Field<'a> {
    id: String,
    name: &'a str,
    label: Markup,
    kind: &'a str,
    value: &'a str,
    error: Option<&'a str>,
    help: Option<Markup>,
    autofocus: bool,
    required: bool,
    max_length: Option<usize>,
    /// An identifier, typed as it is: no capital added, no spelling checked.
    verbatim: bool,
}

impl Field<'_> {
    fn render(self) -> Markup {
        let error_id = format!("{}-error", self.id);
        let help_id = format!("{}-help", self.id);
        let described_by = match (self.error.is_some(), self.help.is_some()) {
            (true, true) => Some(format!("{error_id} {help_id}")),
            (true, false) => Some(error_id.clone()),
            (false, true) => Some(help_id.clone()),
            (false, false) => None,
        };
        html! {
            div class="field" {
                label class="field__label" for=(self.id) { (self.label) }
                input class=(if self.verbatim { "input input--mono" } else { "input" }) id=(self.id) name=(self.name) type=(self.kind)
                    value=[(!self.value.is_empty()).then_some(self.value)] maxlength=[self.max_length] autocomplete="off"
                    autocapitalize=[self.verbatim.then_some("off")] spellcheck=[self.verbatim.then_some("false")]
                    autofocus[self.autofocus] required[self.required]
                    aria-invalid=[self.error.map(|_| "true")] aria-describedby=[described_by];
                @if let Some(message) = self.error { p class="field__error" id=(error_id) { (message) } }
                @if let Some(help) = self.help { p class="field__help" id=(help_id) { (help) } }
            }
        }
    }
}

fn optional_mark() -> Markup {
    html! { span class="field__optional" { "Optional" } }
}

/// Name, node name and auth key. `saved_key` is whether the proxy being
/// edited holds a key, which a blank field keeps.
fn fields(prefix: &str, form: &ProxyForm, saved_key: Option<bool>) -> Markup {
    let values = &form.values;
    html! {
        (Field {
            id: format!("{prefix}-name"),
            name: "name",
            label: html! { "Name" },
            kind: "text",
            value: &values.name,
            error: form.error("name"),
            help: Some(html! { "Shown in this list and in the Network field of an MCP." }),
            // After a refusal, the first field to correct takes the focus.
            autofocus: !form.has_errors(),
            required: true,
            max_length: Some(100),
            verbatim: false,
        }.render())
        (Field {
            id: format!("{prefix}-hostname"),
            name: "tailscaleHostname",
            label: html! { "Node name" },
            kind: "text",
            value: &values.hostname,
            error: form.error("tailscaleHostname"),
            help: Some(html! { "Its name in your tailnet. Lowercase letters, digits, and hyphens." }),
            autofocus: false,
            required: true,
            max_length: Some(63),
            verbatim: true,
        }.render())
        (Field {
            id: format!("{prefix}-auth-key"),
            name: "tailscaleAuthKey",
            label: match saved_key {
                Some(true) => html! { "Auth key (leave blank to keep)" },
                _ => html! { "Auth key " (optional_mark()) },
            },
            kind: "password",
            value: &values.auth_key,
            error: form.error("tailscaleAuthKey"),
            help: Some(html! {
                "From "
                a class="link" href=(KEYS_URL) target="_blank" rel="noopener noreferrer" {
                    "Keys in the Tailscale admin console" (icon("external-link"))
                }
                ". A reusable, tagged key logs the node in again by itself and does not expire. Without a key, a login link appears here after you save."
            }),
            autofocus: false,
            required: false,
            max_length: Some(500),
            verbatim: true,
        }.render())
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

/// What is wrong with the form that none of its fields says.
fn form_error(form: &ProxyForm, title: &str) -> Markup {
    match form.error(FORM_ERROR_FIELD) {
        Some(message) => banner("critical", title, html! { p { (message) } }, html! {}),
        None => html! {},
    }
}

/// The content of the create dialog: what its `[data-fragment]` holds, and
/// what a refused submission of the page's script gets back.
pub fn create_fragment(context: &PageContext, form: &ProxyForm) -> Markup {
    html! {
        (dialog_header("add-proxy-title", "Add proxy", "Run a Tailscale node that MCPs can reach your tailnet through"))
        form class="dialog__body" id="add-proxy-form" method="post" action="/proxies" data-async {
            (context.csrf_field())
            div class="stack" {
                (form_error(form, "This proxy could not be added"))
                (fields("add-proxy", form, None))
            }
        }
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="add-proxy-form" { "Add proxy" }
        }
    }
}

/// What the node is doing now, in the edit dialog.
fn live_status(proxy: &ProxyView) -> Markup {
    let status = &proxy.status;
    let tone = state_tone(&status.state);
    html! {
        dl class="key-values" {
            div class="key-value" {
                dt { "Status" }
                dd { span class=(if tone == "neutral" { "badge".to_string() } else { format!("badge badge--{tone}") }) { (state_label(&status.state)) } }
            }
            div class="key-value" {
                dt { "Tailnet" }
                dd { (status.tailnet.as_deref().filter(|name| !name.is_empty()).unwrap_or("—")) }
            }
            div class="key-value" {
                dt { "Address" }
                dd {
                    @match proxy.address_lines() {
                        Some((name, address)) => {
                            (name)
                            @if let Some(address) = address { br; (address) }
                        }
                        None => { "—" }
                    }
                }
            }
            @if !status.health.is_empty() {
                div class="key-value" {
                    dt { "Health issues" }
                    dd { @for issue in &status.health { p { (issue) } } }
                }
            }
        }
        @if let NodeState::Failed { error } = &status.state {
            p class="text-critical" { (error) }
        }
    }
}

/// The content of the edit dialog: what its `[data-fragment]` holds, what
/// the list fetches to open it, and what a refused save gets back.
pub fn edit_fragment(context: &PageContext, dialog: &EditDialog) -> Markup {
    let proxy = dialog.proxy;
    let form = dialog.form;
    html! {
        (dialog_header("edit-proxy-title", &format!("Edit {}", proxy.name), &proxy.network_label()))
        form class="dialog__body" id="edit-proxy-form" method="post" action=(format!("/proxies/{}?_method=PUT", proxy.id)) data-async {
            (context.csrf_field())
            div class="stack" {
                (form_error(form, "This proxy could not be saved"))
                (live_status(proxy))
                div class="cluster" {
                    @if let Some(address) = proxy.login_url() { (login_button(address)) }
                    button type="submit" class="button button--secondary" form="edit-proxy-reconnect" { (icon("refresh-cw")) "Reconnect" }
                }
                (fields("edit-proxy", form, Some(proxy.has_key)))
            }
        }
        div class="dialog__footer" {
            button type="submit" class="button button--critical dialog__footer-start" form="edit-proxy-delete" { "Delete" }
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="edit-proxy-form" { "Save changes" }
        }
        // The other requests of the dialog are forms of their own: their
        // buttons name them, so Enter in a field always saves.
        form id="edit-proxy-reconnect" method="post" action=(format!("/proxies/{}/reconnect", proxy.id)) hidden { (context.csrf_field()) }
        form id="edit-proxy-delete" method="post" action=(format!("/proxies/{}?_method=DELETE", proxy.id))
            data-confirm=(format!("Delete {}?", proxy.name)) data-confirm-label="Delete" data-confirm-tone="critical" hidden {
            (context.csrf_field())
        }
    }
}

fn create_dialog(context: &PageContext, form: &ProxyForm) -> Markup {
    html! {
        dialog class="dialog" id=(CREATE_DIALOG_ID) aria-labelledby="add-proxy-title" data-dialog-static data-dialog-reset {
            div data-fragment { (create_fragment(context, form)) }
        }
    }
}

fn edit_dialog(context: &PageContext, editing: Option<&EditDialog>) -> Markup {
    html! {
        dialog class="dialog" id=(EDIT_DIALOG_ID) aria-labelledby="edit-proxy-title" data-dialog-static data-dialog-return="/proxies"
            data-open[editing.is_some()] data-dialog-trigger=[editing.map(|dialog| format!("#proxy-{}-edit", dialog.proxy.id))] {
            div data-fragment {
                @if let Some(dialog) = editing { (edit_fragment(context, dialog)) }
            }
        }
    }
}

// ------------------------------------------------------------------------------- the page

/// `GET /proxies`, and the page behind the address of a proxy's dialog.
pub fn proxies_page(context: &PageContext, page: &ProxiesPage) -> Markup {
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Proxies" }
                p class="page-header__subtitle" {
                    "Reach MCPs on a private network through a Tailscale node that MyMCPs runs. Choose a proxy in the Network field of an HTTP MCP."
                }
            }
            div class="page-header__actions" {
                (add_button(page.creation_refusal, false, false))
            }
        }
        @if let Some(reason) = page.creation_refusal {
            p class="text-body-sm text-secondary" id="add-proxy-reason" { (reason) }
        }
        (availability_banner(page.availability))
        @if page.rows.is_empty() {
            (empty_state(page.creation_refusal))
        } @else {
            (live_switch())
            div class="page__body" id="proxies-data" {
                (attention(context, page.rows))
                (proxies_list(context, page.rows))
            }
        }
    };
    let overlays = html! {
        (create_dialog(context, page.create_form))
        (edit_dialog(context, page.editing))
    };
    app_page(context, "Proxies", content, overlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(state: NodeState) -> ProxyView {
        ProxyView {
            id: 1,
            name: "Home".into(),
            hostname: "mymcps".into(),
            has_key: false,
            status: NodeStatus {
                state,
                tailnet: None,
                dns_name: None,
                addresses: Vec::new(),
                health: Vec::new(),
            },
            used_by: 0,
        }
    }

    #[test]
    fn names_every_state_and_its_tone() {
        for (state, label, tone) in [
            (NodeState::Connected, "Connected", "success"),
            (
                NodeState::NeedsLogin { auth_url: None },
                "Needs login",
                "warning",
            ),
            (NodeState::NeedsApproval, "Waiting for approval", "warning"),
            (NodeState::Starting, "Starting", "neutral"),
            (NodeState::Failed { error: "x".into() }, "Error", "critical"),
            (NodeState::Stopped, "Stopped", "neutral"),
        ] {
            assert_eq!(state_label(&state), label);
            assert_eq!(state_tone(&state), tone);
        }
    }

    #[test]
    fn offers_a_login_link_only_for_a_web_address() {
        let login = |address: &str| {
            view(NodeState::NeedsLogin {
                auth_url: Some(address.into()),
            })
        };
        assert_eq!(
            login("https://login.tailscale.com/a/abc").login_url(),
            Some("https://login.tailscale.com/a/abc")
        );
        for refused in ["javascript:alert(1)", "not a url", "data:text/html,x"] {
            assert_eq!(login(refused).login_url(), None, "{refused}");
        }
        assert_eq!(view(NodeState::Connected).login_url(), None);
        assert_eq!(
            view(NodeState::NeedsLogin { auth_url: None }).login_url(),
            None
        );
    }

    #[test]
    fn writes_the_address_and_the_network_of_a_node() {
        let mut proxy = view(NodeState::Connected);
        assert_eq!(proxy.address_lines(), None);
        assert_eq!(proxy.network_label(), "Tailscale");

        proxy.status.tailnet = Some("example.ts.net".into());
        proxy.status.dns_name = Some("mymcps.example.ts.net".into());
        proxy.status.addresses = vec![
            "fd7a:115c:a1e0::1".parse().unwrap(),
            "100.64.0.7".parse().unwrap(),
        ];
        assert_eq!(proxy.network_label(), "Tailscale · example.ts.net");
        assert_eq!(
            proxy.address_lines(),
            Some(("mymcps.example.ts.net".into(), Some("100.64.0.7".into())))
        );

        proxy.status.dns_name = None;
        assert_eq!(proxy.address_lines(), Some(("100.64.0.7".into(), None)));
    }

    #[test]
    fn writes_the_subtitle_of_a_row_of_the_mobile_list() {
        let mut proxy = view(NodeState::Connected);
        assert_eq!(proxy.list_subtitle(), "Tailscale · not used");
        proxy.status.tailnet = Some("example.ts.net".into());
        assert_eq!(
            proxy.list_subtitle(),
            "Tailscale · example.ts.net · not used"
        );
        proxy.used_by = 1;
        assert_eq!(proxy.list_subtitle(), "Tailscale · example.ts.net · 1 MCP");
        proxy.used_by = 2;
        assert_eq!(proxy.list_subtitle(), "Tailscale · example.ts.net · 2 MCPs");
        proxy.status.tailnet = None;
        assert_eq!(proxy.list_subtitle(), "Tailscale · 2 MCPs");
    }

    #[test]
    fn counts_the_mcps_of_a_proxy() {
        let mut proxy = view(NodeState::Connected);
        assert_eq!(proxy.used_by_label(), "None");
        proxy.used_by = 1;
        assert_eq!(proxy.used_by_label(), "1 MCP");
        proxy.used_by = 2;
        assert_eq!(proxy.used_by_label(), "2 MCPs");
    }
}
