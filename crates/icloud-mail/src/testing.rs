//! A mail account served over IMAP and SMTP on 127.0.0.1, for tests: the
//! port of `tests/helpers/icloud_mail.ts`.
//!
//! The Node tests replaced imapflow and nodemailer with objects in memory.
//! Here the account is behind two real servers that speak enough of both
//! protocols for the tools, without encryption, so that a test runs the
//! whole client: what it writes, and what it makes of what it reads.
//!
//! ```no_run
//! # async fn example(core: std::sync::Arc<mymcps_core::Core>) {
//! use mymcps_builtin::BuiltinEnv;
//! use mymcps_icloud_mail::testing::FakeIcloud;
//!
//! let icloud = FakeIcloud::start().await;
//! let env = BuiltinEnv::new(core).with_extension(icloud.servers());
//! // ... run tools with a context made of `env` ...
//! assert_eq!(icloud.logouts(), 0);
//! # }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, TimeZone, Utc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::MailServers;
use crate::imap::wire::{Response, Token, parse_response, string_list};
use crate::message::{Address, BodyStructure, Envelope};

/// The sign-in of the account.
pub const USERNAME: &str = "thomas@icloud.com";
pub const PASSWORD: &str = "abcd-efgh-ijkl-mnop";

/// The decoded bytes of the attachment of message 11.
pub const ATTACHMENT: &str = "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

/// One message of the account.
#[derive(Debug, Clone, Default)]
pub struct FakeMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub envelope: Envelope,
    pub body_structure: BodyStructure,
    /// The content of each part by its number, before any transfer encoding.
    /// A message of one part holds it under `1`.
    pub parts: HashMap<String, Vec<u8>>,
    /// The `References` header.
    pub references: Option<String>,
}

impl FakeMessage {
    pub fn part(mut self, number: &str, content: impl Into<Vec<u8>>) -> Self {
        self.parts.insert(number.to_owned(), content.into());
        self
    }
}

/// A message added with APPEND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendedMessage {
    pub raw: String,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FakeMailbox {
    pub path: String,
    /// `\Sent`, `\Drafts`, `\Trash`, `\Archive` or `\Junk`.
    pub special_use: Option<String>,
    /// Other flags of the mailbox, such as `\Noselect`.
    pub flags: Vec<String>,
    pub messages: Vec<FakeMessage>,
    pub appended: Vec<AppendedMessage>,
}

/// What the SMTP server does with a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SmtpBehaviour {
    /// Take it for every recipient.
    #[default]
    Accept,
    /// Refuse these recipients, with this reply, and take it for the others.
    RejectRecipients {
        recipients: Vec<String>,
        reply: String,
    },
    /// Refuse every recipient with this reply.
    RejectAllRecipients { reply: String },
    /// Refuse the sign-in.
    RejectSignIn,
    /// Hang up once the message is received, without saying whether it was taken.
    HangUp,
    /// Refuse the message once it is received, with this reply.
    RejectMessage { reply: String },
    /// Take the message, but only answer once [`FakeIcloud::release_deliveries`] is called.
    Hold,
}

/// How the account behaves.
#[derive(Debug, Clone, Default)]
pub struct FakeOptions {
    /// Refuse every sign-in over IMAP.
    pub reject_sign_in: bool,
    /// The only IMAP username the account takes. It takes any by default.
    pub imap_username: Option<String>,
    /// Refuse every APPEND.
    pub fail_append: bool,
    pub smtp: SmtpBehaviour,
    /// What the IMAP server says it can do, instead of [`CAPABILITIES`].
    pub capabilities: Option<Vec<&'static str>>,
}

/// What the IMAP server says it can do: what iCloud does, as far as the tools go.
pub const CAPABILITIES: [&str; 12] = [
    "IMAP4rev1",
    "AUTH=PLAIN",
    "LITERAL+",
    "NAMESPACE",
    "UIDPLUS",
    "MOVE",
    "SPECIAL-USE",
    "LIST-STATUS",
    "LIST-EXTENDED",
    "CHILDREN",
    "BINARY",
    "ID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInAttempt {
    pub username: String,
    pub password: String,
}

/// A SELECT or an EXAMINE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub path: String,
    pub read_only: bool,
}

/// A FETCH that asked for something else than the content of a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchCommand {
    pub range: String,
    pub by_uid: bool,
}

/// The first request for the content of a part of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub uid: u32,
    /// The part as the command names it: `1.2`, or `TEXT` for a message of one part.
    pub part: String,
}

/// A message the SMTP server received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub from: String,
    pub to: Vec<String>,
    pub raw: String,
}

#[derive(Default)]
struct State {
    options: FakeOptions,
    mailboxes: Vec<FakeMailbox>,
    sign_ins: Vec<SignInAttempt>,
    smtp_sign_ins: Vec<SignInAttempt>,
    logouts: usize,
    selections: Vec<Selection>,
    searches: Vec<String>,
    fetches: Vec<FetchCommand>,
    downloads: Vec<Download>,
    commands: Vec<String>,
    deliveries: Vec<Delivery>,
    deliveries_started: usize,
}

