//! Delivers one message over SMTP.
//!
//! The Node app sent through nodemailer 10.0.14. What a tool tells the agent
//! depends on where a delivery failed and on which recipients the server
//! refused, so this is a port of the exchange nodemailer has with a server
//! (`smtp-connection/index.js` and `smtp-transport/index.js`), which fails
//! with the same codes at the same steps. A general SMTP crate such as
//! `lettre` gives up at the first recipient the server refuses, where
//! nodemailer goes on and reports the ones that were refused.
//!
//! Left out: connection pools, proxies, LMTP, DSN, OAuth, and CRAM-MD5,
//! which iCloud does not offer.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::imap::client::{IoStream, tls_config};
use crate::mime::smtp_data;

/// Where the SMTP server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SmtpServer {
    pub host: String,
    pub port: u16,
    /// Encrypt the connection with STARTTLS before signing in, and give up
    /// when the server cannot. Only tests do without.
    pub requires_tls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SmtpTimeouts {
    /// For the connection to open, then for the server to greet.
    pub connect: Duration,
    /// How long the server may stay silent.
    pub socket: Duration,
}

/// The step a delivery failed at, under the name nodemailer gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmtpCode {
    /// `EAUTH`: the server refused the sign-in.
    Auth,
    /// `EENVELOPE`: the server refused the sender, every recipient, or to take a message.
    Envelope,
    /// `EMESSAGE`: the server refused the message once it had it.
    Message,
    /// `EDNS`: the name of the server does not resolve.
    Dns,
    /// `ETIMEDOUT`
    Timeout,
    /// `ECONNECTION`: the server closed the connection.
    Connection,
    /// `ESOCKET`
    Socket,
    /// `ETLS`: the connection could not be encrypted.
    Tls,
    /// `EPROTOCOL`: the server answered what SMTP does not allow.
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub(crate) struct SmtpError {
    pub code: SmtpCode,
    pub message: String,
    /// What the server answered, when the failure is an answer of the server.
    pub response: Option<String>,
}

fn failure(code: SmtpCode, message: &str, response: Option<&str>) -> SmtpError {
    let message = match response {
        Some(response) => format!("{message}: {response}"),
        None => message.to_owned(),
    };
    SmtpError {
        code,
        message,
        response: response.map(str::to_owned),
    }
}

/// The addresses a message is delivered for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SmtpEnvelope<'a> {
    pub from: &'a str,
    pub to: &'a [String],
}

/// A reply longer than this is not one.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The message is written in pieces, so that a server that stops reading is noticed.
const WRITE_CHUNK_BYTES: usize = 64 * 1024;

struct Connection {
    stream: BufReader<Box<dyn IoStream>>,
    socket_timeout: Duration,
}

