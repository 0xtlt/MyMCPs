//! Signing in to iCloud Mail, over IMAP and over SMTP: the port of
//! `app/services/builtin/icloud_mail/connection.ts`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use mymcps_builtin::{BuiltinError, BuiltinPasswordContext, BuiltinResult};

use crate::imap::{ImapClient, ImapError, ImapServer, Timeouts};
use crate::message::utf16_prefix;
use crate::smtp::{self, SmtpCode, SmtpEnvelope, SmtpError, SmtpServer, SmtpTimeouts};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(60);

const SIGN_IN_REJECTED: &str = "iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.";

/// One of the two servers of a mail account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailServer {
    pub host: String,
    pub port: u16,
    /// Whether the connection is encrypted: from its first byte for IMAP,
    /// with STARTTLS before signing in for SMTP. Only tests do without.
    pub tls: bool,
}

/// The only two hosts the saved password is ever sent to.
///
/// The tools read this from their context with
/// `context.env.extension::<MailServers>()` and reach Apple's servers when it
/// is absent, which is what the server does. Tests attach their own with
/// `BuiltinEnv::with_extension`, pointing at servers on 127.0.0.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailServers {
    pub imap: MailServer,
    pub smtp: MailServer,
    /// How long a connection may take to open, then the server to greet.
    pub connect_timeout: Duration,
    /// How long a server may stay silent before the connection is given up.
    pub socket_timeout: Duration,
}

impl MailServers {
    /// Apple's servers, as <https://support.apple.com/102525> gives them.
    pub fn icloud() -> Self {
        Self {
            imap: MailServer {
                host: "imap.mail.me.com".to_owned(),
                port: 993,
                tls: true,
            },
            smtp: MailServer {
                host: "smtp.mail.me.com".to_owned(),
                port: 587,
                tls: true,
            },
            connect_timeout: CONNECT_TIMEOUT,
            socket_timeout: SOCKET_TIMEOUT,
        }
    }

    /// Servers on this machine that speak without encryption, for tests.
    pub fn local(imap_port: u16, smtp_port: u16) -> Self {
        Self {
            imap: MailServer {
                host: "127.0.0.1".to_owned(),
                port: imap_port,
                tls: false,
            },
            smtp: MailServer {
                host: "127.0.0.1".to_owned(),
                port: smtp_port,
                tls: false,
            },
            ..Self::icloud()
        }
    }

    fn of(sign_in: &BuiltinPasswordContext) -> Self {
        sign_in
            .env
            .extension::<Self>()
            .map_or_else(Self::icloud, |servers| (*servers).clone())
    }
}

/// The IMAP username that worked for each address, so the other form is not tried again.
#[derive(Debug, Default)]
pub(crate) struct ImapUsernames(Mutex<HashMap<String, String>>);

impl ImapUsernames {
    /// Apple documents the name before the @ as the usual IMAP username and the
    /// full address as the one to try when it fails, without saying which accounts
    /// take which. Try the address as entered, then the name alone.
    fn candidates(&self, address: &str) -> Vec<String> {
        let remembered = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(address)
            .cloned();
        match remembered {
            Some(username) => vec![username],
            None => vec![
                address.to_owned(),
                address.split('@').next().unwrap_or(address).to_owned(),
            ],
        }
    }

    fn remember(&self, address: &str, username: &str) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(address.to_owned(), username.to_owned());
    }
}

/// What can go wrong while a tool talks to iCloud: something the tool itself
/// refuses, or a failure of the IMAP exchange, which [`with_imap`] explains.
#[derive(Debug)]
pub(crate) enum MailError {
    Tool(BuiltinError),
    Imap(ImapError),
}

impl From<BuiltinError> for MailError {
    fn from(error: BuiltinError) -> Self {
        Self::Tool(error)
    }
}

impl From<ImapError> for MailError {
    fn from(error: ImapError) -> Self {
        Self::Imap(error)
    }
}

pub(crate) type MailResult<T> = Result<T, MailError>;

/// `text.slice(0, 200)`
fn first_200(text: &str) -> &str {
    utf16_prefix(text, 200)
}

fn imap_failure(error: MailError) -> BuiltinError {
    match error {
        MailError::Tool(error) => error,
        MailError::Imap(ImapError::AuthenticationFailed(_)) => {
            BuiltinError::authorization(SIGN_IN_REJECTED)
        }
        MailError::Imap(ImapError::Throttled(_)) => BuiltinError::tool(
            "iCloud Mail is limiting requests for this account. Try again in a few minutes.",
        ),
        // Socket and connection failures that happen before iCloud answers a command.
        MailError::Imap(ImapError::Unreachable(_)) => {
            BuiltinError::tool("Could not reach iCloud Mail. Try again.")
        }
        MailError::Imap(ImapError::Refused { text, .. }) if !text.is_empty() => BuiltinError::tool(
            format!("iCloud Mail refused the request: {}", first_200(&text)),
        ),
        MailError::Imap(error) => BuiltinError::internal(error),
    }
}