/// The account, and the two servers in front of it. They stop when it is dropped.
pub struct FakeIcloud {
    state: Arc<Mutex<State>>,
    imap_port: u16,
    smtp_port: u16,
    release: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for FakeIcloud {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl FakeIcloud {
    pub async fn start() -> Self {
        Self::start_with(FakeOptions::default()).await
    }

    pub async fn start_with(options: FakeOptions) -> Self {
        let state = Arc::new(Mutex::new(State {
            options,
            mailboxes: mailboxes(),
            ..State::default()
        }));
        let (release, released) = watch::channel(false);
        let imap = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a port for the IMAP server");
        let smtp = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a port for the SMTP server");
        let imap_port = imap
            .local_addr()
            .expect("the address of the IMAP server")
            .port();
        let smtp_port = smtp
            .local_addr()
            .expect("the address of the SMTP server")
            .port();

        let imap_state = state.clone();
        let imap_task = tokio::spawn(async move {
            while let Ok((stream, _)) = imap.accept().await {
                let state = imap_state.clone();
                tokio::spawn(async move {
                    let _ = serve_imap(stream, state).await;
                });
            }
        });
        let smtp_state = state.clone();
        let smtp_task = tokio::spawn(async move {
            while let Ok((stream, _)) = smtp.accept().await {
                let (state, released) = (smtp_state.clone(), released.clone());
                tokio::spawn(async move {
                    let _ = serve_smtp(stream, state, released).await;
                });
            }
        });
        Self {
            state,
            imap_port,
            smtp_port,
            release,
            tasks: vec![imap_task, smtp_task],
        }
    }

    /// Where the servers listen: what a test attaches to its `BuiltinEnv`.
    pub fn servers(&self) -> MailServers {
        MailServers::local(self.imap_port, self.smtp_port)
    }

    /// Every sign-in tried over IMAP, accepted or not.
    pub fn sign_ins(&self) -> Vec<SignInAttempt> {
        locked(&self.state).sign_ins.clone()
    }

    /// Every sign-in tried over SMTP.
    pub fn smtp_sign_ins(&self) -> Vec<SignInAttempt> {
        locked(&self.state).smtp_sign_ins.clone()
    }

    pub fn logouts(&self) -> usize {
        locked(&self.state).logouts
    }

    pub fn selections(&self) -> Vec<Selection> {
        locked(&self.state).selections.clone()
    }

    /// What each SEARCH asked for, as it was written: `FROM alice UNSEEN`.
    pub fn searches(&self) -> Vec<String> {
        locked(&self.state).searches.clone()
    }

    pub fn fetches(&self) -> Vec<FetchCommand> {
        locked(&self.state).fetches.clone()
    }

    pub fn downloads(&self) -> Vec<Download> {
        locked(&self.state).downloads.clone()
    }

    /// Every IMAP command received, without its tag. Sign-ins are left out.
    pub fn commands(&self) -> Vec<String> {
        locked(&self.state).commands.clone()
    }

    /// The messages the SMTP server took.
    pub fn sent(&self) -> Vec<Delivery> {
        locked(&self.state).deliveries.clone()
    }

    /// How many messages the SMTP server received, taken or not yet.
    pub fn deliveries_started(&self) -> usize {
        locked(&self.state).deliveries_started
    }

    /// Let the deliveries held by [`SmtpBehaviour::Hold`] complete.
    pub fn release_deliveries(&self) {
        let _ = self.release.send(true);
    }

    /// A copy of a mailbox as it is now.
    pub fn mailbox(&self, path: &str) -> FakeMailbox {
        locked(&self.state)
            .mailboxes
            .iter()
            .find(|mailbox| mailbox.path == path)
            .cloned()
            .unwrap_or_else(|| panic!("no mailbox {path}"))
    }

    pub fn push_message(&self, path: &str, message: FakeMessage) {
        let mut state = locked(&self.state);
        match state
            .mailboxes
            .iter_mut()
            .find(|mailbox| mailbox.path == path)
        {
            Some(mailbox) => mailbox.messages.push(message),
            None => panic!("no mailbox {path}"),
        }
    }
}

fn date(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
}

/// A part that is not a multipart.
pub fn leaf(
    part: Option<&str>,
    media_type: &str,
    encoding: Option<&str>,
    size: u64,
) -> BodyStructure {
    BodyStructure {
        part: part.map(str::to_owned),
        media_type: media_type.to_owned(),
        encoding: encoding.map(str::to_owned),
        size: Some(size),
        ..BodyStructure::default()
    }
}

/// A file attached to a message.
pub fn attached(part: &str, media_type: &str, size: u64, filename: &str) -> BodyStructure {
    BodyStructure {
        disposition: Some("attachment".to_owned()),
        disposition_parameters: HashMap::from([("filename".to_owned(), filename.to_owned())]),
        ..leaf(Some(part), media_type, Some("base64"), size)
    }
}

pub fn multipart(part: Option<&str>, subtype: &str, children: Vec<BodyStructure>) -> BodyStructure {
    BodyStructure {
        part: part.map(str::to_owned),
        media_type: format!("multipart/{subtype}"),
        child_nodes: Some(children),
        ..BodyStructure::default()
    }
}

/// The mailboxes of the account, as each test finds them.
fn mailboxes() -> Vec<FakeMailbox> {
    let inbox = vec![
        FakeMessage {
            uid: 11,
            envelope: Envelope {
                date: date(2026, 10, 1, 9, 30),
                subject: Some("Lunch on Thursday?".into()),
                message_id: Some("<lunch@example.com>".into()),
                from: vec![Address::new("Alice Martin", "alice@example.com")],
                reply_to: vec![Address::new("Alice Martin", "alice@example.com")],
                to: vec![
                    Address::new("Thomas", "thomas@icloud.com"),
                    Address::bare("bob@example.com"),
                ],
                cc: vec![Address::new("Carol", "carol@example.com")],
                ..Envelope::default()
            },
            body_structure: multipart(
                None,
                "mixed",
                vec![
                    multipart(
                        Some("1"),
                        "alternative",
                        vec![
                            leaf(Some("1.1"), "text/plain", Some("quoted-printable"), 52),
                            leaf(Some("1.2"), "text/html", Some("quoted-printable"), 120),
                        ],
                    ),
                    attached("2", "application/pdf", 78000, "Menu été.pdf"),
                ],
            ),
            references: Some("<root@example.com>".into()),
            ..FakeMessage::default()
        }
        .part(
            "1.1",
            "Hi Thomas,\r\n\r\nAre you free on Thursday?\r\n\r\nAlice",
        )
        .part(
            "1.2",
            "<p>Hi Thomas,</p><p>Are you free on <b>Thursday</b>?</p><p>Alice</p>",
        )
        .part("2", ATTACHMENT),
        FakeMessage {
            uid: 12,
            flags: vec!["\\Seen".into()],
            envelope: Envelope {
                date: date(2026, 10, 2, 6, 0),
                subject: Some("Autumn sale".into()),
                message_id: Some("<sale@shop.example>".into()),
                from: vec![Address::new("Shop", "news@shop.example")],
                to: vec![Address::bare("thomas@icloud.com")],
                ..Envelope::default()
            },
            body_structure: leaf(None, "text/html", Some("quoted-printable"), 4096),
            ..FakeMessage::default()
        }
        .part(
            "1",
            [
                "<html><head><style>p { color: red }</style></head><body>",
                "<p>Hello\u{200c} \u{200c} \u{200c} \u{200c} </p>",
                "<img src=\"https://shop.example/pixel.gif\">",
                "<p><a href=\"https://shop.example/deals\">See the deals</a></p>",
                "<p><a href=\"https://shop.example\">https://shop.example</a></p>",
                "</body></html>",
            ]
            .concat(),
        ),
        FakeMessage {
            uid: 14,
            flags: vec!["\\Seen".into(), "\\Flagged".into()],
            envelope: Envelope {
                date: date(2026, 10, 3, 15, 45),
                subject: Some("Re: Invoice 42".into()),
                message_id: Some("<invoice@example.com>".into()),
                from: vec![Address::bare("andre@example.com")],
                to: vec![Address::bare("thomas@icloud.com")],
                ..Envelope::default()
            },
            body_structure: leaf(None, "text/plain", Some("7bit"), 4000),
            ..FakeMessage::default()
        }
        .part(
            "1",
            format!("Paid today.\r\n\r\n{}", "Thanks. ".repeat(400)),
        ),
    ];
    let sent = vec![
        FakeMessage {
            uid: 5,
            flags: vec!["\\Seen".into()],
            envelope: Envelope {
                date: date(2026, 9, 30, 8, 0),
                subject: Some("Quote".into()),
                message_id: Some("<quote@icloud.com>".into()),
                from: vec![Address::bare("thomas@icloud.com")],
                to: vec![Address::new("Dave", "dave@example.com")],
                ..Envelope::default()
            },
            body_structure: leaf(None, "text/plain", Some("7bit"), 30),
            ..FakeMessage::default()
        }
        .part("1", "Here is the quote."),
    ];
    let archive = vec![
        FakeMessage {
            uid: 3,
            flags: vec!["\\Seen".into()],
            envelope: Envelope {
                date: date(2026, 9, 20, 10, 0),
                subject: Some("Website enquiry".into()),
                message_id: Some("<enquiry@example.com>".into()),
                from: vec![Address::new("Erin", "erin@example.com")],
                to: vec![Address::bare("Hello@Thomas.example")],
                ..Envelope::default()
            },
            body_structure: leaf(None, "text/plain", Some("7bit"), 40),
            ..FakeMessage::default()
        }
        .part("1", "Do you take new projects?"),
    ];
    let mailbox = |path: &str, special_use: Option<&str>, messages: Vec<FakeMessage>| FakeMailbox {
        path: path.to_owned(),
        special_use: special_use.map(str::to_owned),
        messages,
        ..FakeMailbox::default()
    };
    vec![
        mailbox("INBOX", None, inbox),
        mailbox("Sent Messages", Some("\\Sent"), sent),
        mailbox("Drafts", Some("\\Drafts"), Vec::new()),
        mailbox("Deleted Messages", Some("\\Trash"), Vec::new()),
        mailbox("Archive", Some("\\Archive"), archive),
        FakeMailbox {
            flags: vec!["\\Noselect".into()],
            ..mailbox("Projects", None, Vec::new())
        },
    ]
}

/// A string as a response writes it: quoted, or as an encoded word when it
/// has characters outside ASCII, which is how servers hand headers over.
fn quoted(text: &str) -> String {
    if text.is_ascii() {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("\"=?UTF-8?B?{}?=\"", STANDARD.encode(text))
    }
}

fn nstring(text: Option<&str>) -> String {
    text.map_or_else(|| "NIL".to_owned(), quoted)
}

fn address_list(addresses: &[Address]) -> String {
    if addresses.is_empty() {
        return "NIL".to_owned();
    }
    let entries: Vec<String> = addresses
        .iter()
        .map(|address| {
            let name = (!address.name.is_empty()).then_some(address.name.as_str());
            match address.address.rsplit_once('@') {
                Some((mailbox, host)) => format!(
                    "({} NIL {} {})",
                    nstring(name),
                    quoted(mailbox),
                    quoted(host)
                ),
                None => format!(
                    "({} NIL {} NIL)",
                    nstring(name),
                    nstring((!address.address.is_empty()).then_some(address.address.as_str()))
                ),
            }
        })
        .collect();
    format!("({})", entries.concat())
}

fn envelope(envelope: &Envelope) -> String {
    format!(
        "({} {} {} {} {} {} {} {} {} {})",
        nstring(envelope.date.map(|date| date.to_rfc2822()).as_deref()),
        nstring(envelope.subject.as_deref()),
        address_list(&envelope.from),
        address_list(if envelope.sender.is_empty() {
            &envelope.from
        } else {
            &envelope.sender
        }),
        address_list(&envelope.reply_to),
        address_list(&envelope.to),
        address_list(&envelope.cc),
        address_list(&envelope.bcc),
        nstring(envelope.in_reply_to.as_deref()),
        nstring(envelope.message_id.as_deref()),
    )
}

fn params(params: &HashMap<String, String>, charset: bool) -> String {
    let mut entries: Vec<String> = Vec::new();
    if charset && !params.contains_key("charset") {
        entries.push("\"charset\" \"utf-8\"".to_owned());
    }
    let mut names: Vec<&String> = params.keys().collect();
    names.sort();
    entries.extend(
        names
            .into_iter()
            .map(|name| format!("{} {}", quoted(name), quoted(&params[name]))),
    );
    if entries.is_empty() {
        "NIL".to_owned()
    } else {
        format!("({})", entries.join(" "))
    }
}

fn body_structure(node: &BodyStructure) -> String {
    let disposition = match &node.disposition {
        Some(disposition) => format!(
            "({} {})",
            quoted(disposition),
            params(&node.disposition_parameters, false)
        ),
        None if !node.disposition_parameters.is_empty() => format!(
            "(\"inline\" {})",
            params(&node.disposition_parameters, false)
        ),
        None => "NIL".to_owned(),
    };
    if let Some(subtype) = node.media_type.strip_prefix("multipart/") {
        let children: String = node
            .child_nodes
            .iter()
            .flatten()
            .map(body_structure)
            .collect();
        return format!(
            "({children} {} (\"boundary\" \"b\") {disposition} NIL NIL)",
            quoted(subtype)
        );
    }
    let (kind, subtype) = node
        .media_type
        .split_once('/')
        .unwrap_or((&node.media_type, ""));
    let is_text = kind == "text";
    let mut fields = vec![
        quoted(kind),
        quoted(subtype),
        params(&node.parameters, is_text),
        "NIL".to_owned(),
        "NIL".to_owned(),
        nstring(node.encoding.as_deref()),
        node.size.unwrap_or(0).to_string(),
    ];
    if node.media_type == "message/rfc822" {
        fields.push("(NIL \"Forwarded\" NIL NIL NIL NIL NIL NIL NIL NIL)".to_owned());
        fields.push(node.child_nodes.iter().flatten().next().map_or_else(
            || "(\"text\" \"plain\" NIL NIL NIL \"7bit\" 0 0)".to_owned(),
            body_structure,
        ));
        fields.push("1".to_owned());
    }
    if is_text {
        fields.push("1".to_owned());
    }
    fields.extend([
        "NIL".to_owned(),
        disposition,
        "NIL".to_owned(),
        "NIL".to_owned(),
    ]);
    format!("({})", fields.join(" "))
}

/// The node of a structure with this part number. A message of one part has it under `1`.
fn find_part<'a>(node: &'a BodyStructure, number: &str) -> Option<&'a BodyStructure> {
    if node.part.as_deref() == Some(number)
        || (node.part.is_none() && node.child_nodes.is_none() && number == "1")
    {
        return Some(node);
    }
    node.child_nodes
        .iter()
        .flatten()
        .find_map(|child| find_part(child, number))
}

