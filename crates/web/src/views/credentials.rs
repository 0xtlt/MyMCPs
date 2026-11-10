//! The Credentials page: the OAuth clients a team saves once and signs its
//! built-in MCPs in with, such as one Google Cloud client for Google Ads and
//! Google Search Console, and the dialog that adds or edits one.

use maud::{Markup, html};

use crate::views::icon::icon;
use crate::views::mcps::PublicApp;
use crate::views::shell::{PageContext, app_page};

/// The id of the dialog that adds or edits a credential.
pub const CREDENTIAL_DIALOG_ID: &str = "credential";

/// The name of the "field" that holds what is wrong with the form as a whole.
pub const FORM_ERROR: &str = "form";

/// What the list shows of a credential. Never its secret.
#[derive(Debug, Clone)]
pub struct CredentialRow {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub client_id: String,
    /// The MCPs that sign in with it, by name.
    pub mcps: Vec<String>,
}

/// How the page names a provider.
pub fn provider_label(provider: &str) -> &str {
    match provider {
        "google" => "Google",
        other => other,
    }
}

/// What the dialog holds: what was typed, and what was refused. The secret
/// is never part of it: a form that comes back is asked for it again.
#[derive(Debug, Clone, Default)]
pub struct CredentialForm {
    /// The credential edited. `None`: one is added.
    pub editing: Option<i64>,
    pub name: String,
    pub client_id: String,
    /// How many MCPs use the credential edited.
    pub used_by: usize,
    errors: Vec<(String, String)>,
}

impl CredentialForm {
    pub fn new(name: &str, client_id: &str) -> Self {
        Self {
            name: name.to_owned(),
            client_id: client_id.to_owned(),
            ..Self::default()
        }
    }

    pub fn editing(mut self, id: i64, used_by: usize) -> Self {
        self.editing = Some(id);
        self.used_by = used_by;
        self
    }

    pub fn with_errors(mut self, errors: Vec<(String, String)>) -> Self {
        self.errors = errors;
        self
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

    /// What the fields do not say.
    fn unshown(&self) -> Vec<&str> {
        self.errors
            .iter()
            .filter(|(field, _)| !["name", "clientId", "clientSecret"].contains(&field.as_str()))
            .map(|(_, message)| message.as_str())
            .collect()
    }

    fn action(&self) -> String {
        match self.editing {
            Some(id) => format!("/credentials/{id}?_method=PUT"),
            None => "/credentials".to_owned(),
        }
    }
}

/// What the Credentials page shows.
pub struct CredentialsPage<'a> {
    pub rows: &'a [CredentialRow],
    /// The dialog, open, with this form. `None`: closed.
    pub open: Option<&'a CredentialForm>,
    pub public_app: Option<&'a PublicApp>,
}

fn add_button() -> Markup {
    html! {
        a class="button button--primary" href="/credentials/new" data-dialog-open=(format!("#{CREDENTIAL_DIALOG_ID}")) {
            (icon("plus")) "Add OAuth client"
        }
    }
}

fn used_by(row: &CredentialRow) -> Markup {
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

fn table_row(context: &PageContext, row: &CredentialRow) -> Markup {
    html! {
        tr {
            td {
                div class="cell-stack" {
                    span class="cell-title" { (row.name) }
                    span class="cell-sub" title=(row.client_id) { (row.client_id) }
                }
            }
            td class="cell-secondary" { (provider_label(&row.provider)) }
            (used_by(row))
            td class="cell-end" {
                span class="row-actions" {
                    a class="button" href=(format!("/credentials/{}/edit", row.id))
                        aria-label=(format!("Edit {}", row.name)) { "Edit" }
                    @let menu = format!("credential-{}-menu", row.id);
                    button type="button" class="icon-button" popovertarget=(menu)
                        aria-label=(format!("More actions for {}", row.name)) { (icon("ellipsis")) }
                    div class="menu" id=(menu) popover role="menu" {
                        form method="post" action=(format!("/credentials/{}?_method=DELETE", row.id))
                            data-confirm="MyMCPs forgets this client. It stays in your Google Cloud project." data-confirm-label="Delete" data-confirm-tone="critical"
                            data-confirm-title=(format!("Delete {}?", row.name)) {
                            (context.csrf_field())
                            button type="submit" class="menu__item menu__item--critical" role="menuitem" { (icon("trash")) "Delete" }
                        }
                    }
                }
            }
        }
    }
}

fn table(context: &PageContext, rows: &[CredentialRow]) -> Markup {
    html! {
        div class="card" {
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr {
                            th scope="col" { "Name" }
                            th scope="col" class="col-96" { "Provider" }
                            th scope="col" class="col-96" { "Used by" }
                            th scope="col" class="col-200 cell-end" { "Actions" }
                        }
                    }
                    tbody { @for row in rows { (table_row(context, row)) } }
                }
            }
        }
    }
}

fn empty() -> Markup {
    html! {
        div class="card" {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("lock")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No OAuth clients yet" }
                    p class="empty-state__description" {
                        "Add a Google Cloud OAuth client once, then choose it for Google Ads, Google Search Console, and any other Google MCP."
                    }
                }
            }
        }
    }
}

