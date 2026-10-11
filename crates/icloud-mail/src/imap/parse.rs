//! What a FETCH response says about a message: its envelope, its MIME
//! structure, its flags. The port of `formatMessageResponse`,
//! `parseEnvelope`, `parseBodystructure` and `getStructuredParams` of
//! imapflow 2.2.4 (`tools.js`), which read what servers send rather than
//! what RFC 3501 says they should.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use mymcps_vine::js::{self, JsRegex};

use super::decode::{decode_name, decode_words};
use super::wire::{Response, Token, string_list};
use crate::message::{Address, BodyStructure, Envelope, FetchedMessage};

fn regex(source: &str) -> JsRegex {
    js::regex(source, "").expect("static regex")
}

/// `(token && token.value) || ''`, as text.
fn value_of(token: Option<&Token>) -> String {
    token.and_then(Token::text).unwrap_or_default()
}

/// `parseUintValue`: a number written in digits only, that JavaScript counts exactly.
pub(crate) fn parse_uint(text: &str, max_digits: usize) -> Option<u64> {
    let is_decimal = !text.is_empty()
        && text.len() <= max_digits
        && text.bytes().all(|byte| byte.is_ascii_digit());
    text.parse()
        .ok()
        .filter(|number| is_decimal && *number <= 9_007_199_254_740_991)
}

pub(crate) const MAX_UINT32_DIGITS: usize = 10;
const MAX_NUMBER64_DIGITS: usize = 20;

/// `isValidSequenceValue` of `Number(text)`: a message number or a UID.
pub(crate) fn sequence_value(text: &str) -> Option<u32> {
    let number = js::string_to_number(text);
    (number.fract() == 0.0 && (1.0..=4_294_967_295.0).contains(&number)).then_some(number as u32)
}

