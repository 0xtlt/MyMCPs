//! The mail message nodemailer 10.0.14 builds, byte for byte.
//!
//! The Node version of MyMCPs handed its mail to nodemailer, and the messages
//! it wrote are what recipients and the account's own folders hold. This is a
//! port of the one path the iCloud Mail tools take through that library: a
//! plain text body, files held in memory, bare addresses, `newline: 'windows'`.
//! Whatever nodemailer does on that path is done here the same way, oddities
//! included, and `tests/mime_differential.rs` compares the two on a few
//! thousand mails.
//!
//! Each function names the JavaScript it ports. Addresses are written by
//! `mime_address.rs`, and the media type of a file name comes from the table
//! in `mime_types.rs`.
//!
//! JavaScript strings are UTF-16, and nodemailer measures and cuts them in
//! UTF-16 units. Where a length decides a byte of the message, it is counted
//! in those units here too.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Datelike, Timelike, Utc};

use super::mime_address::{address_list, header_address, is_js_space, js_trim};
use super::mime_types::MEDIA_TYPES;

/// A file as nodemailer attaches it.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    /// Left out, the type is the one nodemailer has for the extension of the
    /// file name. So is a value that is not a media type.
    pub content_type: Option<String>,
    pub content: Vec<u8>,
}

/// One mail, as `composeMail` of the Node version described it.
#[derive(Debug, Clone)]
pub struct Mail {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub in_reply_to: Option<String>,
    pub references: Option<Vec<String>>,
    pub message_id: String,
    pub date: DateTime<Utc>,
}

/// Who the message is from and whom it is delivered to, as nodemailer's
/// `message.getEnvelope()` gives them to the SMTP connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// Empty when the sender address holds nothing but control characters,
    /// which nodemailer sends as `MAIL FROM:<>`.
    pub from: String,
    pub to: Vec<String>,
}

impl Envelope {
    /// Whether an address has a character outside ASCII. nodemailer then adds
    /// `SMTPUTF8` to `MAIL FROM`, if the server offers that extension, and
    /// sends the addresses as UTF-8 either way (`_setEnvelope` of
    /// smtp-connection).
    pub fn needs_smtputf8(&self) -> bool {
        !self.from.is_ascii() || self.to.iter().any(|address| !address.is_ascii())
    }
}

impl Mail {
    /// Port of `MimeNode.getEnvelope`. The addresses are the ones written in
    /// the headers, and a recipient named twice is delivered to once.
    pub fn envelope(&self) -> Envelope {
        let mut seen = HashSet::new();
        let to = [&self.to, &self.cc, &self.bcc]
            .into_iter()
            .flatten()
            .filter_map(|address| header_address(address))
            .filter(|address| seen.insert(address.clone()))
            .collect();

        Envelope {
            from: header_address(&self.from).unwrap_or_default(),
            to,
        }
    }

    /// The message as `new MailComposer(options).compile().build()` writes it.
    ///
    /// `keep_bcc` is `message.keepBcc`: the copy kept in the account names the
    /// blind-copied recipients, the message delivered does not. `boundary` is
    /// the random part of the multipart boundary, from [`random_boundary`].
    pub fn build(&self, keep_bcc: bool, boundary: &str) -> Vec<u8> {
        // `MimeNode` drops control characters from the boundary it is given.
        let boundary: String = boundary
            .chars()
            .filter(|character| !matches!(character, '\0'..='\x1f' | '\x7f'))
            .collect();
        let fields = self.fields(keep_bcc);

        // `MailComposer.compile`: the text and each attachment are a part.
        // Parts are numbered as they are created, after the message itself.
        let has_text = !self.text.is_empty();
        let is_mixed = self.attachments.len() > 1 || (has_text && self.attachments.len() == 1);
        let first_part = if is_mixed { 2 } else { 1 };

        let mut parts = Vec::with_capacity(self.attachments.len() + 1);
        if has_text {
            parts.push(text_part(&self.text));
        }
        for (index, attachment) in self.attachments.iter().enumerate() {
            let node = first_part + parts.len();
            parts.push(attachment_part(attachment, index, &boundary, node));
        }

        if is_mixed {
            return mixed_message(&fields, &parts, &part_boundary(&boundary, 1));
        }

        // A message with nothing in it is an empty `text/plain` part.
        let part = parts.pop().unwrap_or_else(|| Part {
            content_type: "text/plain".to_owned(),
            early_headers: Vec::new(),
            late_headers: Vec::new(),
            body: Body::Encoded(Vec::new()),
            closing: String::new(),
        });
        single_part_message(&fields, &part)
    }

    /// The header fields `MailComposer.compile` sets on the message, in its
    /// order, with the values `MimeNode._encodeHeaderValue` gives them. A
    /// field whose value comes out empty is dropped when the headers are written.
    fn fields(&self, keep_bcc: bool) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            ("From", address_list(std::slice::from_ref(&self.from))),
            ("To", address_list(&self.to)),
            ("Cc", address_list(&self.cc)),
        ];
        if keep_bcc {
            fields.push(("Bcc", address_list(&self.bcc)));
        }
        // An empty string is a missing option to `compile`.
        if let Some(id) = self.in_reply_to.as_deref().filter(|id| !id.is_empty()) {
            fields.push(("In-Reply-To", message_id(id)));
        }
        if let Some(references) = &self.references {
            fields.push(("References", reference_list(references)));
        }
        if !self.subject.is_empty() {
            fields.push(("Subject", unstructured(&self.subject)));
        }
        // nodemailer draws an ID of its own for a message without one, which
        // is not ported: a mail always has one.
        if !self.message_id.is_empty() {
            fields.push(("Message-ID", message_id(&self.message_id)));
        }
        fields.push(("Date", date(self.date)));
        fields
    }
}

