//! What the Node app left to its runtime and that decides here which
//! requests pass: how `Buffer` reads base64 and hex, `decodeURIComponent`,
//! and `String.prototype.slice`.

/// `Buffer.from(text, 'base64')`: both alphabets, characters outside of them
/// skipped, and nothing read past the first `=`.
pub(crate) fn decode_base64(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3 + 2);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in text.bytes() {
        let sextet = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        buffer = (buffer << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    bytes
}

/// `Buffer.from(text, 'hex')`: pairs of digits, up to the first that is not one.
pub(crate) fn decode_hex(text: &str) -> Vec<u8> {
    let digit = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    pairs
        .iter()
        .map_while(|[high, low]| Some((digit(*high)? << 4) | digit(*low)?))
        .collect()
}

/// `decodeURIComponent`, which fails on a malformed escape and on escapes
/// that do not spell UTF-8.
pub(crate) fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digit = |offset: usize| {
                bytes
                    .get(index + offset)
                    .and_then(|byte| (*byte as char).to_digit(16))
            };
            decoded.push(((digit(1)? << 4) | digit(2)?) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// `text.slice(0, length)`, where the length counts UTF-16 code units. A
/// character the cut would split is left out whole.
pub(crate) fn slice_utf16(text: &str, length: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > length {
            return &text[..index];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Node 24 answers for `Buffer.from(text, 'base64')`.
    #[test]
    fn reads_base64_as_buffer_does() {
        for (text, bytes) in [
            ("YTpi", &b"a:b"[..]),
            ("YTpi=", b"a:b"),
            ("YTpi==", b"a:b"),
            ("YTp", b"a:"),
            ("YTpi\n", b"a:b"),
            ("YT pi", b"a:b"),
            ("YT$pi", b"a:b"),
            ("YQ==YQ==", b"a"),
            ("YQ=YQ", b"a"),
            ("Y", b""),
            ("YQ", b"a"),
            ("-_-_", &[251, 255, 191]),
            ("+/+/", &[251, 255, 191]),
            ("w6k6w6k=", "é:é".as_bytes()),
            ("/w==", &[255]),
            ("YTpi!!!!", b"a:b"),
            ("=YTpi", b""),
            ("YT=pi", b"a"),
            ("é", b""),
            ("YTpié", b"a:b"),
            ("YTpi\u{a0}", b"a:b"),
            ("  YTpi  ", b"a:b"),
            ("Y T p i", b"a:b"),
            ("YTpiYQ", b"a:ba"),
            ("YTpiY", b"a:b"),
            ("YTpiYQ=", b"a:ba"),
            ("YTpiYQ=x", b"a:ba"),
            ("////", &[255, 255, 255]),
            ("7aCA", &[237, 160, 128]),
            ("Og==", b":"),
        ] {
            assert_eq!(decode_base64(text), bytes, "{text:?}");
        }
    }

    #[test]
    fn reads_hex_up_to_the_first_pair_that_is_not_hex() {
        assert_eq!(decode_hex("00ff10"), [0, 255, 16]);
        assert_eq!(decode_hex("00FF1"), [0, 255]);
        assert_eq!(decode_hex("00zz10"), [0]);
        assert_eq!(decode_hex(""), [0u8; 0]);
    }

    /// What Node 24 answers for `decodeURIComponent(text)`.
    #[test]
    fn decodes_uri_components_as_javascript_does() {
        for (text, decoded) in [
            ("a%20b", Some("a b")),
            ("%", None),
            ("%2", None),
            ("%zz", None),
            ("%C3%A9", Some("é")),
            ("%c3%a9", Some("é")),
            ("%C3", None),
            ("%C3x", None),
            ("%E0%A4%A", None),
            ("a+b", Some("a+b")),
            ("%ED%A0%80", None),
            ("%F0%9F%98%80", Some("😀")),
            ("%80", None),
            ("%C0%80", None),
            ("%2f%3A", Some("/:")),
            ("é", Some("é")),
            ("%FF", None),
            ("%F4%90%80%80", None),
            ("%EF%BF%BE", Some("\u{fffe}")),
        ] {
            assert_eq!(decode_uri_component(text).as_deref(), decoded, "{text:?}");
        }
    }

    #[test]
    fn slices_by_utf16_code_units() {
        assert_eq!(slice_utf16("abcdef", 3), "abc");
        assert_eq!(slice_utf16("abc", 12), "abc");
        assert_eq!(slice_utf16("é😀é", 3), "é😀");
        assert_eq!(slice_utf16("é😀é", 2), "é");
        assert_eq!(slice_utf16("", 5), "");
    }
}
