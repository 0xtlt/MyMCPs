//! The Tailnets page: the Tailscale networks a team connects, and the dialog
//! that connects another. (Figma frames "Tailnets", "Tailnets empty" and
//! "Connect a tailnet")

use maud::{Markup, html};
use mymcps_tailnet::TailnetState;
use url::Url;

use crate::validators::tailnet::SignInMethod;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page_with_head};

/// The id of the dialog that connects a tailnet.
pub const CONNECT_DIALOG_ID: &str = "connect-tailnet";

/// How often a page that waits for a tailnet asks for itself again.
const REFRESH_SECONDS: u32 = 5;

/// What the list shows of a tailnet.
#[derive(Debug, Clone)]
pub struct TailnetRow {
    pub id: i64,
    pub name: String,
    /// The control server of a Headscale. `None` for Tailscale's own.
    pub control_url: Option<String>,
    /// Where it stands. `None` when the server runs no tailnets.
    pub state: Option<TailnetState>,
    /// The MCPs that reach their upstream through it, by name.
    pub mcps: Vec<String>,
}

/// How a tailnet is told apart in the list: a tone and a label.
struct Standing {
    tone: &'static str,
    label: &'static str,
    /// What the second line of the status cell says.
    note: Option<String>,
    critical_note: bool,
}

impl TailnetRow {
    fn standing(&self) -> Standing {
        let plain = |tone, label| Standing {
            tone,
            label,
            note: None,
            critical_note: false,
        };
        match &self.state {
            None => plain("neutral", "Unavailable"),
            Some(TailnetState::Starting) => plain("neutral", "Starting"),
            Some(TailnetState::NeedsLogin { auth_url }) => Standing {
                tone: "warning",
                label: "Needs sign-in",
                note: auth_url
                    .as_ref()
                    .is_none_or(|url| !is_web_link(url))
                    .then(|| "Waiting for the sign-in link".to_string()),
                critical_note: false,
            },
            Some(TailnetState::NeedsMachineAuth) => Standing {
                tone: "warning",
                label: "Needs approval",
                note: Some("Approve this device in the admin console".to_string()),
                critical_note: false,
            },
            Some(TailnetState::Running { .. }) => plain("success", "Connected"),
            Some(TailnetState::Stopped) => plain("neutral", "Stopped"),
            Some(TailnetState::Failed(reason)) => Standing {
                tone: "critical",
                label: "Error",
                note: Some(reason.clone()),
                critical_note: true,
            },
        }
    }

    /// The name of this server in the tailnet, once it is connected.
    fn device(&self) -> Option<&str> {
        match &self.state {
            Some(TailnetState::Running { dns_name, .. }) if !dns_name.is_empty() => {
                Some(dns_name)
            }
            _ => None,
        }
    }

    /// The link that signs the device in, when the tailnet waits for it. Only
    /// a web address is linked: the control server of a Headscale chooses it.
    fn sign_in_url(&self) -> Option<&Url> {
        match &self.state {
            Some(TailnetState::NeedsLogin {
                auth_url: Some(url),
            }) if is_web_link(url) => Some(url),
            _ => None,
        }
    }

    /// Whether the tailnet is about to change by itself.
    fn is_settling(&self) -> bool {
        matches!(
            self.state,
            Some(
                TailnetState::Starting
                    | TailnetState::NeedsLogin { .. }
                    | TailnetState::NeedsMachineAuth
            )
        )
    }

    fn control_host(&self) -> Option<String> {
        let url = Url::parse(self.control_url.as_deref()?).ok()?;
        url.host_str().map(str::to_string)
    }
}

fn is_web_link(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// What the Connect dialog holds: what was typed, and what was refused.
#[derive(Debug, Clone)]
pub struct ConnectForm {
    pub name: String,
    pub method: SignInMethod,
    pub control_url: String,
    /// The first message of each field that was refused, in field order.
    errors: Vec<(String, String)>,
}

/// The name of the "field" that holds what is wrong with the form as a whole.
pub const FORM_ERROR: &str = "form";

impl Default for ConnectForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            method: SignInMethod::Link,
            control_url: String::new(),
            errors: Vec::new(),
        }
    }
}