/// The random part of a multipart boundary, drawn as `MimeNode` draws it:
/// eight random bytes, written in hexadecimal.
pub fn random_boundary() -> String {
    boundary_from(rand::random())
}

/// The random part of a multipart boundary, from eight bytes of the caller.
pub fn boundary_from(bytes: [u8; 8]) -> String {
    let mut boundary = String::with_capacity(16);
    for byte in bytes {
        let _ = write!(boundary, "{byte:02x}");
    }
    boundary
}

/// What nodemailer writes to the server after `DATA` for a message: port of
/// the `DataStream` of smtp-connection. A line that starts with a dot gets a
/// second one, a line ending that is a lone CR or a lone LF becomes CRLF, and
/// the line with a single dot that ends the message is added.
///
/// The second rule is why this is more than dot-stuffing: [`Mail::build`]
/// keeps a lone CR of the text as it is, like nodemailer.
pub fn smtp_data(message: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(message.len() + message.len() / 64 + 5);
    let mut previous = None;
    let mut rest = message;
    loop {
        // Most bytes need nothing: copy up to the next one that might. The
        // byte after a CR always does.
        let next = if previous == Some(b'\r') {
            Some(0)
        } else {
            rest.iter()
                .position(|byte| matches!(byte, b'\r' | b'\n' | b'.'))
        };
        let Some((plain, special)) = next.and_then(|at| rest.split_at_checked(at)) else {
            break;
        };
        let Some((&byte, after)) = special.split_first() else {
            break;
        };
        data.extend_from_slice(plain);
        let before = plain.last().copied().or(previous);
        if before == Some(b'\r') && byte != b'\n' {
            // A server that ends lines at a lone CR would read "\r.\r" as the
            // end of the message, so a dot after one is doubled too.
            data.push(b'\n');
            if byte == b'.' {
                data.push(b'.');
            }
        } else if byte == b'\n' && before != Some(b'\r') {
            data.push(b'\r');
        } else if byte == b'.' && matches!(before, Some(b'\n') | None) {
            data.push(b'.');
        }
        data.push(byte);
        previous = Some(byte);
        rest = after;
    }
    data.extend_from_slice(rest);

    data.extend_from_slice(match rest.last().copied().or(previous) {
        Some(b'\n') => b".\r\n".as_slice(),
        Some(b'\r') => b"\n.\r\n".as_slice(),
        _ => b"\r\n.\r\n".as_slice(),
    });
    data
}

// Header values

fn utf16_length(value: &str) -> usize {
    value.encode_utf16().count()
}

/// The control characters a header cannot carry: all but tab, LF and CR.
fn is_header_control(character: char) -> bool {
    matches!(character, '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\x7f')
}

/// `.replace(/\r?\n|\r/g, ' ')`: each line break becomes one space.
fn without_line_breaks(value: &str) -> String {
    let mut line = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                characters.next_if_eq(&'\n');
                line.push(' ');
            }
            '\n' => line.push(' '),
            _ => line.push(character),
        }
    }
    line
}

/// What `_encodeHeaderValue` does to each ID of `Message-ID`, `In-Reply-To`
/// and `References` before it looks at the angle brackets.
fn without_header_controls(value: &str) -> String {
    without_line_breaks(value)
        .chars()
        .filter(|&character| !is_header_control(character))
        .collect()
}

fn in_angle_brackets(mut id: String) -> String {
    if !id.starts_with('<') {
        id.insert(0, '<');
    }
    if !id.ends_with('>') {
        id.push('>');
    }
    id
}

/// `Message-ID` and `In-Reply-To` in `_encodeHeaderValue`. An ID is never
/// written as an encoded word: what is not ASCII goes out as UTF-8.
fn message_id(value: &str) -> String {
    in_angle_brackets(without_header_controls(value))
}

/// `References` in `_encodeHeaderValue`: every ID in angle brackets, with one
/// space between two.
fn reference_list(references: &[String]) -> String {
    let mut ids = Vec::new();
    for reference in references {
        let cleaned = without_header_controls(reference);
        let reference = without_spaces_in_brackets(js_trim(&cleaned));
        if reference.is_empty() {
            // JavaScript splits an empty string into one empty string.
            ids.push("<>".to_owned());
        }
        ids.extend(
            reference
                .split(is_js_space)
                .filter(|id| !id.is_empty())
                .map(|id| in_angle_brackets(id.to_owned())),
        );
    }
    ids.join(" ")
}

/// `.replace(/<[^>]*>/g, str => str.replace(/\s/g, ''))`.
fn without_spaces_in_brackets(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((before, bracketed)) = rest.split_once('<') {
        let Some((inside, after)) = bracketed.split_once('>') else {
            break;
        };
        result.push_str(before);
        result.push('<');
        result.extend(inside.chars().filter(|&character| !is_js_space(character)));
        result.push('>');
        rest = after;
    }
    result.push_str(rest);
    result
}