fn quoted_printable(content: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut line = 0;
    for (index, byte) in content.iter().enumerate() {
        let is_line_break = matches!(byte, b'\r' | b'\n');
        let ends_line = matches!(content.get(index + 1), None | Some(b'\r' | b'\n'));
        let piece = match byte {
            b'\r' | b'\n' => vec![*byte],
            b' ' | b'\t' if !ends_line => vec![*byte],
            b'!'..=b'<' | b'>'..=b'~' => vec![*byte],
            _ => format!("={byte:02X}").into_bytes(),
        };
        if is_line_break {
            line = 0;
        } else if line + piece.len() > 75 {
            encoded.extend_from_slice(b"=\r\n");
            line = 0;
        }
        if !is_line_break {
            line += piece.len();
        }
        encoded.extend(piece);
    }
    encoded
}

/// The content of a part as it travels: in its transfer encoding.
fn encoded_part(node: &BodyStructure, content: &[u8]) -> Vec<u8> {
    match node.encoding.as_deref() {
        Some("base64") => {
            let encoded = STANDARD.encode(content);
            encoded
                .as_bytes()
                .chunks(76)
                .flat_map(|line| [line, b"\r\n"].concat())
                .collect()
        }
        Some("quoted-printable") => quoted_printable(content),
        _ => content.to_vec(),
    }
}

