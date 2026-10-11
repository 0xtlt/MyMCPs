//! The gallery of the "Add MCP" action: templates to start from, filtered
//! in the browser. (`inertia/components/mcp_template_gallery.tsx`)

use maud::{Markup, html};

use crate::mcp_templates::{MCP_TEMPLATES, McpTemplate, TemplateCategory};
use crate::validators::mcp::McpListQuery;
use crate::views::icon::icon;
use crate::views::mcps::href;

/// The tab the gallery opens on.
const POPULAR: &str = "popular";

fn tab(value: &str, label: &str, selected: bool) -> Markup {
    html! {
        button type="button" class="tab" role="tab" data-filter-value=(value)
            aria-selected=(if selected { "true" } else { "false" }) tabindex=[(!selected).then_some("-1")] { (label) }
    }
}

fn template_card(template: &McpTemplate, query: &McpListQuery) -> Markup {
    let tags = if template.popular {
        format!("{POPULAR} {}", template.category.key())
    } else {
        template.category.key().to_string()
    };
    html! {
        // Without the page script the gallery stays on its first tab.
        article class="template-card" data-filter-item data-filter-text=(template.searchable_text()) data-filter-tags=(tags)
            hidden[!template.popular] {
            div class="template-card__header" {
                span class="tile tile--lg" { (icon(template.icon)) }
                div class="template-card__text" {
                    h3 class="template-card__name" { (template.name) }
                    p class="template-card__category" { (template.category.label()) }
                }
                @if template.is_builtin() { span class="badge badge--info badge--no-dot" { "Built-in" } }
            }
            p class="template-card__description" { (template.description) }
            // The header has room for one badge: the other one goes with the action.
            div class="cluster cluster--between full-width" {
                a class="button button--secondary" href=(href("/mcps/new", query, &[("template", template.id)])) {
                    "Set up " (template.name)
                }
                @if template.popular { span class="badge badge--brand badge--no-dot" { "Popular" } }
            }
        }
    }
}

/// The dialog of the "Add MCP" action. It is in every page of the MCPs, so
/// the button opens it at once; `/mcps/new` is the page it is open on.
pub(super) fn gallery_dialog(query: &McpListQuery, open: bool) -> Markup {
    let popular = MCP_TEMPLATES
        .iter()
        .filter(|template| template.popular)
        .count();
    html! {
        dialog class="dialog dialog--xl" id="add-mcp" aria-labelledby="add-mcp-title" data-open[open]
            data-dialog-return=(href("/mcps", query, &[])) {
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="add-mcp-title" { "Add an MCP" }
                    p class="dialog__subtitle" { "Start from a trusted template or configure your own server" }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" data-filter {
                label class="input-group search" {
                    span class="visually-hidden" { "Search templates" }
                    (icon("search"))
                    input class="input-group__control" type="search" placeholder="Search Notion, Shopify, GitHub…" autocomplete="off"
                        data-filter-input;
                }
                div class="tabs__list" role="tablist" aria-label="Template categories" {
                    (tab(POPULAR, "Popular", true))
                    (tab("all", "All", false))
                    @for category in TemplateCategory::ALL { (tab(category.key(), category.label(), false)) }
                }
                p class="text-body-sm text-tertiary" data-filter-count data-singular="template" data-plural="templates" {
                    (popular) " templates"
                }
                div class="grid grid--3" {
                    @for template in MCP_TEMPLATES { (template_card(template, query)) }
                }
                div class="empty-state" data-filter-empty hidden {
                    span class="empty-state__icon" { (icon("search")) }
                    div class="empty-state__text" {
                        h3 class="empty-state__title" { "No templates found" }
                        p class="empty-state__description" { "Try another search or clear the active category." }
                    }
                    button type="button" class="button button--secondary" data-filter-clear { "Clear filters" }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                a class="button button--secondary" href=(href("/mcps/new", query, &[("template", "custom")])) { (icon("plus")) "Custom MCP" }
            }
        }
    }
}
