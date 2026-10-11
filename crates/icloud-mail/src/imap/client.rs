//! One connection to an IMAP server, and the commands the tools need.
//!
//! The Node app talked to iCloud through imapflow 2.2.4. This is a port of
//! what it used of it: how a session starts (`imap-flow.js`), and the
//! commands under `commands/`, each named where it is ported. Commands run
//! one after the other, as they did there.
//!
//! Left out, because nothing a tool returns depends on them: `ID`,
//! `COMPRESS`, `ENABLE` (mailbox names are always written in modified
//! UTF-7), `IDLE`, the subscription state of mailboxes, and the waits
//! imapflow makes when Microsoft 365 throttles a request.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed};
use mymcps_builtin::arguments::to_iso;
use mymcps_vine::js;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::decode::{
    Base64Stream, decode_flowed, decode_quoted_printable, decode_utf7, decode_words, encode_utf7,
    first_header, is_plain_charset, parse_header_value,
};
use super::parse::{
    Fetched, MAX_UINT32_DIGITS, expand_range, parse_fetch, parse_uint, sequence_value,
};
use super::special_use::{Source, special_use};
use super::wire::{Arg, Response, Token, compile, parse_response, string_list};
use super::{ImapError, Status};

/// Where the IMAP server is, and whether the connection is encrypted from its first byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImapServer {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timeouts {
    /// For the connection to open, then for the server to greet.
    pub connect: Duration,
    /// How long the server may stay silent while a command waits for it.
    pub socket: Duration,
}

pub(crate) trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<S: AsyncRead + AsyncWrite + Unpin + Send> IoStream for S {}

/// What a response and its literals may take in memory. imapflow reads up to
/// 2 GiB, which a server could use to exhaust the instance.
const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024;

/// How much of a message part is asked for at a time.
const DOWNLOAD_CHUNK_BYTES: u64 = 64 * 1024;

/// How much of a command is written at a time.
const WRITE_PIECE_BYTES: usize = 64 * 1024;

/// The mailbox commands apply to.
#[derive(Debug, Clone)]
struct SelectedMailbox {
    path: String,
    exists: u64,
    read_only: bool,
    /// The flags that can be set for good. `None` when the server did not say.
    permanent_flags: Option<HashSet<String>>,
}

/// A mailbox as LIST describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ListedMailbox {
    pub path: String,
    pub flags: HashSet<String>,
    /// `\Inbox`, `\Sent`, `\Drafts`, `\Trash`, `\Junk`, `\Archive`, `\All` or `\Flagged`.
    pub special_use: Option<&'static str>,
    pub messages: Option<u64>,
    pub unseen: Option<u64>,
    delimiter: Option<String>,
    parent: Vec<String>,
    name: String,
}

/// One condition of a search. A message must meet all of them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Criterion {
    From(String),
    To(String),
    Subject(String),
    Text(String),
    Since(DateTime<Utc>),
    Before(DateTime<Utc>),
    Seen(bool),
    Flagged(bool),
    /// A set of UIDs, such as `11,14`.
    Uid(String),
    /// A set of message sequence numbers.
    Seq(String),
}

/// A range of a message part to ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartQuery {
    pub key: String,
    /// Where to start, and how many bytes to send at most.
    pub range: Option<(u64, u64)>,
}

/// What to ask of each message of a FETCH.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FetchQuery {
    pub uid: bool,
    pub flags: bool,
    pub body_structure: bool,
    pub envelope: bool,
    pub internal_date: bool,
    pub size: bool,
    /// Header fields to return as they were written.
    pub headers: Option<Vec<String>>,
    pub body_parts: Vec<PartQuery>,
}

/// Where moved messages went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Moved {
    pub destination: String,
    /// The UID each message had, and the one it has in the destination,
    /// when the server reports them.
    pub uid_map: Option<Vec<(u32, u32)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Appended {
    pub destination: String,
    pub uid: Option<u32>,
}

/// The responses to one command.
struct Exchange {
    untagged: Vec<Response>,
    tagged: Response,
}

/// What to answer when the server asks for more in the middle of a command.
enum Continuation {
    None,
    /// The response of `AUTHENTICATE PLAIN`.
    Plain(String),
    /// `AUTHENTICATE LOGIN` asks for the name, then for the password.
    Login {
        username: String,
        password: String,
    },
}

static TLS: LazyLock<Result<Arc<rustls::ClientConfig>, String>> = LazyLock::new(|| {
    use rustls_platform_verifier::BuilderVerifierExt;

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_platform_verifier())
        .map_err(|error| error.to_string())?
        .with_no_client_auth();
    Ok(Arc::new(config))
});

/// The TLS settings of the connections to iCloud: the certificates the
/// system trusts, checked for the name of the server.
pub(crate) fn tls_config() -> Result<Arc<rustls::ClientConfig>, ImapError> {
    TLS.clone().map_err(ImapError::Other)
}

/// How `a.localeCompare(b)` orders mailbox names.
static NAME_ORDER: LazyLock<Option<CollatorBorrowed<'static>>> =
    LazyLock::new(|| Collator::try_new(Default::default(), CollatorOptions::default()).ok());

fn locale_compare(a: &str, b: &str) -> Ordering {
    match &*NAME_ORDER {
        Some(order) => order.compare(a, b),
        None => a.cmp(b),
    }
}

/// A failure of the connection itself. The ones imapflow reports with one of
/// the codes the tools read as "could not reach iCloud" are told apart from
/// the rest, such as a certificate that cannot be trusted.
pub(crate) fn io_failure(error: &std::io::Error) -> ImapError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::BrokenPipe
        | ErrorKind::TimedOut
        | ErrorKind::UnexpectedEof
        | ErrorKind::NotConnected => ImapError::Unreachable(error.to_string()),
        _ => ImapError::Other(error.to_string()),
    }
}

/// Open the connection a mail protocol is spoken over. `phase` names what
/// took too long in the error.
pub(crate) async fn open_tcp(
    host: &str,
    port: u16,
    within: Duration,
) -> Result<TcpStream, ImapError> {
    let connecting = async {
        // A name that does not resolve is a server that cannot be reached.
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| ImapError::Unreachable(error.to_string()))?;
        let mut failure = ImapError::Unreachable(format!("{host} has no address"));
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(stream),
                Err(error) => failure = io_failure(&error),
            }
        }
        Err(failure)
    };
    timeout(within, connecting).await.unwrap_or_else(|_| {
        Err(ImapError::Unreachable(
            "Failed to connect in required time".to_owned(),
        ))
    })
}

/// Encrypt an open connection, checking the certificate for `host`.
pub(crate) async fn start_tls(
    host: &str,
    stream: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ImapError> {
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|error| ImapError::Other(error.to_string()))?;
    tokio_rustls::TlsConnector::from(tls_config()?)
        .connect(name, stream)
        .await
        .map_err(|error| io_failure(&error))
}

pub(crate) struct ImapClient {
    stream: BufReader<Box<dyn IoStream>>,
    socket_timeout: Duration,
    tag_counter: u32,
    /// In upper case.
    capabilities: HashSet<String>,
    /// Whether the server listed its capabilities since the sign-in started.
    has_fresh_capabilities: bool,
    namespace_prefix: String,
    namespace_delimiter: Option<String>,
    mailbox: Option<SelectedMailbox>,
    is_closed: bool,
}

impl ImapClient {
    /// Connect and sign in. An [`ImapError::AuthenticationFailed`] means the
    /// server was reached and did not accept the sign-in.
    pub(crate) async fn connect(
        server: &ImapServer,
        username: &str,
        password: &str,
        timeouts: Timeouts,
    ) -> Result<Self, ImapError> {
        let connecting = async {
            let tcp = open_tcp(&server.host, server.port, timeouts.connect).await?;
            let stream: Box<dyn IoStream> = if server.tls {
                Box::new(start_tls(&server.host, tcp).await?)
            } else {
                Box::new(tcp)
            };
            Ok::<_, ImapError>(stream)
        };
        let stream = timeout(timeouts.connect, connecting)
            .await
            .unwrap_or_else(|_| {
                Err(ImapError::Unreachable(
                    "Failed to connect in required time".to_owned(),
                ))
            })?;

        let mut client = Self {
            stream: BufReader::new(stream),
            socket_timeout: timeouts.socket,
            tag_counter: 0,
            capabilities: HashSet::new(),
            has_fresh_capabilities: false,
            namespace_prefix: String::new(),
            namespace_delimiter: None,
            mailbox: None,
            is_closed: false,
        };
        let is_authenticated = timeout(timeouts.connect, client.greeting())
            .await
            .unwrap_or_else(|_| {
                Err(ImapError::Unreachable(
                    "Failed to receive greeting from server in required time".to_owned(),
                ))
            })?;
        client
            .start_session(username, password, is_authenticated)
            .await?;
        Ok(client)
    }