/// The headers of a part. A text part is in UTF-8 unless its parameters say otherwise.
fn mime_headers(node: &BodyStructure) -> Vec<u8> {
    let mut headers = format!("Content-Type: {}", node.media_type);
    if node.media_type.starts_with("text/") && !node.parameters.contains_key("charset") {
        headers.push_str("; charset=utf-8");
    }
    let mut names: Vec<&String> = node.parameters.keys().collect();
    names.sort();
    for name in names {
        headers.push_str(&format!(";\r\n {name}=\"{}\"", node.parameters[name]));
    }
    headers.push_str("\r\n");
    if let Some(encoding) = &node.encoding {
        headers.push_str(&format!("Content-Transfer-Encoding: {encoding}\r\n"));
    }
    if let Some(disposition) = &node.disposition {
        headers.push_str(&format!("Content-Disposition: {disposition}\r\n"));
    }
    headers.push_str("\r\n");
    headers.into_bytes()
}

struct ImapConnection {
    stream: BufReader<TcpStream>,
    state: Arc<Mutex<State>>,
    is_signed_in: bool,
    selected: Option<(String, bool)>,
}

impl ImapConnection {
    async fn send(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.stream.get_mut().write_all(data).await?;
        self.stream.get_mut().flush().await
    }

    async fn line(&mut self, text: &str) -> std::io::Result<()> {
        self.send(format!("{text}\r\n").as_bytes()).await
    }

    /// One command with its literals, or `None` once the client is gone.
    async fn command(&mut self) -> std::io::Result<Option<(Vec<u8>, Vec<Vec<u8>>)>> {
        let mut payload = Vec::new();
        let mut literals = Vec::new();
        loop {
            let mut line = Vec::new();
            if self.stream.read_until(b'\n', &mut line).await? == 0 {
                return Ok(None);
            }
            let text = String::from_utf8_lossy(&line).trim_end().to_owned();
            let marker = text
                .rfind('{')
                .filter(|_| text.ends_with('}'))
                .and_then(|open| {
                    let size = &text[open + 1..text.len() - 1];
                    let (size, is_synchronizing) = match size.strip_suffix('+') {
                        Some(size) => (size, false),
                        None => (size, true),
                    };
                    Some((open, size.parse::<usize>().ok()?, is_synchronizing))
                });
            let Some((open, size, is_synchronizing)) = marker else {
                payload.extend_from_slice(text.as_bytes());
                return Ok(Some((payload, literals)));
            };
            if is_synchronizing {
                self.line("+ Ready for literal data").await?;
            }
            let mut literal = vec![0; size];
            self.stream.read_exact(&mut literal).await?;
            literals.push(literal);
            payload.extend_from_slice(format!("{}{{{size}}}\r\n", &text[..open]).as_bytes());
        }
    }

    fn capabilities(&self) -> String {
        advertised(&locked(&self.state)).join(" ")
    }

    fn has(&self, capability: &str) -> bool {
        self.capabilities()
            .split(' ')
            .any(|listed| listed == capability)
    }

    async fn sign_in(
        &mut self,
        tag: &str,
        username: String,
        password: String,
    ) -> std::io::Result<()> {
        let is_accepted = {
            let mut state = locked(&self.state);
            state.sign_ins.push(SignInAttempt {
                username: username.clone(),
                password,
            });
            !state.options.reject_sign_in
                && state
                    .options
                    .imap_username
                    .as_ref()
                    .is_none_or(|accepted| *accepted == username)
        };
        if is_accepted {
            self.is_signed_in = true;
            let capabilities = self.capabilities();
            self.line(&format!("{tag} OK [CAPABILITY {capabilities}] Signed in"))
                .await
        } else {
            self.line(&format!(
                "{tag} NO [AUTHENTICATIONFAILED] Authentication failed."
            ))
            .await
        }
    }

