//! The Analytics page: the figures of a period, its timeline, and the
//! busiest MCPs, tools and access tokens. (`inertia/pages/analytics/index.tsx`,
//! prototypes `09-analytics.html` and `09-analytics-empty.html`)

use maud::{Markup, html};

use crate::routes::analytics::{Analytics, Breakdown, CustomRangeFields, Range, RangeConfig};
use crate::routes::logs::link;
use crate::views::charts::{fixed_1, line_chart, number};
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};

fn logging_off_banner() -> Markup {
    html! {
        div class="banner banner--warning" role="status" {
            (icon("triangle-alert"))
            div class="banner__content" {
                p class="banner__title" { "Call logging is off" }
                p { "Analytics includes retained records only. Enable logging in Settings to collect new data." }
            }
        }
    }
}

/// The address of a preset range, in the zone the page is drawn in.
fn range_address(range: Range, time_zone: &str) -> String {
    link(
        "/analytics",
        &[("range", range.as_str()), ("timeZone", time_zone)],
    )
}

fn ranges(analytics: &Analytics) -> Markup {
    let current = analytics.config.range;
    html! {
        nav class="segmented" aria-label="Analytics time range" {
            @for (range, label) in [(Range::Hours24, "24 hours"), (Range::Days7, "7 days"), (Range::Days30, "30 days")] {
                a class="segment" href=(range_address(range, &analytics.time_zone))
                    aria-current=[(range == current).then_some("true")] { (label) }
            }
            // Without script the link draws this page with the dialog open.
            // `page` and not `true`: the page script takes `true` off the
            // trigger of a dialog when the dialog closes.
            a class="segment" href=(range_address(Range::Custom, &analytics.time_zone)) data-dialog-open="#custom-range"
                aria-current=[(current == Range::Custom).then_some("page")] { "Custom" }
        }
    }
}

