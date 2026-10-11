//! Server-rendered HTML. Each feature has a module here; they share the
//! page shell and the icons. The markup follows `docs/design-system.md`.

pub mod analytics;
pub mod approvals;
pub mod auth;
pub mod charts;
pub mod home;
pub mod icon;
pub mod invites;
pub mod logs;
pub mod mcp_tools;
pub mod mcps;
pub mod oauth;
pub mod settings;
pub mod shell;
pub mod tokens;

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