    async fn handle(&mut self, command: Response) -> std::io::Result<bool> {
        let tag = command.tag.clone();
        let name = command.command.to_uppercase();
        let arguments = &command.attributes;
        let text = |index: usize| {
            arguments
                .get(index)
                .and_then(Token::text)
                .unwrap_or_default()
        };

        match name.as_str() {
            "CAPABILITY" => {
                let capabilities = self.capabilities();
                self.line(&format!("* CAPABILITY {capabilities}")).await?;
                self.line(&format!("{tag} OK CAPABILITY completed")).await?;
            }
            "LOGIN" => self.sign_in(&tag, text(0), text(1)).await?,
            "AUTHENTICATE PLAIN" => {
                self.line("+ ").await?;
                let mut answer = String::new();
                self.stream.read_line(&mut answer).await?;
                let decoded = STANDARD.decode(answer.trim()).unwrap_or_default();
                let mut fields = decoded
                    .split(|byte| *byte == 0)
                    .map(|field| String::from_utf8_lossy(field).into_owned());
                let (_, username, password) = (
                    fields.next(),
                    fields.next().unwrap_or_default(),
                    fields.next().unwrap_or_default(),
                );
                self.sign_in(&tag, username, password).await?;
            }
            "LOGOUT" => {
                locked(&self.state).logouts += 1;
                self.line("* BYE Logging out").await?;
                self.line(&format!("{tag} OK LOGOUT completed")).await?;
                return Ok(false);
            }
            _ if !self.is_signed_in => self.line(&format!("{tag} BAD Sign in first")).await?,
            "NOOP" | "ID" => self.line(&format!("{tag} OK {name} completed")).await?,
            "NAMESPACE" => {
                self.line("* NAMESPACE ((\"\" \"/\")) NIL NIL").await?;
                self.line(&format!("{tag} OK NAMESPACE completed")).await?;
            }
            "LIST" => self.list(&tag, arguments).await?,
            "STATUS" => {
                let path = text(0);
                let counts = locked(&self.state)
                    .mailboxes
                    .iter()
                    .find(|mailbox| mailbox.path == path)
                    .map(status_counts);
                match counts {
                    Some(counts) => {
                        self.line(&format!("* STATUS {} {counts}", quoted(&path)))
                            .await?;
                        self.line(&format!("{tag} OK STATUS completed")).await?;
                    }
                    None => {
                        self.line(&format!("{tag} NO [NONEXISTENT] Unknown mailbox"))
                            .await?
                    }
                }
            }
            "SELECT" | "EXAMINE" => self.select(&tag, &text(0), name == "EXAMINE").await?,
            "UID SEARCH" => self.search(&tag, arguments).await?,
            "FETCH" | "UID FETCH" => self.fetch(&tag, arguments, name == "UID FETCH").await?,
            "UID STORE" => self.store(&tag, arguments).await?,
            "UID MOVE" | "UID COPY" => self.transfer(&tag, arguments, name == "UID MOVE").await?,
            "UID EXPUNGE" | "EXPUNGE" => self.expunge(&tag).await?,
            "APPEND" => self.append(&tag, arguments).await?,
            _ => self.line(&format!("{tag} BAD Unknown command")).await?,
        }
        Ok(true)
    }

    async fn list(&mut self, tag: &str, arguments: &[Token]) -> std::io::Result<()> {
        let pattern = arguments.get(1).and_then(Token::text).unwrap_or_default();
        let with_status = arguments
            .get(3)
            .and_then(Token::list)
            .is_some_and(|options| {
                options.iter().any(|option| {
                    option
                        .string_value()
                        .is_some_and(|name| name.eq_ignore_ascii_case("STATUS"))
                })
            });
        if arguments.get(2).is_some() && !self.has("LIST-EXTENDED") && !self.has("LIST-STATUS") {
            return self.line(&format!("{tag} BAD Unknown LIST options")).await;
        }
        let mut lines = Vec::new();
        {
            let state = locked(&self.state);
            for mailbox in state
                .mailboxes
                .iter()
                .filter(|mailbox| pattern == "*" || mailbox.path == pattern)
            {
                let mut flags = mailbox.flags.clone();
                if !flags.iter().any(|flag| flag == "\\Noselect") {
                    flags.push("\\HasNoChildren".to_owned());
                }
                flags.extend(
                    mailbox
                        .special_use
                        .clone()
                        .filter(|_| advertised(&state).contains(&"SPECIAL-USE")),
                );
                lines.push(format!(
                    "* LIST ({}) \"/\" {}",
                    flags.join(" "),
                    quoted(&mailbox.path)
                ));
                if with_status && !mailbox.flags.iter().any(|flag| flag == "\\Noselect") {
                    lines.push(format!(
                        "* STATUS {} {}",
                        quoted(&mailbox.path),
                        status_counts(mailbox)
                    ));
                }
            }
        }
        for line in lines {
            self.line(&line).await?;
        }
        self.line(&format!("{tag} OK LIST completed")).await
    }

    async fn select(&mut self, tag: &str, path: &str, read_only: bool) -> std::io::Result<()> {
        let exists = {
            let mut state = locked(&self.state);
            let found = state
                .mailboxes
                .iter()
                .find(|mailbox| {
                    mailbox.path == path && !mailbox.flags.iter().any(|flag| flag == "\\Noselect")
                })
                .map(|mailbox| mailbox.messages.len());
            if found.is_some() {
                state.selections.push(Selection {
                    path: path.to_owned(),
                    read_only,
                });
            }
            found
        };
        let Some(exists) = exists else {
            self.selected = None;
            return self
                .line(&format!(
                    "{tag} NO [NONEXISTENT] Mailbox does not exist, or must be subscribed to."
                ))
                .await;
        };
        self.selected = Some((path.to_owned(), read_only));
        self.line("* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)")
            .await?;
        let permanent = if read_only {
            ""
        } else {
            "\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*"
        };
        self.line(&format!(
            "* OK [PERMANENTFLAGS ({permanent})] Flags permitted."
        ))
        .await?;
        self.line(&format!("* {exists} EXISTS")).await?;
        self.line("* 0 RECENT").await?;
        self.line("* OK [UIDVALIDITY 1] UIDs valid").await?;
        let (access, command) = if read_only {
            ("READ-ONLY", "EXAMINE")
        } else {
            ("READ-WRITE", "SELECT")
        };
        self.line(&format!("{tag} OK [{access}] {command} completed"))
            .await
    }

    /// The criteria the tests filter on. Dates and body text are only recorded.
    async fn search(&mut self, tag: &str, arguments: &[Token]) -> std::io::Result<()> {
        let words: Vec<String> = arguments
            .iter()
            .map(|token| token.text().unwrap_or_default())
            .collect();
        let Some((path, _)) = self.selected.clone() else {
            return self.line(&format!("{tag} BAD No mailbox selected")).await;
        };
        let found: Vec<String> = {
            let mut state = locked(&self.state);
            state.searches.push(words.join(" "));
            let messages = state
                .mailboxes
                .iter()
                .find(|mailbox| mailbox.path == path)
                .map(|mailbox| mailbox.messages.clone())
                .unwrap_or_default();
            messages
                .iter()
                .filter(|message| matches_search(message, &words))
                .map(|message| message.uid.to_string())
                .collect()
        };
        self.line(format!("* SEARCH {}", found.join(" ")).trim_end())
            .await?;
        self.line(&format!("{tag} OK SEARCH completed")).await
    }