    /// Wait for the server to greet. `true` when it already knows who connects.
    async fn greeting(&mut self) -> Result<bool, ImapError> {
        loop {
            let Some(response) = self.read_response(None).await? else {
                continue;
            };
            if response.tag != "*" {
                continue;
            }
            self.note_capabilities(&response);
            match response.command.to_uppercase().as_str() {
                "OK" => return Ok(false),
                "PREAUTH" => return Ok(true),
                "BYE" => {
                    self.is_closed = true;
                    return Err(ImapError::Unreachable(format!(
                        "Connection closed: {}",
                        response.text()
                    )));
                }
                _ => {}
            }
        }
    }

    /// `startSession`
    async fn start_session(
        &mut self,
        username: &str,
        password: &str,
        is_authenticated: bool,
    ) -> Result<(), ImapError> {
        if self.capabilities.is_empty() {
            // Not being told what the server can do is not a reason to give up.
            let _ = self.exec("CAPABILITY", &[], Continuation::None).await;
        }
        if !is_authenticated {
            self.has_fresh_capabilities = false;
            self.authenticate(username, password).await?;
            if !self.has_fresh_capabilities {
                let _ = self.exec("CAPABILITY", &[], Continuation::None).await;
            }
        }
        self.namespace().await
    }

    /// `authenticate` of `imap-flow.js`, and `commands/authenticate.js` and `login.js`.
    /// Whatever goes wrong while signing in counts as a refused sign-in, as it does there.
    async fn authenticate(&mut self, username: &str, password: &str) -> Result<(), ImapError> {
        // Exchange takes the account name of another user this way, which only LOGIN carries.
        let must_use_login = username.contains(['\\', '/']);
        let has_plain = self.capabilities.contains("AUTH=PLAIN");
        let has_login = self.capabilities.contains("AUTH=LOGIN");
        let exchange = if (has_login || has_plain) && !must_use_login {
            if has_plain {
                let response = STANDARD.encode(format!("\0{username}\0{password}"));
                self.exec(
                    "AUTHENTICATE",
                    &[Arg::atom("PLAIN")],
                    Continuation::Plain(response),
                )
                .await
            } else {
                let answers = Continuation::Login {
                    username: username.to_owned(),
                    password: password.to_owned(),
                };
                self.exec("AUTHENTICATE", &[Arg::atom("LOGIN")], answers)
                    .await
            }
        } else if self.capabilities.contains("LOGINDISABLED") {
            return Err(ImapError::AuthenticationFailed(
                "Login is disabled".to_owned(),
            ));
        } else {
            self.exec(
                "LOGIN",
                &[
                    Arg::String(username.to_owned()),
                    Arg::String(password.to_owned()),
                ],
                Continuation::None,
            )
            .await
        };
        match exchange {
            Ok(_) => Ok(()),
            Err(ImapError::Refused { text, .. }) => Err(ImapError::AuthenticationFailed(text)),
            Err(error) => Err(ImapError::AuthenticationFailed(error.to_string())),
        }
    }

    /// `commands/namespace.js`: where the mailboxes of the account are, and what separates their levels.
    async fn namespace(&mut self) -> Result<(), ImapError> {
        if !self.capabilities.contains("NAMESPACE") {
            // The answer to an empty LIST names the root and the separator.
            if let Ok(exchange) = self
                .exec("LIST", &[Arg::atom(""), Arg::atom("")], Continuation::None)
                .await
            {
                for listed in exchange
                    .untagged
                    .iter()
                    .filter(|response| response.command.eq_ignore_ascii_case("LIST"))
                {
                    let delimiter = listed
                        .attributes
                        .get(1)
                        .and_then(Token::string_value)
                        .map(str::to_owned);
                    let mut prefix = listed
                        .attributes
                        .get(2)
                        .and_then(Token::text)
                        .unwrap_or_default();
                    if let Some(delimiter) = delimiter
                        .as_deref()
                        .filter(|delimiter| !delimiter.is_empty())
                    {
                        prefix = prefix.strip_prefix(delimiter).unwrap_or(&prefix).to_owned();
                        if !prefix.is_empty() && !prefix.ends_with(delimiter) {
                            prefix.push_str(delimiter);
                        }
                    }
                    self.namespace_prefix = prefix;
                    self.namespace_delimiter = delimiter;
                }
            }
            return Ok(());
        }

        match self.exec("NAMESPACE", &[], Continuation::None).await {
            Ok(exchange) => {
                let personal = exchange
                    .untagged
                    .iter()
                    .filter(|response| response.command.eq_ignore_ascii_case("NAMESPACE"))
                    .filter_map(|response| response.attributes.first()?.list()?.first()?.list())
                    .next_back();
                match personal {
                    Some(
                        [
                            Token::String(prefix) | Token::Atom { value: prefix, .. },
                            delimiter,
                            ..,
                        ],
                    ) => {
                        let delimiter = delimiter.string_value().map(str::to_owned);
                        let mut prefix = prefix.clone();
                        if let Some(delimiter) = delimiter
                            .as_deref()
                            .filter(|delimiter| !delimiter.is_empty())
                            && !prefix.is_empty()
                            && !prefix.ends_with(delimiter)
                        {
                            prefix.push_str(delimiter);
                        }
                        self.namespace_prefix = prefix;
                        self.namespace_delimiter = delimiter;
                    }
                    _ => self.namespace_delimiter = Some(".".to_owned()),
                }
                Ok(())
            }
            // Exchange accepts the password of an account that may not use IMAP, and only says so here.
            Err(ImapError::Refused {
                status: Status::Bad,
                text,
                ..
            }) if text
                .to_lowercase()
                .contains("user is authenticated but not connected") =>
            {
                Err(ImapError::AuthenticationFailed(text))
            }
            Err(_) => Ok(()),
        }
    }

    /// How many messages the selected mailbox holds.
    pub(crate) fn mailbox_exists(&self) -> u64 {
        self.mailbox.as_ref().map_or(0, |mailbox| mailbox.exists)
    }

    /// `normalizePath`: INBOX whatever its case, and other names under the root of the account.
    fn normalize_path(&self, path: &str, skip_namespace: bool) -> String {
        if path.eq_ignore_ascii_case("INBOX") {
            return "INBOX".to_owned();
        }
        if !skip_namespace
            && !self.namespace_prefix.is_empty()
            && !path.starts_with(&self.namespace_prefix)
        {
            return format!("{}{path}", self.namespace_prefix);
        }
        path.to_owned()
    }

    /// `encodePath`
    fn encode_path(path: &str) -> String {
        let needs_encoding = path.chars().any(|character| {
            character == '&'
                || matches!(character, '\u{0}'..='\u{8}' | '\u{b}'..='\u{c}' | '\u{e}'..='\u{1f}')
                || character >= '\u{80}'
        });
        if needs_encoding {
            encode_utf7(path)
        } else {
            path.to_owned()
        }
    }

    /// `decodePath`
    fn decode_path(path: &str) -> String {
        if path.contains('&') {
            decode_utf7(path)
        } else {
            path.to_owned()
        }
    }

    /// A mailbox name as SELECT and STATUS write it.
    fn path_argument(path: &str) -> Arg {
        let encoded = Self::encode_path(path);
        if encoded.contains('&') {
            Arg::String(encoded)
        } else {
            Arg::Atom(encoded)
        }
    }

