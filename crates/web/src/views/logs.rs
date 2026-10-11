//! The Logs page: the filters, the list of calls, and the panel with the
//! details of one call. (`inertia/pages/logs/index.tsx`, prototypes
//! `07-logs.html` and `08-logs-details.html`)

use maud::{Markup, html};
use mymcps_core::Timestamp;
use mymcps_core::models::{CallOutcome, McpCallLog};
use serde_json::{Value, json};

use crate::routes::analytics::{Zone, ZonedTime};
use crate::routes::logs::{DEFAULT_PAGE_SIZE, LogFilters, LogRange, Logs, PAGE_SIZES, link};
use crate::views::charts::number;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};

/// A date and time to the second, written in the zone of the page for a
/// browser without script. The page script rewrites it in the viewer's zone.
fn local_time(timestamp: Timestamp, zone: &Zone) -> Markup {
    let local = ZonedTime::at(timestamp.as_datetime().timestamp(), *zone);
    html! {
        time datetime=(timestamp.to_iso()) { (local.format("%d/%m/%Y, %H:%M:%S")) }
    }
}

/// The address of the list with these filters. What has its default value
/// is left out.
fn address(filters: &LogFilters, page: i64, log_id: Option<i64>) -> String {
    let optional = |shown: bool, value: i64| {
        if shown {
            value.to_string()
        } else {
            String::new()
        }
    };
    link(
        "/logs",
        &[
            (
                "range",
                if filters.range == LogRange::Hours24 {
                    ""
                } else {
                    filters.range.as_str()
                },
            ),
            ("outcome", &filters.outcome),
            ("mcp", &filters.mcp),
            ("token", &filters.token),
            (
                "pageSize",
                &optional(filters.page_size != DEFAULT_PAGE_SIZE, filters.page_size),
            ),
            ("timeZone", shown_time_zone(&filters.time_zone)),
            ("page", &optional(page > 1, page)),
            (
                "logId",
                &log_id.map(|id| id.to_string()).unwrap_or_default(),
            ),
        ],
    )
}

/// UTC is what a page without a zone is drawn in: it needs no parameter.
fn shown_time_zone(time_zone: &str) -> &str {
    if time_zone == "UTC" { "" } else { time_zone }
}

fn outcome_badge(outcome: CallOutcome) -> Markup {
    html! {
        @match outcome {
            CallOutcome::Success => span class="badge badge--success" { "Success" },
            CallOutcome::Error => span class="badge badge--critical" { "Error" },
        }
    }
}

fn duration(log: &McpCallLog) -> String {
    format!("{} ms", number(log.duration_ms))
}

/// The tool that ran, or the name that was asked for when no tool did.
fn tool_name(log: &McpCallLog) -> &str {
    log.tool_name.as_deref().unwrap_or(&log.requested_tool_name)
}

fn mcp_name(log: &McpCallLog) -> Option<&str> {
    log.mcp_name.as_deref().or(log.mcp_slug.as_deref())
}

fn logging_off_banner() -> Markup {
    html! {
        div class="banner banner--warning" role="status" {
            (icon("triangle-alert"))
            div class="banner__content" {
                p class="banner__title" { "Call logging is off" }
                p { "Existing records remain available until retention removes them. Enable logging in Settings to capture new calls." }
            }
        }
    }
}

/// One choice of a filter menu: a radio of the filter form.
fn choice(name: &str, value: &str, label: &str, checked: bool) -> Markup {
    html! {
        label class="menu__item" {
            input type="radio" class="visually-hidden" name=(name) value=(value) checked[checked];
            (label)
        }
    }
}

/// A filter: its chip, tinted with a way to clear it once it has a value,
/// and the menu the chip opens.
fn filter(
    name: &str,
    title: &str,
    selected: Option<(&str, String)>,
    choices: &[(&str, &str)],
    value: &str,
) -> Markup {
    let menu = format!("filter-{name}");
    html! {
        @if let Some((label, without)) = &selected {
            span class="chip chip--active" {
                button type="button" class="chip__label" popovertarget=(menu) { (title) ": " (label) }
                a class="chip__clear" href=(without) aria-label=(format!("Clear {} filter", title.to_lowercase().replace("mcp", "MCP"))) { (icon("x")) }
            }
        } @else {
            button type="button" class="chip" popovertarget=(menu) { (title) (icon("chevron-down")) }
        }
        div class="menu menu--start" id=(menu) popover role="radiogroup" aria-label=(title) {
            @for (choice_value, label) in choices {
                (choice(name, choice_value, label, *choice_value == value))
            }
        }
    }
}