    async fn fetch(&mut self, tag: &str, arguments: &[Token], by_uid: bool) -> std::io::Result<()> {
        let range = arguments.first().and_then(Token::text).unwrap_or_default();
        let items: Vec<Token> = match arguments.get(1) {
            Some(Token::List(items)) => items.clone(),
            Some(item) => vec![item.clone()],
            None => Vec::new(),
        };
        let Some((path, _)) = self.selected.clone() else {
            return self.line(&format!("{tag} BAD No mailbox selected")).await;
        };
        let numbers = sequence_set(&range);

        let mut responses: Vec<Vec<u8>> = Vec::new();
        {
            let mut state = locked(&self.state);
            let content_part = items.iter().find_map(|item| match item {
                Token::Atom {
                    section: Some(section),
                    partial,
                    ..
                } => {
                    let key = section
                        .first()
                        .and_then(Token::string_value)
                        .unwrap_or_default()
                        .to_uppercase();
                    let is_content = key == "TEXT"
                        || (!key.is_empty()
                            && key
                                .chars()
                                .all(|character| character.is_ascii_digit() || character == '.'));
                    is_content.then(|| (key, partial.clone()))
                }
                _ => None,
            });
            match &content_part {
                Some((part, partial))
                    if partial
                        .as_ref()
                        .is_none_or(|partial| partial.first() == Some(&0)) =>
                {
                    state.downloads.push(Download {
                        uid: numbers.first().map_or(0, |(first, _)| *first),
                        part: part.clone(),
                    });
                }
                Some(_) => {}
                None => state.fetches.push(FetchCommand {
                    range: range.clone(),
                    by_uid,
                }),
            }

            let messages = state
                .mailboxes
                .iter()
                .find(|mailbox| mailbox.path == path)
                .map(|mailbox| mailbox.messages.clone())
                .unwrap_or_default();
            for (index, message) in messages.iter().enumerate() {
                let sequence = index as u32 + 1;
                let number = if by_uid { message.uid } else { sequence };
                if !numbers
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&number))
                {
                    continue;
                }
                let mut response = format!("* {sequence} FETCH (").into_bytes();
                let mut is_first = true;
                let mut item = |response: &mut Vec<u8>, name: &str, value: &[u8]| {
                    if !std::mem::replace(&mut is_first, false) {
                        response.push(b' ');
                    }
                    response.extend_from_slice(name.as_bytes());
                    response.push(b' ');
                    response.extend_from_slice(value);
                };
                let literal = |content: &[u8]| {
                    [format!("{{{}}}\r\n", content.len()).as_bytes(), content].concat()
                };
                if !items.iter().any(|item| {
                    item.string_value()
                        .is_some_and(|name| name.eq_ignore_ascii_case("UID"))
                }) {
                    item(&mut response, "UID", message.uid.to_string().as_bytes());
                }
                for requested in &items {
                    let Token::Atom {
                        value: name,
                        section,
                        partial,
                    } = requested
                    else {
                        continue;
                    };
                    match (name.to_uppercase().as_str(), section) {
                        ("UID", None) => {
                            item(&mut response, "UID", message.uid.to_string().as_bytes())
                        }
                        ("FLAGS", None) => item(
                            &mut response,
                            "FLAGS",
                            format!("({})", message.flags.join(" ")).as_bytes(),
                        ),
                        ("ENVELOPE", None) => item(
                            &mut response,
                            "ENVELOPE",
                            envelope(&message.envelope).as_bytes(),
                        ),
                        ("BODYSTRUCTURE", None) => item(
                            &mut response,
                            "BODYSTRUCTURE",
                            body_structure(&message.body_structure).as_bytes(),
                        ),
                        ("INTERNALDATE", None) => {
                            let received = message
                                .envelope
                                .date
                                .unwrap_or_default()
                                .format("\"%d-%b-%Y %H:%M:%S +0000\"")
                                .to_string();
                            item(&mut response, "INTERNALDATE", received.as_bytes());
                        }
                        ("RFC822.SIZE", None) => {
                            let size: usize = message
                                .parts
                                .values()
                                .map(|part| part.len() * 2 + 1024)
                                .sum();
                            item(&mut response, "RFC822.SIZE", size.to_string().as_bytes());
                        }
                        ("BODY.PEEK" | "BODY", Some(section)) => {
                            let key = section
                                .first()
                                .and_then(Token::string_value)
                                .unwrap_or_default()
                                .to_uppercase();
                            let root = &message.body_structure;
                            let content: Option<Vec<u8>> = if key.starts_with("HEADER.FIELDS") {
                                Some(message.references.as_ref().map_or_else(
                                    || b"\r\n".to_vec(),
                                    |references| {
                                        format!("References: {references}\r\n\r\n").into_bytes()
                                    },
                                ))
                            } else if key == "HEADER" {
                                Some(mime_headers(root))
                            } else if key == "TEXT" {
                                message
                                    .parts
                                    .get("1")
                                    .map(|content| encoded_part(root, content))
                            } else if let Some(number) = key.strip_suffix(".MIME") {
                                find_part(root, number).map(mime_headers)
                            } else {
                                find_part(root, &key).and_then(|node| {
                                    message
                                        .parts
                                        .get(&key)
                                        .map(|content| encoded_part(node, content))
                                })
                            };
                            let mut name =
                                format!("BODY[{}]", key.split(' ').next().unwrap_or_default());
                            if key.starts_with("HEADER.FIELDS") {
                                name = "BODY[HEADER.FIELDS (References)]".to_owned();
                            }
                            let content = content.map(|content| match partial.as_deref() {
                                Some([start, length]) => {
                                    name.push_str(&format!("<{start}>"));
                                    let start = usize::try_from(*start)
                                        .unwrap_or(usize::MAX)
                                        .min(content.len());
                                    let end = start
                                        .saturating_add(
                                            usize::try_from(*length).unwrap_or(usize::MAX),
                                        )
                                        .min(content.len());
                                    content[start..end].to_vec()
                                }
                                _ => content,
                            });
                            match content {
                                Some(content) => item(&mut response, &name, &literal(&content)),
                                None => item(&mut response, &name, b"NIL"),
                            }
                        }
                        _ => {}
                    }
                }
                response.extend_from_slice(b")\r\n");
                responses.push(response);
            }
        }
        for response in responses {
            self.send(&response).await?;
        }
        self.line(&format!("{tag} OK FETCH completed")).await
    }

    async fn store(&mut self, tag: &str, arguments: &[Token]) -> std::io::Result<()> {
        let uids = sequence_set(&arguments.first().and_then(Token::text).unwrap_or_default());
        let operation = arguments
            .get(1)
            .and_then(Token::text)
            .unwrap_or_default()
            .to_uppercase();
        let flags = string_list(arguments.get(2));
        let Some((path, read_only)) = self.selected.clone() else {
            return self.line(&format!("{tag} BAD No mailbox selected")).await;
        };
        // A mailbox selected read-only refuses changes, like the real one.
        if read_only {
            return self
                .line(&format!(
                    "{tag} NO [READ-ONLY] The mailbox was selected read-only"
                ))
                .await;
        }
        let mut lines = Vec::new();
        {
            let mut state = locked(&self.state);
            let Some(mailbox) = state
                .mailboxes
                .iter_mut()
                .find(|mailbox| mailbox.path == path)
            else {
                return Ok(());
            };
            for (index, message) in mailbox.messages.iter_mut().enumerate() {
                if !uids
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&message.uid))
                {
                    continue;
                }
                if operation.starts_with('+') {
                    for flag in &flags {
                        if !message.flags.contains(flag) {
                            message.flags.push(flag.clone());
                        }
                    }
                } else {
                    message.flags.retain(|flag| !flags.contains(flag));
                }
                if !operation.ends_with(".SILENT") {
                    lines.push(format!(
                        "* {} FETCH (UID {} FLAGS ({}))",
                        index + 1,
                        message.uid,
                        message.flags.join(" ")
                    ));
                }
            }
        }
        for line in lines {
            self.line(&line).await?;
        }
        self.line(&format!("{tag} OK STORE completed")).await
    }

    async fn transfer(
        &mut self,
        tag: &str,
        arguments: &[Token],
        is_move: bool,
    ) -> std::io::Result<()> {
        let uids = sequence_set(&arguments.first().and_then(Token::text).unwrap_or_default());
        let destination = arguments.get(1).and_then(Token::text).unwrap_or_default();
        let Some((path, read_only)) = self.selected.clone() else {
            return self.line(&format!("{tag} BAD No mailbox selected")).await;
        };
        let moved = {
            let mut state = locked(&self.state);
            let has_destination = state
                .mailboxes
                .iter()
                .any(|mailbox| mailbox.path == destination);
            if !has_destination || (is_move && read_only) {
                None
            } else {
                let source: Vec<FakeMessage> = state
                    .mailboxes
                    .iter()
                    .find(|mailbox| mailbox.path == path)
                    .map(|mailbox| mailbox.messages.clone())
                    .unwrap_or_default();
                let chosen: Vec<(usize, FakeMessage)> = source
                    .into_iter()
                    .enumerate()
                    .filter(|(_, message)| {
                        uids.iter()
                            .any(|(first, last)| (*first..=*last).contains(&message.uid))
                    })
                    .collect();
                let mut pairs = Vec::new();
                if let Some(target) = state
                    .mailboxes
                    .iter_mut()
                    .find(|mailbox| mailbox.path == destination)
                {
                    for (_, message) in &chosen {
                        let uid = target.messages.len() as u32 + 1;
                        pairs.push((message.uid, uid));
                        target.messages.push(FakeMessage {
                            uid,
                            ..message.clone()
                        });
                    }
                }
                if is_move
                    && let Some(mailbox) = state
                        .mailboxes
                        .iter_mut()
                        .find(|mailbox| mailbox.path == path)
                {
                    mailbox
                        .messages
                        .retain(|message| !pairs.iter().any(|(uid, _)| *uid == message.uid));
                }
                Some((
                    pairs,
                    chosen
                        .iter()
                        .map(|(index, _)| index + 1)
                        .collect::<Vec<_>>(),
                ))
            }
        };
        let Some((pairs, sequences)) = moved else {
            return self
                .line(&format!("{tag} NO [TRYCREATE] Mailbox does not exist"))
                .await;
        };
        let join = |numbers: Vec<u32>| {
            numbers
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let code = format!(
            "[COPYUID 1 {} {}]",
            join(pairs.iter().map(|(uid, _)| *uid).collect()),
            join(pairs.iter().map(|(_, uid)| *uid).collect())
        );
        if !is_move {
            return self.line(&format!("{tag} OK {code} COPY completed")).await;
        }
        if !pairs.is_empty() {
            self.line(&format!("* OK {code} Moved")).await?;
        }
        for sequence in sequences.iter().rev() {
            self.line(&format!("* {sequence} EXPUNGE")).await?;
        }
        self.line(&format!("{tag} OK MOVE completed")).await
    }

    async fn expunge(&mut self, tag: &str) -> std::io::Result<()> {
        if let Some((path, false)) = self.selected.clone() {
            let mut state = locked(&self.state);
            if let Some(mailbox) = state
                .mailboxes
                .iter_mut()
                .find(|mailbox| mailbox.path == path)
            {
                mailbox
                    .messages
                    .retain(|message| !message.flags.iter().any(|flag| flag == "\\Deleted"));
            }
        }
        self.line(&format!("{tag} OK EXPUNGE completed")).await
    }

    async fn append(&mut self, tag: &str, arguments: &[Token]) -> std::io::Result<()> {
        let path = arguments.first().and_then(Token::text).unwrap_or_default();
        let flags = arguments
            .iter()
            .find_map(|argument| argument.list().map(|_| string_list(Some(argument))))
            .unwrap_or_default();
        let raw = arguments.iter().find_map(|argument| match argument {
            Token::Literal(raw) => Some(String::from_utf8_lossy(raw).into_owned()),
            _ => None,
        });
        let uid = {
            let mut state = locked(&self.state);
            let fails = state.options.fail_append;
            match (
                state
                    .mailboxes
                    .iter_mut()
                    .find(|mailbox| mailbox.path == path),
                raw,
            ) {
                (Some(mailbox), Some(raw)) if !fails => {
                    mailbox.appended.push(AppendedMessage { raw, flags });
                    Some(100 + mailbox.appended.len())
                }
                _ => None,
            }
        };
        match uid {
            Some(uid) if self.has("UIDPLUS") => {
                self.line(&format!("{tag} OK [APPENDUID 1 {uid}] APPEND completed"))
                    .await
            }
            Some(_) => self.line(&format!("{tag} OK APPEND completed")).await,
            None => self.line(&format!("{tag} NO APPEND failed")).await,
        }
    }
}