/// `expandRange`: the numbers of a set such as `12,14:16`, in the order written.
pub(crate) fn expand_range(range: &str) -> Vec<u32> {
    /// More numbers than this are not a list of messages.
    const LIMIT: usize = 0x100_0000;
    let mut numbers = Vec::new();
    for entry in range.split(',') {
        if numbers.len() >= LIMIT {
            break;
        }
        let entry = js::trim(entry);
        let Some((first, second)) = entry.split_once(':') else {
            numbers.extend(sequence_value(entry));
            continue;
        };
        let (Some(first), Some(second)) = (sequence_value(first), sequence_value(second)) else {
            continue;
        };
        let remaining = LIMIT - numbers.len();
        if first <= second {
            numbers.extend((first..=second).take(remaining));
        } else {
            numbers.extend((second..=first).rev().take(remaining));
        }
    }
    numbers
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Minutes east of UTC for the zone names of RFC 5322.
fn named_zone(name: &str) -> Option<i64> {
    Some(match name {
        "ut" | "utc" | "gmt" | "z" => 0,
        "est" => -5 * 60,
        "edt" => -4 * 60,
        "cst" => -6 * 60,
        "cdt" => -5 * 60,
        "mst" => -7 * 60,
        "mdt" => -6 * 60,
        "pst" => -8 * 60,
        "pdt" => -7 * 60,
        _ => return None,
    })
}

/// The instant a date written in a mail header or by an IMAP server stands
/// for, as `new Date(text)` reads the formats mail uses: RFC 5322
/// (`Thu, 1 Oct 2026 09:30:00 +0200 (CEST)`), IMAP (`01-Oct-2026 09:30:00 +0000`)
/// and ISO 8601. A date without a zone is taken to be in UTC, which is
/// where the server runs. `None` for what is not a date.
pub(crate) fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    static COMMENTS: LazyLock<JsRegex> = LazyLock::new(|| regex(r"\([^()]*\)"));
    static TIME: LazyLock<JsRegex> =
        LazyLock::new(|| regex(r"^(\d{1,2}):(\d{2})(?::(\d{2})(?:[.,]\d+)?)?$"));
    static OFFSET: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^([+-])(\d{2}):?(\d{2})$"));

    let text = js::trim(text);
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        return Some(date.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return date
            .and_hms_opt(0, 0, 0)
            .map(|midnight| Utc.from_utc_datetime(&midnight));
    }

    let text = COMMENTS.as_regex().replace_all(text, " ").to_lowercase();
    let mut day = None;
    let mut month = None;
    let mut year = None;
    let mut time = None;
    let mut offset = None;
    for word in text
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter(|word| !word.is_empty())
    {
        if let Some(captures) = TIME.as_regex().captures(word) {
            let part = |index: usize| {
                captures
                    .get(index)
                    .and_then(|part| part.as_str().parse::<u32>().ok())
            };
            time = Some((part(1)?, part(2)?, part(3).unwrap_or(0)));
        } else if let Some(captures) = OFFSET.as_regex().captures(word) {
            let part = |index: usize| {
                captures
                    .get(index)
                    .and_then(|part| part.as_str().parse::<i64>().ok())
            };
            let minutes = part(2)? * 60 + part(3)?;
            offset = Some(if &captures[1] == "-" {
                -minutes
            } else {
                minutes
            });
        } else if let Some(minutes) = named_zone(word) {
            offset = Some(minutes);
        } else {
            // A day, a month and a year, also when hyphens hold them together.
            for piece in word.split('-').filter(|piece| !piece.is_empty()) {
                if let Some(index) = MONTHS
                    .iter()
                    .position(|name| piece.len() >= 3 && piece.starts_with(name))
                {
                    month = Some(index as u32 + 1);
                } else if let Ok(number) = piece.parse::<i32>() {
                    match (day, piece.len()) {
                        (None, 1 | 2) => day = Some(number as u32),
                        // Two digits are a year of this century up to 49, of the last one from 50.
                        (_, 1 | 2) => {
                            year = Some(if number < 50 {
                                2000 + number
                            } else {
                                1900 + number
                            })
                        }
                        _ => year = Some(number),
                    }
                } else if !piece.chars().all(char::is_alphabetic) {
                    return None;
                }
            }
        }
    }
    let (hour, minute, second) = time.unwrap_or((0, 0, 0));
    let local = NaiveDate::from_ymd_opt(year?, month?, day?)?.and_hms_opt(hour, minute, second)?;
    // `None` for an instant chrono cannot hold: a date is whatever the
    // sender of a message wrote, and the last minute of the last year it
    // reads, moved by a zone, is past the end.
    Utc.from_utc_datetime(&local)
        .checked_sub_signed(chrono::Duration::minutes(offset.unwrap_or(0)))
}

/// `processAddresses`: the addresses of one header of an envelope.
fn addresses(token: Option<&Token>) -> Vec<Address> {
    let Some(list) = token.and_then(Token::list) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            // A NIL entry inside an address list is skipped.
            let fields = entry.list()?;
            let field = |index: usize| value_of(fields.get(index));
            let name = decode_name(&field(0));
            let (mailbox, host) = (field(2), field(3));
            if host.is_empty() {
                // RFC 9051 7.5.2: without a host this is the name of a group, not an
                // address with an empty domain. It is kept as a name alone.
                let name = if name.is_empty() && !mailbox.is_empty() {
                    decode_name(&mailbox)
                } else {
                    name
                };
                return Some(Address {
                    name,
                    address: String::new(),
                });
            }
            Some(Address {
                name,
                address: format!("{mailbox}@{host}"),
            })
        })
        .filter(|address| !address.name.is_empty() || !address.address.is_empty())
        .collect()
}

/// `parseEnvelope`
pub(crate) fn parse_envelope(entry: &[Token]) -> Envelope {
    let text = |index: usize| Some(value_of(entry.get(index))).filter(|value| !value.is_empty());
    Envelope {
        date: text(0).and_then(|date| parse_date(&date)),
        subject: text(1).map(|subject| decode_words(&subject)),
        from: addresses(entry.get(2)),
        sender: addresses(entry.get(3)),
        reply_to: addresses(entry.get(4)),
        to: addresses(entry.get(5)),
        cc: addresses(entry.get(6)),
        bcc: addresses(entry.get(7)),
        in_reply_to: text(8).map(|id| js::trim(&id).to_owned()),
        message_id: text(9).map(|id| js::trim(&id).to_owned()),
    }
}