/// `Date` in `_encodeHeaderValue`: `date.toUTCString()` with `+0000` for `GMT`.
fn date(date: DateTime<Utc>) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let day = DAYS
        .get(date.weekday().num_days_from_monday() as usize)
        .unwrap_or(&"");
    let month = MONTHS.get(date.month0() as usize).unwrap_or(&"");
    let year = date.year();
    format!(
        "{day}, {:02} {month} {}{:04} {:02}:{:02}:{:02} +0000",
        date.day(),
        if year < 0 { "-" } else { "" },
        year.unsigned_abs(),
        date.hour(),
        date.minute(),
        date.second()
    )
}

/// The default of `_encodeHeaderValue`, which `Subject` takes.
fn unstructured(value: &str) -> String {
    let value = without_line_breaks(value);
    // `_encodeHeaderText`: a control character forces an encoded word too.
    if value.chars().any(is_header_control) {
        encode_word(&value, word_encoding(&value))
    } else {
        encode_words(&value).into_owned()
    }
}

/// The two encodings of an RFC 2047 encoded word.
#[derive(Clone, Copy)]
enum WordEncoding {
    /// Quoted-printable, readable where the text is mostly Latin letters.
    Q,
    /// Base64, shorter for everything else.
    B,
}

/// Port of `MimeNode._getTextEncoding`: Q when the Latin letters outnumber
/// what has to be escaped, counted in UTF-16 units.
fn word_encoding(value: &str) -> WordEncoding {
    let mut escaped = 0usize;
    let mut latin = 0usize;
    for unit in value.encode_utf16() {
        match unit {
            0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x80.. => escaped += 1,
            0x41..=0x5a | 0x61..=0x7a => latin += 1,
            _ => {}
        }
    }
    if escaped < latin {
        WordEncoding::Q
    } else {
        WordEncoding::B
    }
}

/// Port of `MimeNode._encodeWords`: a value with a double quote or a
/// character outside ASCII becomes encoded words as a whole.
fn encode_words(value: &str) -> Cow<'_, str> {
    if value
        .chars()
        .any(|character| character == '"' || !character.is_ascii())
    {
        Cow::Owned(encode_word(value, word_encoding(value)))
    } else {
        Cow::Borrowed(value)
    }
}

/// Port of `encodeWord(data, encoding, 52)` of mime-funcs: one encoded word,
/// or several separated by a space when one would be too long.
fn encode_word(data: &str, encoding: WordEncoding) -> String {
    /// 52, less the `=?UTF-8?Q?` and `?=` around the text.
    const MAX_Q_LENGTH: usize = 40;
    /// The bytes that fit in 40 characters of base64. nodemailer also takes
    /// this for the length in base64 from which it splits the text.
    const MAX_B_BYTES: usize = 30;

    match encoding {
        WordEncoding::Q => {
            let encoded = q_encode(data.as_bytes());
            let words = if encoded.len() > MAX_Q_LENGTH {
                split_q_encoded(&encoded, MAX_Q_LENGTH)
            } else {
                vec![encoded.as_str()]
            };
            format!("=?UTF-8?Q?{}?=", words.join("?= =?UTF-8?Q?"))
        }
        WordEncoding::B => {
            let mut words = Vec::new();
            if data.len().div_ceil(3) * 4 > MAX_B_BYTES {
                // A word holds whole characters only.
                let mut word = String::new();
                for character in data.chars() {
                    if !word.is_empty() && word.len() + character.len_utf8() > MAX_B_BYTES {
                        words.push(BASE64.encode(&word));
                        word.clear();
                    }
                    word.push(character);
                }
                if !word.is_empty() {
                    words.push(BASE64.encode(&word));
                }
            } else {
                words.push(BASE64.encode(data));
            }
            format!("=?UTF-8?B?{}?=", words.join("?= =?UTF-8?B?"))
        }
    }
}

/// The text of a Q encoded word: `qp.encode`, then the escaping of what
/// RFC 2047 does not allow bare in a word.
fn q_encode(data: &[u8]) -> String {
    let mut encoded = String::with_capacity(data.len() * 3);
    for (index, &byte) in data.iter().enumerate() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'*' | b'+' | b'-' | b'/' => {
                encoded.push(char::from(byte));
            }
            // `qp.encode` escapes a space at the end of the text or of a line.
            b' ' if !matches!(data.get(index + 1), None | Some(b'\n' | b'\r')) => {
                encoded.push('_');
            }
            _ => {
                let _ = write!(encoded, "={byte:02X}");
            }
        }
    }
    encoded
}

/// Port of `splitMimeEncodedString` of mime-funcs: cut Q encoded text into
/// pieces of at most `max` characters, without cutting an escape or the
/// escapes of one UTF-8 character apart.
fn split_q_encoded(encoded: &str, max: usize) -> Vec<&str> {
    let max = max.max(12);
    let mut pieces = Vec::new();
    let mut rest = encoded;
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        let widest = bytes.len().min(max);
        let mut end = widest;
        // An escape cut after its "=" or its first digit moves to the next piece.
        let piece = bytes.get(..end).unwrap_or(bytes);
        if let [before @ .., b'='] | [before @ .., b'=', b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'] =
            piece
        {
            end = before.len();
        }
        let fallback = if end > 0 { end } else { widest };
        // So does a character whose continuation bytes would start the next piece.
        while end > 0 && escape_at(bytes, end).is_some_and(|byte| (0x80..0xc2).contains(&byte)) {
            end = end.saturating_sub(3);
        }
        if end == 0 {
            end = fallback;
        }
        // The text is ASCII, so any index is a character boundary.
        let Some((piece, after)) = rest.split_at_checked(end) else {
            pieces.push(rest);
            break;
        };
        pieces.push(piece);
        rest = after;
    }
    pieces
}

