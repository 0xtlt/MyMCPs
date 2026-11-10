//! Server-rendered HTML. Each feature has a module here; they share the
//! page shell and the icons. The markup follows `docs/design-system.md`.

pub mod analytics;
pub mod approvals;
pub mod auth;
pub mod charts;
pub mod credentials;
pub mod home;
pub mod icon;
pub mod invites;
pub mod logs;
pub mod mcp_tools;
pub mod mcps;
pub mod oauth;
pub mod settings;
pub mod shell;
pub mod tailnets;
pub mod teams;
pub mod tokens;
pub mod two_factor;

use maud::{Markup, html};
use mymcps_core::Timestamp;

pub use icon::{icon, icon_with};
pub use shell::{PageContext, app_page, auth_page};

/// A date and time, rewritten by the page script in the viewer's time zone.
/// The text is the UTC time, for a page without script.
pub fn time(timestamp: Timestamp) -> Markup {
    let utc = timestamp.as_datetime();
    html! {
        time datetime=(timestamp.to_iso()) { (utc.format("%d/%m/%Y, %H:%M:%S")) }
    }
}

/// A date alone.
pub fn date(timestamp: Timestamp) -> Markup {
    html! {
        time datetime=(timestamp.to_iso()) data-format="date" { (timestamp.as_datetime().format("%d/%m/%Y")) }
    }
}

/// How long ago, such as `2 min ago`. The script keeps it current.
pub fn relative_time(timestamp: Timestamp) -> Markup {
    html! {
        time datetime=(timestamp.to_iso()) data-format="relative" { (timestamp.as_datetime().format("%d/%m/%Y, %H:%M")) }
    }
}

/// A date and time to the minute, as tables show them. The script rewrites
/// it in the viewer's time zone; the text is the UTC time.
pub fn time_minute(timestamp: Timestamp) -> Markup {
    html! {
        time datetime=(timestamp.to_iso()) data-format="minute" { (timestamp.as_datetime().format("%d/%m/%Y, %H:%M")) }
    }
}

/// A stored value as a label reads: `ready` -> `Ready`. Badges, statuses and
/// filter options all go through it, so every label of the interface starts
/// with a capital and the rest stays as written.
pub fn label(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars)
            .collect::<String>()
            .replace('_', " "),
        None => String::new(),
    }
}

/// A sentence whose `backticked` words are identifiers: they render as code
/// chips, as identifiers do everywhere else.
pub fn inline_code(text: &str) -> Markup {
    html! {
        @for (index, part) in text.split('`').enumerate() {
            @if index % 2 == 1 { code class="code" { (part) } } @else { (part) }
        }
    }
}

/// What an MCP's status reads as, in its badge and in the filter of the list.
pub fn mcp_status_label(status: mymcps_core::models::McpStatus) -> &'static str {
    use mymcps_core::models::McpStatus;
    match status {
        McpStatus::Ready => "Ready",
        McpStatus::Error => "Error",
        McpStatus::Draft => "Draft",
    }
}

/// The badge of an MCP's status, the same on every page.
pub fn mcp_status_badge(status: mymcps_core::models::McpStatus) -> Markup {
    use mymcps_core::models::McpStatus;
    let tone = match status {
        McpStatus::Ready => "badge badge--success",
        McpStatus::Error => "badge badge--critical",
        McpStatus::Draft => "badge badge--warning",
    };
    html! { span class=(tone) { (mcp_status_label(status)) } }
}

/// The badge of a token, the same on Home and on Access tokens: amber while
/// it still works but ends within a week, neutral once nothing can use it.
pub fn token_state_badge(state: crate::routes::home::TokenState) -> Markup {
    use crate::routes::home::TokenState;
    match state {
        TokenState::Active => html! { span class="badge badge--success" { "Active" } },
        TokenState::Expiring => html! { span class="badge badge--warning" { "Expiring" } },
        TokenState::Expired => html! { span class="badge" { "Expired" } },
        TokenState::Revoked => html! { span class="badge" { "Revoked" } },
    }
}

/// A count as people read it: `1,284`.
pub fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        for (count, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1,000"),
            (1_234_567, "1,234,567"),
        ] {
            assert_eq!(grouped(count), text);
        }
    }
}
