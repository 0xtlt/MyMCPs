//! Vine schemas for the built-in iCloud Mail MCP: the arguments of its tools,
//! and what its attachment and upload links refer to. A schema lists the
//! arguments in the order they are checked: of several wrong ones, the agent
//! is told about the first.
//!
//! The port of `app/validators/builtin_icloud_mail.ts`.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use mymcps_builtin::BuiltinPasswordContext;
use mymcps_builtin::arguments::{
    TOOL_VINE, VineArgument, argument, blank_as_missing, boolean, integer, is_blank, iso_date,
    line, list_length, media_type, pattern, text, uploaded_file_name,
};
use mymcps_builtin::upload_store::is_builtin_upload_id;
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Value, json};
use vine::{Rule, Schema, Validator, VineArray};

use crate::message::is_address;

/// The bounds the tools advertise, and the schemas below enforce.
pub mod limits {
    pub const PAGE_SIZE: i64 = 50;
    pub const TEXT_CHARS: usize = 100_000;
    pub const UIDS: usize = 100;
    pub const RECIPIENTS: usize = 50;
    pub const SUBJECT_LENGTH: usize = 255;
    pub const LINK_MINUTES: i64 = 60;
    pub const ATTACHMENTS: usize = 10;
    /// Apple carries 20 MB of files in a message. Encoded for mail they take a
    /// third more, which fits in the 27 MB its server accepts.
    pub const ATTACHMENT_BYTES: u64 = 20_000_000;
    pub const FILENAME_LENGTH: usize = 255;
}

const MAX_UID: i64 = 4_294_967_295;
const MAX_MAILBOX_LENGTH: usize = 255;
const MAX_SEARCH_LENGTH: usize = 200;

/// Left out, it is the inbox.
fn mailbox() -> VineArgument {
    line(MAX_MAILBOX_LENGTH).optional()
}

fn uid() -> VineArgument {
    integer(1..=MAX_UID)
}

/// Anything but a list counts as an empty one, which gets the same answer.
fn uids() -> VineArray {
    vine::array(uid())
        .parse(|value, _| match value {
            Some(Value::Array(list)) => Some(Value::Array(list)),
            _ => Some(json!([])),
        })
        .use_rule(list_length(
            1..=limits::UIDS,
            &format!(
                "{{{{ field }}}} must be a list of 1 to {} message UIDs",
                limits::UIDS
            ),
        ))
}

fn part() -> VineArgument {
    pattern(
        r"^\d{1,3}(\.\d{1,3}){0,9}$",
        "the part of an attachment, such as 2, as returned by get_message",
    )
}

fn search_text() -> VineArgument {
    line(MAX_SEARCH_LENGTH).optional()
}

/// What the recipient sees the file as. It never names a file on the instance.
fn file_name() -> VineArgument {
    uploaded_file_name(limits::FILENAME_LENGTH)
}

fn attachments_sentence() -> String {
    format!(
        "{{{{ field }}}} must be a list of at most {} upload IDs, as returned by create_upload_link",
        limits::ATTACHMENTS
    )
}

fn json_schema_string(rule: Rule) -> Rule {
    rule.json_schema(|schema| {
        schema.insert("type".to_owned(), json!("string"));
    })
}

fn upload_id_rule() -> Rule {
    let sentence = attachments_sentence();
    json_schema_string(vine::rule(move |value, field| {
        if !value.as_str().is_some_and(is_builtin_upload_id) {
            field.report(&sentence, "uploadId");
        }
    }))
}

/// One uploaded file. What is not text is refused like a wrong ID.
fn upload_id() -> VineArgument {
    argument(upload_id_rule()).parse(|value, _| match value {
        Some(Value::String(written)) => {
            Some(Value::String(vine::js::trim(&written).to_lowercase()))
        }
        _ => Some(Value::String(String::new())),
    })
}

/// A single item is a list of one.
fn one_or_many(value: Option<Value>) -> Option<Value> {
    match value {
        value if is_blank(value.as_ref()) => None,
        Some(Value::Array(list)) => Some(Value::Array(list)),
        Some(single) => Some(Value::Array(vec![single])),
        None => None,
    }
}

/// A single ID is a list of one.
fn attachments() -> VineArray {
    vine::array(upload_id())
        .parse(|value, _| one_or_many(value))
        .use_rule(list_length(..=limits::ATTACHMENTS, &attachments_sentence()))
        .optional()
}

fn addresses_sentence() -> String {
    format!(
        "{{{{ field }}}} must be a list of at most {} email addresses such as name@example.com, without display names",
        limits::RECIPIENTS
    )
}

fn address_rule() -> Rule {
    let sentence = addresses_sentence();
    json_schema_string(vine::rule(move |value, field| {
        if !value.as_str().is_some_and(is_address) {
            field.report(&sentence, "address");
        }
    }))
}

/// One address of a list. What is not text is refused like a wrong address.
fn address() -> VineArgument {
    argument(address_rule()).parse(|value, _| match value {
        Some(Value::String(written)) => Some(Value::String(vine::js::trim(&written).to_owned())),
        _ => Some(Value::String(String::new())),
    })
}

/// A single address is a list of one.
fn addresses() -> VineArray {
    vine::array(address())
        .parse(|value, _| one_or_many(value))
        .use_rule(list_length(..=limits::RECIPIENTS, &addresses_sentence()))
        .optional()
}

/// The addresses a message may be sent from. The tools validate with their
/// sign-in as the metadata; anything else that validates a message to write
/// passes this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Senders {
    pub username: String,
    pub aliases: Vec<String>,
}