    fn note_capabilities(&mut self, response: &Response) {
        let listed = match response.code() {
            Some([name, capabilities @ ..])
                if name
                    .string_value()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("CAPABILITY")) =>
            {
                Some(capabilities)
            }
            _ if response.tag == "*" && response.command.eq_ignore_ascii_case("CAPABILITY") => {
                Some(response.attributes.as_slice())
            }
            _ => None,
        };
        if let Some(listed) = listed {
            self.capabilities = listed
                .iter()
                .filter_map(Token::string_value)
                .map(|capability| capability.trim().to_uppercase())
                .collect();
            self.has_fresh_capabilities = true;
        }
    }

    fn fail(&mut self, error: ImapError) -> ImapError {
        self.is_closed = true;
        self.mailbox = None;
        error
    }

    /// Read from the connection for as long as the server may stay silent.
    async fn read_more(&mut self) -> Result<usize, ImapError> {
        let read = match timeout(self.socket_timeout, self.stream.fill_buf()).await {
            Err(_) => Err(ImapError::Unreachable("Socket timeout".to_owned())),
            Ok(Err(error)) => Err(io_failure(&error)),
            Ok(Ok([])) => Err(ImapError::Unreachable("Connection closed".to_owned())),
            Ok(Ok(available)) => Ok(available.len()),
        };
        read.map_err(|error| self.fail(error))
    }

    /// One response: its line without the line break, and its literals.
    /// The port of `handler/imap-stream.js`.
    async fn read_frame(&mut self) -> Result<(Vec<u8>, Vec<Vec<u8>>), ImapError> {
        let mut payload: Vec<u8> = Vec::new();
        let mut literals = Vec::new();
        let mut size = 0;
        loop {
            let line_start = payload.len();
            loop {
                let available = self.read_more().await?;
                let (taken, is_complete) = match self.stream.buffer()[..available]
                    .iter()
                    .position(|byte| *byte == b'\n')
                {
                    Some(end) => (end + 1, true),
                    None => (available, false),
                };
                size += taken;
                if size > MAX_RESPONSE_BYTES {
                    return Err(self.fail(ImapError::Other(
                        "The server sent a response that is too large".to_owned(),
                    )));
                }
                payload.extend_from_slice(&self.stream.buffer()[..taken]);
                self.stream.consume(taken);
                if is_complete {
                    break;
                }
            }

            let Some(literal_size) = literal_marker(&payload[line_start..]) else {
                break;
            };
            size = size.saturating_add(literal_size);
            if size > MAX_RESPONSE_BYTES {
                return Err(self.fail(ImapError::Other(
                    "The server sent a response that is too large".to_owned(),
                )));
            }
            let mut literal = Vec::with_capacity(literal_size);
            while literal.len() < literal_size {
                let available = self.read_more().await?;
                let taken = available.min(literal_size - literal.len());
                literal.extend_from_slice(&self.stream.buffer()[..taken]);
                self.stream.consume(taken);
            }
            literals.push(literal);
        }
        if payload.ends_with(b"\n") {
            payload.pop();
            if payload.ends_with(b"\r") {
                payload.pop();
            }
        }
        Ok((payload, literals))
    }

    /// The next response. `None` for one that cannot be read and does not
    /// answer the command with `tag`: imapflow skips those too.
    async fn read_response(&mut self, tag: Option<&str>) -> Result<Option<Response>, ImapError> {
        let (payload, literals) = self.read_frame().await?;
        if payload.is_empty() {
            return Ok(None);
        }
        match parse_response(&payload, literals) {
            Ok(response) => Ok(Some(response)),
            Err(error) => {
                // Nothing of the response is logged: it may be mail.
                tracing::debug!(
                    code = error.code,
                    "Could not read a response of the IMAP server"
                );
                let first_word = payload
                    .split(|byte| byte.is_ascii_whitespace() || *byte == 0)
                    .find(|word| !word.is_empty());
                let answers_command = tag.is_some_and(|tag| {
                    error.tag.as_deref() == Some(tag) || first_word == Some(tag.as_bytes())
                });
                if answers_command {
                    return Err(ImapError::Other(
                        "Failed to parse the server response for this command".to_owned(),
                    ));
                }
                Ok(None)
            }
        }
    }

    /// Write to the connection. The time the server may stay silent is given
    /// to each piece, so that a message that takes long to send is not taken
    /// for a connection that stopped.
    async fn write(&mut self, data: &[u8]) -> Result<(), ImapError> {
        for piece in data.chunks(WRITE_PIECE_BYTES) {
            let writing = async {
                self.stream.get_mut().write_all(piece).await?;
                self.stream.get_mut().flush().await
            };
            match timeout(self.socket_timeout, writing).await {
                Err(_) => {
                    return Err(self.fail(ImapError::Unreachable("Socket timeout".to_owned())));
                }
                Ok(Err(error)) => return Err(self.fail(io_failure(&error))),
                Ok(Ok(())) => {}
            }
        }
        Ok(())
    }

    /// Send a command and wait for the server to complete it. `exec` and
    /// `handleResponse` and `settleRequest` of `imap-flow.js`.
    async fn exec(
        &mut self,
        command: &str,
        arguments: &[Arg],
        mut continuation: Continuation,
    ) -> Result<Exchange, ImapError> {
        if self.is_closed {
            return Err(ImapError::Unreachable(
                "Connection not available".to_owned(),
            ));
        }
        self.tag_counter += 1;
        let tag = format!("{:X}", self.tag_counter);
        let literal_minus =
            self.capabilities.contains("LITERAL-") || self.capabilities.contains("LITERAL+");
        let mut parts = compile(&tag, command, arguments, literal_minus)
            .map_err(|error| ImapError::Other(error.to_string()))?
            .into_iter();
        if let Some(first) = parts.next() {
            self.write(&first).await?;
        }

        let mut untagged = Vec::new();
        loop {
            let Some(response) = self.read_response(Some(&tag)).await? else {
                continue;
            };
            if response.tag == "+" {
                let answer = match &mut continuation {
                    Continuation::Plain(answer) => Some(std::mem::take(answer)),
                    Continuation::Login { username, password } => {
                        // The server asks in base64 for "Username:", then for "Password:".
                        let question = STANDARD
                            .decode(response.text())
                            .map(|question| String::from_utf8_lossy(&question).to_lowercase());
                        match question
                            .as_deref()
                            .map(|question| question.trim_end_matches([':', '\0']))
                        {
                            Ok("username" | "user name") => {
                                Some(STANDARD.encode(username.as_bytes()))
                            }
                            Ok("password") => Some(STANDARD.encode(password.as_bytes())),
                            _ => None,
                        }
                    }
                    Continuation::None => None,
                };
                match answer {
                    Some(answer) => self.write(format!("{answer}\r\n").as_bytes()).await?,
                    None => {
                        if let Some(next) = parts.next() {
                            self.write(&next).await?;
                        }
                    }
                }
                continue;
            }

            self.note_capabilities(&response);
            if response.tag == "*" {
                self.note_mailbox_change(&response);
                untagged.push(response);
                continue;
            }
            if response.tag != tag {
                continue;
            }
            return match response.command.to_uppercase().as_str() {
                "OK" | "BYE" => Ok(Exchange {
                    untagged,
                    tagged: response,
                }),
                status @ ("NO" | "BAD") => {
                    let text = response.text();
                    // A message deleted in the meantime is not a reason to fail the others.
                    if status == "NO"
                        && text.contains("Some of the requested messages no longer exist")
                    {
                        return Ok(Exchange {
                            untagged,
                            tagged: response,
                        });
                    }
                    let lower = text.to_lowercase();
                    // Microsoft 365 answers BAD with the time to wait when it limits requests.
                    if lower.contains("request is throttled") && lower.contains("backoff time") {
                        return Err(ImapError::Throttled(text));
                    }
                    Err(ImapError::Refused {
                        status: if status == "NO" {
                            Status::No
                        } else {
                            Status::Bad
                        },
                        text,
                        mailbox_missing: false,
                    })
                }
                _ => Err(ImapError::Other("Invalid server response".to_owned())),
            };
        }
    }

    /// `untaggedExists` and `untaggedExpunge`: the server says, at any time,
    /// that the selected mailbox got or lost a message.
    fn note_mailbox_change(&mut self, response: &Response) {
        let Some(mailbox) = &mut self.mailbox else {
            return;
        };
        let Some(number) = parse_uint(&response.command, MAX_UINT32_DIGITS) else {
            return;
        };
        match response
            .attributes
            .first()
            .and_then(Token::string_value)
            .map(str::to_uppercase)
            .as_deref()
        {
            Some("EXISTS") => mailbox.exists = number,
            Some("EXPUNGE") => mailbox.exists = mailbox.exists.saturating_sub(1),
            _ => {}
        }
    }

    /// `commands/list.js`: every mailbox of the account, the special ones
    /// first, then by name. With `with_status`, how many messages each holds
    /// and how many are unread.
    pub(crate) async fn list(
        &mut self,
        with_status: bool,
    ) -> Result<Vec<ListedMailbox>, ImapError> {
        let capabilities = self.capabilities.clone();
        let has = |capability: &str| capabilities.contains(capability);
        let has_special_use = has("SPECIAL-USE");
        // An extension of Gmail from before RFC 6154, only used when the server has nothing else.
        let command = if has("XLIST") && !has_special_use {
            "XLIST"
        } else {
            "LIST"
        };
        let supports_extended_list = has("LIST-EXTENDED");
        let can_request_status = command == "LIST" && with_status && has("LIST-STATUS");
        // With RETURN options a server may only say what was asked for, so
        // what a plain LIST says of itself is asked for as well.
        let mut auxiliary = Vec::new();
        if has_special_use {
            auxiliary.push(Arg::atom("SPECIAL-USE"));
        }
        if has("CHILDREN") || supports_extended_list {
            auxiliary.push(Arg::atom("CHILDREN"));
        }
        let status_items = Arg::List(vec![Arg::atom("MESSAGES"), Arg::atom("UNSEEN")]);

        // Servers that advertise the options sometimes refuse them: ask for
        // less at each attempt, down to a plain LIST.
        let mut attempts: Vec<Vec<Arg>> = Vec::new();
        if can_request_status {
            let mut options = vec![Arg::atom("STATUS"), status_items.clone()];
            if !auxiliary.is_empty() {
                attempts.push(
                    options
                        .iter()
                        .cloned()
                        .chain(auxiliary.iter().cloned())
                        .collect(),
                );
            }
            attempts.push(std::mem::take(&mut options));
        }
        attempts.push(Vec::new());

        let reference = self.normalize_path("", false);
        let mut listing = None;
        let last = attempts.len() - 1;
        for (index, options) in attempts.into_iter().enumerate() {
            let mut arguments = vec![
                Arg::Atom(Self::encode_path(&reference)),
                Arg::Atom(Self::encode_path("*")),
            ];
            if !options.is_empty() {
                arguments.push(Arg::atom("RETURN"));
                arguments.push(Arg::List(options));
            }
            match self.exec(command, &arguments, Continuation::None).await {
                Ok(exchange) => {
                    listing = Some(exchange.untagged);
                    break;
                }
                // A BAD says the server does not know the options. Anything else is a failure.
                Err(ImapError::Refused {
                    status: Status::Bad,
                    ..
                }) if index < last => {}
                Err(error) => return Err(error),
            }
        }
        let mut listing = listing.unwrap_or_default();

        // The inbox may be outside the root the other mailboxes are under.
        let lists_inbox = listing.iter().any(|response| {
            let path = response
                .attributes
                .get(2)
                .and_then(Token::text)
                .unwrap_or_default();
            response.command.eq_ignore_ascii_case(command)
                && Self::decode_path(&path).eq_ignore_ascii_case("INBOX")
        });
        if !reference.is_empty() && !lists_inbox {
            listing.extend(
                self.exec(
                    command,
                    &[Arg::atom(""), Arg::atom("INBOX")],
                    Continuation::None,
                )
                .await?
                .untagged,
            );
        }

        let mut entries: Vec<ListedMailbox> = Vec::new();
        let mut statuses: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
        // The mailboxes that could be each special one, by how that was found out.
        let mut candidates: Vec<(&'static str, Source, usize)> = Vec::new();
        for response in &listing {
            let name = response.command.to_uppercase();
            if name == "STATUS" {
                let path = self.normalize_path(
                    &Self::decode_path(
                        &response
                            .attributes
                            .first()
                            .and_then(Token::text)
                            .unwrap_or_default(),
                    ),
                    false,
                );
                if let (false, Some(items)) = (
                    path.is_empty(),
                    response.attributes.get(1).and_then(Token::list),
                ) {
                    statuses.insert(path, status_counts(items));
                }
                continue;
            }
            if name != command || response.attributes.is_empty() {
                continue;
            }
            let raw_path = response
                .attributes
                .get(2)
                .and_then(Token::text)
                .unwrap_or_default();
            let mut entry = ListedMailbox {
                path: self.normalize_path(&Self::decode_path(&raw_path), false),
                flags: string_list(response.attributes.first())
                    .into_iter()
                    .collect(),
                delimiter: response
                    .attributes
                    .get(1)
                    .and_then(Token::string_value)
                    .map(str::to_owned),
                ..ListedMailbox::default()
            };
            // RFC 5258: a mailbox that does not exist cannot be selected either.
            if entry.flags.contains("\\NonExistent") {
                entry.flags.insert("\\Noselect".to_owned());
            }
            entry.flags.remove("\\Subscribed");
            let index = entries.len();
            if command == "XLIST" && entry.flags.remove("\\Inbox") && entry.path != "INBOX" {
                candidates.push(("\\Inbox", Source::Extension, index));
            }
            if entry.path.eq_ignore_ascii_case("INBOX") && !entry.flags.contains("\\NonExistent") {
                candidates.push(("\\Inbox", Source::Name, index));
            }
            // Some servers put the separator before the name.
            let delimiter = entry
                .delimiter
                .clone()
                .filter(|delimiter| !delimiter.is_empty());
            if let Some(delimiter) = &delimiter
                && let Some(stripped) = entry.path.strip_prefix(delimiter.as_str())
            {
                entry.path = stripped.to_owned();
            }
            entry.parent = match &delimiter {
                Some(delimiter) => entry
                    .path
                    .split(delimiter.as_str())
                    .map(str::to_owned)
                    .collect(),
                None => vec![entry.path.clone()],
            };
            entry.name = entry.parent.pop().unwrap_or_default();

            // Only what the server flags is trusted for a mailbox that does not exist.
            if let Some((flag, source)) =
                special_use(has("XLIST") || has_special_use, &entry.flags, &entry.name)
                && (source == Source::Extension || !entry.flags.contains("\\NonExistent"))
            {
                candidates.push((flag, source, index));
            }
            entries.push(entry);
        }

        if with_status {
            let has_inline_status = !statuses.is_empty();
            for entry in &mut entries {
                if entry.flags.contains("\\Noselect") || entry.flags.contains("\\NonExistent") {
                    continue;
                }
                let counts = match statuses.get(&entry.path) {
                    Some(counts) => Some(*counts),
                    None if has_inline_status => None,
                    // The server does not send the counts with the list: ask for each mailbox.
                    None => self.status(&entry.path).await,
                };
                if let Some((messages, unseen)) = counts {
                    entry.messages = messages;
                    entry.unseen = unseen;
                }
            }
        }

        // Each special use goes to one mailbox and each mailbox has one at
        // most: to the mailbox the server flagged before one that has the
        // name, and to the one nearest to the root before one deeper.
        candidates.sort_by(|a, b| {
            let (left, right) = (&entries[a.2], &entries[b.2]);
            a.1.cmp(&b.1)
                .then(left.parent.len().cmp(&right.parent.len()))
                .then_with(|| locale_compare(&left.path, &right.path))
        });
        let mut assigned: HashSet<&str> = HashSet::new();
        for (flag, _, index) in candidates {
            if !assigned.contains(flag) && entries[index].special_use.is_none() {
                entries[index].special_use = Some(flag);
                assigned.insert(flag);
            }
        }

        const FLAG_SORT_ORDER: [&str; 8] = [
            "\\Inbox",
            "\\Flagged",
            "\\Sent",
            "\\Drafts",
            "\\All",
            "\\Archive",
            "\\Junk",
            "\\Trash",
        ];
        let rank = |flag: &str| FLAG_SORT_ORDER.iter().position(|known| *known == flag);
        entries.sort_by(|a, b| match (a.special_use, b.special_use) {
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(left), Some(right)) => rank(left).cmp(&rank(right)),
            (None, None) => {
                let left = a.parent.iter().chain([&a.name]);
                let mut right = b.parent.iter().chain([&b.name]);
                for part in left {
                    let other = right.next().map_or("", String::as_str);
                    if part != other {
                        return locale_compare(part, other);
                    }
                }
                locale_compare(&a.path, &b.path)
            }
        });
        Ok(entries)
    }

    /// `commands/status.js`: the messages of one mailbox and how many are
    /// unread. `None` when the server does not say.
    async fn status(&mut self, path: &str) -> Option<(Option<u64>, Option<u64>)> {
        let path = self.normalize_path(path, false);
        let arguments = [
            Self::path_argument(&path),
            Arg::List(vec![Arg::atom("MESSAGES"), Arg::atom("UNSEEN")]),
        ];
        let exchange = self
            .exec("STATUS", &arguments, Continuation::None)
            .await
            .ok()?;
        let mut counts = (None, None);
        for response in exchange
            .untagged
            .iter()
            .filter(|response| response.command.eq_ignore_ascii_case("STATUS"))
        {
            if let Some(items) = response.attributes.get(1).and_then(Token::list) {
                let (messages, unseen) = status_counts(items);
                counts = (messages.or(counts.0), unseen.or(counts.1));
            }
        }
        Some(counts)
    }

    /// `getMailboxLock`: make `path` the mailbox the commands apply to.
    /// Read-only access never changes a flag. A mailbox that is already
    /// selected the same way is not selected again.
    pub(crate) async fn open_mailbox(
        &mut self,
        path: &str,
        read_only: bool,
    ) -> Result<(), ImapError> {
        let path = self.normalize_path(path, false);
        if self
            .mailbox
            .as_ref()
            .is_some_and(|mailbox| mailbox.path == path && mailbox.read_only == read_only)
        {
            return Ok(());
        }
        match self.select(&path, read_only).await {
            Err(ImapError::Refused {
                status: Status::No,
                text,
                ..
            }) => {
                // A refusal does not say why. The mailbox is missing when the
                // server does not list it, or lists it as a name that holds no messages.
                let arguments = [Arg::atom(""), Arg::Atom(Self::encode_path(&path))];
                let mailbox_missing = match self.exec("LIST", &arguments, Continuation::None).await
                {
                    Ok(listing) => !listing.untagged.iter().any(|response| {
                        let flags = string_list(response.attributes.first());
                        response.command.eq_ignore_ascii_case("LIST")
                            && !response.attributes.is_empty()
                            && !flags.iter().any(|flag| {
                                flag.eq_ignore_ascii_case("\\Noselect")
                                    || flag.eq_ignore_ascii_case("\\NonExistent")
                            })
                    }),
                    Err(_) => false,
                };
                Err(ImapError::Refused {
                    status: Status::No,
                    text,
                    mailbox_missing,
                })
            }
            other => other,
        }
    }

    /// `commands/select.js`
    async fn select(&mut self, path: &str, read_only: bool) -> Result<(), ImapError> {
        let command = if read_only { "EXAMINE" } else { "SELECT" };
        let exchange = match self
            .exec(command, &[Self::path_argument(path)], Continuation::None)
            .await
        {
            Ok(exchange) => exchange,
            Err(error) => {
                // A failed SELECT leaves no mailbox selected.
                self.mailbox = None;
                return Err(error);
            }
        };

        let mut mailbox = SelectedMailbox {
            path: path.to_owned(),
            exists: 0,
            read_only: false,
            permanent_flags: None,
        };
        for response in &exchange.untagged {
            match response.code() {
                Some([name, flags, ..])
                    if name
                        .string_value()
                        .is_some_and(|name| name.eq_ignore_ascii_case("PERMANENTFLAGS"))
                        && flags.list().is_some() =>
                {
                    mailbox.permanent_flags = Some(string_list(Some(flags)).into_iter().collect());
                }
                _ => {}
            }
            let is_exists = response
                .attributes
                .first()
                .and_then(Token::string_value)
                .is_some_and(|name| name.eq_ignore_ascii_case("EXISTS"));
            if let (true, Some(count)) =
                (is_exists, parse_uint(&response.command, MAX_UINT32_DIGITS))
            {
                mailbox.exists = count;
            }
        }
        // The server says how it opened the mailbox, which is not always how it was asked to.
        if let Some(access) = exchange.tagged.code_name() {
            mailbox.read_only = access == "READ-ONLY";
        }
        self.mailbox = Some(mailbox);
        Ok(())
    }

    /// `commands/search.js`: the messages of the selected mailbox that meet
    /// every criterion, by UID, in ascending order. `None` when the search
    /// failed, whatever the reason: imapflow answers `false` then.
    pub(crate) async fn search(
        &mut self,
        criteria: &[Criterion],
        now: DateTime<Utc>,
    ) -> Option<Vec<u32>> {
        let exists = self.mailbox.as_ref()?.exists;
        let arguments = if criteria.is_empty() {
            vec![Arg::atom("ALL")]
        } else {
            self.search_arguments(criteria, now)
        };
        let exchange = self
            .exec("UID SEARCH", &arguments, Continuation::None)
            .await
            .ok()?;

        let mut found: Vec<u32> = Vec::new();
        for response in &exchange.untagged {
            match response.command.to_uppercase().as_str() {
                "SEARCH" => found.extend(
                    response
                        .attributes
                        .iter()
                        .filter_map(Token::string_value)
                        .filter_map(sequence_value),
                ),
                // An IMAP4rev2 server answers with the set of the matches.
                "ESEARCH" => {
                    let all = response
                        .attributes
                        .iter()
                        .skip_while(|token| {
                            !token
                                .string_value()
                                .is_some_and(|key| key.eq_ignore_ascii_case("ALL"))
                        })
                        .nth(1);
                    if let Some(set) = all.and_then(Token::string_value) {
                        // A set can name more messages than the mailbox holds, which it never has.
                        found.extend(
                            expand_range(&set.replace('*', "0"))
                                .into_iter()
                                .take(usize::try_from(exists).unwrap_or(usize::MAX)),
                        );
                    }
                }
                _ => {}
            }
        }
        found.sort_unstable();
        found.dedup();
        Some(found)
    }

    /// `searchCompiler`
    fn search_arguments(&self, criteria: &[Criterion], now: DateTime<Utc>) -> Vec<Arg> {
        // Text outside ASCII goes as a literal, which the server is told is UTF-8.
        let value = |text: &str| {
            if !text.is_ascii() && !text.contains('\0') {
                Arg::Literal {
                    data: text.as_bytes().to_vec(),
                    is_literal8: false,
                }
            } else {
                Arg::atom(text)
            }
        };
        let mut arguments = Vec::new();
        for criterion in criteria {
            match criterion {
                Criterion::From(text)
                | Criterion::To(text)
                | Criterion::Subject(text)
                | Criterion::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    arguments.push(Arg::atom(match criterion {
                        Criterion::From(_) => "FROM",
                        Criterion::To(_) => "TO",
                        Criterion::Subject(_) => "SUBJECT",
                        _ => "TEXT",
                    }));
                    arguments.push(value(text));
                }
                Criterion::Since(date) | Criterion::Before(date) => {
                    let is_before = matches!(criterion, Criterion::Before(_));
                    if self.capabilities.contains("WITHIN") {
                        // RFC 5032: an age in seconds, which unlike a date has a time.
                        let age = ((now - *date).num_milliseconds().max(0) as f64 / 1000.0).round();
                        arguments.push(Arg::atom(if is_before { "OLDER" } else { "YOUNGER" }));
                        arguments.push(value(&js::number_to_string(age)));
                        continue;
                    }
                    // A date has no time: before the middle of a day is before the next day.
                    let is_midnight = to_iso(*date).ends_with("T00:00:00.000Z");
                    let date = if is_before && !is_midnight {
                        // The last day a date can hold has no next one.
                        date.checked_add_signed(chrono::Duration::days(1))
                            .unwrap_or(*date)
                    } else {
                        *date
                    };
                    arguments.push(Arg::atom(if is_before { "BEFORE" } else { "SINCE" }));
                    arguments.push(value(&search_date(date)));
                }
                Criterion::Seen(seen) => {
                    arguments.push(Arg::atom(if *seen { "SEEN" } else { "UNSEEN" }))
                }
                Criterion::Flagged(flagged) => {
                    arguments.push(Arg::atom(if *flagged { "FLAGGED" } else { "UNFLAGGED" }))
                }
                Criterion::Uid(set) => {
                    if !set.is_empty() {
                        arguments.push(Arg::atom("UID"));
                        arguments.push(Arg::Sequence(set.clone()));
                    }
                }
                Criterion::Seq(set) => {
                    if !set.is_empty() {
                        arguments.push(Arg::Sequence(set.clone()));
                    }
                }
            }
        }
        if arguments
            .iter()
            .any(|argument| matches!(argument, Arg::Literal { .. }))
        {
            arguments.splice(0..0, [Arg::atom("CHARSET"), Arg::atom("UTF-8")]);
        }
        arguments
    }

    /// `commands/fetch.js`: what `query` asks of the messages of `range`,
    /// which names UIDs or, without `by_uid`, positions in the mailbox.
    pub(crate) async fn fetch(
        &mut self,
        range: &str,
        query: &FetchQuery,
        by_uid: bool,
    ) -> Result<Vec<Fetched>, ImapError> {
        if self.mailbox.is_none() || range.is_empty() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for (wanted, name) in [
            (query.uid, "UID"),
            (query.flags, "FLAGS"),
            (query.body_structure, "BODYSTRUCTURE"),
            (query.envelope, "ENVELOPE"),
            (query.internal_date, "INTERNALDATE"),
            (query.size, "RFC822.SIZE"),
        ] {
            if wanted {
                items.push(Arg::atom(name));
            }
        }
        // Messages are always told apart by their UID.
        if !query.uid {
            items.push(Arg::atom("UID"));
        }
        if let Some(headers) = &query.headers {
            let fields = Arg::List(headers.iter().map(Arg::atom).collect());
            items.push(Arg::Section {
                name: "BODY.PEEK".to_owned(),
                section: vec![Arg::atom("HEADER.FIELDS"), fields],
                partial: None,
            });
        }
        for part in &query.body_parts {
            items.push(Arg::Section {
                name: "BODY.PEEK".to_owned(),
                section: vec![Arg::Atom(part.key.to_uppercase())],
                partial: part
                    .range
                    .filter(|(start, length)| *start > 0 || *length > 0)
                    .map(|(start, length)| {
                        if length > 0 {
                            vec![start, length]
                        } else {
                            vec![start]
                        }
                    }),
            });
        }
        let items = if items.len() == 1 {
            items.remove(0)
        } else {
            Arg::List(items)
        };
        let command = if by_uid { "UID FETCH" } else { "FETCH" };
        let exchange = self
            .exec(
                command,
                &[Arg::Sequence(range.to_owned()), items],
                Continuation::None,
            )
            .await?;

        Ok(exchange
            .untagged
            .iter()
            .filter(|response| {
                response
                    .attributes
                    .first()
                    .and_then(Token::string_value)
                    .is_some_and(|name| name.eq_ignore_ascii_case("FETCH"))
            })
            .map(parse_fetch)
            // A response without a UID is the server saying that a flag changed, not an answer.
            .filter(|fetched| fetched.has_uid)
            .collect())
    }

    /// `fetchOne`: what `query` asks of the message with this UID, or `None`
    /// when the selected mailbox has no such message.
    pub(crate) async fn fetch_one(
        &mut self,
        uid: u32,
        query: &FetchQuery,
    ) -> Result<Option<Fetched>, ImapError> {
        let fetched = self.fetch(&uid.to_string(), query, true).await?;
        Ok(fetched
            .into_iter()
            .find(|fetched| fetched.message.uid == uid))
    }

    /// `download.js`: the content of one part of a message, decoded from its
    /// transfer encoding, and to UTF-8 for a text part, in the pieces it
    /// arrived in. At most `max_bytes` are returned, and no more is asked of
    /// the server than it takes to have them. `None` when the message or the
    /// part is not there.
    pub(crate) async fn download(
        &mut self,
        uid: u32,
        part: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<Vec<u8>>>, ImapError> {
        if self.mailbox.is_none() {
            return Ok(None);
        }
        let mut part = js::trim(part).to_lowercase();
        if part == "1" {
            // The body of a message made of one part is not its part 1, but its text.
            let query = FetchQuery {
                uid: true,
                body_structure: true,
                ..FetchQuery::default()
            };
            let Some(message) = self.fetch_one(uid, &query).await? else {
                return Ok(None);
            };
            if message
                .message
                .body_structure
                .is_none_or(|structure| structure.child_nodes.is_none())
            {
                part = "text".to_owned();
            }
        }

        // The headers of the part say how it is encoded.
        let is_numbered = !part.is_empty()
            && part
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.');
        let mime_key = if is_numbered {
            Some(format!("{part}.mime"))
        } else {
            (part == "text").then(|| "header".to_owned())
        };
        let chunk_query = |start: u64| PartQuery {
            key: part.clone(),
            range: Some((start, DOWNLOAD_CHUNK_BYTES)),
        };
        let mut first = FetchQuery {
            uid: true,
            size: true,
            ..FetchQuery::default()
        };
        first
            .body_parts
            .extend(mime_key.clone().map(|key| PartQuery { key, range: None }));
        first.body_parts.push(chunk_query(0));
        let Some(mut response) = self.fetch_one(uid, &first).await? else {
            return Ok(None);
        };
        let Some(Some(chunk)) = response.body_parts.remove(&part) else {
            return Ok(None);
        };
        let mime = match mime_key.as_deref() {
            Some("header") => response.message.headers.take(),
            Some(key) => response.body_parts.remove(key).flatten(),
            None => None,
        };

        let mut pipeline = Pipeline::new(mime.as_deref(), max_bytes);
        let mut processed = chunk.len() as u64;
        let mut has_more = processed == DOWNLOAD_CHUNK_BYTES;
        pipeline.push(&chunk);
        // A server that keeps sending is not sending this part.
        let max_total = response
            .size
            .filter(|size| *size > 0)
            .map(|size| size.saturating_mul(2).saturating_add(DOWNLOAD_CHUNK_BYTES));
        while has_more && !pipeline.is_limited {
            if max_total.is_some_and(|max_total| processed >= max_total) {
                return Err(ImapError::Other(
                    "Download exceeded the expected message size".to_owned(),
                ));
            }
            let next = FetchQuery {
                body_parts: vec![chunk_query(processed)],
                ..FetchQuery::default()
            };
            let Some(mut response) = self.fetch_one(uid, &next).await? else {
                return Err(ImapError::Other(
                    "Message disappeared before the download completed".to_owned(),
                ));
            };
            let Some(Some(chunk)) = response.body_parts.remove(&part) else {
                break;
            };
            processed += chunk.len() as u64;
            has_more = chunk.len() as u64 == DOWNLOAD_CHUNK_BYTES;
            pipeline.push(&chunk);
        }
        Ok(Some(pipeline.finish()))
    }

    /// `commands/store.js`: add flags to messages, or remove them. `false`
    /// when the server did not do it, or the mailbox does not keep these flags.
    pub(crate) async fn store(
        &mut self,
        uids: &[u32],
        flags: &[&str],
        add: bool,
        silent: bool,
    ) -> bool {
        let Some(mailbox) = &self.mailbox else {
            return false;
        };
        if uids.is_empty() {
            return false;
        }
        let can_use = |flag: &str| {
            mailbox
                .permanent_flags
                .as_ref()
                .is_none_or(|permanent| permanent.contains("\\*") || permanent.contains(flag))
        };
        let flags: Vec<Arg> = flags
            .iter()
            .filter(|flag| !add || can_use(flag))
            .map(|flag| Arg::atom(*flag))
            .collect();
        if flags.is_empty() {
            return false;
        }
        let operation = format!(
            "{}FLAGS{}",
            if add { "+" } else { "-" },
            if silent { ".SILENT" } else { "" }
        );
        let arguments = [
            Arg::Sequence(uid_set(uids)),
            Arg::Atom(operation),
            Arg::List(flags),
        ];
        self.exec("UID STORE", &arguments, Continuation::None)
            .await
            .is_ok()
    }

    /// `commands/move.js`: move messages to another mailbox. `None` when the
    /// server refused, which is what it does for a destination that does not exist.
    pub(crate) async fn move_messages(&mut self, uids: &[u32], destination: &str) -> Option<Moved> {
        if self.mailbox.is_none() || uids.is_empty() || destination.is_empty() {
            return None;
        }
        let destination = self.normalize_path(destination, false);
        let arguments = [
            Arg::Sequence(uid_set(uids)),
            Arg::Atom(Self::encode_path(&destination)),
        ];
        let has_move = self.capabilities.contains("MOVE");
        let exchange = self
            .exec(
                if has_move { "UID MOVE" } else { "UID COPY" },
                &arguments,
                Continuation::None,
            )
            .await
            .ok()?;
        // The UIDs in the destination come with an untagged OK for MOVE, and with the completion for COPY.
        let uid_map = exchange
            .untagged
            .iter()
            .chain([&exchange.tagged])
            .find_map(copy_uid);

        if !has_move {
            // Without MOVE, a message is moved by copying it and deleting the original.
            if !self.store(uids, &["\\Deleted"], true, true).await {
                return None;
            }
            let expunged = if self.capabilities.contains("UIDPLUS") {
                self.exec(
                    "UID EXPUNGE",
                    &[Arg::Sequence(uid_set(uids))],
                    Continuation::None,
                )
                .await
            } else {
                self.exec("EXPUNGE", &[], Continuation::None).await
            };
            expunged.ok()?;
        }
        Some(Moved {
            destination,
            uid_map,
        })
    }

    /// `commands/append.js`: add a message to a mailbox.
    pub(crate) async fn append(
        &mut self,
        path: &str,
        content: &[u8],
        flags: &[&str],
    ) -> Result<Option<Appended>, ImapError> {
        if path.is_empty() {
            return Ok(None);
        }
        if let Some(limit) = self
            .capabilities
            .iter()
            .find_map(|capability| capability.strip_prefix("APPENDLIMIT="))
            .and_then(|limit| parse_uint(limit, 20))
            && limit < content.len() as u64
        {
            return Err(ImapError::Other(format!(
                "Message content too big for APPENDLIMIT={limit}"
            )));
        }
        let destination = self.normalize_path(path, false);
        // A mailbox selected for writing says which flags it keeps.
        let target = self
            .mailbox
            .as_ref()
            .filter(|mailbox| mailbox.path == destination);
        let is_selected = target.is_some();
        let permanent_flags = target
            .filter(|mailbox| !mailbox.read_only)
            .and_then(|mailbox| mailbox.permanent_flags.clone());
        let flags: Vec<Arg> = flags
            .iter()
            .filter(|flag| {
                permanent_flags
                    .as_ref()
                    .is_none_or(|permanent| permanent.contains("\\*") || permanent.contains(**flag))
            })
            .map(|flag| Arg::atom(*flag))
            .collect();

        let mut arguments = vec![Arg::Atom(Self::encode_path(&destination))];
        if !flags.is_empty() {
            arguments.push(Arg::List(flags));
        }
        let is_literal8 = self.capabilities.contains("BINARY") && content.contains(&0);
        arguments.push(Arg::Literal {
            data: content.to_vec(),
            is_literal8,
        });
        let exchange = self.exec("APPEND", &arguments, Continuation::None).await?;

        let mut uid = match exchange.tagged.code() {
            Some([name, _, uid, ..])
                if name
                    .string_value()
                    .is_some_and(|name| name.eq_ignore_ascii_case("APPENDUID")) =>
            {
                uid.string_value()
                    .and_then(|uid| parse_uint(uid, MAX_UINT32_DIGITS))
                    .and_then(|uid| u32::try_from(uid).ok())
                    .filter(|uid| *uid > 0)
            }
            _ => None,
        };
        // Without UIDPLUS, the new message is the last one of the mailbox when it is the one selected.
        if uid.is_none() && is_selected {
            let mut position = exists_count(&exchange.untagged);
            if position.is_none()
                && let Ok(exchange) = self.exec("NOOP", &[], Continuation::None).await
            {
                position = exists_count(&exchange.untagged);
            }
            if let Some(position) = position.filter(|position| *position > 0) {
                uid = self
                    .search(&[Criterion::Seq(position.to_string())], Utc::now())
                    .await
                    .and_then(|found| found.first().copied());
            }
        }
        Ok(Some(Appended { destination, uid }))
    }

    /// `commands/logout.js`: say goodbye, and close whatever the server answers.
    pub(crate) async fn logout(&mut self) {
        if !self.is_closed {
            let _ = self.exec("LOGOUT", &[], Continuation::None).await;
        }
        self.is_closed = true;
        self.mailbox = None;
        let _ = timeout(Duration::from_secs(1), self.stream.get_mut().shutdown()).await;
    }
}

