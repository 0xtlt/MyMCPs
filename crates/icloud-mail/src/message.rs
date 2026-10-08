//! What a message read over IMAP looks like, and what of it is told to an
//! agent: the port of `app/services/builtin/icloud_mail/message.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use mymcps_builtin::arguments::to_iso;
use mymcps_vine::js::{self, JsRegex};
use serde_json::{Map, Value, json};

const MAX_REFERENCES: usize = 20;

/// How much of what a sender controls is repeated to the agent, so that one
/// message cannot fill its context. A field that was cut is flagged with
/// `<field>_truncated`.
const MAX_LISTED_ADDRESSES: usize = 50;
const MAX_ADDRESS_CHARS: usize = 320;
/// RFC 5322 allows 998 characters on a line.
const MAX_HEADER_CHARS: usize = 998;
const MAX_NAME_CHARS: usize = 255;
pub const MAX_LISTED_ATTACHMENTS: usize = 100;

/// One bare address. Display names and the characters an address parser could
/// read as a second recipient are refused. The domain is matched up to its
/// first dot that is not its first character, so that there is one way to read
/// it: a message can name thousands of addresses written to be slow to refuse.
static ADDRESS_PATTERN: LazyLock<JsRegex> = LazyLock::new(|| {
    js::regex(
        r#"^[^\s@<>(),;:"\\[\]]+@[^\s@<>(),;:"\\[\]][^\s@<>(),;:"\\[\].]*\.[^\s@<>(),;:"\\[\]]+$"#,
        "",
    )
    .expect("static regex")
});

/// Invisible characters newsletters repeat to pad the preview line of a mail
/// client: three or more, with spaces in between. What follows the third is
/// matched by a single class, which keeps a run of any length off the regex stack.
static PREVIEW_PADDING: LazyLock<JsRegex> = LazyLock::new(|| {
    js::regex(
        r"(?:[\u00ad\u034f\u200b-\u200d\u2007\ufeff][ \u00a0]*){3}[\u00ad\u034f\u200b-\u200d\u2007\ufeff \u00a0]*",
        "",
    )
    .expect("static regex")
});

static MESSAGE_IDS: LazyLock<JsRegex> =
    LazyLock::new(|| js::regex(r"<[^<>\s]+>", "").expect("static regex"));

/// One address of a header, as an IMAP envelope gives it. An empty `address`
/// is the name of a group, and an empty `name` an address written bare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub address: String,
}

impl Address {
    pub fn new(name: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            address: address.into(),
        }
    }

    pub fn bare(address: impl Into<String>) -> Self {
        Self {
            name: String::new(),
            address: address.into(),
        }
    }
}

/// The headers an IMAP server parsed out of a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Envelope {
    /// `None` when the header is missing or is not a date.
    pub date: Option<DateTime<Utc>>,
    pub subject: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub from: Vec<Address>,
    pub sender: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
}

/// One node of the MIME structure of a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyStructure {
    /// The part number, such as `1.2`. The root of a message has none.
    pub part: Option<String>,
    /// The media type in lower case, such as `text/plain`.
    pub media_type: String,
    pub parameters: HashMap<String, String>,
    pub encoding: Option<String>,
    pub size: Option<u64>,
    pub disposition: Option<String>,
    pub disposition_parameters: HashMap<String, String>,
    pub child_nodes: Option<Vec<BodyStructure>>,
}

/// What a FETCH returned for one message. Only what was asked for is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchedMessage {
    pub uid: u32,
    pub flags: HashSet<String>,
    pub envelope: Option<Envelope>,
    pub body_structure: Option<BodyStructure>,
    pub internal_date: Option<DateTime<Utc>>,
    /// The header fields asked for, as the server sent them.
    pub headers: Option<Vec<u8>>,
}

pub fn is_address(value: &str) -> bool {
    js::utf16_len(value) <= 254 && ADDRESS_PATTERN.test(value)
}

/// Keep the first spelling of each address.
pub fn unique_addresses(addresses: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    addresses
        .into_iter()
        .filter(|address| seen.insert(address.to_lowercase()))
        .collect()
}