/// The byte an `=XX` escape at `index` stands for.
fn escape_at(bytes: &[u8], index: usize) -> Option<u8> {
    let escape = bytes.get(index..index + 3)?;
    if escape.first() != Some(&b'=') {
        return None;
    }
    let digits = std::str::from_utf8(escape.get(1..)?).ok()?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u8::from_str_radix(digits, 16).ok()
}

/// Port of `foldLines(header, 76)` of mime-funcs: a line of 76 UTF-16 units
/// or more is broken before its last run of spaces, or after the word that
/// crosses the limit when it has none.
fn fold(header: &str) -> String {
    const LINE_LENGTH: usize = 76;
    let is_space = |unit: &u16| char::from_u32(u32::from(*unit)).is_some_and(is_js_space);

    let units: Vec<u16> = header.encode_utf16().collect();
    if units.len() < LINE_LENGTH {
        return header.to_owned();
    }
    let mut folded = Vec::with_capacity(units.len() + units.len() / 32 + 2);
    let mut rest = units.as_slice();
    while !rest.is_empty() {
        let Some((line, after)) = rest.split_at_checked(LINE_LENGTH) else {
            folded.extend_from_slice(rest);
            break;
        };

        // A header never has a line break of its own here. nodemailer keeps
        // one where it finds it, and so does this.
        if let Some(at) = line.iter().position(|&unit| unit == 0x0a || unit == 0x0d) {
            let is_crlf = line.get(at) == Some(&0x0d) && line.get(at + 1) == Some(&0x0a);
            let Some((kept, after)) = rest.split_at_checked(at + if is_crlf { 2 } else { 1 })
            else {
                break;
            };
            folded.extend_from_slice(kept);
            rest = after;
            continue;
        }

        // The start of the last run of spaces, unless the line starts with it.
        let last_run = line
            .iter()
            .rposition(is_space)
            .map(|last| {
                line.iter()
                    .take(last)
                    .rposition(|unit| !is_space(unit))
                    .map_or(0, |before| before + 1)
            })
            .filter(|&start| start > 0);
        let length = last_run.unwrap_or_else(|| {
            LINE_LENGTH + after.iter().take_while(|unit| !is_space(unit)).count()
        });

        let (line, after) = rest.split_at_checked(length).unwrap_or((rest, &[]));
        folded.extend_from_slice(line);
        if !after.is_empty() {
            folded.extend_from_slice(&[0x0d, 0x0a]);
        }
        rest = after;
    }
    String::from_utf16_lossy(&folded)
}

// Header parameters

/// `isPlainText(value, true)` of mime-funcs: printable ASCII without a
/// double quote, which a parameter can carry as it is.
fn is_plain_parameter(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, 0x20..=0x7e) && byte != b'"')
}

/// Whether a parameter value has to be quoted. nodemailer has two lists of
/// characters for this, which differ on the apostrophe: one takes it anywhere,
/// the other at either end only.
fn needs_quotes(value: &str, apostrophe_anywhere: bool) -> bool {
    value.starts_with('-')
        || if apostrophe_anywhere {
            value.contains('\'')
        } else {
            value.starts_with('\'') || value.ends_with('\'')
        }
        || value.chars().any(|character| {
            is_js_space(character)
                || matches!(
                    character,
                    '"' | '\\'
                        | ';'
                        | ':'
                        | '/'
                        | '='
                        | '('
                        | ')'
                        | ','
                        | '<'
                        | '>'
                        | '@'
                        | '['
                        | ']'
                        | '?'
                )
        })
}

/// `JSON.stringify` of a string, which is how nodemailer quotes a parameter.
fn json_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\x08' => quoted.push_str("\\b"),
            '\x0c' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\0'..='\x1f' => {
                let _ = write!(quoted, "\\u{:04x}", u32::from(character));
            }
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// One parameter as `buildHeaderValue` of mime-funcs appends it to a header
/// value: as it is, quoted, or in the continuations of RFC 2231 when it is
/// long or has a character a parameter cannot carry.
fn push_parameter(header: &mut String, name: &str, value: &str) {
    if !is_plain_parameter(value) || utf16_length(value) >= 75 {
        for (index, section) in parameter_sections(value).iter().enumerate() {
            let star = if section.is_encoded { "*" } else { "" };
            let _ = write!(header, "; {name}*{index}{star}=");
            if !section.is_encoded && needs_quotes(&section.text, false) {
                header.push_str(&json_string(&section.text));
            } else {
                header.push_str(&section.text);
            }
        }
    } else if value.is_empty() || needs_quotes(value, true) {
        let _ = write!(header, "; {name}={}", json_string(value));
    } else {
        let _ = write!(header, "; {name}={value}");
    }
}

/// One continuation of a parameter. An encoded one is percent-encoded UTF-8.
struct ParameterSection {
    text: String,
    is_encoded: bool,
}

