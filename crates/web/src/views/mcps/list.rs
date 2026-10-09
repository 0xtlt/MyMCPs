//! The registry: the search and the filters, then one row for each MCP, as
//! a table on desktop and as a list under 768px.

use maud::{Markup, html};
use mymcps_core::models::{McpStatus, McpTransport};

use crate::validators::mcp::{AUTH_LABELS, McpListQuery};
use crate::views::grouped;
use crate::views::icon::icon;
use crate::views::mcps::{McpView, McpsPage, ROWS_PER_PAGE, href, transport_label};
use crate::views::shell::PageContext;

/// What a control that starts an OAuth flow says while there is no public address.
const NO_APP_URL: &str = "Set APP_URL to connect with OAuth";

/// One choice of a filter: its value in the address, and how it reads.
struct Choice {
    value: &'static str,
    label: &'static str,
}

/// A filter of the list: a chip that opens a menu of links. Each link keeps
/// the search and the other filters, and goes back to the first page.
struct Filter<'a> {
    id: &'static str,
    name: &'static str,
    all: &'static str,
    choices: Vec<Choice>,
    current: Option<&'a str>,
    /// The list with this filter set to a value, or cleared.
    with: fn(&McpListQuery, Option<&str>) -> McpListQuery,
}

impl Filter<'_> {
    fn href(&self, query: &McpListQuery, value: Option<&str>) -> String {
        let mut next = (self.with)(query, value);
        next.page = None;
        href("/mcps", &next, &[])
    }

    fn render(&self, query: &McpListQuery) -> Markup {
        let chosen = self
            .current
            .and_then(|current| self.choices.iter().find(|choice| choice.value == current));
        html! {
            @match chosen {
                // A chip with a value says it, and can be cleared: it stays under 768px for that.
                Some(choice) => {
                    span class="chip chip--active" {
                        button type="button" class="chip__label" popovertarget=(self.id) { (self.name) ": " (choice.label) }
                        a class="chip__clear" href=(self.href(query, None)) aria-label=(format!("Clear {} filter", self.name.to_lowercase())) { (icon("x")) }
                    }
                }
                None => {
                    button type="button" class="chip hide-mobile" popovertarget=(self.id) { (self.name) (icon("chevron-down")) }
                }
            }
            div class="menu menu--start" id=(self.id) popover role="menu" aria-label=(self.name) {
                a class="menu__item" role="menuitemradio" aria-checked=(if chosen.is_none() { "true" } else { "false" })
                    href=(self.href(query, None)) { (self.all) }
                @for choice in &self.choices {
                    a class="menu__item" role="menuitemradio"
                        aria-checked=(if self.current == Some(choice.value) { "true" } else { "false" })
                        href=(self.href(query, Some(choice.value))) { (choice.label) }
                }
            }
        }
    }
}

fn filters(query: &McpListQuery) -> [Filter<'_>; 3] {
    [
        Filter {
            id: "filter-status",
            name: "Status",
            all: "All statuses",
            choices: [McpStatus::Ready, McpStatus::Draft, McpStatus::Error]
                .into_iter()
                .map(|status| Choice {
                    value: status.as_str(),
                    label: status.as_str(),
                })
                .collect(),
            current: query.status.map(|status| status.as_str()),
            with: |query, value| McpListQuery {
                status: value.and_then(McpStatus::parse),
                ..query.clone()
            },
        },
        Filter {
            id: "filter-transport",
            name: "Transport",
            all: "All transports",
            choices: McpTransport::ALL
                .iter()
                .map(|transport| Choice {
                    value: transport.as_str(),
                    label: transport_label(*transport),
                })
                .collect(),
            current: query.transport.map(|transport| transport.as_str()),
            with: |query, value| McpListQuery {
                transport: value.and_then(McpTransport::parse),
                ..query.clone()
            },
        },
        Filter {
            id: "filter-auth",
            name: "Auth",
            all: "All auth types",
            choices: AUTH_LABELS
                .into_iter()
                .map(|label| Choice {
                    value: label,
                    label,
                })
                .collect(),
            current: query.auth.as_deref(),
            with: |query, value| McpListQuery {
                auth: value.map(str::to_string),
                ..query.clone()
            },
        },
    ]
}