impl ConnectForm {
    /// The form with what is wrong with it. The auth key is never part of
    /// it: a form that comes back is asked for it again.
    pub fn refused(
        name: &str,
        method: SignInMethod,
        control_url: &str,
        errors: Vec<(String, String)>,
    ) -> Self {
        Self {
            name: name.to_string(),
            method,
            control_url: control_url.to_string(),
            errors,
        }
    }

    pub fn first_error(&self) -> Option<&str> {
        self.errors.first().map(|(_, message)| message.as_str())
    }

    fn error(&self, field: &str) -> Option<&str> {
        self.errors
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, message)| message.as_str())
    }

    /// What the fields do not say: the cap, a refused connection.
    fn unshown(&self) -> Vec<&str> {
        self.errors
            .iter()
            .filter(|(field, _)| !["name", "method", "authKey", "controlUrl"].contains(&field.as_str()))
            .map(|(_, message)| message.as_str())
            .collect()
    }
}

/// What the Tailnets page shows.
pub struct TailnetsPage<'a> {
    pub rows: &'a [TailnetRow],
    /// Whether this server runs tailnets at all.
    pub enabled: bool,
    /// Whether the team has connected as many as it may.
    pub at_limit: bool,
    pub limit: usize,
    /// The dialog, open, with this form. `None`: closed.
    pub open: Option<&'a ConnectForm>,
}

impl TailnetsPage<'_> {
    fn can_connect(&self) -> bool {
        self.enabled && !self.at_limit
    }
}

fn connect_button(page: &TailnetsPage, class: &str) -> Markup {
    html! {
        @if page.can_connect() {
            a class=(class) href="/tailnets/new" data-dialog-open=(format!("#{CONNECT_DIALOG_ID}")) {
                (icon("plus")) "Connect a tailnet"
            }
        } @else {
            a class=(class) aria-disabled="true"
                title=(if page.enabled { format!("A team can connect up to {} tailnets", page.limit) } else { "Tailscale is off on this server".to_string() }) {
                (icon("plus")) "Connect a tailnet"
            }
        }
    }
}

fn off_banner() -> Markup {
    html! {
        div class="banner banner--warning" role="status" {
            (icon("triangle-alert"))
            div class="banner__content" {
                p class="banner__title" { "Tailscale is off on this server" }
                p {
                    "Tailnets need the " code class="code" { "tailscaled" } " program. Set "
                    code class="code" { "TAILSCALED_BINARY" }
                    " to its path, or install it where the server finds it, then restart MyMCPs. The Docker image already has it."
                }
            }
        }
    }
}

fn mcp_count(row: &TailnetRow) -> Markup {
    let count = row.mcps.len();
    html! {
        @if count == 0 {
            td class="cell-tertiary" { "—" }
        } @else {
            td class="cell-secondary" title=(row.mcps.join(", ")) {
                (count) @if count == 1 { " MCP" } @else { " MCPs" }
            }
        }
    }
}

fn table_row(context: &PageContext, row: &TailnetRow) -> Markup {
    let standing = row.standing();
    html! {
        tr {
            td {
                div class="cell-stack" {
                    span class="cell-title" { (row.name) }
                    @if let Some(host) = row.control_host() { span class="cell-sub" { "Headscale · " (host) } }
                }
            }
            td {
                div class="cell-stack" {
                    span { span class=(format!("badge badge--{}", standing.tone)) { (standing.label) } }
                    @if let Some(note) = &standing.note {
                        @if standing.critical_note {
                            span class="cell-sub cell-sub--critical" title=(note) { (note) }
                        } @else {
                            span class="cell-sub" { (note) }
                        }
                    }
                }
            }
            @match row.device() {
                Some(device) => { td class="cell-code" { (device) } }
                None => { td class="cell-tertiary" { "—" } }
            }
            (mcp_count(row))
            td class="cell-end" {
                span class="row-actions" {
                    @if let Some(url) = row.sign_in_url() {
                        // The page of the control server, which is not this site's.
                        a class="button button--secondary" href=(url.as_str()) target="_blank" rel="noopener noreferrer" { "Sign in" }
                    }
                    form method="post" action=(format!("/tailnets/{}?_method=DELETE", row.id))
                        data-confirm=(format!("Delete {}?", row.name)) data-confirm-label="Delete" data-confirm-tone="critical"
                        data-confirm-title="Delete this tailnet" {
                        (context.csrf_field())
                        button type="submit" class="button" aria-label=(format!("Delete {}", row.name)) { "Delete" }
                    }
                }
            }
        }
    }
}