/// `safeEncodeURIComponent` of mime-funcs for one character: all but ASCII
/// letters, digits and `-_.!~` is percent-encoded.
fn percent_encode(character: char, encoded: &mut String) {
    if character.is_ascii_alphanumeric() || "-_.!~".contains(character) {
        encoded.push(character);
    } else {
        let mut bytes = [0; 4];
        for byte in character.encode_utf8(&mut bytes).bytes() {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
}

/// Port of `buildHeaderParam(key, value, 50)` of mime-funcs, which cuts a
/// parameter value into sections of under 50 characters.
///
/// The first section names the charset, so it is always an encoded one. A
/// later section starts out literal and is redone as an encoded one from its
/// first character when a character in it needs encoding.
fn parameter_sections(value: &str) -> Vec<ParameterSection> {
    const MAX_LENGTH: usize = 50;
    let mut sections = Vec::new();

    if is_plain_parameter(value) {
        // Only its length sent this value here.
        let mut rest = value;
        while let Some((text, after)) = rest.split_at_checked(MAX_LENGTH) {
            sections.push(ParameterSection {
                text: text.to_owned(),
                is_encoded: false,
            });
            rest = after;
        }
        if !rest.is_empty() {
            sections.push(ParameterSection {
                text: rest.to_owned(),
                is_encoded: false,
            });
        }
        return sections;
    }

    let characters: Vec<char> = value.chars().collect();
    let is_literal = |character: char| {
        character == ' ' || character.is_ascii_alphanumeric() || "-_.!~".contains(character)
    };
    let mut line = String::from("utf-8''");
    let mut is_encoded = true;
    // Where the section being written starts, to redo it from there.
    let mut section_start = 0;
    let mut index = 0;
    while let Some(&character) = characters.get(index) {
        let mut piece = String::new();
        if is_encoded || character != ' ' {
            percent_encode(character, &mut piece);
        } else {
            piece.push(' ');
        }

        if !is_encoded && !is_literal(character) {
            // The literal section so far, as long as it would be once encoded.
            let encoded_length = line.len() + 2 * line.matches(' ').count();
            if encoded_length + piece.len() >= MAX_LENGTH {
                // Too long to redo in one section: end it here, literal.
                sections.push(ParameterSection {
                    text: std::mem::take(&mut line),
                    is_encoded,
                });
                is_encoded = true;
            } else {
                is_encoded = true;
                line.clear();
                index = section_start;
                continue;
            }
        }

        if line.len() + piece.len() >= MAX_LENGTH {
            sections.push(ParameterSection {
                text: std::mem::take(&mut line),
                is_encoded,
            });
            is_encoded = !is_literal(character);
            if is_encoded {
                percent_encode(character, &mut line);
            } else {
                line.push(character);
                section_start = index;
            }
        } else {
            line.push_str(&piece);
        }
        index += 1;
    }
    if !line.is_empty() {
        sections.push(ParameterSection {
            text: line,
            is_encoded,
        });
    }
    sections
}

/// The `name` parameter `MimeNode.buildHeaders` adds to `Content-Type` for
/// mail clients that do not read the file name off `Content-Disposition`. It
/// is an encoded word rather than a continuation, which those clients decode.
fn name_parameter(filename: &str) -> String {
    let has_control = filename
        .chars()
        .any(|character| matches!(character, '\0'..='\x1f' | '\x7f'));
    let word = if has_control {
        Cow::Owned(encode_word(filename, word_encoding(filename)))
    } else {
        encode_words(filename)
    };
    if word != filename || needs_quotes(&word, true) {
        json_string(&word)
    } else {
        word.into_owned()
    }
}

// Media types

/// Port of `detectMimeType` of mime-funcs/mime-types.js.
fn detect_media_type(filename: &str) -> &'static str {
    let (name, extension) = name_and_extension(filename);
    // A name without an extension is looked up as if it were one.
    let key = extension
        .get(1..)
        .filter(|key| !key.is_empty())
        .unwrap_or(name);
    let key = key.split('?').next().unwrap_or_default();
    let key = js_trim(key).to_lowercase();
    MEDIA_TYPES
        .binary_search_by(|(extension, _)| (*extension).cmp(key.as_str()))
        .ok()
        .and_then(|index| MEDIA_TYPES.get(index))
        .map_or("application/octet-stream", |(_, media_type)| media_type)
}

/// The `name` and `ext` of Node's `path.parse` on POSIX. The extension starts
/// at the last dot of the last segment of the path, and a name that starts
/// with its only dot has none.
fn name_and_extension(path: &str) -> (&str, &str) {
    let base = path.trim_end_matches('/');
    let base = base.rsplit('/').next().unwrap_or(base);
    match base.rfind('.') {
        Some(dot) if dot > 0 && base != ".." => base.split_at_checked(dot).unwrap_or((base, "")),
        _ => (base, ""),
    }
}

// Parts

/// A part of the message: the text or an attachment.
struct Part<'a> {
    content_type: String,
    /// The fields nodemailer sets when it creates the part. When the part is
    /// the whole message, they come before the fields of the message.
    early_headers: Vec<(&'static str, String)>,
    /// The fields it sets when it writes the part, which come after them.
    late_headers: Vec<(&'static str, String)>,
    body: Body<'a>,
    /// What follows the body: the closing delimiter of an attachment whose
    /// own media type is a multipart one.
    closing: String,
}

enum Body<'a> {
    /// Bytes ready to be written.
    Encoded(Vec<u8>),
    /// A file to write in base64.
    Base64(&'a [u8]),
    /// A file to write as it is, with CRLF for each lone LF.
    EightBit(&'a [u8]),
}

impl Body<'_> {
    fn len(&self) -> usize {
        match self {
            Body::Encoded(bytes) => bytes.len(),
            Body::Base64(content) => base64_length(content.len()),
            Body::EightBit(content) => content.len() + lone_line_feeds(content),
        }
    }

    fn write(&self, message: &mut Vec<u8>) {
        match self {
            Body::Encoded(bytes) => message.extend_from_slice(bytes),
            Body::Base64(content) => push_base64(message, content),
            Body::EightBit(content) => push_with_crlf(message, content),
        }
    }
}

/// The boundary of the multipart node numbered `node` (`_generateBoundary`).
fn part_boundary(boundary: &str, node: usize) -> String {
    format!("--_NmP-{boundary}-Part_{node}")
}

/// The text of the mail, as `MimeNode` encodes a `text/plain` string.
fn text_part(text: &str) -> Part<'static> {
    // Port of `getTransferEncoding`. `isPlainText`: ASCII without control
    // characters other than tab, LF and CR.
    let is_plain = text
        .bytes()
        .all(|byte| byte.is_ascii() && !matches!(byte, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f));
    let (encoding, encoded) = if is_plain && !has_long_lines(text) {
        ("7bit", text.as_bytes().to_vec())
    } else {
        match word_encoding(text) {
            WordEncoding::Q => ("quoted-printable", quoted_printable(text.as_bytes())),
            WordEncoding::B => {
                let mut encoded = Vec::with_capacity(base64_length(text.len()));
                push_base64(&mut encoded, text.as_bytes());
                ("base64", encoded)
            }
        }
    };

    let mut body = Vec::with_capacity(encoded.len() + lone_line_feeds(&encoded));
    push_with_crlf(&mut body, &encoded);
    Part {
        content_type: "text/plain; charset=utf-8".to_owned(),
        early_headers: Vec::new(),
        late_headers: vec![("Content-Transfer-Encoding", encoding.to_owned())],
        body: Body::Encoded(body),
        closing: String::new(),
    }
}

/// `hasLongerLines(text, 76)` of mime-funcs, for ASCII text.
fn has_long_lines(text: &str) -> bool {
    text.len() > 128 * 1024 || text.split(['\n', '\r']).any(|line| line.len() > 76)
}

/// One attachment, as `MailComposer.getAttachments` and `_createContentNode`
/// set it up and `MimeNode.buildHeaders` writes its headers. `index` counts
/// the attachments and `node` the nodes of the message, both of which
/// nodemailer writes into a few values.
fn attachment_part<'a>(
    attachment: &'a Attachment,
    index: usize,
    boundary: &str,
    node: usize,
) -> Part<'a> {
    // A type that is not one, which no caller should pass, is not written
    // into a header: the file is treated as if it came without a type.
    let given_type = attachment
        .content_type
        .as_deref()
        .filter(|media_type| is_media_type(media_type));
    // nodemailer names a file that has no name. It then takes the extension
    // from the media type, which is not ported: a file always has a name.
    let filename = if attachment.filename.is_empty() {
        let extension = if given_type.is_some_and(|media_type| has_type(media_type, "text")) {
            "txt"
        } else {
            "bin"
        };
        Cow::Owned(format!("attachment-{}.{extension}", index + 1))
    } else {
        Cow::Borrowed(attachment.filename.as_str())
    };
    let media_type = given_type.unwrap_or_else(|| detect_media_type(&filename));

    // A message is attached as it is and shown in place. Everything else is
    // in base64, text included.
    let is_message = has_type(media_type, "message");
    let (encoding, disposition, body) = if is_message {
        ("8bit", "inline", Body::EightBit(&attachment.content))
    } else {
        ("base64", "attachment", Body::Base64(&attachment.content))
    };

    // An attachment that says it is a multipart gets a boundary like one,
    // and a closing delimiter after its content.
    let mut content_type = media_type.to_owned();
    let mut closing = String::new();
    if has_type(media_type, "multipart") {
        let boundary = part_boundary(boundary, node);
        push_parameter(&mut content_type, "boundary", &boundary);
        closing = format!("\r\n--{boundary}--\r\n");
    }
    let _ = write!(content_type, "; name={}", name_parameter(&filename));

    let mut content_disposition = disposition.to_owned();
    push_parameter(&mut content_disposition, "filename", &filename);

    Part {
        content_type,
        early_headers: vec![
            ("Content-Transfer-Encoding", encoding.to_owned()),
            ("Content-Disposition", content_disposition),
        ],
        late_headers: Vec::new(),
        body,
        closing,
    }
}