fn ranking(id: &str, title: &str, rows: &[Breakdown], code: bool) -> Markup {
    html! {
        section class="card" aria-labelledby=(id) {
            header class="card__header" { div class="card__heading" { h2 class="card__title" id=(id) { (title) } } }
            @if rows.is_empty() {
                div class="card__body" { p class="text-secondary" { "No data for this period." } }
            } @else {
                div class="table-scroll" {
                    table class="table table--tight" {
                        thead { tr {
                            th { "Name" }
                            th class="col-48 cell-end" { "Calls" }
                            th class="col-40 cell-end" { "Errors" }
                            th class="col-60 cell-end" { "Avg." }
                        } }
                        tbody {
                            @for row in rows {
                                tr {
                                    td class=(if code { "cell-code" } else { "cell-strong" }) { (row.label) }
                                    td class="cell-end" { (number(row.total)) }
                                    td class="cell-end cell-secondary" { (number(row.errors)) }
                                    td class="cell-end cell-secondary" { (number(row.average_duration_ms)) " ms" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The figures, the timeline and the rankings: what the live refresh replaces.
fn data(analytics: &Analytics) -> Markup {
    let metrics = &analytics.metrics;
    let description = "Calls and errors over the selected time range";
    html! {
        div class="page__body" id="analytics-data" {
            dl class="card metric-strip" {
                div class="metric" { dt class="metric__label" { "Total calls" } dd class="metric__value" { (number(metrics.total)) } }
                div class="metric" { dt class="metric__label" { "Success rate" } dd class="metric__value" { (fixed_1(metrics.success_rate())) "%" } }
                div class="metric" { dt class="metric__label" { "Error rate" } dd class="metric__value" { (fixed_1(metrics.error_rate())) "%" } }
                div class="metric" { dt class="metric__label" { "Average duration" } dd class="metric__value" { (number(metrics.average_duration_ms)) " ms" } }
            }
            @if metrics.total == 0 {
                // One empty state stands for the chart and the three rankings:
                // none of them has a row when the period has no call.
                div class="card" {
                    div class="empty-state" {
                        span class="empty-state__icon" { (icon("chart-column")) }
                        div class="empty-state__text" {
                            p class="empty-state__title" { "No calls in this period" }
                            p class="empty-state__description" { "Choose another time range or make a tool call through the MCP gateway." }
                        }
                    }
                }
            } @else {
                section class="card" aria-labelledby="timeline-title" {
                    header class="card__header" {
                        div class="card__heading" {
                            h2 class="card__title" id="timeline-title" { "Calls and errors over time" }
                            // The zone the timeline was cut in: a custom range keeps its own.
                            p class="card__subtitle" { "Successful and failed tool-call attempts in " (analytics.time_zone_label) "." }
                        }
                    }
                    (line_chart(description, &analytics.timeline))
                }
                div class="grid grid--fit" {
                    (ranking("top-mcps-title", "Top MCPs", &analytics.top_mcps, false))
                    (ranking("top-tools-title", "Top tools", &analytics.top_tools, true))
                    (ranking("top-tokens-title", "Top access tokens", &analytics.top_tokens, false))
                }
            }
        }
    }
}

/// The form of the custom range dialog. Its dates and times are those of a
/// clock of the zone named in the subtitle; the server turns them into the
/// two instants of the address.
pub fn custom_range_form(
    config: &RangeConfig,
    time_zone: &str,
    time_zone_label: &str,
    fields: &CustomRangeFields,
    error: Option<&str>,
) -> Markup {
    let value = |field: &Option<String>| field.clone().unwrap_or_default();
    html! {
        form method="get" action="/analytics" data-async {
            input type="hidden" name="range" value="custom";
            input type="hidden" name="timeZone" value=(time_zone);
            // The period the dialog opened on: a time left as it is keeps
            // its instant, which matters when a clock change repeats it.
            input type="hidden" name="start" value=(config.start.to_utc_iso());
            input type="hidden" name="end" value=(config.end.to_utc_iso());
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="custom-range-title" { "Custom analytics range" }
                    p class="dialog__subtitle" { "Choose exact times in " (time_zone_label) }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="field" role="group" aria-labelledby="custom-range-dates" {
                    span class="field__label" id="custom-range-dates" { "Dates" }
                    div class="grid grid--2 grid--keep" {
                        input class="input" type="date" name="startDate" value=(value(&fields.start_date)) aria-label="Start date" required;
                        input class="input" type="date" name="endDate" value=(value(&fields.end_date)) aria-label="End date" required;
                    }
                }
                div class="grid grid--2 grid--keep" {
                    div class="field" {
                        label class="field__label" for="custom-range-start-time" { "Start time" }
                        input class="input" type="time" id="custom-range-start-time" name="startTime"
                            value=(value(&fields.start_time)) step="300" required;
                    }
                    div class="field" {
                        label class="field__label" for="custom-range-end-time" { "End time" }
                        input class="input" type="time" id="custom-range-end-time" name="endTime"
                            value=(value(&fields.end_time)) step="300" required
                            aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| "custom-range-error")];
                        @if let Some(message) = error { p class="field__error" id="custom-range-error" { (message) } }
                    }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { "Apply range" }
            }
        }
    }
}

pub fn analytics_page(context: &PageContext, analytics: &Analytics) -> Markup {
    let config = &analytics.config;
    let content = html! {
        @if analytics.logging_off { (logging_off_banner()) }
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "MCP analytics" }
                p class="page-header__subtitle" { "Track gateway usage, reliability, and tool-call latency." }
            }
            div class="page-header__actions" { a class="button button--secondary" href="/logs" { "View logs" } }
        }
        div class="toolbar" {
            (ranges(analytics))
            div class="live push-end" {
                span class="status-dot status-dot--success" {}
                span id="live-label" { "Live · refreshes every 30 s" }
                button type="button" class="switch" aria-pressed="false" aria-labelledby="live-label"
                    data-live-refresh="#analytics-data" data-live-interval="30000" {}
            }
        }
        // A preset range follows the viewer: the page script submits this
        // once when their time zone is not the one the page was drawn in.
        // A custom range keeps the zone it was chosen in.
        @if config.range != Range::Custom {
            form method="get" action="/analytics" data-timezone-sync hidden {
                input type="hidden" name="range" value=(if analytics.custom_open { "custom" } else { config.range.as_str() });
                input type="hidden" name="timeZone" value=(analytics.time_zone) data-timezone;
            }
        }
        (data(analytics))
    };
    let dialog = html! {
        dialog class="dialog dialog--sm" id="custom-range" aria-labelledby="custom-range-title"
            data-open[analytics.custom_open]
            data-dialog-return=[analytics.custom_open.then_some(analytics.address.as_str())] {
            div data-fragment {
                (custom_range_form(config, &analytics.time_zone, &analytics.time_zone_label, &analytics.custom, analytics.custom_error))
            }
        }
    };
    app_page(context, "MCP analytics", content, dialog)
}