fn format_address(Address { name, address }: &Address) -> String {
    if address.is_empty() {
        return name.clone();
    }
    if !name.is_empty() && name != address {
        format!("{name} <{address}>")
    } else {
        address.clone()
    }
}

/// `text.slice(0, max)`, where `max` counts UTF-16 code units as JavaScript
/// does. A character that would be cut in two is left out.
pub(crate) fn utf16_prefix(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

/// Sender-controlled text, cut to `max` characters.
fn clip(text: &str, max: usize) -> String {
    if js::utf16_len(text) > max {
        format!("{}…", utf16_prefix(text, max))
    } else {
        text.to_owned()
    }
}

/// `<field>_truncated` for each field that was cut.
fn truncated(fields: &[(&str, bool)], into: &mut Map<String, Value>) {
    for (field, is_cut) in fields {
        if *is_cut {
            into.insert(format!("{field}_truncated"), Value::Bool(true));
        }
    }
}

struct ListedAddresses {
    listed: Vec<String>,
    is_cut: bool,
}

/// The addresses of one header, and whether some were left out or shortened.
fn list_addresses(list: &[Address]) -> ListedAddresses {
    let all: Vec<String> = list
        .iter()
        .map(format_address)
        .filter(|address| !address.is_empty())
        .collect();
    let listed: Vec<String> = all
        .iter()
        .take(MAX_LISTED_ADDRESSES)
        .map(|address| clip(address, MAX_ADDRESS_CHARS))
        .collect();
    let is_cut = all.len() > listed.len()
        || listed
            .iter()
            .zip(&all)
            .any(|(listed, whole)| listed != whole);
    ListedAddresses { listed, is_cut }
}

/// Every part that carries content of its own. An attached message counts as
/// one part: its inner parts are not this message's body.
fn content_parts(node: &BodyStructure) -> Vec<&BodyStructure> {
    if node.media_type.starts_with("multipart/") {
        node.child_nodes
            .iter()
            .flatten()
            .flat_map(content_parts)
            .collect()
    } else {
        vec![node]
    }
}

/// `None` stands for `undefined`: a name that is there but empty is `Some("")`.
fn filename_of(part: &BodyStructure) -> Option<&str> {
    part.disposition_parameters
        .get("filename")
        .or_else(|| part.parameters.get("name"))
        .map(String::as_str)
}

fn is_attachment(part: &BodyStructure) -> bool {
    part.disposition.as_deref() == Some("attachment")
        || filename_of(part).is_some_and(|name| !name.is_empty())
}

/// The part to read as the text of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPart {
    pub id: String,
    pub is_html: bool,
}

/// The part to read as the message text: plain text when there is one, else HTML.
pub fn body_part(structure: &BodyStructure) -> Option<BodyPart> {
    let texts: Vec<&BodyStructure> = content_parts(structure)
        .into_iter()
        .filter(|part| !is_attachment(part))
        .collect();
    let part = texts
        .iter()
        .find(|part| part.media_type == "text/plain")
        .or_else(|| texts.iter().find(|part| part.media_type == "text/html"))?;
    // A message made of a single part has no part number: its body is part 1.
    Some(BodyPart {
        id: part.part.clone().unwrap_or_else(|| "1".to_owned()),
        is_html: part.media_type == "text/html",
    })
}

/// Base64 spends 78 bytes, line break included, on every 57 bytes of the file.
fn file_size(part: &BodyStructure) -> u64 {
    let size = part.size.unwrap_or(0);
    if part.encoding.as_deref() == Some("base64") {
        // `Math.round`, which rounds a half up.
        ((size as f64 * 57.0) / 78.0 + 0.5).floor() as u64
    } else {
        size
    }
}

/// An attachment as `get_message` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub part: String,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
}

impl Attachment {
    pub fn to_json(&self) -> Value {
        json!({
            "part": self.part,
            "filename": self.filename,
            "content_type": self.content_type,
            "size": self.size,
        })
    }
}

