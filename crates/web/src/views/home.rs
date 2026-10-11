//! The home page: the "01 Home" screen of the redesign for an
//! administrator, and the home of a member, which leaves out everything
//! that comes from the call log. (prototypes `01-home.html` and
//! `shell-member.html`)

use maud::{Markup, html};
use mymcps_core::models::{
    AccessToken, CallOutcome, Mcp, McpCallLog, McpStatus, McpTransport, TokenSource,
};

use crate::routes::home::{
    ACTIVITY_DAYS, Activity, Client, Home, HomeQuery, Stats, TokenState, connection_expires_at,
    token_state,
};
use crate::routes::logs::{LogRange, link};
use crate::views::charts::{bar_chart, counted, fixed_1, number};
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};
use crate::views::{date, relative_time};

/// How many rows a side card lists before it sums the rest up.
const SIDE_ROWS: usize = 5;

/// The address of the page with another range or another client. What has
/// its default value is left out.
fn address(query: &HomeQuery) -> String {
    link(
        "/",
        &[
            (
                "range",
                if query.range == LogRange::Days7 {
                    ""
                } else {
                    query.range.as_str()
                },
            ),
            (
                "client",
                if query.client == Client::Claude {
                    ""
                } else {
                    query.client.id()
                },
            ),
        ],
    )
}

/// The icon of an MCP, as the MCPs page draws it: the one of its template,
/// else one for its transport.
fn mcp_icon(mcp: &Mcp) -> &'static str {
    crate::mcp_templates::icon_of(mcp)
}

fn transport(mcp: &Mcp) -> &'static str {
    match mcp.transport {
        McpTransport::Http => "HTTP",
        McpTransport::Npm => "npm",
        McpTransport::Builtin => "Built-in",
    }
}

/// `in the last 7 days`, as a sentence ends with it.
fn range_words(range: LogRange) -> &'static str {
    match range {
        LogRange::Hours24 => "in the last 24 hours",
        LogRange::Days30 => "in the last 30 days",
        _ => "in the last 7 days",
    }
}

/// The period, and the four figures of it. Administrators only: three of
/// the figures come from the call log.
fn context_bar(home: &Home, stats: &Stats) -> Markup {
    let query = &home.query;
    // Without a call to compute it from, a rate is unknown, not zero.
    let unknown = |value: String| {
        if stats.has_rates() {
            html! { dd class="stat__value" { (value) } }
        } else {
            html! {
                dd class="stat__value" title=[stats.logging_off.then_some("Call logging is off")] { "—" }
            }
        }
    };
    html! {
        div class="context-bar hide-mobile" {
            button type="button" class="range" popovertarget="home-range" { (query.range.label()) (icon("chevron-down")) }
            div class="menu menu--start" id="home-range" popover role="menu" {
                @for range in [LogRange::Hours24, LogRange::Days7, LogRange::Days30] {
                    a class="menu__item" role="menuitemradio" aria-checked=(if range == query.range { "true" } else { "false" })
                        href=(address(&HomeQuery { range, ..query.clone() })) { (range.label()) }
                }
            }
            dl class="stats" {
                div class="stat" { dt class="stat__label" { "Tool calls" } dd class="stat__value" { (number(stats.metrics.total)) } }
                div class="stat" { dt class="stat__label" { "Success rate" } (unknown(format!("{}%", fixed_1(stats.metrics.success_rate())))) }
                div class="stat" { dt class="stat__label" { "Avg. duration" } (unknown(format!("{} ms", number(stats.metrics.average_duration_ms)))) }
                div class="stat" {
                    dt class="stat__label" { "Tools exposed" }
                    dd class="stat__value" {
                        @match home.tools { Some(tools) => { (number(tools.tools as i64)) }, None => { "—" } }
                    }
                }
            }
        }
    }
}