/// What the tools accept as a media type: `/^[\w.+-]{1,100}\/[\w.+-]{1,100}$/`.
fn is_media_type(value: &str) -> bool {
    let is_token = |token: &str| {
        (1..=100).contains(&token.len())
            && token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
            })
    };
    value
        .split_once('/')
        .is_some_and(|(kind, subtype)| is_token(kind) && is_token(subtype))
}

/// Whether a media type is of the top-level type `kind`, whatever its case.
fn has_type(media_type: &str, kind: &str) -> bool {
    media_type
        .split_once('/')
        .is_some_and(|(top, _)| top.eq_ignore_ascii_case(kind))
}

// Bodies

/// The length of `push_base64` for `length` bytes.
fn base64_length(length: usize) -> usize {
    let characters = length.div_ceil(3) * 4;
    characters + 2 * (characters.saturating_sub(1) / BASE64_LINE)
}

/// Characters on a line of base64, which are 57 bytes of content.
const BASE64_LINE: usize = 76;

/// Port of the `Encoder` of nodemailer's base64 module: lines of 76
/// characters with CRLF between them, and none after the last.
///
/// Each line is encoded on the stack and copied into the message, so a large
/// file is never held in base64 anywhere else.
fn push_base64(message: &mut Vec<u8>, content: &[u8]) {
    let mut line = [0u8; BASE64_LINE];
    let mut is_first = true;
    for chunk in content.chunks(BASE64_LINE / 4 * 3) {
        if !is_first {
            message.extend_from_slice(b"\r\n");
        }
        is_first = false;
        if let Ok(length) = BASE64.encode_slice(chunk, &mut line) {
            message.extend_from_slice(line.get(..length).unwrap_or_default());
        }
    }
}

