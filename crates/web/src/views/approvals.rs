//! The Approvals page and the page of one approval request.
//! (`inertia/pages/approvals/index.tsx`, `show.tsx`,
//! `inertia/components/approval_state.tsx`)

use maud::{Markup, html};
use mymcps_core::Timestamp;
use mymcps_core::models::{AccessToken, ApprovalRequest, ApprovalState, Mcp, User};
use mymcps_gateway::approvals::SavedApprovalSummary;

use crate::mcp_templates::icon_of;
use crate::views::icon::icon;
use crate::views::shell::{PageContext, app_page};
use crate::views::{relative_time, time};

/// An approval request as the pages list it.
/// (`app/transformers/approval_request_transformer.ts`)
#[derive(Debug, Clone)]
pub struct ApprovalView {
    /// The id its link names.
    pub id: String,
    pub state: ApprovalState,
    pub tool_name: String,
    /// What MyMCPs read in the call. `None` when it can no longer be decrypted.
    pub title: Option<String>,
    pub mcp_name: String,
    pub mcp_slug: String,
    pub mcp_icon: &'static str,
    pub token_name: String,
    pub token_prefix: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub decided_at: Option<Timestamp>,
    /// The person who decided. `None` while nobody has, and once their
    /// account is gone.
    pub decided_by: Option<String>,
    pub consumed_at: Option<Timestamp>,
}

impl ApprovalView {
    pub fn of(
        request: &ApprovalRequest,
        mcp: &Mcp,
        access_token: &AccessToken,
        decider: Option<&User>,
        summary: Option<&SavedApprovalSummary>,
    ) -> Self {
        Self {
            id: request.public_id.clone(),
            state: request.state(),
            tool_name: request.tool_name.clone(),
            title: summary.map(|summary| summary.title.clone()),
            mcp_name: mcp.name.clone(),
            mcp_slug: mcp.slug.clone(),
            mcp_icon: icon_of(mcp),
            token_name: access_token.name.clone(),
            token_prefix: access_token.token_prefix.clone(),
            created_at: request.created_at,
            expires_at: request.expires_at,
            decided_at: request.decided_at,
            decided_by: decider.map(|decider| {
                decider
                    .full_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| decider.email.clone())
            }),
            consumed_at: request.consumed_at,
        }
    }

    /// The sentence the call goes by, which names its tool when nothing
    /// more can be read of it.
    pub fn title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("Run the tool \"{}\"", self.tool_name))
    }

    fn origin(&self) -> String {
        format!(
            "{} · {} · token {}",
            self.tool_name, self.mcp_name, self.token_name
        )
    }

    fn path(&self) -> String {
        format!("/approvals/{}", self.id)
    }

    fn is_pending(&self) -> bool {
        self.state == ApprovalState::Pending
    }
}

/// Where a request stands, as a badge.
fn state_badge(state: ApprovalState) -> Markup {
    match state {
        ApprovalState::Pending => html! { span class="badge badge--warning" { "Waiting" } },
        ApprovalState::Approved => html! { span class="badge badge--success" { "Approved" } },
        ApprovalState::Used => html! { span class="badge badge--success" { "Approved and run" } },
        ApprovalState::Denied => html! { span class="badge badge--critical" { "Denied" } },
        ApprovalState::Expired => html! { span class="badge" { "Expired" } },
    }
}

/// `2 requests`, under the title of a list.
fn counted(requests: &[ApprovalView]) -> String {
    match requests.len() {
        1 => "1 request".to_string(),
        count => format!("{count} requests"),
    }
}

/// Which of the two lists of the Approvals page a card holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    Waiting,
    Past,
}

impl Listing {
    fn title(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting for a decision",
            Self::Past => "Decided or expired",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Waiting => "Review",
            Self::Past => "Open",
        }
    }
}

fn listing_header(listing: Listing, requests: &[ApprovalView]) -> Markup {
    html! {
        header class="card__header" {
            div class="card__heading" {
                h2 class="card__title" { (listing.title()) }
                @if !requests.is_empty() { p class="card__subtitle" { (counted(requests)) } }
            }
        }
    }
}