/// `part` identifies an attachment within its message. `size` is approximate.
pub fn attachments_of(structure: Option<&BodyStructure>) -> Vec<Attachment> {
    structure
        .map(content_parts)
        .unwrap_or_default()
        .into_iter()
        .filter(|part| is_attachment(part))
        .map(|part| Attachment {
            part: part.part.clone().unwrap_or_else(|| "1".to_owned()),
            filename: clip(filename_of(part).unwrap_or("untitled"), MAX_NAME_CHARS),
            content_type: clip(&part.media_type, MAX_NAME_CHARS),
            size: file_size(part),
        })
        .collect()
}

/// What an agent needs to pick a message out of a list.
pub fn message_summary(message: &FetchedMessage) -> Map<String, Value> {
    summary(message, true)
}

fn summary(message: &FetchedMessage, with_attachments: bool) -> Map<String, Value> {
    let empty = Envelope::default();
    let envelope = message.envelope.as_ref().unwrap_or(&empty);
    let subject = envelope.subject.as_deref().unwrap_or("");
    let from = list_addresses(&envelope.from);
    let to = list_addresses(&envelope.to);

    let mut summary = Map::new();
    summary.insert("uid".into(), json!(message.uid));
    summary.insert("subject".into(), json!(clip(subject, MAX_HEADER_CHARS)));
    summary.insert("from".into(), json!(from.listed.join(", ")));
    summary.insert("to".into(), json!(to.listed));
    if let Some(date) = envelope.date.or(message.internal_date) {
        summary.insert("date".into(), json!(to_iso(date)));
    }
    summary.insert("unread".into(), json!(!message.flags.contains("\\Seen")));
    summary.insert("flagged".into(), json!(message.flags.contains("\\Flagged")));
    summary.insert(
        "answered".into(),
        json!(message.flags.contains("\\Answered")),
    );
    if with_attachments {
        summary.insert(
            "attachments".into(),
            json!(attachments_of(message.body_structure.as_ref()).len()),
        );
    }
    truncated(
        &[
            ("subject", js::utf16_len(subject) > MAX_HEADER_CHARS),
            ("from", from.is_cut),
            ("to", to.is_cut),
        ],
        &mut summary,
    );
    summary
}

/// The headers of one message, without the fields that are empty.
pub fn message_headers(message: &FetchedMessage) -> Map<String, Value> {
    let empty = Envelope::default();
    let envelope = message.envelope.as_ref().unwrap_or(&empty);
    let mut headers = summary(message, false);
    let reply_addresses = list_addresses(&envelope.reply_to);
    let reply_address = reply_addresses.listed.join(", ");
    let has_reply_address = !reply_address.is_empty()
        && headers.get("from").and_then(Value::as_str) != Some(&reply_address);
    let cc = list_addresses(&envelope.cc);
    let bcc = list_addresses(&envelope.bcc);
    let message_id = envelope.message_id.as_deref().unwrap_or("");

    if has_reply_address {
        headers.insert("reply_to".into(), json!(reply_address));
    }
    if !cc.listed.is_empty() {
        headers.insert("cc".into(), json!(cc.listed));
    }
    if !bcc.listed.is_empty() {
        headers.insert("bcc".into(), json!(bcc.listed));
    }
    if !message_id.is_empty() {
        headers.insert(
            "message_id".into(),
            json!(clip(message_id, MAX_HEADER_CHARS)),
        );
    }
    truncated(
        &[
            ("reply_to", has_reply_address && reply_addresses.is_cut),
            ("cc", cc.is_cut),
            ("bcc", bcc.is_cut),
            ("message_id", js::utf16_len(message_id) > MAX_HEADER_CHARS),
        ],
        &mut headers,
    );
    headers
}

/// `text.replace(/(?<![ \t])[ \t]+\n/g, '\n')`: drop the spaces and tabs that
/// end a line. Each run is looked at once, from its start.
fn without_trailing_blanks(text: &str) -> String {
    let mut tidy = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find([' ', '\t']) {
        tidy.push_str(&rest[..start]);
        let run = &rest[start..];
        let end = run
            .find(|character| character != ' ' && character != '\t')
            .unwrap_or(run.len());
        if !run[end..].starts_with('\n') {
            tidy.push_str(&run[..end]);
        }
        rest = &run[end..];
    }
    tidy.push_str(rest);
    tidy
}