/// The second line of the greeting: what is behind the endpoint.
fn endpoint_sentence(home: &Home) -> String {
    match (home.mcps.len(), home.enabled_mcps()) {
        (0, _) => "Add your first MCP to get started.".into(),
        (_, 0) => "No MCP is enabled right now.".into(),
        (_, 1) => "1 MCP behind one endpoint.".into(),
        (_, enabled) => format!("{} MCPs behind one endpoint.", number(enabled as i64)),
    }
}

/// The gateway endpoint, the tool mode of the instance, and the way to
/// install the gateway in a client.
fn hero_card(context: &PageContext, home: &Home) -> Markup {
    let query = &home.query;
    // The address MCP clients use needs APP_URL, as the sidebar's does.
    let gateway_url = context.gateway_url().filter(|_| context.app_url_configured);
    let tool_mode = format!("Tool mode: {}", home.tool_mode.as_str());
    html! {
        div class="hero-card" {
            div class="hero-card__row" {
                span { "Gateway endpoint" }
                @if gateway_url.is_some() {
                    span class="badge badge--success" { "Online" }
                } @else {
                    span class="badge badge--warning" { "APP_URL not set" }
                }
            }
            div class="copy-field copy-field--lg" {
                @if let Some(gateway_url) = &gateway_url {
                    span class="copy-field__value" id="gateway-url" { (gateway_url) }
                    button type="button" class="icon-button" data-copy-target="#gateway-url" aria-label="Copy gateway URL" {
                        (icon("copy")) (icon("check"))
                    }
                } @else {
                    span class="copy-field__value" { "Configure APP_URL to reveal the gateway URL." }
                    button type="button" class="icon-button" disabled title="Set APP_URL to enable public links"
                        aria-label="Copy gateway URL" { (icon("copy")) }
                }
            }
            div class="hero-card__row" {
                div class="cluster hide-mobile" {
                    // The tool mode is a setting of the instance, which an administrator changes.
                    @if home.admin {
                        a class="chip" href="/settings" { (tool_mode) }
                    } @else {
                        span class="chip" { (tool_mode) }
                    }
                    button type="button" class="chip" popovertarget="home-client" { "Client: " (query.client.label()) (icon("chevron-down")) }
                    div class="menu menu--start" id="home-client" popover role="menu" {
                        @for client in Client::ALL {
                            a class="menu__item" role="menuitemradio" aria-checked=(if client == query.client { "true" } else { "false" })
                                href=(address(&HomeQuery { client, ..query.clone() })) { (client.label()) }
                        }
                    }
                }
                a class="button button--primary" href=(link("/tokens", &[("install", "1"), ("client", query.client.id())])) {
                    (icon("download")) "Install in a client"
                }
            }
        }
    }
}

/// With no MCP at all, the page invites to add the first one.
fn no_mcps_card(classes: &str) -> Markup {
    html! {
        div class=(classes) {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("plug")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No MCPs yet" }
                    p class="empty-state__description" { "Create an MCP to start routing agent traffic." }
                }
                a class="button button--primary" href="/mcps/new" { (icon("plus")) "Add MCP" }
            }
        }
    }
}

fn attention_item(href: &str, icon_name: &str, text: &str) -> Markup {
    html! {
        a class="attention__item" href=(href) {
            (icon(icon_name)) span class="truncate" { (text) } (icon("chevron-right"))
        }
    }
}

