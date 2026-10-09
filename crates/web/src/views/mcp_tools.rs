//! The Tool approvals page of an MCP. (`inertia/pages/mcps/tools.tsx`)

use maud::{Markup, html};
use mymcps_upstream::approvals::ToolApprovalMode;

use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};

/// The id of the table of tools, which the bulk buttons and the count of
/// the tools that ask refer to.
const TABLE_ID: &str = "mcp-tools";

/// One tool of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub name: String,
    pub description: Option<String>,
    pub mode: ToolApprovalMode,
    /// What the tool does until someone chooses. Built-in tools that spend money ask.
    pub default_mode: ToolApprovalMode,
    /// False for a tool with a saved choice that its MCP no longer lists.
    pub is_listed: bool,
    /// False for a tool whose name the form cannot send: it is shown, and
    /// has no choice to make.
    pub can_be_chosen: bool,
}

/// What the Tool approvals page shows.
#[derive(Debug, Clone)]
pub struct McpToolsPage {
    pub mcp_id: i64,
    pub mcp_name: String,
    pub tools: Vec<ToolRow>,
    /// Why the MCP could not be asked for its tools.
    pub list_error: Option<String>,
    /// What is saved could not be read, so every tool asks until it is saved again.
    pub saved_unreadable: bool,
}

impl McpToolsPage {
    fn path(&self) -> String {
        format!("/mcps/{}/tools", self.mcp_id)
    }

    /// The tools the form sends, each with its choice.
    fn choosable(&self) -> impl Iterator<Item = &ToolRow> {
        self.tools.iter().filter(|tool| tool.can_be_chosen)
    }

    fn asking(&self) -> usize {
        self.choosable()
            .filter(|tool| tool.mode == ToolApprovalMode::Ask)
            .count()
    }
}