enum Param {
    Text(String),
    /// A value written in several pieces, as RFC 2231 allows.
    Pieces {
        charset: Option<String>,
        values: Vec<(u64, String)>,
    },
}

/// `getStructuredParams`: the parameters of a media type or a disposition,
/// with the values split or encoded as RFC 2231 describes put back together.
fn structured_params(token: Option<&Token>) -> HashMap<String, String> {
    static ENCODED_FILENAME: LazyLock<JsRegex> =
        LazyLock::new(|| regex(r"^[a-z\-_0-9]+'[a-z]*'[^'\x00-\x08\x0b\x0c\x0e-\x1f\u0080-￿]+"));
    static CONTINUATION: LazyLock<JsRegex> = LazyLock::new(|| regex(r"\*((\d+)\*?)?$"));
    static CHARSET: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^([^']*)'[^']*'(.*)$"));

    // The parameters in the order they were first named, which is the order they are put together in.
    let mut order: Vec<String> = Vec::new();
    let mut params: HashMap<String, Param> = HashMap::new();
    fn set(
        order: &mut Vec<String>,
        params: &mut HashMap<String, Param>,
        key: String,
        value: Param,
    ) {
        if params.insert(key.clone(), value).is_none() {
            order.push(key);
        }
    }

    let entries = token.and_then(Token::list).unwrap_or_default();
    for pair in entries.chunks(2) {
        let key = value_of(pair.first()).to_lowercase();
        if let Some(value) = pair
            .get(1)
            .filter(|_| !["__proto__", "constructor", "prototype"].contains(&key.as_str()))
        {
            set(
                &mut order,
                &mut params,
                key,
                Param::Text(decode_words(&value_of(Some(value)))),
            );
        }
    }

    // A name encoded as RFC 2231 describes, but in the parameter that says it is not.
    if let (Some(Param::Text(filename)), false) =
        (params.get("filename"), params.contains_key("filename*"))
        && ENCODED_FILENAME.test(filename)
    {
        let mut pieces = filename.split('\'');
        let (charset, encoded) = (
            pieces.next().unwrap_or_default(),
            pieces.nth(1).unwrap_or_default(),
        );
        if encoding_rs::Encoding::for_label(charset.as_bytes()).is_some() {
            let value = Param::Text(format!("{charset}''{encoded}"));
            set(&mut order, &mut params, "filename*".to_owned(), value);
        }
    }

    for key in order.clone() {
        let Some(matched) = CONTINUATION.as_regex().captures(&key) else {
            continue;
        };
        let suffix = matched.get(0).map_or("", |suffix| suffix.as_str());
        let name = key[..key.len() - suffix.len()].to_owned();
        let number = matched
            .get(2)
            .and_then(|number| number.as_str().parse().ok())
            .unwrap_or(0);
        let Some(Param::Text(mut value)) = params.remove(&key) else {
            continue;
        };
        if !matches!(params.get(&name), Some(Param::Pieces { .. })) {
            set(
                &mut order,
                &mut params,
                name.clone(),
                Param::Pieces {
                    charset: None,
                    values: Vec::new(),
                },
            );
        }
        let Some(Param::Pieces { charset, values }) = params.get_mut(&name) else {
            continue;
        };
        // The first piece may say which charset and language the value is in.
        if number == 0
            && suffix.ends_with('*')
            && let Some(first) = CHARSET.as_regex().captures(&value)
        {
            let named = &first[1];
            *charset = Some(if named.is_empty() {
                "utf-8".to_owned()
            } else {
                named.to_owned()
            });
            value = first[2].to_owned();
        }
        values.push((number, value));
    }

    let mut decoded = HashMap::new();
    for key in order {
        match params.remove(&key) {
            Some(Param::Text(value)) => {
                decoded.insert(key, value);
            }
            Some(Param::Pieces {
                charset,
                mut values,
            }) => {
                values.sort_by_key(|(number, _)| *number);
                let value: String = values.into_iter().map(|(_, value)| value).collect();
                let value = match charset {
                    // The value is percent-encoded: written as an encoded word, it is decoded like one.
                    Some(charset) => {
                        let mut word = String::with_capacity(value.len());
                        for character in value.chars() {
                            match character {
                                ' ' => word.push('_'),
                                '%' => word.push('='),
                                '=' | '?' | '_' => {
                                    word.push_str(&format!("={:02x}", character as u32))
                                }
                                character if js::is_whitespace(character) => {
                                    word.push_str(&format!("={:02x}", character as u32))
                                }
                                character => word.push(character),
                            }
                        }
                        decode_words(&format!("=?{charset}?Q?{word}?="))
                    }
                    None => decode_words(&value),
                };
                decoded.insert(key, value);
            }
            None => {}
        }
    }
    decoded
}

/// `Number(value || 0) || 0`, for a size.
fn size_of(token: Option<&Token>) -> u64 {
    let size = js::string_to_number(&value_of(token));
    if size.is_finite() && size > 0.0 {
        size as u64
    } else {
        0
    }
}

/// `parseBodystructure`: the tree of the parts of a message. Parts are
/// numbered as IMAP numbers them: the children of the root from 1, theirs
/// from `1.1`.
pub(crate) fn parse_body_structure(entry: &[Token]) -> BodyStructure {
    walk_structure(entry, &[])
}

fn walk_structure(node: &[Token], path: &[usize]) -> BodyStructure {
    let mut current = BodyStructure::default();
    if !path.is_empty() {
        current.part = Some(
            path.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("."),
        );
    }
    let is_set = |index: usize| node.get(index).is_some_and(|token| *token != Token::Nil);
    let lower = |index: usize| value_of(node.get(index)).to_lowercase();
    let mut index = 0;

    if matches!(node.first(), Some(Token::List(_))) {
        // A multipart: its parts, then its subtype.
        let mut children = Vec::new();
        while let Some(Token::List(child)) = node.get(index) {
            index += 1;
            let mut child_path = path.to_vec();
            child_path.push(index);
            children.push(walk_structure(child, &child_path));
        }
        current.child_nodes = Some(children);
        current.media_type = format!("multipart/{}", lower(index));
        index += 1;
        // What follows is not sent for a BODY request.
        if index < node.len() {
            if is_set(index) {
                current.parameters = structured_params(node.get(index));
            }
            index += 1;
        }
    } else {
        current.media_type = format!("{}/{}", lower(0), lower(1));
        index = 2;
        if is_set(index) {
            current.parameters = structured_params(node.get(index));
        }
        // The parameters, the ID and the description.
        index += 3;
        if is_set(index) {
            current.encoding = Some(lower(index));
        }
        index += 1;
        if is_set(index) {
            current.size = Some(size_of(node.get(index)));
        }
        index += 1;

        if current.media_type == "message/rfc822" {
            // An attached message comes with its envelope, its own structure and its number of lines.
            index += 1;
            if let Some(Token::List(inner)) = node.get(index) {
                // The attached message has the number of the part that holds it.
                current.child_nodes = Some(vec![walk_structure(inner, path)]);
            }
            index += 2;
        }
        if current.media_type.starts_with("text/") {
            // A text part says how many lines it has, but some servers leave that out.
            let is_shifted = node.len() == 11
                && matches!(node.get(index + 1), Some(Token::List(_)))
                && !matches!(node.get(index + 2), Some(Token::List(_)));
            if !is_shifted {
                index += 1;
            }
        }
        // The MD5, which is not sent for a BODY request.
        if index < node.len() {
            index += 1;
        }
    }

    if let Some(Token::List(disposition)) = node.get(index)
        && !disposition.is_empty()
    {
        current.disposition = Some(value_of(disposition.first()).to_lowercase());
        if matches!(disposition.get(1), Some(Token::List(_))) {
            current.disposition_parameters = structured_params(disposition.get(1));
        }
    }
    current
}

/// What one untagged FETCH response holds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Fetched {
    pub message: FetchedMessage,
    /// `false` for a response that is not about a message the command asked for.
    pub has_uid: bool,
    /// `RFC822.SIZE`
    pub size: Option<u64>,
    /// The content asked for with `BODY[...]`, by section in lower case.
    /// `None` when the server has nothing for a section.
    pub body_parts: HashMap<String, Option<Vec<u8>>>,
}

/// `formatMessageResponse`
pub(crate) fn parse_fetch(untagged: &Response) -> Fetched {
    let mut fetched = Fetched::default();
    let items = untagged
        .attributes
        .get(1)
        .and_then(Token::list)
        .unwrap_or_default();
    for pair in items.chunks(2) {
        let (
            Some(Token::Atom {
                value: name,
                section,
                ..
            }),
            Some(value),
        ) = (pair.first(), pair.get(1))
        else {
            continue;
        };
        match (name.to_lowercase().as_str(), section) {
            ("uid", None) => {
                if let Some(uid) = value
                    .text()
                    .and_then(|uid| parse_uint(&uid, MAX_UINT32_DIGITS))
                    .and_then(|uid| u32::try_from(uid).ok())
                    .filter(|uid| *uid > 0)
                {
                    fetched.message.uid = uid;
                    fetched.has_uid = true;
                }
            }
            ("flags", None) => {
                fetched.message.flags = string_list(Some(value)).into_iter().collect()
            }
            ("envelope", None) => fetched.message.envelope = value.list().map(parse_envelope),
            ("bodystructure", None) => {
                fetched.message.body_structure = value.list().map(parse_body_structure)
            }
            ("internaldate", None) => {
                fetched.message.internal_date = value.text().and_then(|date| parse_date(&date))
            }
            ("rfc822.size", None) => {
                fetched.size = Some(
                    value
                        .text()
                        .and_then(|size| parse_uint(&size, MAX_NUMBER64_DIGITS))
                        .unwrap_or(0),
                )
            }
            ("body" | "binary", Some(section)) => {
                // `HEADER.FIELDS (References)` is the header, whichever fields were asked for.
                let key = value_of(section.first()).to_lowercase();
                let key = key.split(".fields").next().unwrap_or_default().to_owned();
                let content = if *value == Token::Nil {
                    None
                } else {
                    value.bytes()
                };
                if key == "header" {
                    fetched.message.headers = content;
                } else if !key.is_empty() {
                    // `BODY[]` is the whole message, which nothing here asks for.
                    fetched.body_parts.insert(key, content);
                }
            }
            _ => {}
        }
    }
    fetched
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::super::wire::parse_response;
    use super::*;

    fn fetch(line: &str, literals: Vec<Vec<u8>>) -> Fetched {
        parse_fetch(&parse_response(line.as_bytes(), literals).unwrap())
    }

    #[test]
    fn reads_the_dates_mail_is_written_with() {
        let expected = Utc.with_ymd_and_hms(2026, 10, 1, 9, 30, 0).unwrap();
        for written in [
            "Thu, 01 Oct 2026 09:30:00 +0000",
            "Thu, 1 Oct 2026 11:30:00 +0200 (CEST)",
            "1 Oct 2026 09:30 GMT",
            "01-Oct-2026 09:30:00 +0000",
            " 1-Oct-2026 04:30:00 -0500",
            "Thu, 1 Oct 26 05:30:00 EDT",
            "October 1, 2026 09:30:00 UTC",
            "2026-10-01T09:30:00Z",
            "2026-10-01T11:30:00+02:00",
            "Thu, 01 Oct 2026 09:30:00",
        ] {
            assert_eq!(parse_date(written), Some(expected), "{written}");
        }
        for not_a_date in [
            "",
            "yesterday",
            "32 Oct 2026 09:30:00 +0000",
            "1 Oct",
            "Thu, 01 Oct 2026 25:00:00 +0000",
            // Past the last, and before the first, instant a date can hold.
            "Fri, 31 Dec 262142 23:59:59 -0001",
            "1 Jan -262143 00:00:00 +0001",
        ] {
            assert_eq!(parse_date(not_a_date), None, "{not_a_date}");
        }
    }

    #[test]
    fn reads_an_envelope_with_names_groups_and_encoded_words() {
        let fetched = fetch(
            concat!(
                r#"* 1 FETCH (UID 11 FLAGS (\Seen $Forwarded) INTERNALDATE "01-Oct-2026 09:31:00 +0000" RFC822.SIZE 4096 "#,
                r#"ENVELOPE ("Thu, 01 Oct 2026 09:30:00 +0000" "=?UTF-8?Q?D=C3=A9jeuner_jeudi_=3F?=" "#,
                r#"(("=?UTF-8?Q?Alice_Martin?=" NIL "alice" "example.com")) NIL NIL "#,
                r#"((NIL NIL "undisclosed-recipients" NIL)(NIL NIL NIL NIL)("\"Thomas\"" NIL "thomas" "icloud.com") NIL (NIL NIL "bob" "example.com")) "#,
                r#"NIL NIL "<root@example.com> " " <lunch@example.com>"))"#,
            ),
            Vec::new(),
        );
        let message = fetched.message;
        assert!(fetched.has_uid);
        assert_eq!(message.uid, 11);
        assert_eq!(fetched.size, Some(4096));
        assert_eq!(
            message.flags,
            ["\\Seen".to_owned(), "$Forwarded".to_owned()]
                .into_iter()
                .collect()
        );
        assert_eq!(
            message.internal_date,
            Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 31, 0).unwrap())
        );

        let envelope = message.envelope.unwrap();
        assert_eq!(
            envelope.date,
            Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 30, 0).unwrap())
        );
        assert_eq!(envelope.subject.as_deref(), Some("Déjeuner jeudi ?"));
        assert_eq!(
            envelope.from,
            [Address::new("Alice Martin", "alice@example.com")]
        );
        assert_eq!(
            envelope.to,
            [
                Address::new("undisclosed-recipients", ""),
                Address::new("Thomas", "thomas@icloud.com"),
                Address::bare("bob@example.com")
            ]
        );
        assert_eq!(
            (envelope.sender, envelope.reply_to, envelope.cc),
            (vec![], vec![], vec![])
        );
        assert_eq!(envelope.in_reply_to.as_deref(), Some("<root@example.com>"));
        assert_eq!(envelope.message_id.as_deref(), Some("<lunch@example.com>"));

        // A server with nothing to say answers NIL, and a response may be about a flag alone.
        let bare = fetch(
            "* 2 FETCH (FLAGS (\\Seen) ENVELOPE NIL BODYSTRUCTURE NIL)",
            Vec::new(),
        );
        assert!(!bare.has_uid);
        assert_eq!(
            (bare.message.envelope, bare.message.body_structure),
            (None, None)
        );
    }

    #[test]
    fn reads_the_structure_of_a_message_and_numbers_its_parts() {
        let fetched = fetch(
            concat!(
                r#"* 1 FETCH (UID 11 BODYSTRUCTURE ((("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "QUOTED-PRINTABLE" 52 3 NIL NIL NIL NIL)"#,
                r#"("text" "html" ("charset" "utf-8") NIL NIL "quoted-printable" 120 4 NIL NIL NIL NIL) "ALTERNATIVE" ("boundary" "b2") NIL NIL NIL)"#,
                r#"("application" "pdf" ("name" "=?UTF-8?Q?Menu_=C3=A9t=C3=A9.pdf?=") NIL NIL "base64" 78000 NIL ("ATTACHMENT" ("filename*" "utf-8''Menu%20%C3%A9t%C3%A9.pdf")) NIL NIL)"#,
                r#"("message" "rfc822" NIL NIL NIL "7bit" 9000 ("Thu, 01 Oct 2026 09:30:00 +0000" "Fwd" NIL NIL NIL NIL NIL NIL NIL NIL) ("text" "plain" NIL NIL NIL "7bit" 500 10) 200 NIL ("attachment" ("filename*0" "Fwd" "filename*1" ".eml")) NIL NIL)"#,
                r#" "mixed" ("boundary" "b1") NIL NIL NIL))"#,
            ),
            Vec::new(),
        );
        let root = fetched.message.body_structure.unwrap();
        assert_eq!(
            (root.part.as_deref(), root.media_type.as_str()),
            (None, "multipart/mixed")
        );
        assert_eq!(root.parameters["boundary"], "b1");
        let children = root.child_nodes.unwrap();
        assert_eq!(children.len(), 3);

        let alternative = &children[0];
        assert_eq!(
            (alternative.part.as_deref(), alternative.media_type.as_str()),
            (Some("1"), "multipart/alternative")
        );
        let texts = alternative.child_nodes.as_ref().unwrap();
        assert_eq!(texts[0].part.as_deref(), Some("1.1"));
        assert_eq!(
            (
                texts[0].media_type.as_str(),
                texts[0].encoding.as_deref(),
                texts[0].size
            ),
            ("text/plain", Some("quoted-printable"), Some(52))
        );
        assert_eq!(
            (texts[1].part.as_deref(), texts[1].media_type.as_str()),
            (Some("1.2"), "text/html")
        );

        let pdf = &children[1];
        assert_eq!(
            (pdf.part.as_deref(), pdf.media_type.as_str(), pdf.size),
            (Some("2"), "application/pdf", Some(78000))
        );
        assert_eq!(pdf.disposition.as_deref(), Some("attachment"));
        assert_eq!(pdf.disposition_parameters["filename"], "Menu été.pdf");
        assert_eq!(pdf.parameters["name"], "Menu été.pdf");
        // A name encoded as RFC 2231 describes, in the parameter that says it is not.
        let misplaced = fetch(
            r#"* 1 FETCH (BODYSTRUCTURE ("application" "pdf" NIL NIL NIL "base64" 780 NIL ("attachment" ("FileName" "utf-8'en'Devis%20%C3%A9t%C3%A9.pdf")) NIL NIL))"#,
            Vec::new(),
        );
        assert_eq!(
            misplaced
                .message
                .body_structure
                .unwrap()
                .disposition_parameters["filename"],
            "Devis été.pdf"
        );

        let forwarded = &children[2];
        assert_eq!(
            (forwarded.part.as_deref(), forwarded.media_type.as_str()),
            (Some("3"), "message/rfc822")
        );
        assert_eq!(forwarded.disposition_parameters["filename"], "Fwd.eml");
        assert_eq!(
            forwarded.child_nodes.as_ref().unwrap()[0].part.as_deref(),
            Some("3")
        );

        // A message of one part has no part number, and a server may leave out the line count.
        let single = fetch(
            r#"* 1 FETCH (BODYSTRUCTURE ("text" "html" ("charset" "utf-8") NIL NIL "7bit" 4096 NIL ("inline" NIL) NIL NIL))"#,
            Vec::new(),
        );
        let single = single.message.body_structure.unwrap();
        assert_eq!(
            (
                single.part,
                single.media_type.as_str(),
                single.disposition.as_deref()
            ),
            (None, "text/html", Some("inline"))
        );
    }

    /// A token as `fixtures/dump-imap-responses.mjs` writes it.
    fn token_json(token: &Token) -> serde_json::Value {
        use base64::Engine;
        use serde_json::json;
        match token {
            Token::Nil => serde_json::Value::Null,
            Token::List(items) => items.iter().map(token_json).collect(),
            Token::Literal(bytes) => {
                json!({ "t": "LITERAL", "b64": base64::engine::general_purpose::STANDARD.encode(bytes) })
            }
            Token::String(value) => json!({ "t": "STRING", "v": value }),
            Token::Sequence(value) => json!({ "t": "SEQUENCE", "v": value }),
            Token::Text(value) => json!({ "t": "TEXT", "v": value }),
            Token::Atom {
                value,
                section,
                partial,
            } => {
                let mut written = json!({ "t": "ATOM", "v": value });
                if let Some(section) = section {
                    written["section"] = section.iter().map(token_json).collect();
                }
                if let Some(partial) = partial {
                    written["partial"] = json!(partial);
                }
                written
            }
        }
    }

    fn structure_json(node: &BodyStructure) -> serde_json::Value {
        serde_json::json!({
            "part": node.part,
            "type": node.media_type,
            "parameters": node.parameters,
            "encoding": node.encoding,
            "size": node.size,
            "disposition": node.disposition,
            "dispositionParameters": node.disposition_parameters,
            "childNodes": node.child_nodes.as_ref().map(|children| children.iter().map(structure_json).collect::<Vec<_>>()),
        })
    }

    fn message_json(fetched: &Fetched) -> serde_json::Value {
        use base64::Engine;
        use mymcps_builtin::arguments::to_iso;
        use serde_json::json;
        let encode = |bytes: &Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
        let addresses = |list: &[Address]| {
            list.iter()
                .map(|address| json!({ "name": address.name, "address": address.address }))
                .collect::<Vec<_>>()
        };
        let message = &fetched.message;
        let mut flags: Vec<&String> = message.flags.iter().collect();
        flags.sort();
        json!({
            "uid": fetched.has_uid.then_some(message.uid),
            "flags": flags,
            "size": fetched.size,
            "internalDate": message.internal_date.map(to_iso),
            "envelope": message.envelope.as_ref().map(|envelope| json!({
                "date": envelope.date.map(to_iso),
                "subject": envelope.subject,
                "messageId": envelope.message_id,
                "inReplyTo": envelope.in_reply_to,
                "from": addresses(&envelope.from),
                "sender": addresses(&envelope.sender),
                "replyTo": addresses(&envelope.reply_to),
                "to": addresses(&envelope.to),
                "cc": addresses(&envelope.cc),
                "bcc": addresses(&envelope.bcc),
            })),
            "bodyStructure": message.body_structure.as_ref().map(structure_json),
            "headers": message.headers.as_ref().map(encode),
            "bodyParts": fetched.body_parts.iter().map(|(key, content)| (key.clone(), json!(content.as_ref().map(encode)))).collect::<serde_json::Map<_, _>>(),
        })
    }

    /// The responses of `fixtures/imap_responses.json` are read here as imapflow reads them.
    #[test]
    fn reads_responses_as_imapflow_does() {
        use base64::Engine;
        let decode = |text: &serde_json::Value| {
            base64::engine::general_purpose::STANDARD
                .decode(text.as_str().unwrap())
                .unwrap()
        };
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/imap_responses.json")).unwrap();
        assert!(cases.len() > 70);

        for case in cases {
            let payload = decode(&case["payload"]);
            let line = String::from_utf8_lossy(&payload).into_owned();
            let literals = case["literals"]
                .as_array()
                .unwrap()
                .iter()
                .map(decode)
                .collect();
            let response = match parse_response(&payload, literals) {
                Ok(response) => response,
                Err(error) => {
                    assert_eq!(case["error"], error.code, "{line}");
                    continue;
                }
            };
            assert_eq!(case.get("error"), None, "{line}");
            assert_eq!(case["response"]["tag"], response.tag, "{line}");
            assert_eq!(case["response"]["command"], response.command, "{line}");
            let attributes: serde_json::Value =
                response.attributes.iter().map(token_json).collect();
            assert_eq!(attributes, case["response"]["attributes"], "{line}");
            if let Some(message) = case.get("message") {
                assert_eq!(&message_json(&parse_fetch(&response)), message, "{line}");
            }
        }
    }

    #[test]
    fn reads_the_content_a_fetch_asked_for() {
        let fetched = fetch(
            "* 1 FETCH (UID 11 BODY[1.2.MIME] {4}\r\n BODY[1.2]<0> {5}\r\n BODY[HEADER.FIELDS (References)] \"References: <a@b>\" BODY[3] NIL)",
            vec![b"mime".to_vec(), b"hello".to_vec()],
        );
        assert_eq!(
            fetched.body_parts["1.2.mime"].as_deref(),
            Some(b"mime".as_slice())
        );
        assert_eq!(
            fetched.body_parts["1.2"].as_deref(),
            Some(b"hello".as_slice())
        );
        assert_eq!(fetched.body_parts["3"], None);
        assert_eq!(
            fetched.message.headers.as_deref(),
            Some(b"References: <a@b>".as_slice())
        );
        assert_eq!(expand_range("12,14:16,20:19,0,x"), [12, 14, 15, 16, 20, 19]);
    }
}
