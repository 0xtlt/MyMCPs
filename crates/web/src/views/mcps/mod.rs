//! The MCPs page: the registry of upstream MCP servers, the gallery of
//! templates, and the dialogs that create and edit an MCP.
//! (`inertia/pages/mcps/index.tsx` and its components)

mod builtin;
mod form;
mod gallery;
mod list;

use std::collections::BTreeMap;

use maud::{Markup, html};
use mymcps_core::Config;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_upstream::Upstream;
use url::Url;

use crate::mcp_templates::icon_of;
use crate::validators::mcp::McpListQuery;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};

pub use builtin::{SetupGuide, SignIn, builtin_provider_name, builtin_setup_guide};
pub use form::{
    CreateDialog, EditDialog, EnvRow, FormValues, McpForm, create_fragment, edit_fragment,
    keeps_saved_credentials,
};

/// How many MCPs a page of the list holds.
pub const ROWS_PER_PAGE: usize = 25;

/// The id of the edit dialog, which the page script names when it asks for
/// the dialog's content alone.
pub const EDIT_DIALOG_ID: &str = "edit-mcp";
/// The id of the create dialog.
pub const CREATE_DIALOG_ID: &str = "new-mcp";

/// The instance's public origin, which providers ask for when the admin
/// registers an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicApp {
    pub url: String,
    pub hostname: String,
}

impl PublicApp {
    /// `None` until `APP_URL` is an origin OAuth can use.
    pub fn of(config: &Config) -> Option<Self> {
        let url = config.public_oauth_app_url()?;
        let hostname = Url::parse(&url).ok()?.host_str()?.to_string();
        Some(Self { url, hostname })
    }
}

fn is_set(column: &Option<String>) -> bool {
    column.as_deref().is_some_and(|value| !value.is_empty())
}

fn words(column: &Option<String>) -> Vec<String> {
    column
        .as_deref()
        .map(|value| value.split(' ').map(str::to_string).collect())
        .unwrap_or_default()
}

/// What the pages know of an MCP. It never holds a secret, decrypted or
/// not: only whether one is saved. (`McpTransformer`)
#[derive(Debug, Clone)]
pub struct McpView {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub transport: McpTransport,
    pub builtin_key: Option<String>,
    pub http_url: Option<String>,
    pub npm_package: Option<String>,
    pub npm_version: Option<String>,
    pub auth_type: McpAuthType,
    pub auth_header_name: Option<String>,
    pub status: McpStatus,
    pub last_error: Option<String>,
    pub enabled: bool,
    pub npm_args: String,
    /// The names of the saved environment variables.
    pub npm_env: Vec<String>,
    pub has_auth_bearer: bool,
    pub has_auth_header_value: bool,
    pub has_oauth_access_token: bool,
    /// Other transports register their OAuth client automatically.
    pub oauth_client_id: Option<String>,
    pub has_oauth_client_secret: bool,
    pub builtin_username: Option<String>,
    pub has_builtin_password: bool,
    pub builtin_permissions: Vec<String>,
    pub builtin_aliases: Vec<String>,
    pub builtin_settings: BTreeMap<String, String>,
    pub builtin_write_enabled: bool,
    /// False when write access is on but the saved authorization predates it.
    pub builtin_write_granted: bool,
    pub oauth_required: bool,
    pub oauth_pasted_callback: bool,
    pub npm_cached_version: Option<String>,
    /// How many tools it listed the last time it was asked, when it was.
    pub tool_count: Option<usize>,
    pub icon: &'static str,
}

