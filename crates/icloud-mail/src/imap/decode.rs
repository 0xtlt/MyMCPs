//! The encodings of mail: words encoded in headers, the transfer encodings
//! and charsets of message parts, flowed text, and mailbox names.
//!
//! imapflow leaves these to `libmime`, `libqp`, `libbase64`, `iconv-lite` and
//! `mailsplit`. Each function says which of their functions it ports. Mail
//! is written by strangers, so all of them take anything and never fail.

use std::sync::LazyLock;

use base64::Engine;
use encoding_rs::{Encoding, REPLACEMENT, UTF_8, X_USER_DEFINED};
use mymcps_vine::js::{self, JsRegex};
use regex::Captures;

fn regex(source: &str, flags: &str) -> JsRegex {
    js::regex(source, flags).expect("static regex")
}

/// The charset a label stands for. `None` when nothing here can decode it.
///
/// libmime resolves labels with the table of the Encoding Standard, which is
/// the one `encoding_rs` has, then tidies the spellings mail software uses.
fn charset(label: &str) -> Option<&'static Encoding> {
    static UTF: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^utf[-_]?(\d+)", ""));
    static ASCII: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^(?:us[-_]?)ascii", ""));
    static WINDOWS: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^win(?:dows)?[-_]?(\d+)", ""));
    static LATIN: LazyLock<JsRegex> =
        LazyLock::new(|| regex(r"^(?:latin|iso[-_]?8859)?[-_]?(\d+)", ""));
    static L: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^l[-_]?(\d+)", ""));

    let label = js::trim(label).to_lowercase();
    let known = |label: &str| {
        Encoding::for_label(label.as_bytes())
            .filter(|encoding| *encoding != REPLACEMENT && *encoding != X_USER_DEFINED)
    };
    if let Some(encoding) = known(&label) {
        return Some(encoding);
    }
    let label = UTF.as_regex().replace(&label, "utf-$1");
    let label = ASCII.as_regex().replace(&label, "windows-1252");
    let label = WINDOWS.as_regex().replace(&label, "windows-$1");
    let label = LATIN.as_regex().replace(&label, "iso-8859-$1");
    let label = L.as_regex().replace(&label, "iso-8859-$1");
    known(&label)
}

/// `libcharset.normalizeCharset`, as far as telling two labels apart goes.
fn charset_name(label: &str) -> String {
    match charset(label) {
        Some(encoding) => encoding.name().to_owned(),
        None => js::trim(label).to_uppercase(),
    }
}

