//! The built-in iCloud Mail MCP: the port of
//! `app/services/builtin/icloud_mail/`.
//!
//! Apple has no mail API and no OAuth for third parties. iCloud Mail is
//! reached over IMAP and SMTP with an app-specific password the admin
//! creates at <https://account.apple.com>, which Apple only offers with
//! two-factor authentication turned on. Apple cannot limit what that
//! password reaches, so the admin chooses what agents may do with it when
//! adding the MCP. See `docs/icloud-mail.md`.
//!
//! [`definition`] is all the server needs. The rest is public for the tests
//! of this crate, and for the crates that run the tools in their own tests:
//! the [`MailServers`] and [`HtmlConversion`] seams, and, with the
//! `test-util` feature, a mail account served on 127.0.0.1 ([`testing`]).

mod connection;
pub mod html;
mod imap;
pub mod message;
pub mod mime;
mod mime_address;
mod mime_host_tables;
mod mime_types;
mod smtp;
#[cfg(feature = "test-util")]
pub mod testing;
mod tools;
pub mod validators;

use std::sync::Arc;

use mymcps_builtin::{
    BuiltinMcpDefinition, BuiltinPasswordConfig, BuiltinPasswordContext, BuiltinProvider,
};
use mymcps_vine::js;
use serde_json::Value;

pub use connection::{MailServer, MailServers};
pub use html::HtmlConversion;
pub use tools::ICLOUD_MAIL_PERMISSIONS;

use connection::with_imap;
use tools::{Shared, attachment_upload, download_attachment, icloud_mail_tools};

/// The key of the MCP: the `builtin_key` of its rows.
pub const KEY: &str = "icloud-mail";

/// The iCloud Mail MCP. Build it once: its tools share what they remember
/// between calls, such as the IMAP username that worked for an address.
pub fn definition() -> BuiltinMcpDefinition {
    let shared = Arc::new(Shared::new());

    let verifying = shared.clone();
    let downloading = shared.clone();
    let provider = BuiltinProvider::new(
        KEY,
        "iCloud Mail",
        icloud_mail_tools(&shared),
        move |sign_in: Arc<BuiltinPasswordContext>| {
            let shared = verifying.clone();
            async move { with_imap(&shared.imap_usernames, &sign_in, async |_| Ok(())).await }
        },
    )
    .download(
        move |reference: Value, sign_in: Arc<BuiltinPasswordContext>| {
            let shared = downloading.clone();
            async move { download_attachment(&shared, reference, &sign_in).await }
        },
    )
    .upload(
        |reference: Value, sign_in: Arc<BuiltinPasswordContext>| async move {
            attachment_upload(&reference, &sign_in)
        },
    );

    let pattern = |source: &str| {
        js::regex(source, "")
            .expect("static regex")
            .as_regex()
            .clone()
    };
    BuiltinMcpDefinition::Password {
        provider,
        password: BuiltinPasswordConfig {
            username_pattern: pattern(r"^[^\s@]+@[^\s@]+\.[^\s@]+$"),
            username_hint: "Enter your iCloud Mail address, such as name@icloud.com",
            // Apple shows them as four groups of four lowercase letters, which an
            // Apple Account password cannot match by accident.
            password_pattern: pattern(r"^[a-z]{4}(-?[a-z]{4}){3}$"),
            password_hint: "Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop. Your Apple Account password does not work here.",
            permissions: ICLOUD_MAIL_PERMISSIONS.to_vec(),
            alias_hint: "Enter up to 20 other addresses of this iCloud account, such as alias@icloud.com, separated by commas",
        },
    }
}