impl McpView {
    pub fn of(upstream: &Upstream, mcp: &Mcp, tool_count: Option<usize>) -> Self {
        let definition = upstream.builtin_mcp(mcp.builtin_key.as_deref());
        let builtin = mcp.transport == McpTransport::Builtin;
        Self {
            id: mcp.id,
            name: mcp.name.clone(),
            slug: mcp.slug.clone(),
            description: mcp.description.clone(),
            transport: mcp.transport,
            builtin_key: mcp.builtin_key.clone(),
            http_url: mcp.http_url.clone(),
            npm_package: mcp.npm_package.clone(),
            npm_version: mcp.npm_version.clone(),
            auth_type: mcp.auth_type,
            auth_header_name: mcp.auth_header_name.clone(),
            status: mcp.status,
            last_error: mcp.last_error.clone(),
            enabled: mcp.enabled,
            npm_args: mcp.npm_args_list().join(" "),
            npm_env: mcp.npm_env_names(),
            has_auth_bearer: is_set(&mcp.auth_bearer),
            has_auth_header_value: is_set(&mcp.auth_header_value),
            has_oauth_access_token: is_set(&mcp.oauth_access_token),
            oauth_client_id: mcp.oauth_client_id.clone().filter(|_| builtin),
            has_oauth_client_secret: builtin && is_set(&mcp.oauth_client_secret),
            builtin_username: mcp.builtin_username.clone().filter(|_| builtin),
            has_builtin_password: is_set(&mcp.builtin_password),
            builtin_permissions: words(&mcp.builtin_permissions),
            builtin_aliases: words(&mcp.builtin_aliases),
            builtin_settings: definition
                .map(|definition| upstream.builtin_settings(definition, mcp))
                .unwrap_or_default(),
            builtin_write_enabled: mcp.builtin_write_enabled,
            builtin_write_granted: definition.is_some()
                && upstream.builtin_write_granted(mcp).unwrap_or(false),
            oauth_required: mcp.oauth_required,
            oauth_pasted_callback: upstream.uses_pasted_oauth_callback(mcp),
            npm_cached_version: match (mcp.transport, mcp.npm_package.as_deref()) {
                (McpTransport::Npm, Some(package)) => {
                    upstream.cached_npm_package_version(package, mcp.npm_version.as_deref())
                }
                _ => None,
            },
            tool_count,
            icon: icon_of(mcp),
        }
    }

    /// Where the MCP is reached, as the list writes it.
    pub fn endpoint_label(&self) -> String {
        match self.transport {
            McpTransport::Builtin => format!(
                "Built-in · {}",
                builtin_setup_guide(self.builtin_key.as_deref())
                    .map_or("unknown", |guide| guide.endpoint)
            ),
            McpTransport::Http => non_empty(&self.http_url).unwrap_or("—").to_string(),
            McpTransport::Npm => non_empty(&self.npm_package).unwrap_or("—").to_string(),
        }
    }

    /// How the MCP signs in: its authentication, or for a built-in one the
    /// way its provider signs in.
    pub fn auth_label(&self) -> &'static str {
        match self.transport {
            McpTransport::Builtin => builtin_setup_guide(self.builtin_key.as_deref())
                .map_or("oauth", |guide| guide.sign_in.label()),
            _ => self.auth_type.as_str(),
        }
    }

    /// An OAuth sign-in holds a token. A password is only known to work once it was tested.
    pub fn is_connected(&self) -> bool {
        if self.has_builtin_password {
            self.status == McpStatus::Ready
        } else {
            self.has_oauth_access_token && !self.oauth_required
        }
    }

    /// Write access was allowed after the account was connected with read-only scopes.
    pub fn awaits_write_authorization(&self) -> bool {
        self.transport == McpTransport::Builtin
            && self.builtin_write_enabled
            && !self.builtin_write_granted
            && self.has_oauth_access_token
            && !self.oauth_required
    }

    /// The account is connected through OAuth, and can be authorized again.
    pub fn can_reauthorize(&self) -> bool {
        self.auth_type == McpAuthType::Auto && self.has_oauth_access_token && !self.oauth_required
    }

    /// An npm MCP that follows the latest version of its package can be updated.
    pub fn tracks_latest(&self) -> bool {
        mymcps_upstream::is_tracking_latest(self.transport, self.npm_version.as_deref())
    }

    /// Whether the search of the list finds this MCP.
    pub fn matches(&self, search: &str) -> bool {
        let search = search.to_lowercase();
        [
            Some(self.name.as_str()),
            Some(self.slug.as_str()),
            self.description.as_deref(),
            self.http_url.as_deref(),
            self.npm_package.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(&search))
            || self.endpoint_label().to_lowercase().contains(&search)
    }
}

fn non_empty(column: &Option<String>) -> Option<&str> {
    column.as_deref().filter(|value| !value.is_empty())
}

pub(crate) fn transport_label(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Http => "HTTP",
        McpTransport::Npm => "npm",
        McpTransport::Builtin => "Built-in",
    }
}

/// The address of a page of the MCPs, with the search and the filters of
/// the list behind it, and whatever `extra` adds.
pub(crate) fn href(path: &str, query: &McpListQuery, extra: &[(&str, &str)]) -> String {
    let mut pairs = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in extra {
        pairs.append_pair(name, value);
    }
    if let Some(search) = &query.q {
        pairs.append_pair("q", search);
    }
    if let Some(status) = query.status {
        pairs.append_pair("status", status.as_str());
    }
    if let Some(transport) = query.transport {
        pairs.append_pair("transport", transport.as_str());
    }
    if let Some(auth) = &query.auth {
        pairs.append_pair("auth", auth);
    }
    if let Some(page) = query.page.filter(|page| *page > 1) {
        pairs.append_pair("page", &page.to_string());
    }
    let pairs = pairs.finish();
    if pairs.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{pairs}")
    }
}