/// `{123}` at the end of a line: the number of bytes that follow it.
fn literal_marker(line: &[u8]) -> Option<usize> {
    let line = line.strip_suffix(b"\n")?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let digits_end = line.strip_suffix(b"}")?;
    let digits = digits_end
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let open = digits_end.len().checked_sub(digits + 1)?;
    if digits == 0 || digits > 19 || digits_end[open] != b'{' {
        return None;
    }
    std::str::from_utf8(&digits_end[open + 1..])
        .ok()?
        .parse()
        .ok()
}

/// A set of UIDs as a command names it: `11,14`.
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// `formatDate`: a date as SEARCH takes it, such as `04-Oct-2026`.
fn search_date(date: DateTime<Utc>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let iso = to_iso(date);
    let mut parts: Vec<&str> = iso.get(..10).unwrap_or(&iso).split('-').collect();
    parts.reverse();
    let month = parts
        .get(1)
        .and_then(|month| month.parse::<usize>().ok())
        .and_then(|month| MONTHS.get(month.wrapping_sub(1)));
    if let (Some(month), Some(slot)) = (month, parts.get_mut(1)) {
        *slot = month;
    }
    parts.join("-")
}

/// The MESSAGES and UNSEEN counts of a STATUS response.
fn status_counts(items: &[Token]) -> (Option<u64>, Option<u64>) {
    let mut counts = (None, None);
    for pair in items.chunks(2) {
        let (Some(name), Some(value)) = (
            pair.first().and_then(Token::string_value),
            pair.get(1).and_then(Token::string_value),
        ) else {
            continue;
        };
        let count = parse_uint(value, MAX_UINT32_DIGITS);
        match name.to_uppercase().as_str() {
            "MESSAGES" => counts.0 = count.or(counts.0),
            "UNSEEN" => counts.1 = count.or(counts.1),
            _ => {}
        }
    }
    counts
}