/// `8 MCPs · 7 enabled`: what the search and the filters keep.
fn summary(page: &McpsPage) -> String {
    format!(
        "{} {} · {} enabled",
        grouped(page.matching),
        if page.matching == 1 { "MCP" } else { "MCPs" },
        grouped(page.matching_enabled)
    )
}

fn toolbar(page: &McpsPage) -> Markup {
    let query = page.query;
    html! {
        form class="toolbar" method="get" action="/mcps" role="search" {
            label class="input-group search" {
                span class="visually-hidden" { "Search MCPs" }
                (icon("search"))
                input class="input-group__control" type="search" name="q" value=[query.q.as_deref()] placeholder="Search MCPs"
                    autocomplete="off" data-shortcut="mod+k";
                kbd class="kbd" data-shortcut-hint { "⌘K" }
            }
            // A new search keeps the filters, and starts on the first page.
            @if let Some(status) = query.status { input type="hidden" name="status" value=(status.as_str()); }
            @if let Some(transport) = query.transport { input type="hidden" name="transport" value=(transport.as_str()); }
            @if let Some(auth) = &query.auth { input type="hidden" name="auth" value=(auth); }
            @for filter in filters(query) { (filter.render(query)) }
            p class="toolbar__note push-end hide-mobile" { (summary(page)) }
        }
    }
}

fn status_tone(status: McpStatus) -> &'static str {
    match status {
        McpStatus::Ready => "success",
        McpStatus::Draft => "warning",
        McpStatus::Error => "critical",
    }
}

/// What opens the edit dialog of an MCP: a link to the page that has it
/// open, which the page script follows without leaving the list.
fn edit_href(mcp: &McpView, query: &McpListQuery) -> String {
    href(&format!("/mcps/{}/edit", mcp.id), query, &[])
}

/// The second line of the endpoint: why the last connection failed, else
/// the version of the package Deno has cached.
fn endpoint_note(mcp: &McpView) -> Markup {
    let error = mcp
        .last_error
        .as_deref()
        .filter(|error| mcp.status == McpStatus::Error && !error.is_empty());
    html! {
        @if let Some(error) = error {
            span class="cell-sub cell-sub--critical" title=(error) { (error) }
        } @else if let (McpTransport::Npm, Some(cached)) = (mcp.transport, &mcp.npm_cached_version) {
            span class="cell-sub" { "cached " (cached) }
        }
    }
}

fn endpoint_cell(mcp: &McpView) -> Markup {
    let note = endpoint_note(mcp);
    html! {
        @if mcp.transport != McpTransport::Builtin {
            td { div class="cell-stack" { span class="cell-code" { (mcp.endpoint_label()) } (note) } }
        } @else if note.0.is_empty() {
            // What a built-in MCP talks to is not an address: it is written as text.
            td class="cell-secondary" { (mcp.endpoint_label()) }
        } @else {
            td { div class="cell-stack" { span class="text-secondary" { (mcp.endpoint_label()) } (note) } }
        }
    }
}

/// "Connect" or "Re-authorize" in the menu of a row, when the MCP signs in with OAuth.
fn oauth_menu_item(context: &PageContext, mcp: &McpView, query: &McpListQuery) -> Markup {
    let (label, icon_name) = if mcp.oauth_required && !mcp.has_oauth_access_token {
        ("Connect", "link")
    } else if mcp.oauth_required || mcp.can_reauthorize() {
        ("Re-authorize", "key-round")
    } else {
        return html! {};
    };
    html! {
        @if !context.app_url_configured {
            a class="menu__item" role="menuitem" aria-disabled="true" title=(NO_APP_URL) { (icon(icon_name)) (label) }
        } @else if mcp.oauth_pasted_callback {
            // The provider redirects to a loopback address, which is pasted back in the edit dialog.
            a class="menu__item" role="menuitem" href=(edit_href(mcp, query)) data-dialog-open="#edit-mcp" data-dialog-fetch data-dialog-history {
                (icon(icon_name)) (label)
            }
        } @else {
            a class="menu__item" role="menuitem" href=(format!("/mcps/{}/oauth/start", mcp.id)) { (icon(icon_name)) (label) }
        }
    }
}