/// The dialog a page of the MCPs is opened on.
pub enum Dialog<'a> {
    None,
    /// `/mcps/new`: the gallery of templates.
    Gallery,
    /// `/mcps/new?template=…`: the form that adds an MCP.
    Create(&'a CreateDialog<'a>),
    /// `/mcps/{id}/edit`, and `/mcps` after a save that left something to do.
    Edit(&'a EditDialog<'a>),
}

/// What the MCPs page shows.
pub struct McpsPage<'a> {
    /// The search and the filters, with the page actually shown.
    pub query: &'a McpListQuery,
    /// The MCPs of this page of the list, by name.
    pub rows: &'a [McpView],
    /// How many MCPs the search and the filters keep, and how many of those are enabled.
    pub matching: usize,
    pub matching_enabled: usize,
    /// How many MCPs are registered at all.
    pub registered: usize,
    pub dialog: Dialog<'a>,
}

/// The dialog that edits an MCP. It is always in the page: the list opens it
/// with content the page script fetches from `/mcps/{id}/edit`.
fn edit_dialog(context: &PageContext, page: &McpsPage) -> Markup {
    let list = href("/mcps", page.query, &[]);
    let editing = match page.dialog {
        Dialog::Edit(dialog) => Some(dialog),
        _ => None,
    };
    html! {
        dialog class="dialog" id=(EDIT_DIALOG_ID) aria-labelledby="edit-mcp-title" data-dialog-static data-dialog-return=(list)
            data-open[editing.is_some()] data-dialog-trigger=[editing.map(|dialog| format!("#mcp-{}-edit", dialog.mcp.id))] {
            div data-fragment {
                @if let Some(dialog) = editing { (edit_fragment(context, dialog)) }
            }
        }
    }
}

fn create_dialog(context: &PageContext, page: &McpsPage, dialog: &CreateDialog) -> Markup {
    html! {
        dialog class="dialog" id=(CREATE_DIALOG_ID) aria-labelledby="new-mcp-title" data-open data-dialog-static
            data-dialog-return=(href("/mcps", page.query, &[])) {
            div data-fragment { (create_fragment(context, dialog)) }
        }
    }
}

/// `GET /mcps`, and the pages whose address means that one of its dialogs is open.
pub fn mcps_page(context: &PageContext, page: &McpsPage) -> Markup {
    // What the open dialog already says is not said again in a toast.
    let mut context = context.clone();
    if let Dialog::Edit(dialog) = page.dialog
        && dialog.shows_last_error()
        && context.flash_error.is_some()
        && context.flash_error == dialog.mcp.last_error
    {
        context.flash_error = None;
    }
    let context = &context;

    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "MCPs" }
                p class="page-header__subtitle" {
                    "Register upstream MCP servers. Agents reach them through MyMCPs with an access token."
                }
            }
            div class="page-header__actions" {
                a class="button button--primary" href=(href("/mcps/new", page.query, &[])) data-dialog-open="#add-mcp" {
                    (icon("plus")) "Add MCP"
                }
            }
        }
        (list::registry(context, page))
    };
    let overlays = html! {
        (gallery::gallery_dialog(page.query, matches!(page.dialog, Dialog::Gallery)))
        @if let Dialog::Create(dialog) = page.dialog { (create_dialog(context, page, dialog)) }
        (edit_dialog(context, page))
    };
    app_page(context, "MCPs", content, overlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_addresses_that_keep_the_list_as_it_is() {
        let none = McpListQuery::default();
        assert_eq!(href("/mcps", &none, &[]), "/mcps");
        assert_eq!(
            href("/mcps/new", &none, &[("template", "notion")]),
            "/mcps/new?template=notion"
        );

        let filtered = McpListQuery {
            q: Some("a b&c".into()),
            status: Some(McpStatus::Error),
            transport: Some(McpTransport::Builtin),
            auth: Some("oauth".into()),
            page: Some(3),
            template: Some("ignored".into()),
        };
        assert_eq!(
            href("/mcps", &filtered, &[]),
            "/mcps?q=a+b%26c&status=error&transport=builtin&auth=oauth&page=3"
        );
        // The first page is the address without a page.
        let first = McpListQuery {
            page: Some(1),
            ..McpListQuery::default()
        };
        assert_eq!(href("/mcps/7/edit", &first, &[]), "/mcps/7/edit");
    }
}