/// The last `* n EXISTS` among responses.
fn exists_count(responses: &[Response]) -> Option<u64> {
    responses
        .iter()
        .filter(|response| {
            response
                .attributes
                .first()
                .and_then(Token::string_value)
                .is_some_and(|name| name.eq_ignore_ascii_case("EXISTS"))
        })
        .filter_map(|response| parse_uint(&response.command, MAX_UINT32_DIGITS))
        .next_back()
}

/// `parseCopyUid`: `[COPYUID 7 12,14 1:2]` pairs the UIDs of copied messages
/// with the ones they have in the destination.
fn copy_uid(response: &Response) -> Option<Vec<(u32, u32)>> {
    let [name, _, source, destination, ..] = response.code()? else {
        return None;
    };
    if name.string_value() != Some("COPYUID") {
        return None;
    }
    let (source, destination) = (
        expand_range(source.string_value()?),
        expand_range(destination.string_value()?),
    );
    (source.len() == destination.len()).then(|| source.into_iter().zip(destination).collect())
}

enum Transfer {
    Plain,
    Base64(Base64Stream),
    /// Quoted-printable is decoded once it is whole, as `libqp` does.
    QuotedPrintable(Vec<u8>),
}

/// What turns the bytes of a message part into its content: the transfer
/// encoding first, then, for text shown as the body of a message, flowed
/// lines and the charset.
struct Pipeline {
    transfer: Transfer,
    /// The text to join the lines of, and whether to delete the space that ends each.
    flowed: Option<(Vec<u8>, bool)>,
    charset: Option<encoding_rs::Decoder>,
    max_bytes: u64,
    emitted: u64,
    output: Vec<Vec<u8>>,
    /// Enough was read: more of the part would not change what is returned.
    is_limited: bool,
}

