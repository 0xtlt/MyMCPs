//! An address as nodemailer 10.0.14 writes it in a header and in the envelope.
//!
//! nodemailer cleans the address, quotes the local part when it has to, and
//! hands the domain to `url.domainToASCII` of Node: `Bücher.example` becomes
//! `xn--bcher-kva.example`. That function is the host parser of the WHATWG URL
//! Standard, which Node 24 runs on the ada library (4.0.0), and ada has ways of
//! its own: this is a port of what it does, not of what the standard says.
//! The `idna` crate follows the standard, and would write other addresses.
//!
//! Each function names the JavaScript or the C++ it ports, and
//! `tests/mime_differential.rs` compares the result with Node on addresses
//! built around every Unicode code point.

use std::borrow::Cow;
use std::fmt::Write as _;

use icu_normalizer::ComposingNormalizerBorrowed;
use icu_normalizer::properties::{CanonicalCombiningClassMap, CanonicalComposition};
use icu_normalizer::uts46::Uts46Mapper;

use super::mime_host_tables::{
    ARABIC_NUMBER, CHARACTERS, CLASS_MASK, COMBINING, EUROPEAN_NUMBER, INERT, JOINS_NEXT,
    JOINS_PREVIOUS, NEUTRAL, NONSPACING_MARK, OTHER, RIGHT_TO_LEFT, VIRAMA,
};

/// The address as it is written in a header and in the envelope, or `None`
/// for one nodemailer leaves out because nothing remains of it.
///
/// `_parseAddresses` normalizes the address of each object it is given, and
/// `_convertAddresses` normalizes the result once more. The second pass can
/// change a domain the first one could only encode by its fallback.
pub(super) fn header_address(address: &str) -> Option<String> {
    let once = normalize_address(address);
    if once.is_empty() {
        return None;
    }
    let twice = normalize_address(&once);
    (!twice.is_empty()).then_some(twice)
}

/// Port of `MimeNode._convertAddresses` for addresses without a display name.
pub(super) fn address_list(addresses: &[String]) -> String {
    let mut list = String::new();
    for address in addresses
        .iter()
        .filter_map(|address| header_address(address))
    {
        if !list.is_empty() {
            list.push_str(", ");
        }
        // Outside angle brackets, a comma or a semicolon in the address would
        // read as one more recipient than the envelope has.
        if is_plain_address(&address) {
            list.push_str(&address);
        } else {
            let _ = write!(list, "<{address}>");
        }
    }
    list
}

/// `PLAIN_ADDRESS` of mime-node: no special character on either side of one `@`.
fn is_plain_address(address: &str) -> bool {
    let is_special = |character: char| {
        is_js_space(character)
            || matches!(
                character,
                '"' | '(' | ')' | ',' | ':' | ';' | '<' | '>' | '[' | '\\' | ']'
            )
    };
    address.split_once('@').is_some_and(|(user, domain)| {
        !user.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && !address.chars().any(is_special)
    })
}

/// Port of `MimeNode._normalizeAddress`.
fn normalize_address(address: &str) -> String {
    // Control characters and angle brackets are turned into spaces, a run of
    // them into one.
    let mut cleaned = String::with_capacity(address.len());
    let mut in_run = false;
    for character in address.chars() {
        let is_removed = matches!(character, '\0'..='\x1f' | '\x7f' | '<' | '>');
        if !is_removed {
            cleaned.push(character);
        } else if !in_run {
            cleaned.push(' ');
        }
        in_run = is_removed;
    }
    let address = js_trim(&cleaned);

    let Some((user, domain)) = address.rsplit_once('@') else {
        return if address.is_empty() {
            String::new()
        } else {
            normalize_local_part(address).into_owned()
        };
    };

    // An address whose local part is not ASCII needs SMTPUTF8 anyway, so its
    // domain is written in Unicode as well. Otherwise the domain is encoded.
    let is_international = !user.is_ascii();
    let encoded = normalize_domain(&domain.to_lowercase(), is_international);
    // A domain the bundled codec cannot convert is kept as it was given.
    let domain = encoded.as_deref().unwrap_or(domain);
    format!("{}@{domain}", normalize_local_part(user))
}