/// What a save is answered with, for the page's script to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice<'a> {
    Saved(&'a str),
    Refused(&'a str),
}

fn notice(notice: Notice) -> Markup {
    let (message, error) = match notice {
        Notice::Saved(message) => (message, false),
        Notice::Refused(message) => (message, true),
    };
    super::shell::toast(message, error)
}

fn banners(page: &McpToolsPage) -> Markup {
    html! {
        @if let Some(error) = &page.list_error {
            div class="banner banner--critical" role="alert" {
                (icon("circle-x"))
                div class="banner__content" {
                    p class="banner__title" { (page.mcp_name) " did not list its tools" }
                    p { (error) " Only the tools with a saved choice are shown." }
                }
            }
        }
        @if page.saved_unreadable {
            div class="banner banner--warning" role="status" {
                (icon("triangle-alert"))
                div class="banner__content" {
                    p class="banner__title" { "The saved choices could not be read" }
                    p { "Every tool of this MCP asks for approval until you save this page again." }
                }
            }
        }
    }
}

fn mode_choice(index: usize, tool: &ToolRow, mode: ToolApprovalMode, label: &str) -> Markup {
    html! {
        label class="segment" {
            input type="radio" class="visually-hidden" name=(format!("tools[{index}][mode]")) value=(mode.as_str())
                checked[tool.mode == mode] data-select-item[mode == ToolApprovalMode::Ask];
            (label)
        }
    }
}

fn tool_row(index: usize, tool: &ToolRow) -> Markup {
    let description = tool
        .description
        .as_deref()
        .filter(|description| !description.is_empty());
    let searched = match description {
        Some(description) => format!("{} {description}", tool.name),
        None => tool.name.clone(),
    };
    html! {
        tr data-filter-item data-filter-text=(searched) {
            td class="cell-wrap" {
                div class="cell-stack cell-stack--wrap gap-050" {
                    span class="cell-code" { (tool.name) }
                    @if let Some(description) = description {
                        span class="cell-sub clamp-3" { (description) }
                    }
                    @if !tool.is_listed {
                        span class="status" { "No longer listed by the MCP" }
                    }
                    @if tool.default_mode == ToolApprovalMode::Ask {
                        span class="status status--warning" { "Asks by default" }
                    }
                }
            }
            td class="cell-end" {
                input type="hidden" name=(format!("tools[{index}][name]")) value=(tool.name);
                div class="segmented" role="radiogroup" aria-label=(format!("When an agent calls {}", tool.name)) {
                    (mode_choice(index, tool, ToolApprovalMode::Auto, "Runs"))
                    (mode_choice(index, tool, ToolApprovalMode::Ask, "Asks"))
                }
            }
        }
    }
}

/// A tool whose name the form cannot send. It asks, whatever is chosen
/// for the others, and the form is saved without it.
fn fixed_row(tool: &ToolRow) -> Markup {
    html! {
        tr data-filter-item data-filter-text=(tool.name) {
            td class="cell-wrap" {
                div class="cell-stack cell-stack--wrap gap-050" {
                    span class="cell-code" { (tool.name) }
                    span class="status status--warning" { "Always asks: no choice can be saved for a name that is blank or this long" }
                }
            }
            td class="cell-end" {}
        }
    }
}

/// The search, the two bulk buttons, the table and the save bar, in one
/// form: every tool is sent, in view or not.
fn tools_form(context: &PageContext, page: &McpToolsPage) -> Markup {
    let table = format!("#{TABLE_ID}");
    html! {
        form class="page__body" id="mcp-tools-form" method="post" action=(format!("{}?_method=PUT", page.path()))
            data-async data-dirty data-filter {
            (context.csrf_field())
            // Sent before the tools: a form too long to arrive whole is not saved in part.
            input type="hidden" name="toolCount" value=(page.choosable().count());

            div class="toolbar" {
                label class="input-group search" {
                    span class="visually-hidden" { "Find a tool" }
                    (icon("search"))
                    input class="input-group__control" type="search" placeholder="Name or description"
                        autocomplete="off" data-filter-input;
                }
                button type="button" class="button button--secondary" data-check-all=(table) data-check-value="auto" { "All run" }
                button type="button" class="button button--secondary" data-check-all=(table) data-check-value="ask" { "All ask" }
            }

            section class="card" aria-labelledby="mcp-tools-title" {
                header class="card__header" {
                    div class="card__heading" {
                        h2 class="card__title" id="mcp-tools-title" {
                            span data-select-count=(table) { (page.asking()) }
                            " of " (page.choosable().count()) " tools ask for approval"
                        }
                    }
                }
                div class="table-scroll" {
                    table class="table table--fluid" id=(TABLE_ID) {
                        thead { tr {
                            th scope="col" { "Tool" }
                            th scope="col" class="col-136 cell-end" { "When an agent calls it" }
                        } }
                        tbody {
                            @for (index, tool) in page.choosable().enumerate() { (tool_row(index, tool)) }
                            @for tool in page.tools.iter().filter(|tool| !tool.can_be_chosen) { (fixed_row(tool)) }
                        }
                    }
                }
                div class="empty-state" data-filter-empty hidden {
                    span class="empty-state__icon" { (icon("search")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No tool matches “" span data-filter-query {} "”." }
                    }
                }
                footer class="card__footer" {
                    p class="card__footer-note" data-dirty-count data-zero="No unsaved changes"
                        data-singular="unsaved change" data-plural="unsaved changes" { "No unsaved changes" }
                    a class="button button--secondary" href="/mcps" { "Back to MCPs" }
                    // Enabled here: without the script it has to work.
                    button type="submit" class="button button--primary" data-dirty-submit { "Save" }
                }
            }
        }
    }
}

/// What a save replaces: the banners, which a save can make untrue, and
/// the form with what is now saved.
pub fn tools_fragment(context: &PageContext, page: &McpToolsPage, shown: Option<Notice>) -> Markup {
    html! {
        (banners(page))
        @if page.tools.is_empty() {
            section class="card" aria-label="Tools" {
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("wrench")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No tools to set up" }
                        p class="empty-state__description" {
                            @if page.list_error.is_some() {
                                "Fix the connection from the MCPs page, then come back."
                            } @else {
                                "This MCP has no tools."
                            }
                        }
                    }
                    a class="button button--secondary" href="/mcps" { "Back to MCPs" }
                }
            }
        } @else {
            (tools_form(context, page))
        }
        @if let Some(shown) = shown { (notice(shown)) }
    }
}

/// `GET /mcps/{id}/tools`
pub fn mcp_tools_page(context: &PageContext, page: &McpToolsPage) -> Markup {
    let content = html! {
        div class="stack gap-300" {
            nav class="breadcrumb" aria-label="Breadcrumb" {
                a href="/mcps" { "MCPs" }
                (icon("chevron-right"))
                a href=(format!("/mcps/{}", page.mcp_id)) { (page.mcp_name) }
                (icon("chevron-right"))
                span aria-current="page" { "Tool approvals" }
            }
            header class="page-header" {
                div class="page-header__text" {
                    h1 class="page-header__title" { "Tool approvals" }
                    p class="page-header__subtitle" {
                        "Choose what happens when an agent calls a tool of " (page.mcp_name)
                        ". A tool that asks is not run: the agent gets a link to give you, and the call runs once you have signed in and approved it. The page you approve on is written by MyMCPs from the call itself, never by the agent."
                    }
                }
            }
        }

        div class="page__body" data-fragment { (tools_fragment(context, page, None)) }
    };
    app_page(
        context,
        &format!("Tool approvals · {}", page.mcp_name),
        content,
        html! {},
    )
}