/// The choices of a filter whose values have names: the first one is "all",
/// and a value that is filtered on without being among them is added, so
/// that the next submit of the form keeps it.
fn named_choices<'a>(
    all: &'a str,
    options: &'a [(String, String)],
    value: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let mut choices = vec![("", all)];
    let mut seen = Vec::new();
    for (option, label) in options {
        // A token renamed since its first call is listed once.
        if !seen.contains(&option.as_str()) {
            seen.push(option.as_str());
            choices.push((option.as_str(), label.as_str()));
        }
    }
    if !value.is_empty() && !seen.contains(&value) {
        choices.push((value, value));
    }
    choices
}

fn filters_form(logs: &Logs) -> Markup {
    let filters = &logs.filters;
    let ranges: Vec<(&str, &str)> = LogRange::ALL
        .iter()
        .map(|range| (range.as_str(), range.label()))
        .collect();
    let outcomes = [
        ("", "All outcomes"),
        ("success", "Success"),
        ("error", "Error"),
    ];
    let mcps = named_choices("All MCPs", &logs.mcp_options, &filters.mcp);
    let tokens = named_choices("All tokens", &logs.token_options, &filters.token);
    let label_of = |choices: &[(&str, &str)], value: &str| {
        choices
            .iter()
            .find(|(choice, _)| *choice == value)
            .map_or(value.to_string(), |(_, label)| label.to_string())
    };
    let without = |change: fn(&mut LogFilters)| {
        let mut filters = filters.clone();
        change(&mut filters);
        address(&filters, 1, None)
    };
    let range_menu = "filter-range";

    html! {
        form class="cluster" method="get" action="/logs" data-autosubmit data-async data-async-history data-timezone-sync {
            input type="hidden" name="timeZone" value=(filters.time_zone) data-timezone;
            @if filters.page_size != DEFAULT_PAGE_SIZE {
                input type="hidden" name="pageSize" value=(filters.page_size);
            }
            // The period always has a value: its chip only reads it.
            button type="button" class="chip" popovertarget=(range_menu) { (filters.range.label()) (icon("chevron-down")) }
            div class="menu menu--start" id=(range_menu) popover role="radiogroup" aria-label="Time range" {
                @for (value, label) in &ranges { (choice("range", value, label, *value == filters.range.as_str())) }
            }
            (filter(
                "outcome",
                "Outcome",
                (!filters.outcome.is_empty()).then(|| (
                    if filters.outcome == "error" { "Error" } else { "Success" },
                    without(|filters| filters.outcome.clear()),
                )),
                &outcomes,
                &filters.outcome,
            ))
            @let mcp_label = label_of(&mcps, &filters.mcp);
            (filter(
                "mcp",
                "MCP",
                (!filters.mcp.is_empty()).then(|| (mcp_label.as_str(), without(|filters| filters.mcp.clear()))),
                &mcps,
                &filters.mcp,
            ))
            @let token_label = label_of(&tokens, &filters.token);
            (filter(
                "token",
                "Access token",
                (!filters.token.is_empty()).then(|| (token_label.as_str(), without(|filters| filters.token.clear()))),
                &tokens,
                &filters.token,
            ))
            a class="button" href=(link("/logs", &[("timeZone", shown_time_zone(&filters.time_zone))])) { "Clear filters" }
            noscript { button type="submit" class="button button--secondary" { "Apply filters" } }
        }
    }
}

/// The switch that refreshes the list every 30 seconds. Off when the page
/// loads: the page script restores the choice of the session.
fn live_switch(target: &str) -> Markup {
    html! {
        div class="live push-end" {
            span class="status-dot status-dot--success" {}
            span id="live-label" { "Live · refreshes every 30 s" }
            button type="button" class="switch" aria-pressed="false" aria-labelledby="live-label"
                data-live-refresh=(target) data-live-interval="30000" {}
        }
    }
}