fn table(context: &PageContext, page: &TailnetsPage) -> Markup {
    html! {
        div class="card" {
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr {
                            th scope="col" { "Name" }
                            th scope="col" class="col-200" { "Status" }
                            th scope="col" { "Device" }
                            th scope="col" class="col-96" { "Used by" }
                            th scope="col" class="col-160 cell-end" { "Actions" }
                        }
                    }
                    tbody { @for row in page.rows { (table_row(context, row)) } }
                }
            }
        }
    }
}

fn empty(page: &TailnetsPage) -> Markup {
    html! {
        div class="card" {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("network")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No tailnet yet" }
                    p class="empty-state__description" {
                        "Connect a Tailscale network, then choose it on an HTTP MCP to reach its devices."
                    }
                }
                // Under 768px the header action sits right above the card.
                @if page.can_connect() { (connect_button(page, "button button--secondary hide-mobile")) }
            }
        }
    }
}

fn dialog_header(title: &str, subtitle: &str) -> Markup {
    html! {
        div class="dialog__header" {
            div class="dialog__heading" {
                h2 class="dialog__title" id="connect-tailnet-title" { (title) }
                p class="dialog__subtitle" { (subtitle) }
            }
            button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
        }
    }
}

fn method_choice(form: &ConnectForm, method: SignInMethod, title: &str, description: &str) -> Markup {
    let value = match method {
        SignInMethod::Link => "link",
        SignInMethod::Key => "key",
    };
    html! {
        label class="choice" {
            input type="radio" class="radio" name="method" value=(value) checked[form.method == method];
            span class="choice__text" {
                span class="choice__label" { (title) }
                span class="choice__description" { (description) }
            }
        }
    }
}

/// The content of the dialog: what its `[data-fragment]` holds, and what a
/// refused submission gets back.
pub fn connect_fragment(context: &PageContext, form: &ConnectForm) -> Markup {
    let keyed = form.method == SignInMethod::Key;
    html! {
        (dialog_header("Connect a tailnet", "MyMCPs joins it as a device."))
        form class="dialog__body" id="connect-tailnet-form" method="post" action="/tailnets" data-async {
            (context.csrf_field())
            @for message in form.unshown() {
                div class="banner banner--critical" role="alert" {
                    (icon("circle-x"))
                    div class="banner__content" {
                        p class="banner__title" { "This tailnet could not be connected" }
                        p { (message) }
                    }
                }
            }
            div class="field" {
                label class="field__label" for="connect-tailnet-name" { "Name" }
                input class="input" id="connect-tailnet-name" name="name" type="text" value=(form.name) placeholder="pro"
                    autocomplete="off" autofocus aria-invalid=[form.error("name").map(|_| "true")]
                    aria-describedby=[form.error("name").map(|_| "connect-tailnet-name-error")];
                @if let Some(message) = form.error("name") { p class="field__error" id="connect-tailnet-name-error" { (message) } }
            }
            fieldset class="field-group" {
                legend class="field-group__label" { "Authentication" }
                div class="stack gap-200" {
                    (method_choice(form, SignInMethod::Link, "Sign in with Tailscale", "You get a sign-in link after Connect."))
                    (method_choice(form, SignInMethod::Key, "Auth key", "A key from the Tailscale admin console. Nothing to sign in to."))
                    div class="field" data-show-when="method=key" hidden[!keyed] {
                        label class="visually-hidden" for="connect-tailnet-key" { "Auth key" }
                        div class="input-group" {
                            // Never filled in: a key is typed again after a refusal.
                            input class="input-group__control" id="connect-tailnet-key" name="authKey" type="password"
                                placeholder="tskey-auth-…" autocomplete="off" autocapitalize="off" spellcheck="false"
                                required aria-invalid=[form.error("authKey").map(|_| "true")]
                                aria-describedby=[form.error("authKey").map(|_| "connect-tailnet-key-error")];
                            button type="button" class="input-group__action" data-password-toggle="#connect-tailnet-key"
                                aria-pressed="false" aria-label="Show auth key" data-label-pressed="Hide auth key" {
                                (icon("eye")) (icon("eye-off"))
                            }
                        }
                        @if let Some(message) = form.error("authKey") { p class="field__error" id="connect-tailnet-key-error" { (message) } }
                    }
                }
            }
            div class="field" {
                label class="field__label" for="connect-tailnet-control" { "Control server URL " span class="field__optional" { "Optional" } }
                input class="input" id="connect-tailnet-control" name="controlUrl" type="text" inputmode="url" value=(form.control_url)
                    placeholder="https://headscale.example.com" autocomplete="off" autocapitalize="off" spellcheck="false"
                    aria-invalid=[form.error("controlUrl").map(|_| "true")]
                    aria-describedby=(if form.error("controlUrl").is_some() { "connect-tailnet-control-error connect-tailnet-control-help" } else { "connect-tailnet-control-help" });
                @if let Some(message) = form.error("controlUrl") { p class="field__error" id="connect-tailnet-control-error" { (message) } }
                p class="field__help" id="connect-tailnet-control-help" { "Leave empty for Tailscale. Set it for Headscale." }
            }
        }
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="connect-tailnet-form" { "Connect" }
        }
    }
}