/// `libcharset.decode`: the text `bytes` are in `label`, or in UTF-8 when the
/// charset is unknown.
pub(crate) fn decode_charset(bytes: &[u8], label: &str) -> String {
    match charset(label) {
        Some(encoding) if encoding != UTF_8 => {
            encoding.decode_with_bom_removal(bytes).0.into_owned()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Whether text in this charset is read as it is: `ascii`, `usascii` and `utf8`.
pub(crate) fn is_plain_charset(label: &str) -> bool {
    let name: String = label
        .to_lowercase()
        .chars()
        .filter(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        .collect();
    ["ascii", "usascii", "utf8"].contains(&name.as_str())
}

/// A decoder for the charset of a text part, fed in pieces. `None` when the
/// charset is unknown, and the text is then left as it is.
pub(crate) fn charset_decoder(label: &str) -> Option<encoding_rs::Decoder> {
    charset(label).map(Encoding::new_decoder_with_bom_removal)
}

const LENIENT_BASE64: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    base64::engine::GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
);

/// What `Buffer.from(text, 'base64')` gives for one run without padding:
/// characters outside the alphabet are skipped, and an incomplete group gives
/// the bytes it has.
fn decode_base64_run(text: &str) -> Vec<u8> {
    let mut alphabet: Vec<u8> = text
        .bytes()
        .filter_map(|byte| match byte {
            b'-' => Some(b'+'),
            b'_' => Some(b'/'),
            byte if byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/' => Some(byte),
            _ => None,
        })
        .collect();
    if alphabet.len() % 4 == 1 {
        alphabet.pop();
    }
    // A last group of two or three characters leaves bits over, which are dropped.
    LENIENT_BASE64.decode(&alphabet).unwrap_or_default()
}

/// `libbase64.decode`: Node stops at the first padding, so input made of
/// several padded pieces is decoded piece by piece.
pub(crate) fn decode_base64(text: &str) -> Vec<u8> {
    text.split('=')
        .filter(|run| !run.is_empty())
        .flat_map(decode_base64_run)
        .collect()
}

/// `libbase64.Decoder`: base64 that arrives in pieces cut anywhere.
#[derive(Debug, Default)]
pub(crate) struct Base64Stream {
    carried: String,
}

impl Base64Stream {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut text = std::mem::take(&mut self.carried);
        text.extend(
            chunk
                .iter()
                .copied()
                .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
                .map(char::from),
        );

        let (padded, rest) = match text.rfind('=') {
            Some(last) => text.split_at(last + 1),
            None => ("", text.as_str()),
        };
        let whole = rest.len() - rest.len() % 4;
        let decoded = decode_base64(&format!("{padded}{}", &rest[..whole]));
        self.carried = rest[whole..].to_owned();
        decoded
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        decode_base64(&std::mem::take(&mut self.carried))
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).map(|digit| digit as u8)
}

/// `libqp.decode`: quoted-printable, with the spaces that end a line and the
/// soft line breaks removed.
pub(crate) fn decode_quoted_printable(input: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let byte = input[index];
        match byte {
            b' ' | b'\t' => {
                // `/[\t ]+$/gm`: blanks that end a line or the text are not content.
                let run = input[index..]
                    .iter()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
                let next = input.get(index + run);
                if !matches!(next, None | Some(b'\r' | b'\n')) {
                    decoded.extend_from_slice(&input[index..index + run]);
                }
                index += run;
            }
            b'=' => {
                // Blanks between the sign and the end of the line were removed before it is read.
                let blanks = input[index + 1..]
                    .iter()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
                let after = &input[index + 1 + blanks..];
                let is_line_end =
                    after.is_empty() || after.starts_with(b"\r") || after.starts_with(b"\n");
                if after.is_empty() {
                    index = input.len();
                } else if after.starts_with(b"\r\n") {
                    index += 1 + blanks + 2;
                } else if after.starts_with(b"\n") {
                    index += 1 + blanks + 1;
                } else if let (false, Some(high), Some(low)) = (
                    is_line_end,
                    input.get(index + 1).copied().and_then(hex_value),
                    input.get(index + 2).copied().and_then(hex_value),
                ) {
                    decoded.push(high * 16 + low);
                    index += 3;
                } else {
                    decoded.push(b'=');
                    index += 1;
                }
            }
            _ => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    decoded
}

/// `libmime.decodeFlowed`: the paragraphs of `format=flowed` text, where a
/// line that ends in a space goes on in the next.
pub(crate) fn decode_flowed(input: &[u8], delete_space: bool) -> Vec<u8> {
    let mut paragraphs: Vec<Vec<u8>> = Vec::new();
    let mut current: Option<Vec<u8>> = None;

    let mut lines = Vec::new();
    let mut rest = input;
    while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
        let line = &rest[..end];
        lines.push(line.strip_suffix(b"\r").unwrap_or(line));
        rest = &rest[end + 1..];
    }
    lines.push(rest);

    for line in lines {
        // A signature separator is not a line that goes on.
        let is_soft_break = current
            .as_ref()
            .is_some_and(|paragraph| paragraph.ends_with(b" ") && paragraph != b"-- ");
        match (&mut current, is_soft_break) {
            (Some(paragraph), true) => {
                if delete_space {
                    paragraph.pop();
                }
                paragraph.extend_from_slice(line);
            }
            _ => {
                paragraphs.extend(current.take());
                current = Some(line.to_vec());
            }
        }
    }
    paragraphs.extend(current.filter(|paragraph| !paragraph.is_empty()));

    // `/^ /gm`: the space a line was stuffed with to keep its first character.
    let mut text = Vec::with_capacity(input.len());
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            text.push(b'\n');
        }
        let mut at_line_start = true;
        for byte in paragraph {
            if !(at_line_start && *byte == b' ') {
                text.push(*byte);
            }
            at_line_start = *byte == b'\r';
        }
    }
    text
}