fn advertised(state: &State) -> Vec<&'static str> {
    state
        .options
        .capabilities
        .clone()
        .unwrap_or_else(|| CAPABILITIES.to_vec())
}

fn status_counts(mailbox: &FakeMailbox) -> String {
    let unseen = mailbox
        .messages
        .iter()
        .filter(|message| !message.flags.iter().any(|flag| flag == "\\Seen"))
        .count();
    format!("(MESSAGES {} UNSEEN {unseen})", mailbox.messages.len())
}

/// `11,14:16` as ranges. `*` is the largest number there is.
fn sequence_set(set: &str) -> Vec<(u32, u32)> {
    let number = |text: &str| {
        if text == "*" {
            Some(u32::MAX)
        } else {
            text.parse().ok()
        }
    };
    set.split(',')
        .filter_map(|part| match part.split_once(':') {
            Some((first, last)) => {
                let (first, last) = (number(first)?, number(last)?);
                Some((first.min(last), first.max(last)))
            }
            None => number(part).map(|single| (single, single)),
        })
        .collect()
}

fn matches_search(message: &FakeMessage, words: &[String]) -> bool {
    let has_flag = |flag: &str| message.flags.iter().any(|set| set == flag);
    let contains = |text: &str, wanted: &str| text.to_lowercase().contains(&wanted.to_lowercase());
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let is_match = match word.to_uppercase().as_str() {
            "UID" => words.next().is_some_and(|set| {
                sequence_set(set)
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&message.uid))
            }),
            "SEEN" => has_flag("\\Seen"),
            "UNSEEN" => !has_flag("\\Seen"),
            "FLAGGED" => has_flag("\\Flagged"),
            "UNFLAGGED" => !has_flag("\\Flagged"),
            "SUBJECT" => words.next().is_some_and(|wanted| {
                contains(
                    message.envelope.subject.as_deref().unwrap_or_default(),
                    wanted,
                )
            }),
            "FROM" => words.next().is_some_and(|wanted| {
                let senders: Vec<String> = message
                    .envelope
                    .from
                    .iter()
                    .map(|address| format!("{} {}", address.name, address.address))
                    .collect();
                contains(&senders.join(" "), wanted)
            }),
            "TO" | "TEXT" | "SINCE" | "BEFORE" | "YOUNGER" | "OLDER" | "CHARSET" => {
                words.next();
                true
            }
            _ => true,
        };
        if !is_match {
            return false;
        }
    }
    true
}