impl Pipeline {
    fn new(mime: Option<&[u8]>, max_bytes: u64) -> Self {
        let mut pipeline = Self {
            transfer: Transfer::Plain,
            flowed: None,
            charset: None,
            max_bytes: max_bytes.max(1),
            emitted: 0,
            output: Vec::new(),
            is_limited: false,
        };
        let Some(headers) = mime else { return pipeline };

        let content_type = parse_header_value(&first_header(headers, "content-type"));
        let media_type = js::trim(&content_type.value.to_lowercase()).to_owned();
        let encoding = first_header(headers, "content-transfer-encoding");
        let encoding = parse_header_value(&encoding).value;
        // A comment may follow the encoding.
        let encoding = match (encoding.find('('), encoding.rfind(')')) {
            (Some(open), Some(close)) if open < close => {
                format!("{}{}", &encoding[..open], &encoding[close + 1..])
            }
            _ => encoding,
        };
        pipeline.transfer = match js::trim(&encoding.to_lowercase()) {
            "base64" => Transfer::Base64(Base64Stream::default()),
            "quoted-printable" => Transfer::QuotedPrintable(Vec::new()),
            _ => Transfer::Plain,
        };

        let disposition = decode_words(js::trim(
            &parse_header_value(&first_header(headers, "content-disposition"))
                .value
                .to_lowercase(),
        ));
        let is_text = ["text/html", "text/plain", "text/x-amp-html"].contains(&media_type.as_str());
        if (disposition.is_empty() || disposition == "inline") && is_text {
            let param = |name: &str| {
                content_type
                    .param(name)
                    .map(|value| js::trim(&value.to_lowercase()).to_owned())
            };
            if param("format").as_deref() == Some("flowed") {
                pipeline.flowed = Some((Vec::new(), param("delsp").as_deref() == Some("yes")));
            }
            if let Some(charset) =
                param("charset").filter(|charset| !charset.is_empty() && !is_plain_charset(charset))
            {
                pipeline.charset = super::decode::charset_decoder(&charset);
            }
        }
        pipeline
    }