/// The actions of a row, in the order the design lists them. The ones that
/// test or update the MCP say they come from the list, so the edit dialog
/// does not open on their answer.
fn row_menu(context: &PageContext, mcp: &McpView, query: &McpListQuery) -> Markup {
    let from_list = html! { input type="hidden" name="from" value="list"; };
    html! {
        div class="menu" id=(format!("mcp-{}-menu", mcp.id)) popover role="menu" {
            form method="post" action=(format!("/mcps/{}/probe", mcp.id)) data-busy-message=(format!("Testing {}…", mcp.name)) {
                (context.csrf_field()) (from_list)
                button type="submit" class="menu__item" role="menuitem" { (icon("refresh-cw")) "Test connection" }
            }
            @if mcp.tracks_latest() {
                form method="post" action=(format!("/mcps/{}/update", mcp.id)) data-busy-message=(format!("Updating {}…", mcp.name)) {
                    (context.csrf_field()) (from_list)
                    button type="submit" class="menu__item" role="menuitem" { (icon("download")) "Update MCP" }
                }
            }
            a class="menu__item" role="menuitem" href=(format!("/mcps/{}/tools", mcp.id)) { (icon("shield-check")) "Tool approvals" }
            (oauth_menu_item(context, mcp, query))
            div class="menu__separator" {}
            form method="post" action=(format!("/mcps/{}/toggle", mcp.id)) {
                (context.csrf_field())
                button type="submit" class="menu__item" role="menuitem" name="enabled" value=(if mcp.enabled { "false" } else { "true" }) {
                    (icon("power")) (if mcp.enabled { "Disable" } else { "Enable" })
                }
            }
            form method="post" action=(format!("/mcps/{}?_method=DELETE", mcp.id))
                data-confirm=(format!("Delete {}?", mcp.name)) data-confirm-label="Delete" data-confirm-tone="critical" {
                (context.csrf_field())
                button type="submit" class="menu__item menu__item--critical" role="menuitem" { (icon("trash")) "Delete" }
            }
        }
    }
}

fn table_row(context: &PageContext, mcp: &McpView, query: &McpListQuery) -> Markup {
    html! {
        tr {
            td { span class=(format!("status status--{}", status_tone(mcp.status))) { (mcp.status.as_str()) } }
            td {
                div class="cell-media" {
                    span class="tile" { (icon(mcp.icon)) }
                    div class="cell-stack" {
                        span class="cell-title" { (mcp.name) }
                        span class="cell-sub cell-sub--code" { (mcp.slug) }
                    }
                }
            }
            (endpoint_cell(mcp))
            td { code class="code" { (mcp.auth_label()) } }
            @match mcp.tool_count {
                Some(count) => { td class="cell-secondary tabular" { (grouped(count)) } }
                None => { td class="cell-tertiary" { "—" } }
            }
            td {
                form method="post" action=(format!("/mcps/{}/toggle", mcp.id)) {
                    (context.csrf_field())
                    // The button names the state it asks for, so that a stale page cannot undo a change.
                    button type="submit" class="switch" role="switch" name="enabled" value=(if mcp.enabled { "false" } else { "true" })
                        aria-checked=(if mcp.enabled { "true" } else { "false" }) aria-label=(format!("{} enabled", mcp.name)) {}
                }
            }
            td class="cell-end" {
                span class="row-actions" {
                    a class="button" id=(format!("mcp-{}-edit", mcp.id)) href=(edit_href(mcp, query))
                        data-dialog-open="#edit-mcp" data-dialog-fetch data-dialog-history { "Edit" }
                    button type="button" class="icon-button" popovertarget=(format!("mcp-{}-menu", mcp.id))
                        aria-label=(format!("More actions for {}", mcp.name)) { (icon("ellipsis")) }
                }
                (row_menu(context, mcp, query))
            }
        }
    }
}

/// A row of the list shown under 768px: it opens the edit dialog, which
/// holds the actions the table has in its row menu.
fn list_row(mcp: &McpView, query: &McpListQuery) -> Markup {
    // A disabled MCP is not what its status says to agents: its badge is neutral.
    let badge = if mcp.enabled {
        format!("badge badge--{}", status_tone(mcp.status))
    } else {
        "badge".to_string()
    };
    html! {
        a class="list-row" href=(edit_href(mcp, query)) data-dialog-open="#edit-mcp" data-dialog-fetch data-dialog-history {
            span class="tile" { (icon(mcp.icon)) }
            span class="list-row__text" {
                span class="list-row__title" { (mcp.name) }
                span class="list-row__subtitle" {
                    (mcp.slug) " · " (transport_label(mcp.transport))
                    @if !mcp.enabled { " · off" }
                }
            }
            span class=(badge) { (mcp.status.as_str()) }
        }
    }
}