/// `libmime.decodeWord`: the text of one `=?charset?encoding?payload?=`.
fn decode_word(label: &str, encoding: &str, payload: &str) -> String {
    static SPLIT_HEX: LazyLock<JsRegex> = LazyLock::new(|| regex(r"=\s+([0-9a-fA-F])", ""));

    // RFC 2231 lets a language follow the charset. It changes nothing here.
    let label = label.split('*').next().unwrap_or_default();
    let bytes = if encoding.eq_ignore_ascii_case("Q") {
        // Spaces between a sign and its digits come from lines split in the wrong place.
        let text = SPLIT_HEX.as_regex().replace_all(payload, "=$1");
        let text: Vec<u8> = text
            .chars()
            .map(|character| {
                if character == '_' || js::is_whitespace(character) {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>()
            .into_bytes();
        let mut bytes = Vec::with_capacity(text.len());
        let mut index = 0;
        while index < text.len() {
            let hex = (
                text.get(index + 1).copied().and_then(hex_value),
                text.get(index + 2).copied().and_then(hex_value),
            );
            match (text[index], hex) {
                (b'=', (Some(high), Some(low))) => {
                    bytes.push(high * 16 + low);
                    index += 3;
                }
                (byte, _) => {
                    bytes.push(byte);
                    index += 1;
                }
            }
        }
        bytes
    } else {
        decode_base64(payload)
    };
    decode_charset(&bytes, label)
}

/// `libmime.decodeWords`: a header value with its encoded words decoded.
/// Words that follow each other in the same charset and encoding are joined
/// first, since a character may be split between them.
pub(crate) fn decode_words(text: &str) -> String {
    static WORD: LazyLock<JsRegex> =
        LazyLock::new(|| regex(r"=\?([^?]+)\?([QqBb])\?([^?]*)\?=", ""));
    static DECODABLE: LazyLock<JsRegex> = LazyLock::new(|| regex(r"^[\w_\-*]+$", ""));

    if !text.contains("=?") {
        return text.to_owned();
    }

    struct Word {
        label: String,
        encoding: String,
        payload: String,
    }
    let decode = |word: &Word| {
        if DECODABLE.test(&word.label) {
            decode_word(&word.label, &word.encoding, &word.payload)
        } else {
            format!("=?{}?{}?{}?=", word.label, word.encoding, word.payload)
        }
    };

    let mut decoded = String::with_capacity(text.len());
    let mut pending: Option<Word> = None;
    let mut position = 0;
    for captures in WORD.as_regex().captures_iter(text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let between = &text[position..whole.start()];
        let group = |captures: &Captures<'_>, index: usize| {
            captures
                .get(index)
                .map_or("", |group| group.as_str())
                .to_owned()
        };
        let word = Word {
            label: group(&captures, 1),
            encoding: group(&captures, 2),
            payload: group(&captures, 3),
        };
        let follows = between.chars().all(js::is_whitespace);

        match pending.take() {
            Some(mut previous)
                if follows
                    && previous.encoding.eq_ignore_ascii_case(&word.encoding)
                    && charset_name(&previous.label) == charset_name(&word.label) =>
            {
                previous.payload.push_str(&word.payload);
                pending = Some(previous);
            }
            Some(previous) => {
                decoded.push_str(&decode(&previous));
                // Spaces between two encoded words only separate them.
                if !(follows && !between.is_empty()) {
                    decoded.push_str(between);
                }
                pending = Some(word);
            }
            None => {
                decoded.push_str(between);
                pending = Some(word);
            }
        }
        position = whole.end();
    }
    if let Some(last) = pending {
        decoded.push_str(&decode(&last));
    }
    decoded.push_str(&text[position..]);
    decoded
}

/// `decodeText` of imapflow: a name with its encoded words decoded, then
/// without the quotes some servers leave around it.
pub(crate) fn decode_name(value: &str) -> String {
    let decoded = decode_words(value);
    if js::utf16_len(&decoded) > 2 && decoded.starts_with('"') && decoded.ends_with('"') {
        decoded[1..decoded.len() - 1].to_owned()
    } else {
        decoded
    }
}

/// A header value such as `text/plain; charset="utf-8"`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeaderValue {
    pub value: String,
    pub params: Vec<(String, String)>,
}

impl HeaderValue {
    pub(crate) fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// `libmime.parseHeaderValue`, without the parameters split over several
/// lines, which the headers read here do not use.
pub(crate) fn parse_header_value(text: &str) -> HeaderValue {
    let is_blank =
        |character: char| matches!(character, ' ' | '\t' | '\r' | '\n' | '\u{c}' | '\u{b}');
    let mut parsed = HeaderValue::default();
    let mut key: Option<String> = None;
    let mut collected = String::new();
    let mut end = 0;
    let mut in_key = false;
    let mut in_quote = false;
    let mut is_escaped = false;

    let commit = |parsed: &mut HeaderValue,
                  in_key: bool,
                  key: &Option<String>,
                  collected: &mut String,
                  end: &mut usize| {
        let text = collected[..*end].to_owned();
        collected.clear();
        *end = 0;
        match key {
            _ if in_key => {
                if !text.is_empty() {
                    parsed.params.push((text.to_lowercase(), String::new()));
                }
            }
            Some(key) => parsed.params.push((key.clone(), text)),
            None => parsed.value = text,
        }
    };

    for character in text.chars() {
        if in_key {
            match character {
                '=' => {
                    key = Some(collected[..end].to_lowercase());
                    collected.clear();
                    end = 0;
                    in_key = false;
                }
                ';' => commit(&mut parsed, true, &key, &mut collected, &mut end),
                character if is_blank(character) => {
                    if !collected.is_empty() {
                        collected.push(character);
                    }
                }
                character => {
                    collected.push(character);
                    end = collected.len();
                }
            }
            continue;
        }
        if is_escaped {
            collected.push(character);
            end = collected.len();
        } else if character == '\\' {
            is_escaped = true;
            continue;
        } else if character == '"' {
            in_quote = !in_quote;
        } else if !in_quote && character == ';' {
            commit(&mut parsed, false, &key, &mut collected, &mut end);
            in_key = true;
        } else if !in_quote && is_blank(character) {
            if !collected.is_empty() {
                collected.push(character);
            }
        } else {
            collected.push(character);
            end = collected.len();
        }
        is_escaped = false;
    }
    commit(&mut parsed, in_key, &key, &mut collected, &mut end);
    parsed
}

/// `Headers.getFirst` of mailsplit: the value of the first header named
/// `name` among the headers of a message part, unfolded.
pub(crate) fn first_header(headers: &[u8], name: &str) -> String {
    let end = headers
        .iter()
        .rposition(|byte| !matches!(byte, b'\r' | b'\n'))
        .map_or(0, |last| last + 1);
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for line in headers[..end].split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match lines.last_mut() {
            Some(previous) if matches!(line.first(), Some(b' ' | b'\t')) => {
                let folded = line
                    .iter()
                    .position(|byte| !matches!(byte, b' ' | b'\t'))
                    .unwrap_or(line.len());
                previous.push(b' ');
                previous.extend_from_slice(&line[folded..]);
            }
            _ => lines.push(line.to_vec()),
        }
    }
    lines
        .iter()
        .filter_map(|line| {
            let colon = line.iter().position(|byte| *byte == b':')?;
            Some((&line[..colon], &line[colon + 1..]))
        })
        .find(|(key, _)| js::trim(&String::from_utf8_lossy(key)).eq_ignore_ascii_case(name))
        .map(|(_, value)| {
            // A header written in UTF-8 is read as such, and byte for byte otherwise.
            let value = match std::str::from_utf8(value) {
                Ok(text) => text.to_owned(),
                Err(_) => value.iter().map(|byte| char::from(*byte)).collect(),
            };
            js::trim(&value.replace('\r', " ")).to_owned()
        })
        .unwrap_or_default()
}

const MODIFIED_BASE64: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::IMAP_MUTF7,
    base64::engine::GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
);