fn field(
    form: &CredentialForm,
    id: &str,
    name: &str,
    label: Markup,
    input: impl FnOnce(Option<&str>, String) -> Markup,
    help: Option<&str>,
) -> Markup {
    let error = form.error(name);
    let help_id = format!("{id}-help");
    let error_id = format!("{id}-error");
    let describedby = match (error.is_some(), help.is_some()) {
        (true, true) => format!("{error_id} {help_id}"),
        (true, false) => error_id.clone(),
        (false, _) => help_id.clone(),
    };
    html! {
        div class="field" {
            label class="field__label" for=(id) { (label) }
            (input(error, describedby))
            @if let Some(message) = error { p class="field__error" id=(error_id) { (message) } }
            @if let Some(help) = help { p class="field__help" id=(help_id) { (help) } }
        }
    }
}

fn copy_field(label: &str, value: &str) -> Markup {
    html! {
        div class="field" {
            span class="field__label" { (label) }
            div class="copy-field copy-field--wrap" {
                span class="copy-field__value" { (value) }
                button type="button" class="icon-button" data-copy=(value) aria-label=(format!("Copy {}", label.to_lowercase())) {
                    (icon("copy")) (icon("check"))
                }
            }
        }
    }
}

/// The content of the dialog: what its `[data-fragment]` holds, and what a
/// refused submission gets back.
pub fn credential_fragment(
    context: &PageContext,
    form: &CredentialForm,
    public_app: Option<&PublicApp>,
) -> Markup {
    let editing = form.editing.is_some();
    let title = if editing {
        format!("Edit {}", form.name)
    } else {
        "Add an OAuth client".to_owned()
    };
    html! {
        div class="dialog__header" {
            div class="dialog__heading" {
                h2 class="dialog__title" id="credential-title" { (title) }
                p class="dialog__subtitle" { "A Google Cloud OAuth client your Google MCPs share" }
            }
            button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
        }
        form class="dialog__body" id="credential-form" method="post" action=(form.action()) data-async {
            (context.csrf_field())
            @if !editing { input type="hidden" name="provider" value="google"; }
            @for message in form.unshown() {
                div class="banner banner--critical" role="alert" {
                    (icon("circle-x"))
                    div class="banner__content" {
                        p class="banner__title" { "This OAuth client could not be saved" }
                        p { (message) }
                    }
                }
            }
            @if form.used_by > 0 {
                div class="banner banner--warning" role="status" {
                    (icon("triangle-alert"))
                    div class="banner__content" {
                        p class="banner__title" {
                            "Used by " (form.used_by) @if form.used_by == 1 { " MCP" } @else { " MCPs" }
                        }
                        p { "A new Client Secret applies to them at once. A new Client ID disconnects them: connect each one again." }
                    }
                }
            }
            p class="text-body-sm" {
                "In a Google Cloud project, set up the OAuth consent screen and create an OAuth client of the type Web application. Enable the API of each Google MCP in the same project: the Google Ads API, the Google Search Console API."
            }
            @if let Some(public_app) = public_app {
                (copy_field("Authorized redirect URI", &format!("{}/mcps/oauth/callback", public_app.url)))
            }
            (field(form, "credential-name", "name", html! { "Name" }, |error, describedby| html! {
                input class="input" id="credential-name" name="name" type="text" value=(form.name) placeholder="Google Cloud"
                    autocomplete="off" autofocus aria-invalid=[error.map(|_| "true")] aria-describedby=(describedby);
            }, Some("What the dialog of an MCP lists it as.")))
            (field(form, "credential-client-id", "clientId", html! { "Client ID" }, |error, describedby| html! {
                input class="input" id="credential-client-id" name="clientId" type="text" value=(form.client_id)
                    placeholder="1234567890-abc123.apps.googleusercontent.com" autocomplete="off" autocapitalize="off" spellcheck="false"
                    aria-invalid=[error.map(|_| "true")] aria-describedby=(describedby);
            }, None))
            (field(form, "credential-client-secret", "clientSecret",
                html! { "Client Secret" @if editing { " (leave blank to keep) " span class="field__optional" { "Optional" } } },
                |error, describedby| html! {
                    div class="input-group" {
                        // Never filled in: a secret is typed again after a refusal.
                        input class="input-group__control" id="credential-client-secret" name="clientSecret" type="password"
                            autocomplete="off" autocapitalize="off" spellcheck="false" required[!editing]
                            aria-invalid=[error.map(|_| "true")] aria-describedby=(describedby);
                        button type="button" class="input-group__action" data-password-toggle="#credential-client-secret"
                            aria-pressed="false" aria-label="Show client secret" data-label-pressed="Hide client secret" {
                            (icon("eye")) (icon("eye-off"))
                        }
                    }
                }, Some("Encrypted at rest and only sent to Google.")))
        }
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--primary" form="credential-form" {
                @if editing { "Save" } @else { "Add OAuth client" }
            }
        }
    }
}

fn dialog(context: &PageContext, page: &CredentialsPage) -> Markup {
    let default = CredentialForm::default();
    let form = page.open.unwrap_or(&default);
    html! {
        dialog class="dialog" id=(CREDENTIAL_DIALOG_ID) aria-labelledby="credential-title" data-dialog-static data-dialog-reset
            data-dialog-return="/credentials" data-open[page.open.is_some()] {
            div data-fragment { (credential_fragment(context, form, page.public_app)) }
        }
    }
}

/// `GET /credentials`, and `/credentials/new` and `/credentials/{id}/edit`,
/// which have the dialog open.
pub fn credentials_page(context: &PageContext, page: &CredentialsPage) -> Markup {
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Credentials" }
                p class="page-header__subtitle" { "OAuth clients your built-in MCPs sign in with, saved once for the team." }
            }
            div class="page-header__actions" { (add_button()) }
        }
        @if page.rows.is_empty() { (empty()) } @else { (table(context, page.rows)) }
    };
    app_page(context, "Credentials", content, dialog(context, page))
}