impl Connection {
    /// One reply of the server, which may take several lines: `250-a`, `250 b`.
    async fn reply(&mut self) -> Result<String, SmtpError> {
        let mut reply = String::new();
        loop {
            let mut line = Vec::new();
            let read = timeout(
                self.socket_timeout,
                (&mut self.stream)
                    .take(MAX_RESPONSE_BYTES as u64)
                    .read_until(b'\n', &mut line),
            )
            .await;
            match read {
                Err(_) => return Err(failure(SmtpCode::Timeout, "Timeout", None)),
                Ok(Err(error)) => return Err(failure(SmtpCode::Socket, &error.to_string(), None)),
                Ok(Ok(_)) if !line.ends_with(b"\n") => {
                    if line.len() >= MAX_RESPONSE_BYTES {
                        return Err(failure(
                            SmtpCode::Protocol,
                            "Server response exceeds maximum allowed size",
                            None,
                        ));
                    }
                    // What the server said before it hung up counts when it is a refusal.
                    let last_words = decode_reply(&line);
                    let last_words = last_words.trim();
                    let is_refusal = last_words.len() > 3
                        && matches!(last_words.as_bytes()[0], b'4' | b'5')
                        && last_words.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
                        && matches!(last_words.as_bytes()[3], b' ' | b'-');
                    return Err(failure(
                        SmtpCode::Connection,
                        "Connection closed unexpectedly",
                        is_refusal.then_some(last_words),
                    ));
                }
                Ok(Ok(_)) => {}
            }
            line.pop();
            if line.ends_with(b"\r") {
                line.pop();
            }
            let line = decode_reply(&line);
            if line.trim().is_empty() && reply.is_empty() {
                continue;
            }
            // `/^\d+-/`: more lines follow.
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            let is_partial = digits > 0 && line.as_bytes().get(digits) == Some(&b'-');
            if !reply.is_empty() {
                reply.push('\n');
            }
            reply.push_str(&line);
            if reply.len() > MAX_RESPONSE_BYTES {
                return Err(failure(
                    SmtpCode::Protocol,
                    "Server response exceeds maximum allowed size",
                    None,
                ));
            }
            if !is_partial {
                return Ok(reply);
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), SmtpError> {
        for chunk in data.chunks(WRITE_CHUNK_BYTES) {
            let writing = async {
                self.stream.get_mut().write_all(chunk).await?;
                self.stream.get_mut().flush().await
            };
            match timeout(self.socket_timeout, writing).await {
                Err(_) => return Err(failure(SmtpCode::Timeout, "Timeout", None)),
                Ok(Err(error)) => return Err(failure(SmtpCode::Socket, &error.to_string(), None)),
                Ok(Ok(())) => {}
            }
        }
        Ok(())
    }

    async fn command(&mut self, command: &str) -> Result<String, SmtpError> {
        self.write(format!("{command}\r\n").as_bytes()).await?;
        self.reply().await
    }
}

/// `decodeServerResponse`: a reply in UTF-8 is read as such, and byte for byte otherwise.
fn decode_reply(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

fn is_positive(reply: &str) -> bool {
    reply.starts_with('2')
}

/// What the server said it can do, in its answer to EHLO.
#[derive(Debug, Default)]
struct Extensions {
    smtp_utf8: bool,
    allows_auth: bool,
    /// The ways to sign in that are known here, in the order nodemailer prefers them.
    auth: Vec<&'static str>,
}

/// `/[ -]NAME\b/im`
fn advertises(reply: &str, name: &str) -> bool {
    let upper = reply.to_uppercase();
    upper.match_indices(name).any(|(index, _)| {
        let before = upper[..index].chars().next_back();
        let after = upper[index + name.len()..].chars().next();
        matches!(before, Some(' ' | '-'))
            && !after.is_some_and(|after| after.is_ascii_alphanumeric() || after == '_')
    })
}

fn extensions(reply: &str) -> Extensions {
    let mut extensions = Extensions {
        smtp_utf8: advertises(reply, "SMTPUTF8"),
        allows_auth: advertises(reply, "AUTH"),
        auth: Vec::new(),
    };
    let mut mechanisms: Vec<String> = Vec::new();
    // The first line names the server, the others one extension each.
    for line in reply.split('\n').skip(1) {
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        let line = line[digits..]
            .strip_prefix([' ', '-'])
            .unwrap_or(&line[digits..])
            .trim();
        let upper = line.to_uppercase();
        if let Some(listed) = upper
            .strip_prefix("AUTH")
            .filter(|listed| listed.starts_with([' ', '\t', '=']))
        {
            mechanisms.extend(
                listed
                    .split([' ', '\t', '='])
                    .filter(|mechanism| !mechanism.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    for mechanism in ["PLAIN", "LOGIN"] {
        if mechanisms.iter().any(|listed| listed == mechanism) {
            extensions.auth.push(mechanism);
        }
    }
    extensions
}

/// Connect, greet, and encrypt the connection when the server must be
/// reached that way: what nodemailer does before it signs in.
async fn open(
    server: &SmtpServer,
    timeouts: SmtpTimeouts,
) -> Result<(Connection, Extensions), SmtpError> {
    let connecting = async {
        let addresses = tokio::net::lookup_host((server.host.as_str(), server.port))
            .await
            .map_err(|error| failure(SmtpCode::Dns, &error.to_string(), None))?;
        let mut last = failure(SmtpCode::Dns, "The server has no address", None);
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last = failure(SmtpCode::Socket, &error.to_string(), None),
            }
        }
        Err(last)
    };
    let tcp = timeout(timeouts.connect, connecting)
        .await
        .unwrap_or_else(|_| Err(failure(SmtpCode::Timeout, "Connection timeout", None)))?;
    let plain: Box<dyn IoStream> = Box::new(tcp);
    let mut connection = Connection {
        stream: BufReader::new(plain),
        socket_timeout: timeouts.socket,
    };

    let greeting = timeout(timeouts.connect, connection.reply())
        .await
        .unwrap_or_else(|_| Err(failure(SmtpCode::Timeout, "Greeting never received", None)))?;
    if !greeting.starts_with("220") {
        return Err(failure(
            SmtpCode::Protocol,
            &format!("Invalid greeting. response={greeting}"),
            Some(&greeting),
        ));
    }

    // A server in a container has no name of its own to give, and nodemailer says this then.
    const NAME: &str = "[127.0.0.1]";
    let hello = |reply: String| {
        if reply.starts_with("421") {
            return Err(failure(
                SmtpCode::Connection,
                &format!("Server terminates connection. response={reply}"),
                Some(&reply),
            ));
        }
        Ok(reply)
    };
    let mut reply = hello(connection.command(&format!("EHLO {NAME}")).await?)?;
    let mut extensions = if is_positive(&reply) {
        extensions(&reply)
    } else if server.requires_tls {
        return Err(failure(
            SmtpCode::Connection,
            &format!("EHLO failed but HELO does not support required STARTTLS. response={reply}"),
            Some(&reply),
        ));
    } else {
        reply = connection.command(&format!("HELO {NAME}")).await?;
        if !is_positive(&reply) {
            return Err(failure(
                SmtpCode::Protocol,
                &format!("Invalid HELO. response={reply}"),
                Some(&reply),
            ));
        }
        Extensions {
            allows_auth: true,
            ..Extensions::default()
        }
    };

    if server.requires_tls {
        // Asked for whether or not the server offers it: without it, nothing more is said.
        reply = connection.command("STARTTLS").await?;
        if !is_positive(&reply) {
            return Err(failure(
                SmtpCode::Tls,
                "Error upgrading connection with STARTTLS",
                Some(&reply),
            ));
        }
        // Whatever was read ahead came before the encryption, and is not to be trusted.
        let tcp = connection.stream.into_inner();
        let name =
            rustls::pki_types::ServerName::try_from(server.host.clone()).map_err(|error| {
                failure(
                    SmtpCode::Tls,
                    &format!("Error initiating TLS - {error}"),
                    None,
                )
            })?;
        let config = tls_config().map_err(|error| {
            failure(
                SmtpCode::Tls,
                &format!("Error initiating TLS - {error}"),
                None,
            )
        })?;
        let encrypting = tokio_rustls::TlsConnector::from(config).connect(name, tcp);
        let encrypted = match timeout(timeouts.socket, encrypting).await {
            Err(_) => return Err(failure(SmtpCode::Timeout, "Timeout", None)),
            Ok(Err(error)) => {
                return Err(failure(
                    SmtpCode::Tls,
                    &format!("Error initiating TLS - {error}"),
                    None,
                ));
            }
            Ok(Ok(encrypted)) => encrypted,
        };
        let encrypted: Box<dyn IoStream> = Box::new(encrypted);
        connection.stream = BufReader::new(encrypted);
        reply = hello(connection.command(&format!("EHLO {NAME}")).await?)?;
        if !is_positive(&reply) {
            return Err(failure(
                SmtpCode::Connection,
                &format!(
                    "EHLO failed but HELO does not support required STARTTLS. response={reply}"
                ),
                Some(&reply),
            ));
        }
        extensions = self::extensions(&reply);
    }
    Ok((connection, extensions))
}

/// Deliver `message` and return the recipients the server refused. The
/// message went out when this returns `Ok`, even if only to some of them.
pub(crate) async fn send(
    server: &SmtpServer,
    username: &str,
    password: &str,
    envelope: &SmtpEnvelope<'_>,
    message: &[u8],
    timeouts: SmtpTimeouts,
) -> Result<Vec<String>, SmtpError> {
    let (mut connection, extensions) = open(server, timeouts).await?;

    if extensions.allows_auth {
        let mut reply = match extensions.auth.first().copied().unwrap_or("PLAIN") {
            "LOGIN" => {
                let asked = connection.command("AUTH LOGIN").await?;
                if !asked.starts_with("334 ") && !asked.starts_with("334-") {
                    return Err(failure(
                        SmtpCode::Auth,
                        "Invalid login sequence while waiting for \"334 VXNlcm5hbWU6\"",
                        Some(&asked),
                    ));
                }
                let asked = connection.command(&STANDARD.encode(username)).await?;
                if !asked.starts_with("334 ") && !asked.starts_with("334-") {
                    return Err(failure(
                        SmtpCode::Auth,
                        "Invalid login sequence while waiting for \"334 UGFzc3dvcmQ6\"",
                        Some(&asked),
                    ));
                }
                connection.command(&STANDARD.encode(password)).await?
            }
            _ => {
                connection
                    .command(&format!(
                        "AUTH PLAIN {}",
                        STANDARD.encode(format!("\0{username}\0{password}"))
                    ))
                    .await?
            }
        };
        // A server that asks for more gets an empty answer, once.
        if reply.starts_with("334") {
            reply = connection.command("").await?;
            if reply.starts_with("334") {
                reply = connection.command("").await?;
            }
        }
        if !is_positive(&reply) {
            return Err(failure(SmtpCode::Auth, "Invalid login", Some(&reply)));
        }
    }

    // `_setEnvelope`
    let from = envelope.from.trim();
    if envelope.to.is_empty() {
        return Err(failure(SmtpCode::Envelope, "No recipients defined", None));
    }
    if from.contains(['\r', '\n', '<', '>']) {
        return Err(failure(
            SmtpCode::Envelope,
            &format!("Invalid sender {from:?}"),
            None,
        ));
    }
    let recipients: Vec<&str> = envelope
        .to
        .iter()
        .map(|recipient| recipient.trim())
        .collect();
    if let Some(invalid) = recipients
        .iter()
        .find(|recipient| recipient.is_empty() || recipient.contains(['\r', '\n', '<', '>']))
    {
        return Err(failure(
            SmtpCode::Envelope,
            &format!("Invalid recipient {invalid:?}"),
            None,
        ));
    }
    let needs_utf8 = !from.is_ascii() || recipients.iter().any(|recipient| !recipient.is_ascii());
    let uses_utf8 = needs_utf8 && extensions.smtp_utf8;

    let reply = connection
        .command(&format!(
            "MAIL FROM:<{from}>{}",
            if uses_utf8 { " SMTPUTF8" } else { "" }
        ))
        .await?;
    if !is_positive(&reply) {
        let message = if uses_utf8 && reply.starts_with("550 ") && !from.is_ascii() {
            "Internationalized mailbox name not allowed"
        } else {
            "Mail command failed"
        };
        return Err(failure(SmtpCode::Envelope, message, Some(&reply)));
    }

    let mut rejected = Vec::new();
    let mut refusals: Vec<String> = Vec::new();
    let mut last_reply = String::new();
    for recipient in &recipients {
        last_reply = connection
            .command(&format!("RCPT TO:<{recipient}>"))
            .await?;
        if !is_positive(&last_reply) {
            rejected.push((*recipient).to_owned());
            refusals.push(last_reply.clone());
        }
    }
    if rejected.len() == recipients.len() {
        // A refusal that may not last says more than one that will.
        let deferred = refusals.iter().find(|reply| {
            let code: String = reply.chars().take_while(char::is_ascii_digit).collect();
            code.parse::<u32>().is_ok_and(|code| code > 0 && code < 500)
        });
        let reply = deferred.unwrap_or(&last_reply);
        return Err(failure(
            SmtpCode::Envelope,
            "Can't send mail - all recipients were rejected",
            Some(reply),
        ));
    }

    let reply = connection.command("DATA").await?;
    if !reply.starts_with(['2', '3']) {
        return Err(failure(
            SmtpCode::Envelope,
            "Data command failed",
            Some(&reply),
        ));
    }
    connection.write(&smtp_data(message)).await?;
    let reply = connection.reply().await?;
    if !is_positive(&reply) {
        return Err(failure(SmtpCode::Message, "Message failed", Some(&reply)));
    }
    Ok(rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_server_can_do() {
        let reply = "250-smtp.mail.me.com\n250-PIPELINING\n250-SIZE 28319744\n250-STARTTLS\n250-AUTH LOGIN PLAIN\n250-8BITMIME\n250 SMTPUTF8";
        let extensions = extensions(reply);
        assert!(extensions.smtp_utf8 && extensions.allows_auth);
        assert_eq!(extensions.auth, ["PLAIN", "LOGIN"]);

        let legacy = self::extensions("250-mail.example\n250-AUTH=LOGIN\n250 OK");
        assert_eq!(legacy.auth, ["LOGIN"]);
        let bare = self::extensions("250 mail.example greets you, no AUTHORITY here");
        assert!(!bare.allows_auth && !bare.smtp_utf8 && bare.auth.is_empty());
    }
}