/// What needs the person, each a link to where it is dealt with. Nothing
/// when there is nothing to say.
fn attention(home: &Home) -> Markup {
    let mut items = Vec::new();
    let mut mcps = |mcps: Vec<&Mcp>, icon_name: &'static str, one: &str, several: &str| match mcps
        .as_slice()
    {
        [] => {}
        [mcp] => items.push((
            format!("/mcps/{}/edit", mcp.id),
            icon_name,
            format!("{} {one}", mcp.name),
        )),
        many => items.push((
            "/mcps".to_string(),
            icon_name,
            format!("{} MCPs {several}", number(many.len() as i64)),
        )),
    };
    mcps(
        home.mcps_awaiting_authorization(),
        "triangle-alert",
        "needs authorization",
        "need authorization",
    );
    mcps(
        home.mcps_in_error(),
        "unplug",
        "is in error",
        "are in error",
    );
    if home.pending_approvals > 0 {
        items.push((
            "/approvals".to_string(),
            "shield-check",
            if home.pending_approvals == 1 {
                "1 tool call waits for approval".to_string()
            } else {
                format!(
                    "{} tool calls wait for approval",
                    number(home.pending_approvals)
                )
            },
        ));
    }
    let expiring = home.token_count(TokenState::Expiring) as i64;
    if expiring > 0 {
        items.push((
            "/tokens".to_string(),
            "key-round",
            if expiring == 1 {
                "1 token expires this week".to_string()
            } else {
                format!("{} tokens expire this week", number(expiring))
            },
        ));
    }
    if let Some(stats) = home.stats.filter(|stats| stats.metrics.errors > 0) {
        items.push((
            link(
                "/logs",
                &[("range", home.query.range.as_str()), ("outcome", "error")],
            ),
            "circle-alert",
            format!(
                "{} {}",
                counted(stats.metrics.errors, "error", "errors"),
                range_words(home.query.range)
            ),
        ));
    }

    html! {
        @if !items.is_empty() {
            div class="attention" {
                @for (href, icon_name, text) in &items { (attention_item(href, icon_name, text)) }
            }
        }
    }
}

/// A count in the footer of a KPI card: tinted when it says something.
fn count_badge(count: i64, tone: &str, sign: &str) -> Markup {
    html! {
        @if count > 0 {
            span class=(format!("badge badge--{tone} badge--no-dot")) { (sign) (number(count)) }
        } @else {
            span class="badge badge--no-dot" { "0" }
        }
    }
}

fn kpis(home: &Home) -> Markup {
    let enabled = |count: usize| counted(count as i64, "enabled MCP", "enabled MCPs");
    html! {
        div class=(if home.team.is_some() { "grid grid--4 grid--keep-2" } else { "grid grid--3" }) {
            div class="kpi" {
                p class="kpi__label" { "MCPs" }
                p class="kpi__value" { (number(home.mcps.len() as i64)) }
                p class="kpi__footer" {
                    (count_badge(home.recent_mcps() as i64, "success", "+"))
                    span class="hide-mobile" { "in the last 30 days" } span class="hide-desktop" { "in 30 days" }
                }
            }
            div class="kpi" {
                p class="kpi__label" { "Tools exposed" }
                @if let Some(tools) = home.tools {
                    p class="kpi__value" { (number(tools.tools as i64)) }
                    p class="kpi__footer" {
                        span class="hide-mobile" { "across " (enabled(tools.mcps)) } span class="hide-desktop" { (enabled(tools.mcps)) }
                    }
                } @else {
                    // Tool lists are not stored: a count is known once an MCP was asked for its tools.
                    p class="kpi__value" { "—" }
                    p class="kpi__footer" { span { "listed on first use" } }
                }
            }
            div class="kpi" {
                p class="kpi__label" { "Active tokens" }
                p class="kpi__value" { (number(home.active_tokens() as i64)) }
                p class="kpi__footer" {
                    (count_badge(home.tokens_used_recently() as i64, "success", ""))
                    span { "used this week" }
                }
            }
            @if let Some((members, invites)) = home.team {
                div class="kpi" {
                    p class="kpi__label" { "Teammates" }
                    p class="kpi__value" { (number(members)) }
                    p class="kpi__footer" {
                        (count_badge(invites, "info", ""))
                        span { (if invites == 1 { "pending invite" } else { "pending invites" }) }
                    }
                }
            }
        }
    }
}