/// One of the account's own addresses, in the spelling the administrator saved.
fn own_address_rule() -> Rule {
    vine::rule(|value, field| {
        let allowed: Vec<&str> = match (
            field.meta::<BuiltinPasswordContext>(),
            field.meta::<Senders>(),
        ) {
            (Some(sign_in), _) => std::iter::once(sign_in.username.as_str())
                .chain(sign_in.aliases.iter().map(String::as_str))
                .collect(),
            (None, Some(senders)) => std::iter::once(senders.username.as_str())
                .chain(senders.aliases.iter().map(String::as_str))
                .collect(),
            (None, None) => Vec::new(),
        };
        let wanted = value.as_str().unwrap_or_default().to_lowercase();
        match allowed.iter().find(|candidate| candidate.to_lowercase() == wanted) {
            Some(saved) => field.mutate(*saved),
            None => field.report_with(
                "{{ field }} must be one of the sender addresses allowed for this MCP: {{ allowed }}",
                "ownAddress",
                json!({ "allowed": allowed.join(", ") }),
            ),
        }
    })
}

/// For an argument that may only be left out when `other` is given.
fn required_unless_rule(other: &'static str, sentence: Option<&'static str>) -> Rule {
    vine::rule(move |value, field| {
        let is_missing = field.is_undefined() || is_blank(Some(value));
        if is_missing && is_blank(field.parent_get(other)) {
            field.report_with(
                sentence.unwrap_or("{{ field }} is required unless {{ other }} is set"),
                "requiredUnless",
                json!({ "other": other }),
            );
        }
    })
    .implicit()
}

/// An argument that says more about `other`, and is not looked at without it.
fn only_with(other: &'static str, schema: impl Into<Schema>) -> Schema {
    let schema = schema.into();
    let parse = schema.parser();
    schema.parse(move |value, context| {
        if is_blank(context.parent_get(other)) {
            return None;
        }
        match &parse {
            Some(parse) => parse.call(value, context),
            None => value,
        }
    })
}

pub static LIST_MESSAGES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "page" => integer(1..).optional(),
        "per_page" => integer(1..=limits::PAGE_SIZE).optional(),
        "from" => search_text(),
        "to" => search_text(),
        "subject" => search_text(),
        "text" => search_text(),
        "since" => iso_date().optional(),
        "before" => iso_date().optional(),
        "unread" => boolean().optional(),
        "flagged" => boolean().optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ListMessages {
    pub mailbox: Option<String>,
    pub page: Option<u64>,
    pub per_page: Option<u64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub text: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub unread: Option<bool>,
    pub flagged: Option<bool>,
}

pub static GET_MESSAGE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "uid" => uid(),
        "max_chars" => integer(500..=limits::TEXT_CHARS as i64).optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GetMessage {
    pub mailbox: Option<String>,
    pub uid: u32,
    pub max_chars: Option<usize>,
}

pub static GET_ATTACHMENT_LINK_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "uid" => uid(),
        "part" => part(),
        "expires_in_minutes" => integer(1..=limits::LINK_MINUTES).optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GetAttachmentLink {
    pub mailbox: Option<String>,
    pub uid: u32,
    pub part: String,
    pub expires_in_minutes: Option<u64>,
}

/// What get_attachment_link puts in a link, and gets back when the link is opened.
pub static ATTACHMENT_REFERENCE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "uid" => uid(),
        "part" => part(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AttachmentReference {
    pub mailbox: Option<String>,
    pub uid: u32,
    pub part: String,
}

pub static CREATE_UPLOAD_LINK_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "filename" => file_name(),
        "content_type" => media_type().optional(),
        "expires_in_minutes" => integer(1..=limits::LINK_MINUTES).optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CreateUploadLink {
    pub filename: String,
    pub content_type: Option<String>,
    pub expires_in_minutes: Option<u64>,
}

/// What create_upload_link puts in a link, and gets back when a file is sent to it.
pub static UPLOAD_REFERENCE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "upload" => upload_id(),
        "filename" => file_name(),
        "content_type" => media_type().optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UploadReference {
    pub upload: String,
    pub filename: String,
    pub content_type: Option<String>,
}

/// A message to draft or to send. The sign-in says which addresses it may come from.
pub static COMPOSITION_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "reply_to_uid" => uid().optional(),
        "subject" => line(limits::SUBJECT_LENGTH).optional().use_rule(required_unless_rule("reply_to_uid", None)),
        "from" => line(254).use_rule(own_address_rule()).optional(),
        "to" => addresses(),
        "cc" => addresses(),
        "bcc" => addresses(),
        "text" => text(limits::TEXT_CHARS).parse(blank_as_missing),
        "attachments" => attachments(),
        "reply_to_mailbox" => only_with("reply_to_uid", line(MAX_MAILBOX_LENGTH)).optional(),
        "reply_all" => only_with("reply_to_uid", boolean()).optional(),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Composition {
    pub reply_to_uid: Option<u32>,
    pub subject: Option<String>,
    pub from: Option<String>,
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub text: String,
    pub attachments: Option<Vec<String>>,
    pub reply_to_mailbox: Option<String>,
    pub reply_all: Option<bool>,
}

pub static MARK_MESSAGES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "uids" => uids(),
        "unread" => boolean().optional(),
        "flagged" => boolean().optional().use_rule(required_unless_rule("unread", Some("Set unread, flagged, or both"))),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MarkMessages {
    pub mailbox: Option<String>,
    pub uids: Vec<u32>,
    pub unread: Option<bool>,
    pub flagged: Option<bool>,
}

pub static MOVE_MESSAGES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => mailbox(),
        "uids" => uids(),
        "destination" => line(MAX_MAILBOX_LENGTH),
    })
});

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MoveMessages {
    pub mailbox: Option<String>,
    pub uids: Vec<u32>,
    pub destination: String,
}