/// Until when a past request counted: an approval the agent can still use,
/// or the moment a request lapsed. Nothing for a call that was denied or run.
fn expiry_cell(approval: &ApprovalView) -> Markup {
    match approval.state {
        ApprovalState::Denied | ApprovalState::Used => html! { td class="cell-tertiary" { "—" } },
        _ => html! { td class="cell-secondary" { (time(approval.expires_at)) } },
    }
}

/// From 768px: a table.
fn requests_table(listing: Listing, requests: &[ApprovalView]) -> Markup {
    html! {
        section class="card hide-mobile" {
            (listing_header(listing, requests))
            div class="table-scroll" {
                table class="table table--wide" {
                    thead { tr {
                        th { "Call" }
                        th class="col-156" { "Asked" }
                        th class="col-156" { @if listing == Listing::Waiting { "Waits until" } @else { "Expires" } }
                        th class="col-132" { "Status" }
                        th class="col-76 cell-end" { "Actions" }
                    } }
                    tbody {
                        @for approval in requests {
                            @let title = approval.title();
                            tr {
                                td { div class="cell-media" {
                                    span class="tile" { (icon(approval.mcp_icon)) }
                                    div class="cell-stack cell-stack--wrap" {
                                        span class="cell-title" { (title) }
                                        span class="cell-sub" { (approval.origin()) }
                                    }
                                } }
                                td class="cell-secondary" { (time(approval.created_at)) }
                                @if listing == Listing::Waiting {
                                    td class="cell-secondary" { (time(approval.expires_at)) }
                                } @else {
                                    (expiry_cell(approval))
                                }
                                td { (state_badge(approval.state)) }
                                td class="cell-end" { span class="row-actions" {
                                    a class=(if listing == Listing::Waiting { "button button--secondary" } else { "button" })
                                        href=(approval.path()) aria-label=(format!("{}: {title}", listing.action())) { (listing.action()) }
                                } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The same requests under 768px: a list card, the whole row opens the request.
fn requests_list(listing: Listing, requests: &[ApprovalView]) -> Markup {
    html! {
        section class="card hide-desktop" {
            (listing_header(listing, requests))
            @for approval in requests {
                a class="list-row list-row--wrap" href=(approval.path()) {
                    span class="tile" { (icon(approval.mcp_icon)) }
                    span class="list-row__text" {
                        span class="list-row__title" { (approval.title()) }
                        span class="list-row__subtitle" { (approval.origin()) }
                        span class="list-row__footer" {
                            (state_badge(approval.state))
                            span class="list-row__meta" { "Asked " (relative_time(approval.created_at)) }
                        }
                    }
                    (icon("chevron-right"))
                }
            }
        }
    }
}

/// What the Approvals page shows.
pub struct ApprovalsPage<'a> {
    /// Newest first.
    pub waiting: &'a [ApprovalView],
    /// Decided or expired, newest first.
    pub past: &'a [ApprovalView],
}

/// `GET /approvals`
pub fn approvals_page(context: &PageContext, page: &ApprovalsPage) -> Markup {
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Approvals" }
                p class="page-header__subtitle" {
                    "Tool calls that agents may not make on their own. Administrators see all of them, and members the ones made with their own access tokens. Choose which tools ask from the Tool approvals of each MCP."
                }
            }
        }

        @if page.waiting.is_empty() {
            section class="card" {
                (listing_header(Listing::Waiting, page.waiting))
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("shield-check")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "Nothing is waiting" }
                        p class="empty-state__description" {
                            "When an agent calls a tool that asks for approval, it gets a link to give you, and the call is listed here."
                        }
                    }
                }
            }
        } @else {
            (requests_table(Listing::Waiting, page.waiting))
            (requests_list(Listing::Waiting, page.waiting))
        }

        @if !page.past.is_empty() {
            (requests_table(Listing::Past, page.past))
            (requests_list(Listing::Past, page.past))
        }
    };
    app_page(context, "Approvals", content, html! {})
}

/// What the page of one request shows.
pub struct ApprovalPage<'a> {
    pub approval: &'a ApprovalView,
    /// `None` when the summary can no longer be decrypted.
    pub summary: Option<&'a SavedApprovalSummary>,
    /// The exact arguments, as JSON. `None` when they can no longer be decrypted.
    pub arguments: Option<&'a str>,
    /// Whether the call could still run if it were approved.
    pub runnable: bool,
}

fn banner(tone: &str, icon_name: &str, alert: bool, title: &str, description: Markup) -> Markup {
    let class = if tone.is_empty() {
        "banner".to_string()
    } else {
        format!("banner banner--{tone}")
    };
    html! {
        div class=(class) role=(if alert { "alert" } else { "status" }) {
            (icon(icon_name))
            div class="banner__content" {
                p class="banner__title" { (title) }
                (description)
            }
        }
    }
}

/// Who decided, and when: `Olga Owner on <time>`.
fn decision(approval: &ApprovalView) -> Markup {
    html! {
        (approval.decided_by.as_deref().unwrap_or("a member who has since left")) " on "
        @if let Some(decided_at) = approval.decided_at { (time(decided_at)) }
    }
}

/// What became of a request that no longer waits.
fn state_banner(approval: &ApprovalView) -> Markup {
    match approval.state {
        ApprovalState::Pending => html! {},
        ApprovalState::Approved => banner(
            "success",
            "circle-check",
            false,
            "Approved",
            html! { p {
                "Approved by " (decision(approval))
                ". The agent can run this call once, with these exact arguments, until "
                (time(approval.expires_at)) "."
            } },
        ),
        ApprovalState::Used => banner(
            "success",
            "circle-check",
            false,
            "Approved and run",
            html! { p {
                "Approved by " (decision(approval)) ". The agent ran the call on "
                @if let Some(consumed_at) = approval.consumed_at { (time(consumed_at)) } @else { "an unknown date" }
                "."
            } },
        ),
        ApprovalState::Denied => banner(
            "critical",
            "circle-x",
            false,
            "Denied",
            html! { p { "Denied by " (decision(approval)) ". The call was not run." } },
        ),
        ApprovalState::Expired => banner(
            "warning",
            "triangle-alert",
            false,
            "Expired",
            html! { p {
                @if approval.decided_at.is_some() {
                    "Approved by " (decision(approval))
                    ", but the agent did not run the call in time. It was not run."
                } @else {
                    "Nobody decided in time. The call was not run, and the agent has to ask again."
                }
            } },
        ),
    }
}

/// The banners above the card, in the order a person should read them: the
/// decision, what stops the call from running, what the summary warns
/// about, then how much MyMCPs knows of the call.
fn banners(page: &ApprovalPage) -> Markup {
    let approval = page.approval;
    html! {
        (state_banner(approval))
        @if approval.is_pending() && !page.runnable {
            (banner("warning", "triangle-alert", false, "This call can no longer run", html! {
                p { "Its MCP is disabled, or its access token was revoked or has expired. Approving it changes nothing." }
            }))
        }
        @match page.summary {
            Some(summary) => {
                @for warning in summary.warnings.iter().flatten() {
                    (banner("warning", "triangle-alert", false, warning, html! {}))
                }
                @if !summary.interpreted {
                    (banner("", "info", false, "MyMCPs does not know what this tool does", html! {
                        p {
                            "It lists the arguments exactly as the agent sent them. " (approval.mcp_name)
                            @match &summary.tool_description {
                                Some(description) => { " describes the tool as: " (description) }
                                None => { " does not describe the tool." }
                            }
                        }
                    }))
                }
            }
            None => {
                (banner("critical", "circle-x", true, "This request can no longer be read", html! {
                    p { "It was encrypted with another APP_KEY. Deny it and have the agent ask again." }
                }))
            }
        }
    }
}

/// What MyMCPs read in the call. Every value is shown as it is.
fn details(summary: &SavedApprovalSummary) -> Markup {
    html! {
        div class="stack gap-300" {
            div class="heading" {
                h2 class="heading__title" { @if summary.interpreted { "What it changes" } @else { "Arguments" } }
            }
            @if summary.details.is_empty() {
                p class="text-secondary" { "The call has no arguments." }
            } @else {
                dl class="stack" {
                    // Two rows may carry the same label.
                    @for detail in &summary.details {
                        div class="key-value" {
                            dt { (detail.label) }
                            dd class="preserve-lines" { (detail.value) }
                            @if let Some(before) = &detail.before {
                                dd class="key-value__before preserve-lines" { "Now: " (before) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `GET /approvals/{id}`
pub fn approval_page(context: &PageContext, page: &ApprovalPage) -> Markup {
    let approval = page.approval;
    let title = approval.title();
    let is_pending = approval.is_pending();
    let banners = banners(page);
    let content = html! {
        div class="page page--narrow" {
            div class="stack gap-300" {
                nav class="breadcrumb breadcrumb--truncate" aria-label="Breadcrumb" {
                    a href="/approvals" { "Approvals" }
                    (icon("chevron-right"))
                    span aria-current="page" { (title) }
                }
                p class="cluster gap-300" {
                    (state_badge(approval.state))
                    span class="text-body-sm text-secondary" { "Asked on " (time(approval.created_at)) }
                }
                header class="page-header" {
                    div class="page-header__text" {
                        h1 class="page-header__title break-anywhere" { (title) }
                        p class="page-header__subtitle" {
                            "An agent using the access token “" (approval.token_name) "” "
                            @if is_pending {
                                "wants to do this on " (approval.mcp_name) ". Nothing has been done yet."
                            } @else {
                                "asked to do this on " (approval.mcp_name) "."
                            }
                            " MyMCPs wrote this page from the call itself: the agent cannot change what it says."
                        }
                    }
                }
            }

            @if !banners.0.is_empty() { div class="stack gap-300" { (banners) } }

            section class="card" data-fragment[is_pending] {
                div class="card__body gap-600" {
                    @if let Some(summary) = page.summary { (details(summary)) }

                    div class="stack gap-300" {
                        div class="heading" { h2 class="heading__title" { "Request" } }
                        dl class="grid grid--2" {
                            div class="key-value" { dt { "MCP" } dd { (approval.mcp_name) " (" (approval.mcp_slug) ")" } }
                            div class="key-value" { dt { "Tool" } dd { (approval.tool_name) } }
                            div class="key-value" { dt { "Access token" } dd { (approval.token_name) " (" (approval.token_prefix) "…)" } }
                            @if is_pending {
                                div class="key-value" { dt { "Waits until" } dd { (time(approval.expires_at)) } }
                            }
                        }
                    }

                    @if let Some(arguments) = page.arguments {
                        div class="code-block" {
                            div class="code-block__header" {
                                span class="code-block__title" { "Exact arguments sent by the agent" }
                                button type="button" class="icon-button" data-copy-target="#approval-arguments" aria-label="Copy the arguments" {
                                    (icon("copy")) (icon("check"))
                                }
                            }
                            pre class="code-block__body" id="approval-arguments" { (arguments) }
                        }
                    }

                    @if is_pending {
                        p class="text-body-sm text-secondary" {
                            "Approving lets the agent run this call once, with these exact arguments. It then has to call the tool again: tell it once you have decided."
                        }
                    }
                }

                // One form, two submit buttons: the one pressed says what was decided.
                @if is_pending {
                    form class="card__footer" method="post" action=(approval.path()) data-async {
                        (context.csrf_field())
                        button type="submit" class="button button--secondary" name="decision" value="deny" { "Deny" }
                        button type="submit" class="button button--primary" name="decision" value="approve" { "Approve" }
                    }
                }
            }
        }
    };
    app_page(context, "Approval request", content, html! {})
}