/// Port of `MimeNode._normalizeLocalPart`: a local part that is neither a
/// dot-atom nor a quoted string goes out quoted.
fn normalize_local_part(user: &str) -> Cow<'_, str> {
    if is_dot_atom(user) || is_quoted_string(user) {
        Cow::Borrowed(user)
    } else {
        Cow::Owned(quote_string(user))
    }
}

/// `DOT_ATOM` of mime-node: atoms of RFC 5321 `atext` or of any character
/// outside ASCII, separated by single dots.
fn is_dot_atom(user: &str) -> bool {
    let is_atext = |character: char| {
        !character.is_ascii()
            || character.is_ascii_alphanumeric()
            || "!#$%&'*+-/=?^_`{|}~".contains(character)
    };
    user.split('.')
        .all(|atom| !atom.is_empty() && atom.chars().all(is_atext))
}

/// `QUOTED_STRING` of mime-node: between the outer quotes, a backslash pairs
/// with the character after it, and no quote stands alone.
fn is_quoted_string(user: &str) -> bool {
    let Some(inner) = user
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return false;
    };
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return false,
            // A backslash at the very end would escape the closing quote.
            '\\' if characters.next().is_none() => return false,
            _ => {}
        }
    }
    true
}

/// `quoteString` of mime-funcs.
fn quote_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

// Domains

/// Port of `normalizeDomain` of mime-node, for a domain already in lower
/// case. `None` is the codec of the fallback giving up, where nodemailer keeps
/// the domain as it was supplied.
fn normalize_domain(domain: &str, to_unicode: bool) -> Option<String> {
    // `URL_PARSER_UNSAFE`: what Node's host parser would cut the domain at,
    // drop or decode is kept away from it.
    let is_unsafe = domain
        .bytes()
        .any(|byte| matches!(byte, b'/' | b'\\' | b'?' | b'#' | b'%' | 0x00..=0x20 | 0x7f));
    if !is_unsafe && let Some(host) = url_host(domain) {
        return Some(if to_unicode {
            host_to_unicode(&host)
        } else {
            host
        });
    }

    // The codec nodemailer bundles maps nothing: a label is encoded as written.
    if to_unicode {
        punycode_to_unicode(domain)
    } else {
        punycode_to_ascii(domain)
    }
}

/// What `url.domainToASCII` of Node 24 returns, or `None` where it returns an
/// empty string. Node sets the value as the host name of a `ws:` URL, so this
/// is the host parser of the WHATWG URL Standard as the ada library (4.0.0)
/// runs it: the domain to ASCII, the forbidden code points, then the reading
/// of a host that ends in a number as an IPv4 address.
///
/// Not ported: a host in square brackets, which is an IPv6 address there. An
/// address cannot hold a bracket or a colon, and both fail here.
fn url_host(domain: &str) -> Option<String> {
    if domain.contains([':', '[', ']']) {
        return None;
    }
    // A domain that is ASCII is only put in lower case: its `xn--` labels are
    // not looked at.
    let host = if domain.is_ascii() {
        domain.to_ascii_lowercase()
    } else {
        domain_to_ascii(domain)?
    };
    let is_forbidden = |byte: u8| {
        !matches!(byte, 0x21..=0x7e)
            || matches!(
                byte,
                b'#' | b'%'
                    | b'/'
                    | b':'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'@'
                    | b'['
                    | b'\\'
                    | b']'
                    | b'^'
                    | b'|'
            )
    };
    if host.is_empty() || host.bytes().any(is_forbidden) {
        return None;
    }
    if ends_in_a_number(&host) {
        return ipv4_host(&host);
    }
    Some(host)
}