fn pagination(logs: &Logs, classes: &str) -> Markup {
    let size = logs.filters.page_size;
    let first = (logs.page - 1).saturating_mul(size) + 1;
    let last = logs.page.saturating_mul(size).min(logs.total);
    let previous = (logs.page > 1).then(|| address(&logs.filters, logs.page - 1, None));
    let next = (logs.page < logs.total_pages).then(|| address(&logs.filters, logs.page + 1, None));
    html! {
        nav class=(classes) aria-label="Pagination" {
            span class="pagination__range" { (number(first)) "–" (number(last)) " of " (number(logs.total)) }
            span class="pagination__buttons" {
                a class="icon-button icon-button--secondary" href=[previous.as_deref()]
                    aria-disabled=[previous.is_none().then_some("true")] aria-label="Previous page" { (icon("chevron-left")) }
                a class="icon-button icon-button--secondary" href=[next.as_deref()]
                    aria-disabled=[next.is_none().then_some("true")] aria-label="Next page" { (icon("chevron-right")) }
            }
        }
    }
}

/// The chip that opens the choice of how many calls a page lists.
fn page_size_chip(logs: &Logs) -> Markup {
    html! {
        button type="button" class="chip" popovertarget="logs-page-size" {
            (logs.filters.page_size) " per page" (icon("chevron-down"))
        }
    }
}

fn page_size_menu(logs: &Logs) -> Markup {
    html! {
        div class="menu" id="logs-page-size" popover role="menu" aria-label="Calls per page" {
            @for size in PAGE_SIZES {
                @let filters = LogFilters { page_size: size, ..logs.filters.clone() };
                a class="menu__item" role="menuitemradio" href=(address(&filters, 1, None))
                    aria-checked=(if size == logs.filters.page_size { "true" } else { "false" }) { (size) " per page" }
            }
        }
    }
}

/// What makes a row open the details of its call: a link the page script
/// loads into the panel, and a page of its own without script.
struct Row<'a> {
    log: &'a McpCallLog,
    href: String,
    return_to: &'a str,
    current: bool,
}