async fn serve_imap(stream: TcpStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let mut connection = ImapConnection {
        stream: BufReader::new(stream),
        state,
        is_signed_in: false,
        selected: None,
    };
    let capabilities = connection.capabilities();
    connection
        .line(&format!(
            "* OK [CAPABILITY {capabilities}] Fake iCloud ready"
        ))
        .await?;
    while let Some((payload, literals)) = connection.command().await? {
        if payload.is_empty() {
            continue;
        }
        let Ok(command) = parse_response(&payload, literals) else {
            connection.line("* BAD Could not parse the command").await?;
            continue;
        };
        if !command.command.to_uppercase().starts_with("LOGIN")
            && !command.command.to_uppercase().starts_with("AUTHENTICATE")
        {
            let line = String::from_utf8_lossy(&payload);
            let without_tag = line.split_once(' ').map_or("", |(_, rest)| rest).to_owned();
            locked(&connection.state).commands.push(without_tag);
        }
        if !connection.handle(command).await? {
            break;
        }
    }
    Ok(())
}

async fn smtp_reply(stream: &mut BufReader<TcpStream>, text: String) -> std::io::Result<()> {
    stream
        .get_mut()
        .write_all(format!("{text}\r\n").as_bytes())
        .await?;
    stream.get_mut().flush().await
}

async fn serve_smtp(
    stream: TcpStream,
    state: Arc<Mutex<State>>,
    mut released: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let behaviour = locked(&state).options.smtp.clone();
    smtp_reply(&mut stream, "220 fake.mail.me.com ESMTP ready".to_owned()).await?;

    let mut from = String::new();
    let mut to: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let line = line.trim_end();
        let upper = line.to_uppercase();
        if upper.starts_with("EHLO") {
            smtp_reply(&mut stream, "250-fake.mail.me.com\r\n250-PIPELINING\r\n250-SIZE 28319744\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SMTPUTF8".to_owned()).await?;
        } else if let Some(answer) = line.strip_prefix("AUTH PLAIN ") {
            let decoded = STANDARD.decode(answer).unwrap_or_default();
            let mut fields = decoded
                .split(|byte| *byte == 0)
                .map(|field| String::from_utf8_lossy(field).into_owned());
            let (_, username, password) = (
                fields.next(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            );
            locked(&state)
                .smtp_sign_ins
                .push(SignInAttempt { username, password });
            if behaviour == SmtpBehaviour::RejectSignIn {
                smtp_reply(
                    &mut stream,
                    "535 5.7.8 Error: authentication failed".to_owned(),
                )
                .await?;
            } else {
                smtp_reply(
                    &mut stream,
                    "235 2.7.0 Authentication successful".to_owned(),
                )
                .await?;
            }
        } else if let Some(sender) = upper.starts_with("MAIL FROM:").then(|| &line[10..]) {
            from = sender
                .split('>')
                .next()
                .unwrap_or_default()
                .trim_start_matches('<')
                .to_owned();
            to.clear();
            smtp_reply(&mut stream, "250 2.1.0 Ok".to_owned()).await?;
        } else if let Some(recipient) = upper.starts_with("RCPT TO:").then(|| &line[8..]) {
            let recipient = recipient
                .split('>')
                .next()
                .unwrap_or_default()
                .trim_start_matches('<')
                .to_owned();
            let refusal = match &behaviour {
                SmtpBehaviour::RejectAllRecipients { reply } => Some(reply.clone()),
                SmtpBehaviour::RejectRecipients { recipients, reply }
                    if recipients.contains(&recipient) =>
                {
                    Some(reply.clone())
                }
                _ => None,
            };
            match refusal {
                Some(refusal) => smtp_reply(&mut stream, refusal).await?,
                None => {
                    to.push(recipient);
                    smtp_reply(&mut stream, "250 2.1.5 Ok".to_owned()).await?;
                }
            }
        } else if upper == "DATA" {
            smtp_reply(
                &mut stream,
                "354 End data with <CR><LF>.<CR><LF>".to_owned(),
            )
            .await?;
            let mut raw = Vec::new();
            loop {
                let mut line = Vec::new();
                if stream.read_until(b'\n', &mut line).await? == 0 {
                    return Ok(());
                }
                if line == b".\r\n" {
                    break;
                }
                // A line that starts with a dot was sent with one more.
                raw.extend_from_slice(line.strip_prefix(b".").unwrap_or(&line));
            }
            locked(&state).deliveries_started += 1;
            match &behaviour {
                SmtpBehaviour::HangUp => return Ok(()),
                SmtpBehaviour::RejectMessage { reply: refusal } => {
                    smtp_reply(&mut stream, refusal.clone()).await?;
                    continue;
                }
                SmtpBehaviour::Hold
                    if released.wait_for(|is_released| *is_released).await.is_err() =>
                {
                    return Ok(());
                }
                _ => {}
            }
            locked(&state).deliveries.push(Delivery {
                from: from.clone(),
                to: to.clone(),
                raw: String::from_utf8_lossy(&raw).into_owned(),
            });
            smtp_reply(&mut stream, "250 2.0.0 Ok: queued".to_owned()).await?;
        } else if upper == "QUIT" {
            smtp_reply(&mut stream, "221 2.0.0 Bye".to_owned()).await?;
            return Ok(());
        } else {
            smtp_reply(&mut stream, "250 Ok".to_owned()).await?;
        }
    }
}