/// Port of `ada::idna::to_ascii` for a domain with a character outside ASCII:
/// UTS 46 processing as ada 4.0.0 does it, which is not how the standard and
/// the `idna` crate do it. ada checks the Bidi rule on each label by itself,
/// so a label without a right-to-left character is never held to it. It
/// stops checking a label at its first zero width joiner. It leaves some text
/// that is not normalized as it is. And its tables are older than the
/// characters it maps: see `mime_host_tables.rs`.
fn domain_to_ascii(domain: &str) -> Option<String> {
    let mapped = normalize_as_ada(uts46_map(domain.chars())?);

    let mut host = String::with_capacity(mapped.len() + 8);
    for (index, label) in mapped.split(|&character| character == '.').enumerate() {
        if index > 0 {
            host.push('.');
        }
        if let Some(encoded) = label.strip_prefix(&['x', 'n', '-', '-']) {
            // A label that is already encoded has to decode to a valid one.
            let encoded: String = encoded.iter().collect();
            decode_ace_label(&encoded)?;
            host.push_str("xn--");
            host.push_str(&encoded);
        } else if label.iter().all(char::is_ascii) {
            host.extend(label);
        } else {
            if !is_valid_label(label) {
                return None;
            }
            host.push_str("xn--");
            host.push_str(&punycode_encode(label)?);
        }
    }
    Some(host)
}

/// The mapping step of UTS 46, as `map` of ada: each character is kept,
/// replaced or dropped. `None` when one of them is disallowed.
fn uts46_map(text: impl Iterator<Item = char>) -> Option<Vec<char>> {
    // ICU4X maps and normalizes in one go, where ada looks at the text in
    // between. Character by character there is nothing to normalize: what
    // UTS 46 maps a character to is already in normal form.
    let mapper = Uts46Mapper::new();
    let mapped: Vec<char> = text
        .flat_map(|character| mapper.map_normalize(std::iter::once(character)))
        .collect();
    // What is disallowed comes out as U+FFFD, which is disallowed itself.
    (!mapped.contains(&char::REPLACEMENT_CHARACTER)).then_some(mapped)
}