/// A mailbox name in the modified UTF-7 of RFC 3501, which is how names
/// with other characters than ASCII are written in commands.
pub(crate) fn encode_utf7(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    let mut shifted: Vec<u16> = Vec::new();
    let flush = |shifted: &mut Vec<u16>, encoded: &mut String| {
        if shifted.is_empty() {
            return;
        }
        let bytes: Vec<u8> = shifted.drain(..).flat_map(u16::to_be_bytes).collect();
        encoded.push('&');
        encoded.push_str(&MODIFIED_BASE64.encode(bytes));
        encoded.push('-');
    };
    for character in name.chars() {
        if (' '..='~').contains(&character) {
            flush(&mut shifted, &mut encoded);
            encoded.push(character);
            if character == '&' {
                encoded.push('-');
            }
        } else {
            let mut units = [0; 2];
            shifted.extend_from_slice(character.encode_utf16(&mut units));
        }
    }
    flush(&mut shifted, &mut encoded);
    encoded
}

/// The name a server wrote in modified UTF-7. What is not valid is kept as written.
pub(crate) fn decode_utf7(name: &str) -> String {
    let mut decoded = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let shifted = &rest[start + 1..];
        let end = shifted.find('-');
        let payload = &shifted[..end.unwrap_or(shifted.len())];
        match (end, MODIFIED_BASE64.decode(payload)) {
            (Some(_), _) if payload.is_empty() => decoded.push('&'),
            (Some(_), Ok(bytes)) => {
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect();
                decoded.push_str(&String::from_utf16_lossy(&units));
            }
            (end, _) => {
                decoded.push('&');
                decoded.push_str(payload);
                if end.is_some() {
                    decoded.push('-');
                }
            }
        }
        rest = match end {
            Some(end) => &shifted[end + 1..],
            None => "",
        };
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_encoded_words_and_joins_the_ones_that_belong_together() {
        assert_eq!(decode_words("Lunch on Thursday?"), "Lunch on Thursday?");
        assert_eq!(decode_words("=?UTF-8?Q?Menu_=C3=A9t=C3=A9?="), "Menu été");
        assert_eq!(decode_words("=?utf-8?B?TWVudSDDqXTDqQ==?="), "Menu été");
        assert_eq!(
            decode_words("=?ISO-8859-1?Q?Andr=E9?= Martin"),
            "André Martin"
        );
        assert_eq!(decode_words("=?iso-8859-1*fr?q?Andr=E9?="), "André");
        // A character cut in two by the limit on the length of a word.
        assert_eq!(decode_words("=?UTF-8?B?w6k=?= =?utf8?b?w6k=?="), "éé");
        assert_eq!(
            decode_words("=?UTF-8?Q?=C3?=\r\n =?UTF-8?Q?=A9t=C3=A9?="),
            "été"
        );
        // Words in other charsets are only brought together.
        assert_eq!(decode_words("=?UTF-8?Q?a?= =?ISO-8859-1?Q?=E9?= b"), "aé b");
        assert_eq!(decode_words("=?UTF-8?Q?a?= b =?UTF-8?Q?c?="), "a b c");
        assert_eq!(decode_words("=?unknown-charset?Q?=C3=A9?="), "é");
        assert_eq!(decode_words("=?utf-8?Q?broken"), "=?utf-8?Q?broken");
        assert_eq!(decode_words("=?shift_jis?B?k/qWe4zq?="), "日本語");
        assert_eq!(decode_name("\"=?UTF-8?Q?Alice?= Martin\""), "Alice Martin");
        assert_eq!(decode_name("\"\""), "\"\"");
    }

    #[test]
    fn decodes_the_transfer_encodings_of_a_part() {
        assert_eq!(
            decode_quoted_printable(b"Caf=C3=A9 =\r\nau lait  \r\n=3D=3d =ZZ=\n="),
            b"Caf\xc3\xa9 au lait\r\n== =ZZ"
        );
        assert_eq!(
            decode_quoted_printable(b"soft =  \r\nbreak\t\nend \t"),
            b"soft break\nend"
        );

        assert_eq!(decode_base64("aGVsbG8="), b"hello");
        assert_eq!(decode_base64("aGVs\r\nbG8"), b"hello");
        assert_eq!(decode_base64("aGk=aGk="), b"hihi");
        assert_eq!(decode_base64("a"), b"");

        let mut stream = Base64Stream::default();
        let mut decoded = stream.push(b"aGVsb");
        decoded.extend(stream.push(b"G8gd29y\r\nbGQ"));
        decoded.extend(stream.finish());
        assert_eq!(decoded, b"hello world");
    }

    #[test]
    fn joins_the_lines_of_flowed_text() {
        assert_eq!(
            decode_flowed(
                b"A line that \r\ngoes on \r\nand ends.\r\n\r\n-- \r\nSignature\r\n",
                false
            ),
            b"A line that goes on and ends.\n\n-- \nSignature"
        );
        assert_eq!(decode_flowed(b"deleted \r\nspace", true), b"deletedspace");
        assert_eq!(
            decode_flowed(b" From stuffed\n> quoted \n> more", false),
            b"From stuffed\n> quoted > more"
        );
    }

    #[test]
    fn reads_the_headers_of_a_part() {
        let headers = b"Content-Type: text/plain;\r\n\tcharset=\"ISO-8859-1\"; Format=Flowed; delsp=yes\r\nContent-Transfer-Encoding: Quoted-Printable (comment)\r\n\r\n";
        let content_type = parse_header_value(&first_header(headers, "content-type"));
        assert_eq!(content_type.value, "text/plain");
        assert_eq!(content_type.param("charset"), Some("ISO-8859-1"));
        assert_eq!(content_type.param("format"), Some("Flowed"));
        assert_eq!(
            first_header(headers, "Content-Transfer-Encoding"),
            "Quoted-Printable (comment)"
        );
        assert_eq!(first_header(headers, "Content-Disposition"), "");
        assert_eq!(
            parse_header_value("attachment; filename=\"a; b.pdf\"").param("filename"),
            Some("a; b.pdf")
        );

        assert_eq!(decode_charset(b"Andr\xe9", "latin1"), "André");
        assert_eq!(decode_charset(b"Andr\xe9", "us-ascii"), "André");
        assert_eq!(
            decode_charset(b"\x93quoted\x94", "windows-1252"),
            "\u{201c}quoted\u{201d}"
        );
        assert_eq!(decode_charset(b"caf\xc3\xa9", "x-unknown"), "café");
        assert!(
            is_plain_charset("UTF-8")
                && is_plain_charset("us-ascii")
                && !is_plain_charset("latin1")
        );
    }

    /// Every decoder here against the library it ports, on the corpus of
    /// `fixtures/dump-decoders.mjs`. A count of the cases that differ is kept
    /// for the ones that are known to, and explained where it is.
    #[test]
    fn decodes_as_the_node_libraries_do() {
        use base64::Engine;
        use serde_json::Value;
        let decode = |text: &Value| {
            base64::engine::general_purpose::STANDARD
                .decode(text.as_str().unwrap())
                .unwrap()
        };
        let text = |value: &Value| value.as_str().unwrap().to_owned();
        let latin1 = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| char::from(*byte))
                .collect::<String>()
        };
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/decoders.json")).unwrap();
        let cases = |name: &str| fixture[name].as_array().unwrap().clone();

        for case in cases("words") {
            assert_eq!(
                decode_words(&text(&case[0])),
                text(&case[1]),
                "{:?}",
                case[0]
            );
        }
        for case in cases("quotedPrintable") {
            assert_eq!(
                decode_quoted_printable(&decode(&case[0])),
                decode(&case[1]),
                "{:?}",
                latin1(&decode(&case[0]))
            );
        }
        for case in cases("base64") {
            assert_eq!(
                decode_base64(&text(&case[0])),
                decode(&case[1]),
                "{:?}",
                case[0]
            );
            // Cut anywhere, the pieces decode to the same bytes.
            let written = text(&case[0]);
            let mut stream = Base64Stream::default();
            let mut streamed = Vec::new();
            for chunk in written.as_bytes().chunks(3) {
                streamed.extend(stream.push(chunk));
            }
            streamed.extend(stream.finish());
            let whole: String = written
                .chars()
                .filter(|character| character.is_ascii_alphanumeric() || "+/=".contains(*character))
                .collect();
            assert_eq!(streamed, decode_base64(&whole), "{:?}", case[0]);
        }
        for case in cases("flowed") {
            assert_eq!(
                latin1(&decode_flowed(&decode(&case[0]), case[1] == true)),
                latin1(&decode(&case[2])),
                "{:?}",
                latin1(&decode(&case[0]))
            );
        }
        for case in cases("utf7") {
            assert_eq!(
                encode_utf7(&text(&case[0])),
                text(&case[1]),
                "{:?}",
                case[0]
            );
            assert_eq!(
                decode_utf7(&text(&case[1])),
                text(&case[2]),
                "{:?}",
                case[1]
            );
        }
        // Bytes that are not text in a charset are replaced, by one mark or by several.
        let marks = |text: String| {
            let mut collapsed = String::new();
            for character in text.chars() {
                if !(character == '\u{fffd}' && collapsed.ends_with('\u{fffd}')) {
                    collapsed.push(character);
                }
            }
            collapsed
        };
        let mut different = Vec::new();
        for case in cases("charsets") {
            let (label, bytes) = (text(&case[0]), decode(&case[1]));
            if marks(decode_charset(&bytes, &label)) != marks(text(&case[2])) {
                different.push(format!("{label}: \"{}\"", latin1(&bytes)));
            }
        }
        // What is left: bytes with their high bit set in a charset of seven bits,
        // and one pair of bytes EUC-KR has no character for. `encoding_rs` replaces
        // them as the Encoding Standard says, the Node libraries their own way.
        assert_eq!(
            different,
            [
                "iso-2022-jp: \"caf\u{e9}\"",
                "iso-2022-jp: \"caf\u{c3}\u{a9}\"",
                "iso-2022-jp: \"\u{d6}\u{d0}\u{ce}\u{c4}\"",
                "iso-2022-jp: \"\u{f0}\u{d2}\u{c9}\u{d7}\u{c5}\u{d4}\"",
                "euc-kr: \"\u{f0}\u{d2}\u{c9}\u{d7}\u{c5}\u{d4}\"",
            ]
        );
        for case in cases("headerValues") {
            let parsed = parse_header_value(&text(&case[0]));
            let param = |name: &str| {
                parsed
                    .param(name)
                    .map(str::to_owned)
                    .map_or(Value::Null, Value::String)
            };
            assert_eq!(parsed.value, text(&case[1]), "{:?}", case[0]);
            assert_eq!(
                [param("charset"), param("format"), param("filename")],
                [case[2].clone(), case[3].clone(), case[4].clone()],
                "{:?}",
                case[0]
            );
        }
        for case in cases("headerBlocks") {
            let block = decode(&case[0]);
            for (index, name) in [
                "content-type",
                "Content-Transfer-Encoding",
                "content-disposition",
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    first_header(&block, name),
                    text(&case[index + 1]),
                    "{name} of {:?}",
                    latin1(&block)
                );
            }
        }
    }

    #[test]
    fn writes_and_reads_mailbox_names_in_modified_utf7() {
        for (name, encoded) in [
            ("Sent Messages", "Sent Messages"),
            ("Entwürfe", "Entw&APw-rfe"),
            ("R&D", "R&-D"),
            ("日本語", "&ZeVnLIqe-"),
            ("Envoyés/été 😀", "Envoy&AOk-s/&AOk-t&AOk- &2D3eAA-"),
        ] {
            assert_eq!(encode_utf7(name), encoded);
            assert_eq!(decode_utf7(encoded), name);
        }
        assert_eq!(decode_utf7("broken &!!- name &"), "broken &!!- name &");
    }
}