    fn push(&mut self, chunk: &[u8]) {
        match &mut self.transfer {
            Transfer::Plain => self.decoded(chunk),
            Transfer::Base64(stream) => {
                let decoded = stream.push(chunk);
                self.decoded(&decoded);
            }
            Transfer::QuotedPrintable(collected) => collected.extend_from_slice(chunk),
        }
    }

    /// Content without its transfer encoding.
    fn decoded(&mut self, content: &[u8]) {
        match &mut self.flowed {
            Some((collected, _)) => {
                // The lines are joined once the text is whole, and no more of it is kept than may be returned.
                let room = usize::try_from(self.max_bytes)
                    .unwrap_or(usize::MAX)
                    .saturating_sub(collected.len());
                collected.extend_from_slice(&content[..content.len().min(room)]);
                if collected.len() as u64 >= self.max_bytes {
                    self.is_limited = true;
                }
            }
            None => self.text(content, false),
        }
    }

    /// Text in the charset of the part.
    fn text(&mut self, content: &[u8], is_last: bool) {
        let Some(mut decoder) = self.charset.take() else {
            return self.emit(content);
        };
        let mut input = content;
        loop {
            let mut text = String::with_capacity(
                decoder
                    .max_utf8_buffer_length(input.len())
                    .unwrap_or(input.len() * 3 + 16),
            );
            let (result, read, _) = decoder.decode_to_string(input, &mut text, is_last);
            input = &input[read..];
            self.emit(text.as_bytes());
            if result == encoding_rs::CoderResult::InputEmpty {
                break;
            }
        }
        self.charset = Some(decoder);
    }

