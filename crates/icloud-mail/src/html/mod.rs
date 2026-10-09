//! The text of an HTML message: the port of
//! `app/services/builtin/icloud_mail/html.ts`.
//!
//! The Node app converted with the `html-to-text` package, which reads a
//! whole document in one go, and which markup written for the purpose keeps
//! busy for many seconds or makes allocate hundreds of megabytes. It
//! therefore ran each conversion in a process of its own, killed after five
//! seconds or 128 MB.
//!
//! [`convert::convert`] writes the same text, in time and memory that grow
//! with the size of the message and nothing else, so no process is needed to
//! contain it. The same bounds are kept all the same: a conversion gives up
//! once it has taken five seconds or holds 128 MB, runs away from the threads
//! that serve requests, and conversions still run one at a time. The few
//! documents `html-to-text` itself throws on have no text here either.

pub mod convert;

use std::time::{Duration, Instant};

use convert::Budget;

use crate::message::utf16_prefix;

/// A newsletter converts in a few milliseconds.
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BYTES: usize = 128 * 1024 * 1024;

/// How long a conversion may take. The tools read this from their context
/// with `context.env.extension::<HtmlConversion>()`, and allow five seconds
/// when it is absent. Tests shorten it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtmlConversion {
    pub timeout: Duration,
}

impl Default for HtmlConversion {
    fn default() -> Self {
        Self { timeout: TIMEOUT }
    }
}

/// The text of an HTML message, cut to the number of characters asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub text: String,
    /// The text was longer.
    pub is_truncated: bool,
}

/// Converts HTML messages, one at a time, in the order they are asked for.
#[derive(Debug, Default)]
pub struct HtmlConverter {
    queue: tokio::sync::Mutex<()>,
}

impl HtmlConverter {
    /// The text of an HTML message, or `None` when it could not be converted
    /// within `conversion.timeout`.
    ///
    /// The text is cut to `max_chars` characters, counted as JavaScript
    /// counts them, with `is_truncated` set when it was longer.
    pub async fn html_to_text(
        &self,
        html: String,
        max_chars: usize,
        conversion: HtmlConversion,
    ) -> Option<Converted> {
        // Tokio serves the waiters of a mutex in the order they came.
        let _turn = self.queue.lock().await;
        let budget = Budget {
            deadline: Some(Instant::now() + conversion.timeout),
            max_bytes: MAX_BYTES,
        };
        // The conversion checks its budget as it goes: it ends on its own, and frees its thread.
        // Whatever makes it give up, the message has no text: the Node app made no difference either.
        let text = tokio::task::spawn_blocking(move || convert::convert(&html, &budget))
            .await
            .ok()?
            .ok()?;
        let kept = utf16_prefix(&text, max_chars);
        Some(Converted {
            is_truncated: kept.len() < text.len(),
            text: kept.to_owned(),
        })
    }
}