/// `text.replace(/\n{3,}/g, '\n\n')`
fn without_blank_runs(text: &str) -> String {
    let mut tidy = String::with_capacity(text.len());
    let mut newlines = 0;
    for character in text.chars() {
        if character == '\n' {
            newlines += 1;
            if newlines > 2 {
                continue;
            }
        } else {
            newlines = 0;
        }
        tidy.push(character);
    }
    tidy
}

/// Normalize line endings and drop the padding and blank runs that only cost
/// tokens. The text is written by a stranger, so every step must stay linear.
pub fn tidy_text(text: &str) -> String {
    // `/\r\n?/g`
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = PREVIEW_PADDING.as_regex().replace_all(&text, "");
    let text = without_blank_runs(&without_trailing_blanks(&text));
    js::trim(&text).to_owned()
}

/// Sender, recipients, subject, and threading headers of a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub from: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// Latin-1 gives every byte a character, so nothing of a header written in
/// another charset is lost or read as two addresses.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

/// Sender, recipients, subject, and threading headers of a reply to
/// `original`, which must be fetched with its `References` header: the
/// envelope does not carry it. `own_addresses` are the account's own.
pub fn reply_to(original: &FetchedMessage, own_addresses: &[String], reply_all: bool) -> Reply {
    let empty = Envelope::default();
    let envelope = original.envelope.as_ref().unwrap_or(&empty);
    let owned: Vec<(String, &String)> = own_addresses
        .iter()
        .map(|address| (address.to_lowercase(), address))
        .collect();
    let own = |address: &str| {
        let wanted = address.to_lowercase();
        owned
            .iter()
            .find(|(candidate, _)| *candidate == wanted)
            .map(|(_, saved)| (*saved).clone())
    };
    let is_own = |address: &String| own(address).is_some();
    // These come from the message itself, so they are checked like agent input.
    let addresses = |list: &mut dyn Iterator<Item = &Address>| {
        unique_addresses(
            list.map(|entry| entry.address.clone())
                .filter(|address| is_address(address))
                .collect(),
        )
    };

    let author = addresses(&mut if envelope.reply_to.is_empty() {
        envelope.from.iter()
    } else {
        envelope.reply_to.iter()
    });
    // Replying to a message you sent continues with the people it was sent to.
    let to = if author.iter().all(is_own) {
        addresses(&mut envelope.to.iter())
    } else {
        author.clone()
    };
    let recipients = addresses(&mut envelope.to.iter().chain(&envelope.cc));
    // A set, because the sender decides how long both lists are.
    let addressed: HashSet<String> = to.iter().map(|address| address.to_lowercase()).collect();
    let cc = if reply_all {
        recipients
            .iter()
            .filter(|address| !is_own(address) && !addressed.contains(&address.to_lowercase()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let subject = envelope.subject.as_deref().unwrap_or("");
    let is_reply = subject
        .get(..3)
        .is_some_and(|start| start.eq_ignore_ascii_case("re:"));
    let headers = original.headers.as_deref().map(latin1).unwrap_or_default();
    let message_id = envelope.message_id.clone().filter(|id| !id.is_empty());
    let mut references: Vec<String> = MESSAGE_IDS
        .as_regex()
        .find_iter(&headers)
        .map(|id| id.as_str().to_owned())
        .chain(message_id.clone())
        .collect();
    if references.len() > MAX_REFERENCES {
        references.drain(..references.len() - MAX_REFERENCES);
    }

    Reply {
        // Answer from the address the message was written to, like a mail app,
        // and follow up on a sent message from the address that sent it.
        from: recipients
            .iter()
            .chain(&author)
            .find_map(|address| own(address)),
        to,
        cc,
        subject: if is_reply {
            subject.to_owned()
        } else {
            js::trim(&format!("Re: {subject}")).to_owned()
        },
        in_reply_to: envelope.message_id.clone(),
        references,
    }
}