fn table(logs: &Logs, rows: &[Row<'_>]) -> Markup {
    html! {
        div class="card hide-mobile" {
            div class="table-scroll" {
                table class="table table--dense table--interactive" {
                    thead { tr {
                        th scope="col" class="col-156" { "Time" }
                        th scope="col" class="col-120" { "MCP" }
                        th scope="col" { "Tool" }
                        th scope="col" class="col-140" { "Token" }
                        th scope="col" class="col-92" { "Outcome" }
                        th scope="col" class="col-80 cell-end" { "Duration" }
                    } }
                    tbody {
                        @for row in rows {
                            tr {
                                td class="cell-secondary" {
                                    a class="row-link" id=(format!("log-{}", row.log.id)) href=(row.href)
                                        data-dialog-open="#call-details" data-dialog-fetch data-dialog-history
                                        data-dialog-return=(row.return_to) aria-current=[row.current.then_some("true")] {
                                        span class="visually-hidden" { "Call details, " }
                                        (local_time(row.log.created_at, &logs.zone))
                                    }
                                }
                                td { (mcp_name(row.log).unwrap_or("—")) }
                                td class="cell-code-strong" { (tool_name(row.log)) }
                                td { (row.log.access_token_name) }
                                td { (outcome_badge(row.log.outcome)) }
                                td class="cell-end tabular" { (duration(row.log)) }
                            }
                        }
                    }
                }
            }
            div class="table-footer" {
                p class="table-footer__note" { "Times shown in " span data-timezone-label { (logs.time_zone_label) } "." }
                div class="cluster cluster--nowrap" {
                    (page_size_chip(logs))
                    (pagination(logs, "pagination"))
                }
            }
        }
    }
}

/// Under 768px: the same calls as a list.
fn list(logs: &Logs, rows: &[Row<'_>]) -> Markup {
    html! {
        section class="card hide-desktop" aria-labelledby="logs-list-title" {
            header class="card__header" {
                div class="card__heading" {
                    h2 class="card__title" id="logs-list-title" { "Tool calls" }
                    p class="card__subtitle" { "Times shown in " span data-timezone-label { (logs.time_zone_label) } "." }
                }
            }
            @for row in rows {
                a class="list-row" href=(row.href) data-dialog-open="#call-details" data-dialog-fetch data-dialog-history
                    data-dialog-return=(row.return_to) aria-current=[row.current.then_some("true")] {
                    span class="list-row__text" {
                        span class="list-row__title list-row__title--code" { (tool_name(row.log)) }
                        span class="list-row__subtitle" { (mcp_name(row.log).unwrap_or("Unknown MCP")) " · " (row.log.access_token_name) }
                        span class="list-row__subtitle" { (local_time(row.log.created_at, &logs.zone)) " · " (duration(row.log)) }
                    }
                    (outcome_badge(row.log.outcome))
                }
            }
            div class="table-footer" {
                (page_size_chip(logs))
                (pagination(logs, "pagination push-end"))
            }
        }
    }
}

/// The list of calls, which the live refresh replaces.
fn results(logs: &Logs) -> Markup {
    let filters = &logs.filters;
    let return_to = address(filters, logs.page, None);
    let rows: Vec<Row<'_>> = logs
        .logs
        .iter()
        .map(|log| Row {
            log,
            href: address(filters, logs.page, Some(log.id)),
            return_to: &return_to,
            current: logs
                .selected
                .as_ref()
                .is_some_and(|selected| selected.id == log.id),
        })
        .collect();
    let filtered =
        !(filters.outcome.is_empty() && filters.mcp.is_empty() && filters.token.is_empty());
    let unfiltered = LogFilters {
        outcome: String::new(),
        mcp: String::new(),
        token: String::new(),
        ..filters.clone()
    };

    html! {
        div class="page__body" id="logs-results" {
            @if rows.is_empty() {
                div class="card" {
                    div class="empty-state" {
                        span class="empty-state__icon" { (icon("search")) }
                        div class="empty-state__text" {
                            p class="empty-state__title" { "No calls in this view" }
                            p class="empty-state__description" { "Change the filters or make a tool call through the MCP gateway." }
                        }
                        @if filtered {
                            a class="button button--secondary" href=(address(&unfiltered, 1, None)) { "Clear filters" }
                        }
                    }
                }
            } @else {
                (table(logs, &rows))
                (list(logs, &rows))
                (page_size_menu(logs))
            }
        }
    }
}

/// The filters and the list: what a change of filter replaces.
pub fn logs_content(logs: &Logs) -> Markup {
    html! {
        div class="toolbar toolbar--tight" {
            (filters_form(logs))
            (live_switch("#logs-results"))
        }
        (results(logs))
    }
}

pub fn logs_page(context: &PageContext, logs: &Logs) -> Markup {
    let content = html! {
        @if logs.logging_off { (logging_off_banner()) }
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "MCP call logs" }
                p class="page-header__subtitle" { "Inspect individual tool calls, failures, arguments, and timing." }
            }
            div class="page-header__actions" { a class="button button--secondary" href="/analytics" { "View analytics" } }
        }
        div class="page__body" id="logs" data-fragment { (logs_content(logs)) }
    };
    // The panel is in the page from the start: the page script fills it
    // with the call of the row that was chosen. Without script, the address
    // of a call draws the page with its panel open.
    let selected = logs.selected.as_ref();
    let panel = html! {
        dialog class="drawer" id="call-details" aria-labelledby="call-details-title" data-dialog-modal="false"
            data-open[selected.is_some()] data-dialog-trigger=[selected.map(|log| format!("#log-{}", log.id))]
            data-dialog-return=(address(&logs.filters, logs.page, None)) {
            div data-fragment {
                @if selected.is_some() { (call_details(selected, &logs.zone)) }
            }
        }
    };
    app_page(context, "MCP call logs", content, panel)
}

/// What was captured of the arguments or of the response of a call, as the
/// panel shows it: `None` when nothing was to be captured, an empty text
/// when there was nothing to capture.
fn captured_text(captured: bool, value: Option<&str>) -> Option<String> {
    if !captured {
        return None;
    }
    let Some(value) = value else {
        return Some(String::new());
    };
    Some(match serde_json::from_str::<Value>(value) {
        Ok(parsed) => serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| value.to_string()),
        Err(_) => value.to_string(),
    })
}

/// The same, as a value of the JSON of the call.
fn captured_value(value: Option<&str>) -> Value {
    match value {
        Some(value) => serde_json::from_str(value).unwrap_or_else(|_| Value::from(value)),
        None => Value::Null,
    }
}