/// The tool a call was for, written as agents name it: `notion__search`.
fn called_tool(log: &McpCallLog) -> String {
    match (&log.mcp_slug, &log.tool_name) {
        (Some(slug), Some(tool)) => format!("{slug}__{tool}"),
        _ => log.requested_tool_name.clone(),
    }
}

/// The inside of the "Gateway activity" card: the page script asks for it
/// again once, in the viewer's time zone, when the page was not drawn in it.
pub fn activity_content(activity: &Activity, mcps: &[Mcp]) -> Markup {
    // A call is drawn with the icon of its MCP when the MCP has one.
    let call_icon = |log: &McpCallLog| {
        mcps.iter()
            .find(|mcp| Some(mcp.id) == log.mcp_id)
            .map(mcp_icon)
            .unwrap_or("square-terminal")
    };
    let first = activity.days.first().map(|day| day.label.as_str());
    let middle = activity
        .days
        .get(ACTIVITY_DAYS / 2 - 1)
        .map(|day| day.label.as_str());
    let last = activity.days.last().map(|day| day.label.as_str());
    html! {
        header class="card__header" {
            div class="card__heading" {
                h2 class="card__title" id="activity-title" { "Gateway activity" }
                p class="card__subtitle" { "Tool calls · last " (ACTIVITY_DAYS) " days" }
            }
            a class="button hide-mobile" href="/logs" { "Open logs" }
        }
        @if activity.total == 0 {
            div class="empty-state" {
                span class="empty-state__icon" { (icon("activity")) }
                div class="empty-state__text" {
                    p class="empty-state__title" { "No tool calls in the last " (ACTIVITY_DAYS) " days" }
                    p class="empty-state__description" {
                        @if activity.logging_off {
                            "Call logging is off. Enable logging in Settings to capture new calls."
                        } @else {
                            "Make a tool call through the MCP gateway to see it here."
                        }
                    }
                }
                @if activity.logging_off { a class="button button--secondary" href="/settings" { "Open settings" } }
            }
        } @else {
            div class="chart-section" {
                p class="chart-total" {
                    span class="chart-total__value" { (number(activity.total)) }
                    @if let Some(change) = activity.change() {
                        @if change > 0 {
                            span class="badge badge--success badge--no-dot" { "+" (number(change)) "%" }
                        } @else {
                            span class="badge badge--no-dot" { (number(change)) "%" }
                        }
                    }
                    span class="hide-mobile" { "including " (counted(activity.errors, "error", "errors")) }
                }
                (bar_chart(&format!("Tool calls per day, last {ACTIVITY_DAYS} days"), ("call", "calls"), &activity.days))
                div class="chart-axis hide-mobile" aria-hidden="true" {
                    @for label in [first, middle, last].into_iter().flatten() { span { (label) } }
                }
            }
        }
        @for (index, log) in activity.recent.iter().enumerate() {
            // A phone shows three of the five.
            a class=(if index % 2 == 1 { "list-row hide-mobile" } else { "list-row" })
                href=(link("/logs", &[("range", "all"), ("logId", &log.id.to_string())])) {
                span class="tile" { (icon(call_icon(log))) }
                span class="list-row__text" {
                    span class="list-row__title list-row__title--code" { (called_tool(log)) }
                    span class="list-row__subtitle" {
                        (log.mcp_name.as_deref().or(log.mcp_slug.as_deref()).unwrap_or("Unknown MCP")) " · " (log.access_token_name)
                    }
                }
                span class="list-row__meta hide-mobile" { (relative_time(log.created_at)) }
                @match log.outcome {
                    CallOutcome::Success => { span class="badge badge--success" { "Success" } },
                    CallOutcome::Error => { span class="badge badge--critical" { "Error" } },
                }
            }
        }
    }
}

fn status_badge(status: McpStatus) -> Markup {
    let tone = match status {
        McpStatus::Ready => "badge badge--success",
        McpStatus::Error => "badge badge--critical",
        McpStatus::Draft => "badge badge--warning",
    };
    html! { span class=(tone) { (status.as_str()) } }
}