    fn emit(&mut self, content: &[u8]) {
        let room = usize::try_from(self.max_bytes - self.emitted).unwrap_or(usize::MAX);
        let content = &content[..content.len().min(room)];
        if !content.is_empty() {
            self.emitted += content.len() as u64;
            self.output.push(content.to_vec());
        }
        if self.emitted >= self.max_bytes {
            self.is_limited = true;
        }
    }

    fn finish(mut self) -> Vec<Vec<u8>> {
        match std::mem::replace(&mut self.transfer, Transfer::Plain) {
            Transfer::Plain => {}
            Transfer::Base64(mut stream) => {
                let decoded = stream.finish();
                self.decoded(&decoded);
            }
            Transfer::QuotedPrintable(collected) => {
                let decoded = decode_quoted_printable(&collected);
                self.decoded(&decoded);
            }
        }
        if let Some((collected, delete_space)) = self.flowed.take() {
            let text = decode_flowed(&collected, delete_space);
            self.text(&text, false);
        }
        self.text(&[], true);
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(mime: &str, max_bytes: u64, chunks: &[&[u8]]) -> (String, bool) {
        let mut pipeline = Pipeline::new(Some(mime.as_bytes()), max_bytes);
        let mut is_limited = false;
        for chunk in chunks {
            pipeline.push(chunk);
            is_limited |= pipeline.is_limited;
        }
        (
            String::from_utf8(pipeline.finish().concat()).unwrap(),
            is_limited,
        )
    }

    #[test]
    fn decodes_a_part_from_its_transfer_encoding_and_charset() {
        let quoted = "Content-Type: text/plain; charset=ISO-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n";
        assert_eq!(
            decoded(quoted, 1000, &[b"Andr=E9 a =", b"\r\nfaim"]),
            ("André a faim".to_owned(), false)
        );
        // The limit counts bytes of the text in UTF-8, wherever that falls.
        let mut limited = Pipeline::new(Some(quoted.as_bytes()), 5);
        limited.push(b"Andr=E9 a faim");
        assert_eq!(limited.finish().concat(), b"Andr\xc3");

        let base64 = "Content-Type: application/pdf; name=menu.pdf\r\nContent-Transfer-Encoding: BASE64\r\nContent-Disposition: attachment\r\n\r\n";
        assert_eq!(
            decoded(base64, 1000, &[b"JVBERi0x", b"\r\nLjQK"]),
            ("%PDF-1.4\n".to_owned(), false)
        );
        assert_eq!(
            decoded(base64, 4, &[b"JVBERi0x", b"LjQK"]),
            ("%PDF".to_owned(), true)
        );

        let flowed = "Content-Type: text/plain; format=flowed; delsp=yes; charset=utf-8\r\n\r\n";
        assert_eq!(
            decoded(flowed, 1000, &[b"A line \r\nthat ", b"\r\ngoes on.\r\n"]).0,
            "A linethatgoes on."
        );

        // An attached text file is not the text of the message: it is served as it is.
        let attached = "Content-Type: text/plain; charset=ISO-8859-1\r\nContent-Disposition: attachment; filename=notes.txt\r\n\r\n";
        let mut pipeline = Pipeline::new(Some(attached.as_bytes()), 1000);
        pipeline.push(b"Andr\xe9");
        assert_eq!(pipeline.finish().concat(), b"Andr\xe9");
    }

    #[test]
    fn finds_the_literals_a_line_announces() {
        assert_eq!(literal_marker(b"* 1 FETCH (BODY[1] {120}\r\n"), Some(120));
        assert_eq!(literal_marker(b"x {0}\n"), Some(0));
        assert_eq!(literal_marker(b"* OK done\r\n"), None);
        assert_eq!(literal_marker(b"{12}\r\n"), Some(12));
        assert_eq!(literal_marker(b"x {}\r\n"), None);
        assert_eq!(literal_marker(b"x {12+}\r\n"), None);
        assert_eq!(
            search_date(
                DateTime::parse_from_rfc3339("2026-10-04T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            ),
            "04-Oct-2026"
        );
        assert_eq!(uid_set(&[11, 14]), "11,14");
    }
}