/// How many LF of `content` do not follow a CR.
fn lone_line_feeds(content: &[u8]) -> usize {
    let mut previous = b'\n';
    let mut count = 0;
    for &byte in content {
        count += usize::from(byte == b'\n' && previous != b'\r');
        previous = byte;
    }
    count
}

/// Port of `LeWindows` of mime-node, which `newline: 'windows'` puts at the
/// end of the stream: a CR is added before each LF that does not follow one.
/// A lone CR stays as it is.
///
/// Only the bodies need it, since nodemailer writes everything else with
/// CRLF. A body follows the empty line that ends its headers.
fn push_with_crlf(message: &mut Vec<u8>, content: &[u8]) {
    let mut previous = b'\n';
    for line in content.split_inclusive(|&byte| byte == b'\n') {
        let Some((&last, before)) = line.split_last() else {
            continue;
        };
        if last == b'\n' && before.last().copied().unwrap_or(previous) != b'\r' {
            message.extend_from_slice(before);
            message.extend_from_slice(b"\r\n");
        } else {
            message.extend_from_slice(line);
        }
        previous = last;
    }
}

/// Port of `encode` then `wrap` of nodemailer's qp module, as its `Encoder`
/// applies them to a text that arrives in one piece.
fn quoted_printable(text: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(text.len() + text.len() / 2);
    for (index, &byte) in text.iter().enumerate() {
        let is_literal = match byte {
            // Kept, unless it ends a line or the text.
            b' ' | b'\t' => !matches!(text.get(index + 1), None | Some(b'\n' | b'\r')),
            b'\n' | b'\r' | 0x21..=0x3c | 0x3e..=0x7e => true,
            _ => false,
        };
        if is_literal {
            encoded.push(byte);
        } else {
            encoded.extend_from_slice(&escape(byte));
        }
    }
    wrap_quoted_printable(&encoded)
}

/// The `=XX` of a byte in quoted-printable.
fn escape(byte: u8) -> [u8; 3] {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    [
        b'=',
        DIGITS.get(usize::from(byte >> 4)).copied().unwrap_or(b'0'),
        DIGITS
            .get(usize::from(byte & 0x0f))
            .copied()
            .unwrap_or(b'0'),
    ]
}

/// The byte of an `=XX` that ends `line`.
fn trailing_escape(line: &[u8]) -> Option<u8> {
    escape_at(line, line.len().checked_sub(3)?)
}

/// Port of `wrap(text, 76)` of nodemailer's qp module: soft line breaks, put
/// after a space or a punctuation mark where one is near the end of the line,
/// and never inside an escape or the escapes of one UTF-8 character.
fn wrap_quoted_printable(encoded: &[u8]) -> Vec<u8> {
    const LINE_LENGTH: usize = 76;
    const MARGIN: usize = LINE_LENGTH / 3;

    if encoded.len() <= LINE_LENGTH {
        return encoded.to_vec();
    }
    let mut wrapped = Vec::with_capacity(encoded.len() + encoded.len() / 24 + 3);
    let mut rest = encoded;
    while !rest.is_empty() {
        let window = rest.get(..LINE_LENGTH).unwrap_or(rest);

        // A line break of the text within reach ends the line.
        let hard_break = if let Some(at) = window.windows(2).position(|pair| pair == b"\r\n") {
            Some(at + 2)
        } else if window.last() == Some(&b'\n') {
            Some(window.len())
        } else {
            // `/\n.*?$/` on the end of the window: its last LF, unless a CR
            // follows it.
            let tail_start = window.len().saturating_sub(MARGIN);
            let tail = window.get(tail_start..).unwrap_or_default();
            tail.iter()
                .rposition(|&byte| byte == b'\n')
                .filter(|&at| !tail.get(at..).unwrap_or_default().contains(&b'\r'))
                .map(|at| tail_start + at + 1)
        };
        if let Some(length) = hard_break {
            let (line, after) = rest.split_at_checked(length).unwrap_or((rest, &[]));
            wrapped.extend_from_slice(line);
            rest = after;
            continue;
        }

        let mut length = window.len();
        let tail_start = window.len().saturating_sub(MARGIN);
        let last_stop = if window.len() > LINE_LENGTH - MARGIN {
            window
                .get(tail_start..)
                .unwrap_or_default()
                .iter()
                .rposition(|byte| matches!(byte, b' ' | b'\t' | b'.' | b',' | b'!' | b'?'))
        } else {
            None
        };
        if let Some(at) = last_stop {
            length = tail_start + at + 1;
        } else if let Some(cut) = cut_escape(window) {
            length = cut;
            // Then the escapes of a UTF-8 character that is cut in two move
            // to the next line with it.
            while length > 3 && length < rest.len() {
                let line = window.get(..length).unwrap_or_default();
                if is_escapes_only(line) {
                    break;
                }
                let Some(byte) = trailing_escape(line) else {
                    break;
                };
                if byte < 0x80 {
                    break;
                }
                length = length.saturating_sub(3);
                if byte >= 0xc0 {
                    break;
                }
            }
        }
        if length == 0 {
            length = window.len();
        }

        let needs_soft_break = length < rest.len()
            && length.checked_sub(1).and_then(|last| window.get(last)) != Some(&b'\n');
        if needs_soft_break && length == LINE_LENGTH {
            // Make room for the "=" of the soft break.
            let line = window.get(..length).unwrap_or_default();
            length = length.saturating_sub(if trailing_escape(line).is_some() {
                3
            } else {
                1
            });
        }
        let (line, after) = rest.split_at_checked(length).unwrap_or((rest, &[]));
        wrapped.extend_from_slice(line);
        if needs_soft_break {
            wrapped.extend_from_slice(b"=\r\n");
        }
        rest = after;
    }
    wrapped
}