/// What an MCP row says under the name, from what is stored about it.
fn mcp_subtitle(mcp: &Mcp) -> String {
    if mcp.oauth_required {
        return format!("{} · awaiting OAuth", transport(mcp));
    }
    match mcp.status {
        McpStatus::Error => mcp
            .last_error
            .clone()
            .filter(|error| !error.is_empty())
            .unwrap_or_else(|| format!("{} · connection failed", transport(mcp))),
        McpStatus::Draft => format!("{} · not tested yet", transport(mcp)),
        McpStatus::Ready if mcp.enabled => transport(mcp).to_string(),
        McpStatus::Ready => format!("{} · disabled", transport(mcp)),
    }
}

fn mcp_row(mcp: &Mcp) -> Markup {
    html! {
        a class="list-row" href=(format!("/mcps/{}/edit", mcp.id)) {
            span class="tile" { (icon(mcp_icon(mcp))) }
            span class="list-row__text" {
                span class="list-row__title" { (mcp.name) }
                span class="list-row__subtitle" { (mcp_subtitle(mcp)) }
            }
            (status_badge(mcp.status))
        }
    }
}

/// Connection status: the MCPs that need someone first, then the ready
/// ones, summed up in one row when there are more than three.
fn mcps_card(home: &Home) -> Markup {
    let needs_someone = |mcp: &&Mcp| mcp.oauth_required || mcp.status != McpStatus::Ready;
    let mut waiting: Vec<&Mcp> = home.mcps.iter().filter(needs_someone).collect();
    // Authorization first, then errors, then the ones never tested.
    waiting.sort_by_key(|mcp| (!mcp.oauth_required, mcp.status != McpStatus::Error));
    let ready: Vec<&Mcp> = home.mcps.iter().filter(|mcp| !needs_someone(mcp)).collect();
    let hidden = waiting.len().saturating_sub(SIDE_ROWS);
    let disabled = ready.iter().filter(|mcp| !mcp.enabled).count();

    html! {
        section class="card" aria-labelledby="home-mcps-title" {
            header class="card__header" {
                div class="card__heading" {
                    h2 class="card__title" id="home-mcps-title" { "MCPs" }
                    p class="card__subtitle" { "Connection status" }
                }
                a class="button" href="/mcps" { "View all" }
            }
            @for mcp in waiting.iter().take(SIDE_ROWS) { (mcp_row(mcp)) }
            @if hidden > 0 {
                a class="list-row" href="/mcps" {
                    span class="tile" { (icon("plug")) }
                    span class="list-row__text" {
                        span class="list-row__title" { (number(hidden as i64)) " more" }
                        span class="list-row__subtitle" { "Not connected yet" }
                    }
                }
            }
            @if ready.len() > 3 {
                a class="list-row" href="/mcps" {
                    span class="tile" { (icon("plug")) }
                    span class="list-row__text" {
                        span class="list-row__title" {
                            (number(ready.len() as i64)) (if waiting.is_empty() { " MCPs" } else { " other MCPs" })
                        }
                        span class="list-row__subtitle" {
                            (number((ready.len() - disabled) as i64)) " enabled"
                            @if disabled > 0 { " · " (number(disabled as i64)) " disabled" }
                        }
                    }
                    (status_badge(McpStatus::Ready))
                }
            } @else {
                @for mcp in &ready { (mcp_row(mcp)) }
            }
        }
    }
}