async fn sign_in_to_imap(
    usernames: &ImapUsernames,
    sign_in: &BuiltinPasswordContext,
) -> Result<ImapClient, ImapError> {
    let servers = MailServers::of(sign_in);
    let server = ImapServer {
        host: servers.imap.host,
        port: servers.imap.port,
        tls: servers.imap.tls,
    };
    let timeouts = Timeouts {
        connect: servers.connect_timeout,
        socket: servers.socket_timeout,
    };

    let candidates = usernames.candidates(&sign_in.username);
    let last = candidates.len() - 1;
    let mut refusal = ImapError::AuthenticationFailed(String::new());
    for (index, username) in candidates.iter().enumerate() {
        match ImapClient::connect(&server, username, &sign_in.password, timeouts).await {
            Ok(client) => {
                usernames.remember(&sign_in.username, username);
                return Ok(client);
            }
            Err(error @ ImapError::AuthenticationFailed(_)) if index < last => refusal = error,
            Err(error) => return Err(error),
        }
    }
    Err(refusal)
}

/// Sign in to iCloud Mail over IMAP for the duration of `use_client`.
pub(crate) async fn with_imap<T>(
    usernames: &ImapUsernames,
    sign_in: &BuiltinPasswordContext,
    use_client: impl AsyncFnOnce(&mut ImapClient) -> MailResult<T>,
) -> BuiltinResult<T> {
    let mut client = sign_in_to_imap(usernames, sign_in)
        .await
        .map_err(|error| imap_failure(error.into()))?;
    let result = use_client(&mut client).await;
    client.logout().await;
    result.map_err(imap_failure)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
}

/// Select `path` for what follows. Read-only access never changes a flag.
pub(crate) async fn open_mailbox(
    client: &mut ImapClient,
    path: &str,
    access: Access,
) -> MailResult<()> {
    match client.open_mailbox(path, access == Access::Read).await {
        Err(ImapError::Refused {
            mailbox_missing: true,
            ..
        }) => Err(BuiltinError::tool(format!(
            "Mailbox \"{path}\" does not exist. Call list_mailboxes for the exact paths."
        ))
        .into()),
        other => Ok(other?),
    }
}

/// Explain a failed SMTP delivery. Once the connection is open, a socket error
/// does not say whether iCloud had already accepted the message.
fn smtp_failure(error: SmtpError) -> BuiltinError {
    let reply = error
        .response
        .as_deref()
        .map(|response| format!(": {}", first_200(response)))
        .unwrap_or_default();
    match error.code {
        SmtpCode::Auth => BuiltinError::authorization(SIGN_IN_REJECTED),
        SmtpCode::Envelope => {
            BuiltinError::tool(format!("iCloud Mail rejected the recipients{reply}"))
        }
        SmtpCode::Message => BuiltinError::tool(format!("iCloud Mail refused the message{reply}")),
        SmtpCode::Dns => {
            BuiltinError::tool("Could not reach iCloud Mail. Nothing was sent. Try again.")
        }
        SmtpCode::Timeout | SmtpCode::Connection | SmtpCode::Socket | SmtpCode::Tls => {
            BuiltinError::tool(
                "iCloud Mail did not confirm the message, so it may or may not have been sent. Check with the user before sending it again.",
            )
        }
        SmtpCode::Protocol => BuiltinError::internal(error),
    }
}

/// Deliver one message through iCloud's SMTP server. Returns the recipients it refused.
pub(crate) async fn send_through_smtp(
    sign_in: &BuiltinPasswordContext,
    from: &str,
    to: &[String],
    message: &[u8],
) -> BuiltinResult<Vec<String>> {
    let servers = MailServers::of(sign_in);
    let server = SmtpServer {
        host: servers.smtp.host,
        port: servers.smtp.port,
        requires_tls: servers.smtp.tls,
    };
    let timeouts = SmtpTimeouts {
        connect: servers.connect_timeout,
        socket: servers.socket_timeout,
    };
    smtp::send(
        &server,
        &sign_in.username,
        &sign_in.password,
        &SmtpEnvelope { from, to },
        message,
        timeouts,
    )
    .await
    .map_err(smtp_failure)
}