fn connect_dialog(context: &PageContext, page: &TailnetsPage) -> Markup {
    let default = ConnectForm::default();
    let form = page.open.unwrap_or(&default);
    html! {
        dialog class="dialog" id=(CONNECT_DIALOG_ID) aria-labelledby="connect-tailnet-title" data-dialog-static data-dialog-reset
            data-dialog-return="/tailnets" data-open[page.open.is_some()] {
            div data-fragment { (connect_fragment(context, form)) }
        }
    }
}

/// `GET /tailnets`, and `/tailnets/new`, which has the dialog open.
pub fn tailnets_page(context: &PageContext, page: &TailnetsPage) -> Markup {
    // A page that waits for a tailnet to start or to be signed in asks for
    // itself again, unless a dialog is open: it would lose what is typed.
    let refresh = page.open.is_none() && page.rows.iter().any(TailnetRow::is_settling);
    let head = html! {
        @if refresh { meta http-equiv="refresh" content=(REFRESH_SECONDS); }
    };
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Tailnets" }
                p class="page-header__subtitle" { "Private Tailscale networks your HTTP MCPs can reach." }
            }
            div class="page-header__actions" { (connect_button(page, "button button--primary")) }
        }
        @if !page.enabled { (off_banner()) }
        @if page.rows.is_empty() { (empty(page)) } @else { (table(context, page)) }
    };
    let overlays = html! {
        @if page.can_connect() { (connect_dialog(context, page)) }
    };
    app_page_with_head(context, "Tailnets", head, content, overlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: TailnetState) -> TailnetRow {
        TailnetRow {
            id: 1,
            name: "pro".into(),
            control_url: None,
            state: Some(state),
            mcps: Vec::new(),
        }
    }

    #[test]
    fn links_only_a_web_address_to_sign_in() {
        let web = row(TailnetState::NeedsLogin {
            auth_url: Url::parse("https://login.tailscale.com/a/abc").ok(),
        });
        assert!(web.sign_in_url().is_some());
        assert_eq!(web.standing().note, None);
        for hostile in ["javascript:alert(1)", "data:text/html,x", "file:///etc/passwd"] {
            let row = row(TailnetState::NeedsLogin {
                auth_url: Url::parse(hostile).ok(),
            });
            assert!(row.sign_in_url().is_none(), "{hostile}");
        }
    }

    #[test]
    fn a_tailnet_that_will_change_by_itself_is_settling() {
        assert!(row(TailnetState::Starting).is_settling());
        assert!(row(TailnetState::NeedsLogin { auth_url: None }).is_settling());
        assert!(!row(TailnetState::Stopped).is_settling());
        assert!(!row(TailnetState::Failed("x".into())).is_settling());
    }
}