fn token_row(home: &Home, token: &AccessToken) -> Markup {
    let oauth = token.source == TokenSource::Oauth;
    let state = token_state(token, home.now);
    html! {
        a class="list-row" href="/tokens" {
            span class="tile" { (icon(if oauth { "bot" } else { "key-round" })) }
            span class="list-row__text" {
                span class="list-row__title" { (token.name) }
                span class="list-row__subtitle" {
                    (if oauth { "OAuth" } else { "Manual" }) " · "
                    @match (state, connection_expires_at(token), token.revoked_at, token.last_used_at) {
                        (TokenState::Revoked, _, Some(revoked_at), _) => { "revoked " (date(revoked_at)) },
                        (TokenState::Expired, Some(expires_at), ..) => { "expired " (date(expires_at)) },
                        (TokenState::Expiring, Some(expires_at), ..) => { "expires " (relative_time(expires_at)) },
                        (.., Some(used_at)) => { "used " (relative_time(used_at)) },
                        _ => { "never used" },
                    }
                }
            }
            @match state {
                TokenState::Active => { span class="badge badge--success" { "Active" } },
                TokenState::Expiring => { span class="badge badge--warning" { "Expiring" } },
                TokenState::Expired => { span class="badge" { "Expired" } },
                TokenState::Revoked => { span class="badge" { "Revoked" } },
            }
        }
    }
}

/// The clients connected to the gateway, the most recently used first.
fn tokens_card(home: &Home) -> Markup {
    html! {
        section class="card" aria-labelledby="home-tokens-title" {
            header class="card__header" {
                div class="card__heading" {
                    h2 class="card__title" id="home-tokens-title" { "Access tokens" }
                    p class="card__subtitle" { "Clients connected to the gateway" }
                }
                a class="button" href="/tokens" { "View all" }
            }
            @if home.tokens.is_empty() {
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("key-round")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No access tokens yet" }
                        p class="empty-state__description" { "Create a token, or connect a client with OAuth." }
                    }
                    a class="button button--secondary" href="/tokens/new" { (icon("plus")) "Create token" }
                }
            } @else {
                @for token in home.tokens.iter().take(SIDE_ROWS) { (token_row(home, token)) }
            }
        }
    }
}

pub fn home_page(context: &PageContext, home: &Home) -> Markup {
    let empty = home.mcps.is_empty();
    let content = html! {
        @if let Some(stats) = &home.stats { (context_bar(home, stats)) }
        @if home.admin {
            section class="hero" {
                // The server does not know the viewer's hour: the page script
                // turns the first words into the greeting of that hour.
                h1 class="hero__greeting" data-greeting="Welcome back" {
                    "Welcome back"
                    @if let Some(name) = &home.first_name { ", " (name) }
                    "."
                    span { (endpoint_sentence(home)) }
                }
                @if empty { (no_mcps_card("card full-width")) } @else { (hero_card(context, home)) }
                (attention(home))
            }
        } @else {
            // The home of a member: no figure of the call log, so no context
            // bar, and the plain page header in place of the greeting.
            header class="page-header" {
                div class="page-header__text" {
                    h1 class="page-header__title" { "Home" }
                    p class="page-header__subtitle" {
                        "Signed in as " (home.display_name) ". Register upstream MCPs and issue access tokens for your agents."
                    }
                }
            }
            @if empty {
                (no_mcps_card("card"))
            } @else {
                section class="hero" { (hero_card(context, home)) (attention(home)) }
            }
        }
        @if !empty {
            (kpis(home))
            @if let Some(activity) = &home.activity {
                // Drawn in the days of the zone the session remembers, or of UTC.
                // The page script names the viewer's zone when it is another one.
                form method="get" action="/" data-async data-async-target="#home-activity" data-timezone-sync hidden {
                    input type="hidden" name="timeZone" value=(activity.time_zone) data-timezone;
                }
                div class="overview" {
                    section class="card" id="home-activity" aria-labelledby="activity-title" { (activity_content(activity, &home.mcps)) }
                    div class="overview__side hide-mobile" { (mcps_card(home)) (tokens_card(home)) }
                }
            } @else {
                div class="grid grid--2" { (mcps_card(home)) (tokens_card(home)) }
            }
        }
    };
    app_page(context, "Home", content, html! {})
}