/// The call as the log holds it, for "Copy as JSON".
fn call_json(log: &McpCallLog) -> String {
    let call = json!({
        "id": log.id,
        "createdAt": log.created_at.to_iso(),
        "outcome": log.outcome.as_str(),
        "requestedToolName": log.requested_tool_name,
        "toolName": log.tool_name,
        "mcpName": log.mcp_name,
        "mcpSlug": log.mcp_slug,
        "accessTokenName": log.access_token_name,
        "accessTokenPrefix": log.access_token_prefix,
        "callerIp": log.caller_ip,
        "durationMs": log.duration_ms,
        "errorCategory": log.error_category.map(|category| category.as_str()),
        "errorSummary": log.error_summary,
        "arguments": captured_value(log.arguments.as_deref()),
        "response": captured_value(log.response.as_deref()),
    });
    serde_json::to_string_pretty(&call).unwrap_or_default()
}

fn info_banner(title: &str, description: &str) -> Markup {
    html! {
        div class="banner" role="status" {
            (icon("info"))
            div class="banner__content" { p class="banner__title" { (title) } p { (description) } }
        }
    }
}

fn code_block(title: &str, id: &str, copy_label: &str, code: &str) -> Markup {
    html! {
        div class="code-block" {
            div class="code-block__header" {
                span class="code-block__title" { (title) }
                button type="button" class="icon-button" data-copy-target=(format!("#{id}")) aria-label=(copy_label) {
                    (icon("copy")) (icon("check"))
                }
            }
            pre class="code-block__body" id=(id) { (code) }
        }
    }
}

/// The content of the call details panel. `None` is a call that is no
/// longer in the log: the retention removed it while its row was on screen.
pub fn call_details(log: Option<&McpCallLog>, zone: &Zone) -> Markup {
    let Some(log) = log else {
        return html! {
            div class="drawer__header" {
                div class="drawer__heading" { h2 class="drawer__title" id="call-details-title" { "Call details" } }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="drawer__body" {
                (info_banner("This call is no longer in the log", "Records are removed once the retention period has passed."))
            }
        };
    };
    let arguments = captured_text(log.arguments_captured, log.arguments.as_deref());
    let response = captured_text(log.response_captured, log.response.as_deref());

    html! {
        div class="drawer__header" {
            div class="drawer__heading" {
                h2 class="drawer__title" id="call-details-title" { "Call details" }
                p class="drawer__meta" { (local_time(log.created_at, zone)) }
            }
            (outcome_badge(log.outcome))
            button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
        }
        div class="drawer__body" {
            @if let Some(category) = log.error_category {
                div class="banner banner--critical" role="alert" {
                    (icon("circle-x"))
                    div class="banner__content" {
                        p class="banner__title" { (category.as_str().replace('_', " ")) }
                        p { (log.error_summary.as_deref().unwrap_or("No error summary available.")) }
                    }
                }
            }
            dl class="key-values" {
                div class="key-value" { dt { "Requested tool" } dd { (log.requested_tool_name) } }
                div class="key-value" { dt { "MCP" } dd { (mcp_name(log).unwrap_or("Unknown")) } }
                div class="key-value" { dt { "Access token" } dd { (log.access_token_name) " (" (log.access_token_prefix) "…)" } }
                div class="key-value" { dt { "Caller IP" } dd { (log.caller_ip.as_deref().unwrap_or("Unknown")) } }
                div class="key-value" { dt { "Started" } dd { (local_time(log.created_at, zone)) } }
                div class="key-value" { dt { "Duration" } dd { (duration(log)) } }
            }
            @match arguments.as_deref() {
                None => (info_banner("Arguments were not captured", "The metadata logging level was active for this call.")),
                Some("") => (info_banner("No arguments supplied", "Argument capture was active, but this call did not include arguments.")),
                Some(code) => (code_block("Arguments", "call-arguments", "Copy arguments", code)),
            }
            @match response.as_deref() {
                None => (info_banner("Response was not captured", "Response capture was not active for this call.")),
                Some("") => (info_banner("No MCP response received", "Response capture was active, but the upstream MCP did not return a result.")),
                Some(code) => (code_block("Response", "call-response", "Copy response", code)),
            }
        }
        div class="drawer__footer" {
            // A deleted MCP leaves its calls in the log, without their link to it.
            @if let Some(mcp_id) = log.mcp_id {
                a class="button button--secondary" href=(format!("/mcps/{mcp_id}/edit")) { "Open MCP" }
            }
            button type="button" class="button" data-copy-target="#call-json" {
                (icon("copy")) (icon("check")) span data-copy-label { "Copy as JSON" }
            }
            pre id="call-json" hidden { (call_json(log)) }
        }
    }
}