fn pagination(page: &McpsPage) -> Markup {
    let query = page.query;
    let current = query.page.unwrap_or(1).max(1) as usize;
    let pages = page.matching.div_ceil(ROWS_PER_PAGE).max(1);
    let first = (current - 1) * ROWS_PER_PAGE + 1;
    let last = first + page.rows.len().saturating_sub(1);
    let to = |number: usize| {
        let next = McpListQuery {
            page: u32::try_from(number).ok(),
            ..query.clone()
        };
        href("/mcps", &next, &[])
    };
    html! {
        nav class="pagination" aria-label="Pagination" {
            span class="pagination__range" { (grouped(first)) "–" (grouped(last)) " of " (grouped(page.matching)) }
            span class="pagination__buttons" {
                @if current > 1 {
                    a class="icon-button icon-button--secondary" href=(to(current - 1)) aria-label="Previous page" { (icon("chevron-left")) }
                } @else {
                    a class="icon-button icon-button--secondary" aria-disabled="true" aria-label="Previous page" { (icon("chevron-left")) }
                }
                @if current < pages {
                    a class="icon-button icon-button--secondary" href=(to(current + 1)) aria-label="Next page" { (icon("chevron-right")) }
                } @else {
                    a class="icon-button icon-button--secondary" aria-disabled="true" aria-label="Next page" { (icon("chevron-right")) }
                }
            }
        }
    }
}

fn table_footer(page: &McpsPage) -> Markup {
    html! {
        div class="table-footer" {
            p class="table-footer__note" { (ROWS_PER_PAGE) " rows per page" }
            (pagination(page))
        }
    }
}

fn empty_registry(page: &McpsPage) -> Markup {
    html! {
        div class="card" {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("plug")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No MCPs yet" }
                    p class="empty-state__description" { "Create an MCP to start routing agent traffic." }
                }
                // Under 768px the header action sits right above the card: the second button would repeat it.
                a class="button button--secondary hide-mobile" href=(href("/mcps/new", page.query, &[])) data-dialog-open="#add-mcp" {
                    (icon("plus")) "Add MCP"
                }
            }
        }
    }
}

fn no_match() -> Markup {
    html! {
        div class="card" {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("search")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No MCPs match" }
                    p class="empty-state__description" { "Try another search or clear the filters." }
                }
                a class="button button--secondary" href="/mcps" { "Clear filters" }
            }
        }
    }
}

/// Everything under the page header.
pub(super) fn registry(context: &PageContext, page: &McpsPage) -> Markup {
    // Nothing to search or to filter before the first MCP.
    if page.registered == 0 {
        return empty_registry(page);
    }
    let query = page.query;
    html! {
        (toolbar(page))
        @if page.rows.is_empty() {
            (no_match())
        } @else {
            div class="card hide-mobile" {
                div class="table-scroll" {
                    table class="table" {
                        thead {
                            tr {
                                th scope="col" class="col-88" { "Status" }
                                th scope="col" { "Name" }
                                th scope="col" { "Endpoint" }
                                th scope="col" class="col-96" { "Auth" }
                                th scope="col" class="col-72" { "Tools" }
                                th scope="col" class="col-72" { "Enabled" }
                                th scope="col" class="col-96 cell-end" { "Actions" }
                            }
                        }
                        tbody {
                            @for mcp in page.rows { (table_row(context, mcp, query)) }
                        }
                    }
                }
                (table_footer(page))
            }

            section class="card hide-desktop" aria-labelledby="registered-mcps-title" {
                header class="card__header" {
                    div class="card__heading" {
                        h2 class="card__title" id="registered-mcps-title" { "Registered MCPs" }
                        p class="card__subtitle" { (summary(page)) }
                    }
                }
                @for mcp in page.rows { (list_row(mcp, query)) }
                // One page holds most registries: the footer only shows when there is another.
                @if page.matching > ROWS_PER_PAGE { (table_footer(page)) }
            }
        }
    }
}