/// What ada knows about a character: its flags in `mime_host_tables.rs`.
fn character_flags(character: char) -> u8 {
    let point = u32::from(character);
    CHARACTERS
        .binary_search_by(|&(start, end, _)| {
            if end < point {
                std::cmp::Ordering::Less
            } else if start > point {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .ok()
        .and_then(|index| CHARACTERS.get(index))
        .map_or(OTHER, |&(_, _, flags)| flags)
}

/// A character ada has no normalization data for, which it neither reorders
/// nor composes.
fn is_inert(character: char) -> bool {
    character_flags(character) & INERT != 0
}

/// The normalization step of UTS 46, as ada 4.0.0 does it: to NFC, unless
/// its quick check finds the text normalized already.
fn normalize_as_ada(mapped: Vec<char>) -> Vec<char> {
    if mapped.iter().all(char::is_ascii) || seems_normalized(&mapped) {
        return mapped;
    }
    // An inert character separates what is before it from what is after it,
    // so each stretch between two of them is normalized by itself.
    let normalizer = ComposingNormalizerBorrowed::new_nfc();
    let mut normalized = Vec::with_capacity(mapped.len());
    for stretch in mapped.split_inclusive(|&character| is_inert(character)) {
        let (text, inert) = match stretch.split_last() {
            Some((&last, text)) if is_inert(last) => (text, Some(last)),
            _ => (stretch, None),
        };
        normalized.extend(normalizer.normalize_iter(text.iter().copied()));
        normalized.extend(inert);
    }
    normalized
}

/// Port of `is_already_nfc` of ada 4.0.0, for mapped text: the combining
/// marks are in order and nothing would compose.
///
/// The check has two holes, which later versions of ada closed and Node 24
/// still has. A mark is not compared with the marks inside the composed
/// character before it, so "é" followed by a cedilla passes, where NFC makes
/// it "ȩ" followed by an acute accent. And a Hangul syllable without a final
/// consonant passes when one follows it as a letter of its own.
fn seems_normalized(text: &[char]) -> bool {
    const HANGUL_LEADING: std::ops::Range<u32> = 0x1100..0x1100 + 19;
    const HANGUL_VOWELS: std::ops::Range<u32> = 0x1161..0x1161 + 21;
    const HANGUL_TRAILING: std::ops::RangeInclusive<u32> = 0x11a8..=0x11c2;
    const HANGUL_SYLLABLES: std::ops::Range<u32> = 0xac00..0xac00 + 11172;
    /// Every 28th syllable has no final consonant.
    const HANGUL_FINALS: u32 = 28;
    let classes = CanonicalCombiningClassMap::new();
    let compositions = CanonicalComposition::new();
    let class = |character: char| {
        if is_inert(character) {
            0
        } else {
            classes.get_u8(character)
        }
    };

    let mut previous_class = 0;
    for &character in text {
        let class = class(character);
        if class != 0 && previous_class > class {
            return false;
        }
        previous_class = class;
    }

    // `would_compose`: each character that is not a combining mark, with the
    // marks after it and the character that ends them.
    let mut index = 0;
    while let Some(&current) = text.get(index) {
        let point = u32::from(current);
        let next = text.get(index + 1).map(|&character| u32::from(character));
        if HANGUL_LEADING.contains(&point) {
            if next.is_some_and(|next| HANGUL_VOWELS.contains(&next)) {
                return false;
            }
            index += 1;
            continue;
        }
        if HANGUL_SYLLABLES.contains(&point) {
            // The hole: this asks for a syllable that has a final consonant.
            let has_final = !(point - HANGUL_SYLLABLES.start).is_multiple_of(HANGUL_FINALS);
            if has_final && next.is_some_and(|next| HANGUL_TRAILING.contains(&next)) {
                return false;
            }
            index += 1;
            continue;
        }

        let mut blocking_class = None;
        let mut last = index;
        while let Some(&following) = text.get(last + 1) {
            let class = class(following);
            let is_blocked = blocking_class.is_some_and(|blocking| blocking >= class);
            let composes = !is_inert(current)
                && !is_inert(following)
                && compositions.compose(current, following).is_some();
            if !is_blocked && composes {
                return false;
            }
            if class == 0 {
                break;
            }
            blocking_class = Some(class);
            last += 1;
        }
        index = last + 1;
    }
    true
}

/// Port of `ada::idna::to_unicode`, which Node's `url.domainToUnicode` runs
/// on the host: each `xn--` label that decodes to a valid label is decoded,
/// and any other label is left as it is.
fn host_to_unicode(host: &str) -> String {
    let labels: Vec<Cow<'_, str>> = host
        .split('.')
        .map(|label| {
            label
                .strip_prefix("xn--")
                .and_then(decode_ace_label)
                .map_or(Cow::Borrowed(label), |decoded| {
                    Cow::Owned(decoded.iter().collect())
                })
        })
        .collect();
    labels.join(".")
}

/// The label that what follows `xn--` stands for, when ada takes it: it has a
/// character outside ASCII, UTS 46 mapping and normalization leave it as it
/// is, and it is a valid label.
fn decode_ace_label(encoded: &str) -> Option<Vec<char>> {
    // ada 4.0.0 skips a hyphen that starts the encoded text, which RFC 3492
    // reads as a digit and refuses.
    let points = punycode_points(encoded.as_bytes(), true)?;
    // A label encoded twice is refused.
    if points.starts_with(&[0x78, 0x6e, 0x2d, 0x2d]) {
        return None;
    }
    let label = points
        .into_iter()
        .map(|point| u32::try_from(point).ok().and_then(char::from_u32))
        .collect::<Option<Vec<char>>>()?;
    if label.iter().all(char::is_ascii) {
        return None;
    }
    let mapped = uts46_map(label.iter().copied()).filter(|mapped| *mapped == label)?;
    (normalize_as_ada(mapped) == label && is_valid_label(&label)).then_some(label)
}

/// Port of `is_label_valid` of ada 4.0.0, for a label that is mapped and
/// normalized.
fn is_valid_label(label: &[char]) -> bool {
    const ZERO_WIDTH_NON_JOINER: char = '\u{200c}';
    const ZERO_WIDTH_JOINER: char = '\u{200d}';
    let Some(&first) = label.first() else {
        return true;
    };
    if character_flags(first) & COMBINING != 0 {
        return false;
    }

    // The first joiner of the label settles it, and nothing after it is
    // looked at, the Bidi rule included. A joiner is fine after a virama. The
    // non-joiner is also fine with a joining character somewhere on each
    // side: ada does not ask for them to be next to it, as RFC 5892 does.
    if let Some(index) = label
        .iter()
        .position(|&character| matches!(character, ZERO_WIDTH_NON_JOINER | ZERO_WIDTH_JOINER))
    {
        let Some((before, joiner_and_after)) = label.split_at_checked(index) else {
            return false;
        };
        let has = |characters: &[char], flag: u8| {
            characters
                .iter()
                .any(|&character| character_flags(character) & flag != 0)
        };
        if has(before.last().map_or(&[], std::slice::from_ref), VIRAMA) {
            return true;
        }
        if joiner_and_after.first() == Some(&ZERO_WIDTH_JOINER) {
            return false;
        }
        let after = joiner_and_after.get(1..).unwrap_or_default();
        return has(before, JOINS_NEXT) && has(after, JOINS_PREVIOUS);
    }

    // The Bidi rule of RFC 5893, for a label with a right-to-left character.
    let class = |character: &char| character_flags(*character) & CLASS_MASK;
    let Some(last) = label
        .iter()
        .rposition(|character| class(character) != NONSPACING_MARK)
    else {
        return false;
    };
    let is_right_to_left = label
        .iter()
        .any(|character| matches!(class(character), RIGHT_TO_LEFT | ARABIC_NUMBER));
    if !is_right_to_left {
        return true;
    }
    // It starts with a right-to-left letter. A left-to-right start could only
    // be followed by left-to-right characters, and this label has another.
    if class(&first) != RIGHT_TO_LEFT {
        return false;
    }
    let mut has_european_number = false;
    let mut has_arabic_number = false;
    for (index, character) in label.iter().enumerate().take(last + 1) {
        let class = class(character);
        has_european_number |= class == EUROPEAN_NUMBER;
        has_arabic_number |= class == ARABIC_NUMBER;
        let is_allowed = matches!(
            class,
            RIGHT_TO_LEFT | ARABIC_NUMBER | EUROPEAN_NUMBER | NONSPACING_MARK | NEUTRAL
        );
        let ends_well =
            index < last || matches!(class, RIGHT_TO_LEFT | ARABIC_NUMBER | EUROPEAN_NUMBER);
        if !is_allowed || !ends_well || (has_european_number && has_arabic_number) {
            return false;
        }
    }
    true
}

/// The "ends in a number" check of the URL Standard, as `is_ipv4` of ada: the
/// last label, a trailing dot aside, is made of digits or is a hexadecimal number.
fn ends_in_a_number(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let last = host.rsplit('.').next().unwrap_or(host);
    if last.is_empty() {
        return false;
    }
    last.bytes().all(|byte| byte.is_ascii_digit())
        || last
            .strip_prefix("0x")
            .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// The IPv4 parser of the URL Standard, as `parse_ipv4` of ada. Up to four
/// numbers in decimal, octal (`010`) or hexadecimal (`0x10`), where the last
/// one fills the bytes that are left: `127.1` is `127.0.0.1`.
fn ipv4_host(host: &str) -> Option<String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 4 {
        return None;
    }

    let mut address: u64 = 0;
    let mut is_decimal = true;
    for (index, part) in parts.iter().enumerate() {
        let (digits, radix) = if let Some(hexadecimal) = part.strip_prefix("0x") {
            (hexadecimal, 16)
        } else if part.len() >= 2 && part.starts_with('0') {
            (part.get(1..)?, 8)
        } else {
            (*part, 10)
        };
        is_decimal &= radix == 10;
        // "0x" alone is zero. Anything else needs a digit, and takes no sign.
        let number = if digits.is_empty() && radix == 16 {
            0
        } else if digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            u64::from(u32::from_str_radix(digits, radix).ok()?)
        } else {
            return None;
        };

        let is_last = index + 1 == parts.len();
        let bits_left = 32 - 8 * u32::try_from(index).ok()?;
        if is_last {
            if number >= 1u64 << bits_left {
                return None;
            }
            address = (address << bits_left) | number;
        } else {
            if number > 255 {
                return None;
            }
            address = (address << 8) | number;
        }
    }

    // Four decimal numbers are already written the way they are serialized.
    if is_decimal && parts.len() == 4 {
        return Some(host.to_owned());
    }
    Some(format!(
        "{}.{}.{}.{}",
        (address >> 24) & 0xff,
        (address >> 16) & 0xff,
        (address >> 8) & 0xff,
        address & 0xff
    ))
}

// Punycode (RFC 3492), whose variable names these functions keep. nodemailer
// bundles punycode.js 2.3.1 for the domains Node's host parser refuses, and
// ada has a codec of its own. They encode alike and differ in what they
// decode.

const PUNYCODE_BASE: u64 = 36;
const PUNYCODE_T_MIN: u64 = 1;
const PUNYCODE_T_MAX: u64 = 26;
const PUNYCODE_SKEW: u64 = 38;
const PUNYCODE_DAMP: u64 = 700;
const PUNYCODE_INITIAL_BIAS: u64 = 72;
const PUNYCODE_INITIAL_N: u64 = 128;
/// The largest number both codecs count to before they give up.
const PUNYCODE_MAX_INT: u64 = 0x7fff_ffff;

/// The labels of a domain as `mapDomain` of punycode.js splits them: at a dot
/// or at one of the three other characters RFC 3490 reads as one.
fn punycode_labels(domain: &str) -> impl Iterator<Item = &str> {
    domain.split(['.', '\u{3002}', '\u{ff0e}', '\u{ff61}'])
}

/// `punycode.toASCII` of punycode.js.
fn punycode_to_ascii(domain: &str) -> Option<String> {
    let mut labels = Vec::new();
    for label in punycode_labels(domain) {
        labels.push(if label.is_ascii() {
            Cow::Borrowed(label)
        } else {
            let characters: Vec<char> = label.chars().collect();
            Cow::Owned(format!("xn--{}", punycode_encode(&characters)?))
        });
    }
    Some(labels.join("."))
}

/// `punycode.toUnicode` of punycode.js.
fn punycode_to_unicode(domain: &str) -> Option<String> {
    let mut labels = Vec::new();
    for label in punycode_labels(domain) {
        labels.push(match label.strip_prefix("xn--") {
            Some(encoded) => {
                let points = punycode_points(encoded.to_lowercase().as_bytes(), false)?;
                // `String.fromCodePoint` refuses what is past the last code
                // point and writes a surrogate as it is. Two of them side by
                // side then read as one character, and one alone is replaced
                // when the header is written.
                let mut units = Vec::with_capacity(points.len());
                let mut pair = [0; 2];
                for point in points {
                    match u32::try_from(point)
                        .ok()
                        .filter(|&point| point <= 0x0010_ffff)?
                    {
                        point @ 0xd800..=0xdfff => units.push(u16::try_from(point).ok()?),
                        point => {
                            units.extend_from_slice(char::from_u32(point)?.encode_utf16(&mut pair))
                        }
                    }
                }
                Cow::Owned(String::from_utf16_lossy(&units))
            }
            None => Cow::Borrowed(label),
        });
    }
    Some(labels.join("."))
}

fn punycode_threshold(k: u64, bias: u64) -> u64 {
    if k <= bias {
        PUNYCODE_T_MIN
    } else if k >= bias + PUNYCODE_T_MAX {
        PUNYCODE_T_MAX
    } else {
        k - bias
    }
}

fn punycode_adapt(delta: u64, points: u64, is_first: bool) -> u64 {
    let mut delta = if is_first {
        delta / PUNYCODE_DAMP
    } else {
        delta / 2
    };
    delta += delta / points;
    let mut k = 0;
    while delta > ((PUNYCODE_BASE - PUNYCODE_T_MIN) * PUNYCODE_T_MAX) / 2 {
        delta /= PUNYCODE_BASE - PUNYCODE_T_MIN;
        k += PUNYCODE_BASE;
    }
    k + ((PUNYCODE_BASE - PUNYCODE_T_MIN + 1) * delta) / (delta + PUNYCODE_SKEW)
}

/// 0 to 25 are `a` to `z`, 26 to 35 are `0` to `9`.
fn punycode_digit(digit: u64) -> char {
    let digit = u8::try_from(digit).unwrap_or(0);
    char::from(if digit < 26 {
        b'a' + digit
    } else {
        b'0' + (digit - 26)
    })
}

/// The encoder of both codecs. `None` is its overflow error.
fn punycode_encode(label: &[char]) -> Option<String> {
    let mut output: String = label
        .iter()
        .filter(|character| character.is_ascii())
        .collect();
    let basic = u64::try_from(output.len()).ok()?;
    let total = u64::try_from(label.len()).ok()?;
    if basic > 0 {
        output.push('-');
    }

    let points = || {
        label
            .iter()
            .map(|&character| u64::from(u32::from(character)))
    };
    let mut n = PUNYCODE_INITIAL_N;
    let mut delta: u64 = 0;
    let mut bias = PUNYCODE_INITIAL_BIAS;
    let mut handled = basic;
    while handled < total {
        let next = points().filter(|&point| point >= n).min()?;
        if next - n > (PUNYCODE_MAX_INT - delta) / (handled + 1) {
            return None;
        }
        delta += (next - n) * (handled + 1);
        n = next;
        for point in points() {
            if point < n {
                delta += 1;
                if delta > PUNYCODE_MAX_INT {
                    return None;
                }
            }
            if point == n {
                let mut q = delta;
                let mut k = PUNYCODE_BASE;
                loop {
                    let t = punycode_threshold(k, bias);
                    if q < t {
                        break;
                    }
                    output.push(punycode_digit(t + (q - t) % (PUNYCODE_BASE - t)));
                    q = (q - t) / (PUNYCODE_BASE - t);
                    k += PUNYCODE_BASE;
                }
                output.push(punycode_digit(q));
                bias = punycode_adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(output)
}

/// The decoder of both codecs, which gives code points: they may be
/// surrogates, or past the last one of Unicode. `None` is an error of the
/// codec.
///
/// Everything before the last hyphen is copied, and the rest are digits. With
/// its only hyphen in front, the text has nothing to copy: punycode.js then
/// reads the hyphen as a digit, which it is not, and ada skips it.
fn punycode_points(input: &[u8], skips_leading_hyphen: bool) -> Option<Vec<u64>> {
    if !input.is_ascii() {
        return None;
    }
    let (copied, digits) = match input.iter().rposition(|&byte| byte == b'-') {
        Some(0) if !skips_leading_hyphen => return None,
        Some(hyphen) => (input.get(..hyphen)?, input.get(hyphen + 1..)?),
        None => (&[][..], input),
    };
    let mut output: Vec<u64> = copied.iter().map(|&byte| u64::from(byte)).collect();

    let mut i: u64 = 0;
    let mut n = PUNYCODE_INITIAL_N;
    let mut bias = PUNYCODE_INITIAL_BIAS;
    let mut digits = digits.iter();
    let mut next = digits.next();
    while next.is_some() {
        let old_i = i;
        let mut w: u64 = 1;
        let mut k = PUNYCODE_BASE;
        loop {
            let digit = match *next? {
                byte @ b'0'..=b'9' => u64::from(byte - b'0') + 26,
                byte @ b'a'..=b'z' => u64::from(byte - b'a'),
                _ => return None,
            };
            next = digits.next();
            if digit > (PUNYCODE_MAX_INT - i) / w {
                return None;
            }
            i += digit * w;
            let t = punycode_threshold(k, bias);
            if digit < t {
                break;
            }
            if w > PUNYCODE_MAX_INT / (PUNYCODE_BASE - t) {
                return None;
            }
            w *= PUNYCODE_BASE - t;
            k += PUNYCODE_BASE;
        }
        let length = u64::try_from(output.len()).ok()? + 1;
        bias = punycode_adapt(i - old_i, length, old_i == 0);
        if i / length > PUNYCODE_MAX_INT - n {
            return None;
        }
        n += i / length;
        i %= length;
        output.insert(usize::try_from(i).ok()?, n);
        i += 1;
    }
    Some(output)
}

// JavaScript

/// `\s` of a JavaScript regular expression, which is also what `trim()` removes.
pub(super) fn is_js_space(character: char) -> bool {
    const SPACES: [char; 14] = [
        '\t', '\n', '\x0b', '\x0c', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2028}', '\u{2029}',
        '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
    ];
    SPACES.contains(&character) || ('\u{2000}'..='\u{200a}').contains(&character)
}

pub(super) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_space)
}