/// Where to cut a line that ends with an escape, whole or not: before the
/// escape when it is incomplete, at the end otherwise. `None` when the line
/// does not end with one.
fn cut_escape(line: &[u8]) -> Option<usize> {
    match line {
        [before @ .., b'='] => Some(before.len()),
        [before @ .., b'=', digit] if digit.is_ascii_hexdigit() => Some(before.len()),
        [.., b'=', first, second] if first.is_ascii_hexdigit() && second.is_ascii_hexdigit() => {
            Some(line.len())
        }
        _ => None,
    }
}

/// `/^(?:=[\da-f]{2}){1,4}$/i`: one to four escapes and nothing else.
fn is_escapes_only(line: &[u8]) -> bool {
    matches!(line.len(), 3 | 6 | 9 | 12)
        && (0..line.len())
            .step_by(3)
            .all(|index| escape_at(line, index).is_some())
}

// Messages

/// The header block of a node: each field folded, as `buildHeaders` does it.
fn push_headers(block: &mut String, fields: &[(&'static str, String)]) {
    for (name, value) in fields {
        // A field without a value is left out.
        if js_trim(value).is_empty() {
            continue;
        }
        if !block.is_empty() {
            block.push_str("\r\n");
        }
        block.push_str(&fold(&format!("{name}: {value}")));
    }
}

/// The headers of a part of a multipart message.
fn part_headers(part: &Part) -> String {
    let mut block = String::new();
    let content_type = [("Content-Type", part.content_type.clone())];
    push_headers(&mut block, &content_type);
    push_headers(&mut block, &part.early_headers);
    push_headers(&mut block, &part.late_headers);
    block.push_str("\r\n\r\n");
    block
}

/// A message that is `multipart/mixed`: the text, then the attachments.
fn mixed_message(fields: &[(&'static str, String)], parts: &[Part], boundary: &str) -> Vec<u8> {
    let mut content_type = "multipart/mixed".to_owned();
    push_parameter(&mut content_type, "boundary", boundary);
    let mut head = String::new();
    push_headers(&mut head, fields);
    push_headers(
        &mut head,
        &[
            ("MIME-Version", "1.0".to_owned()),
            ("Content-Type", content_type),
        ],
    );
    head.push_str("\r\n\r\n");

    let delimiter = format!("--{boundary}\r\n");
    let end = format!("\r\n--{boundary}--\r\n");
    let headers: Vec<String> = parts.iter().map(part_headers).collect();
    let between_parts = 2 * parts.len().saturating_sub(1);
    let length = head.len()
        + between_parts
        + end.len()
        + parts
            .iter()
            .zip(&headers)
            .map(|(part, headers)| {
                delimiter.len() + headers.len() + part.body.len() + part.closing.len()
            })
            .sum::<usize>();

    // The attachments are written straight into the message, which is
    // allocated once: with 20 MB of files it is 27 MB long.
    let mut message = Vec::with_capacity(length);
    message.extend_from_slice(head.as_bytes());
    for (index, (part, headers)) in parts.iter().zip(&headers).enumerate() {
        if index > 0 {
            message.extend_from_slice(b"\r\n");
        }
        message.extend_from_slice(delimiter.as_bytes());
        message.extend_from_slice(headers.as_bytes());
        part.body.write(&mut message);
        message.extend_from_slice(part.closing.as_bytes());
    }
    message.extend_from_slice(end.as_bytes());
    message
}

/// A message that is its only part. nodemailer moves `Content-Type` to the
/// end of the headers of the message.
fn single_part_message(fields: &[(&'static str, String)], part: &Part) -> Vec<u8> {
    let mut head = String::new();
    push_headers(&mut head, &part.early_headers);
    push_headers(&mut head, fields);
    push_headers(&mut head, &part.late_headers);
    push_headers(
        &mut head,
        &[
            ("MIME-Version", "1.0".to_owned()),
            ("Content-Type", part.content_type.clone()),
        ],
    );
    head.push_str("\r\n\r\n");

    let mut message = Vec::with_capacity(head.len() + part.body.len() + part.closing.len() + 2);
    message.extend_from_slice(head.as_bytes());
    part.body.write(&mut message);
    message.extend_from_slice(part.closing.as_bytes());
    // Port of `LastNewline` of mime-node: the message ends with a line break.
    match message.last() {
        Some(b'\n') => {}
        Some(b'\r') => message.push(b'\n'),
        _ => message.extend_from_slice(b"\r\n"),
    }
    message
}
