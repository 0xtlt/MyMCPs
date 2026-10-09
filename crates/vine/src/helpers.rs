//! `vine.helpers`: the checks Vine's rules are made of. The ones about
//! addresses are ports of validator.js 13.15, which Vine calls for `email()`
//! and `url()`. They follow it decision by decision, UTF-16 code unit by
//! code unit, so that the same strings are accepted and refused.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::js;

/// `helpers.exists`: neither `null` nor `undefined` (`None`).
pub fn exists(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null())
}

/// `helpers.isMissing`: `null` or `undefined` (`None`).
pub fn is_missing(value: Option<&Value>) -> bool {
    !exists(value)
}

/// `helpers.asBoolean`: `true`, `1`, `"1"`, `"true"` and `"on"` are true;
/// `false`, `0`, `"0"` and `"false"` are false; anything else is neither.
pub fn as_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => match number.as_f64() {
            Some(1.0) => Some(true),
            Some(0.0) => Some(false),
            _ => None,
        },
        Value::String(text) => match text.as_str() {
            "1" | "true" | "on" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `helpers.asNumber`: `Number(value)`, except that `null` is not a number.
pub fn as_number(value: &Value) -> f64 {
    if value.is_null() {
        f64::NAN
    } else {
        js::to_number(Some(value))
    }
}

const IPV4_SEGMENT: &str = "(?:[0-9]|[1-9][0-9]|1[0-9][0-9]|2[0-4][0-9]|25[0-5])";

static IPV4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("^({IPV4_SEGMENT}[.]){{3}}{IPV4_SEGMENT}$")).expect("static regex")
});

static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    let v4 = format!("({IPV4_SEGMENT}[.]){{3}}{IPV4_SEGMENT}");
    let s = "(?:[0-9a-fA-F]{1,4})";
    let pattern = [
        "^(".to_owned(),
        format!("(?:{s}:){{7}}(?:{s}|:)|"),
        format!("(?:{s}:){{6}}(?:{v4}|:{s}|:)|"),
        format!("(?:{s}:){{5}}(?::{v4}|(:{s}){{1,2}}|:)|"),
        format!("(?:{s}:){{4}}(?:(:{s}){{0,1}}:{v4}|(:{s}){{1,3}}|:)|"),
        format!("(?:{s}:){{3}}(?:(:{s}){{0,2}}:{v4}|(:{s}){{1,4}}|:)|"),
        format!("(?:{s}:){{2}}(?:(:{s}){{0,3}}:{v4}|(:{s}){{1,5}}|:)|"),
        format!("(?:{s}:){{1}}(?:(:{s}){{0,4}}:{v4}|(:{s}){{1,6}}|:)|"),
        format!("(?::((?::{s}){{0,5}}:{v4}|(?::{s}){{1,7}}|:))"),
        ")(%[0-9a-zA-Z.]{1,})?$".to_owned(),
    ]
    .concat();
    Regex::new(&pattern).expect("static regex")
});

/// `isIP(text, 4)`.
pub fn is_ipv4(text: &str) -> bool {
    IPV4.is_match(text)
}

/// `isIP(text, 6)`.
pub fn is_ipv6(text: &str) -> bool {
    IPV6.is_match(text)
}

/// `isIP(text)`: an IPv4 or an IPv6 address.
pub fn is_ip(text: &str) -> bool {
    is_ipv4(text) || is_ipv6(text)
}

/// Options of [`is_fqdn`], with validator.js's names and defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FqdnOptions {
    pub require_tld: bool,
    pub allow_underscores: bool,
    pub allow_trailing_dot: bool,
    pub allow_numeric_tld: bool,
    pub allow_wildcard: bool,
    pub ignore_max_length: bool,
}

impl Default for FqdnOptions {
    fn default() -> Self {
        Self {
            require_tld: true,
            allow_underscores: false,
            allow_trailing_dot: false,
            allow_numeric_tld: false,
            allow_wildcard: false,
            ignore_max_length: false,
        }
    }
}

/// `¡-퟿豈-﷏ﷰ-￯`, with or without U+00A9 by `skip`.
fn is_international(unit: u16, skip_copyright: bool) -> bool {
    match unit {
        0x00A9 => !skip_copyright,
        0x00A1..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFEF => true,
        _ => false,
    }
}

fn is_ascii_alphanumeric(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_alphanumeric())
}

fn is_ascii_alphabetic(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_alphabetic())
}

/// `/^([a-z¡-¨ª-퟿豈-﷏ﷰ-￯]{2,}|xn[a-z0-9-]{2,})$/i`
fn is_tld(tld: &str) -> bool {
    let units: Vec<u16> = tld.encode_utf16().collect();
    let letters = units.len() >= 2
        && units
            .iter()
            .all(|unit| is_ascii_alphabetic(*unit) || is_international(*unit, true));
    let punycode = units.len() >= 4
        && (units[0] == u16::from(b'x') || units[0] == u16::from(b'X'))
        && (units[1] == u16::from(b'n') || units[1] == u16::from(b'N'))
        && units[2..]
            .iter()
            .all(|unit| is_ascii_alphanumeric(*unit) || *unit == u16::from(b'-'));
    letters || punycode
}

/// `isFQDN(text, options)`.
pub fn is_fqdn(text: &str, options: &FqdnOptions) -> bool {
    let mut text = text;
    if options.allow_trailing_dot {
        text = text.strip_suffix('.').unwrap_or(text);
    }
    if options.allow_wildcard {
        text = text.strip_prefix("*.").unwrap_or(text);
    }
    let parts: Vec<&str> = text.split('.').collect();
    let tld = parts.last().copied().unwrap_or("");
    if options.require_tld {
        if parts.len() < 2 {
            return false;
        }
        if !options.allow_numeric_tld && !is_tld(tld) {
            return false;
        }
        if tld.chars().any(js::is_whitespace) {
            return false;
        }
    }
    if !options.allow_numeric_tld
        && !tld.is_empty()
        && tld.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    parts.iter().all(|part| {
        if js::utf16_len(part) > 63 && !options.ignore_max_length {
            return false;
        }
        // `/^[a-z_¡-￿0-9-]+$/i`
        let allowed = |unit: u16| {
            is_ascii_alphanumeric(unit)
                || unit == u16::from(b'_')
                || unit == u16::from(b'-')
                || unit >= 0x00A1
        };
        if part.is_empty() || !part.encode_utf16().all(allowed) {
            return false;
        }
        // Full-width characters.
        if part
            .encode_utf16()
            .any(|unit| (0xFF01..=0xFF5E).contains(&unit))
        {
            return false;
        }
        if part.starts_with('-') || part.ends_with('-') {
            return false;
        }
        options.allow_underscores || !part.contains('_')
    })
}

/// Options of [`is_url`] and of `vine.string().url(...)`, with validator.js's
/// names and defaults. `host_whitelist` and `host_blacklist` are not ported:
/// no validator of the app uses them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlOptions {
    pub protocols: Vec<String>,
    pub require_tld: bool,
    pub require_protocol: bool,
    pub require_host: bool,
    pub require_port: bool,
    pub require_valid_protocol: bool,
    pub allow_underscores: bool,
    pub allow_trailing_dot: bool,
    pub allow_protocol_relative_urls: bool,
    pub allow_fragments: bool,
    pub allow_query_components: bool,
    pub disallow_auth: bool,
    pub validate_length: bool,
    pub max_allowed_length: usize,
}

impl Default for UrlOptions {
    fn default() -> Self {
        Self {
            protocols: vec!["http".to_owned(), "https".to_owned(), "ftp".to_owned()],
            require_tld: true,
            require_protocol: false,
            require_host: true,
            require_port: false,
            require_valid_protocol: true,
            allow_underscores: false,
            allow_trailing_dot: false,
            allow_protocol_relative_urls: false,
            allow_fragments: true,
            allow_query_components: true,
            disallow_auth: false,
            validate_length: true,
            max_allowed_length: 2084,
        }
    }
}

/// The length of what `/^([a-z][a-z0-9+\-.]*):/i` matches, colon included.
fn scheme_length(url: &str) -> Option<usize> {
    let bytes = url.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let name = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        .count();
    (bytes.get(name) == Some(&b':')).then_some(name + 1)
}

/// `/%[0-9a-fA-F]{2}/`
fn has_percent_encoding(text: &str) -> bool {
    text.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && window[1].is_ascii_hexdigit() && window[2].is_ascii_hexdigit()
    })
}

/// `/^\[([^\]]+)\](?::([0-9]+))?$/`: the address between the brackets and
/// the port after them.
fn wrapped_ipv6(hostname: &str) -> Option<(&str, Option<&str>)> {
    let rest = hostname.strip_prefix('[')?;
    let close = rest.find(']')?;
    if close == 0 {
        return None;
    }
    let address = &rest[..close];
    let after = &rest[close + 1..];
    if after.is_empty() {
        return Some((address, None));
    }
    let port = after.strip_prefix(':')?;
    (!port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some((address, Some(port)))
}

/// `isURL(text, options)`.
pub fn is_url(text: &str, options: &UrlOptions) -> bool {
    if text.is_empty()
        || text
            .chars()
            .any(|c| js::is_whitespace(c) || c == '<' || c == '>')
    {
        return false;
    }
    if text.starts_with("mailto:") {
        return false;
    }
    if options.validate_length && js::utf16_len(text) > options.max_allowed_length {
        return false;
    }
    if !options.allow_fragments && text.contains('#') {
        return false;
    }
    if !options.allow_query_components && (text.contains('?') || text.contains('&')) {
        return false;
    }
    let mut url = text.split('#').next().unwrap_or("");
    url = url.split('?').next().unwrap_or("");

    // A colon before an `@` may belong to `user:password@host`, and one
    // before digits to `host:port`: neither names a protocol.
    let mut had_explicit_protocol = false;
    if let Some(end) = scheme_length(url) {
        let after_colon = &url[end..];
        let is_protocol = if after_colon.starts_with("//") {
            true
        } else {
            let before_slash = after_colon.split('/').next().unwrap_or("");
            match before_slash.find('@') {
                Some(at) => {
                    let before_at = &before_slash[..at];
                    let is_valid_auth = before_at.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.' | b'%' | b':')
                    });
                    // Encoded content before the `@` is how a script URL hides.
                    !(is_valid_auth && !has_percent_encoding(before_at))
                }
                None => !after_colon
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_digit),
            }
        };
        if is_protocol {
            had_explicit_protocol = true;
            let protocol = url[..end - 1].to_ascii_lowercase();
            if options.require_valid_protocol && !options.protocols.contains(&protocol) {
                return false;
            }
            url = after_colon;
        } else if options.require_protocol {
            return false;
        }
    } else if options.require_protocol {
        return false;
    }

    if let Some(rest) = url.strip_prefix("//") {
        if !had_explicit_protocol && !options.allow_protocol_relative_urls {
            return false;
        }
        url = rest;
    }
    if url.is_empty() {
        return false;
    }
    let authority = url.split('/').next().unwrap_or("");
    if authority.is_empty() && !options.require_host {
        return true;
    }

    let mut pieces: Vec<&str> = authority.split('@').collect();
    if pieces.len() > 1 {
        if options.disallow_auth {
            return false;
        }
        let auth = pieces.remove(0);
        if auth.is_empty() {
            return false;
        }
        let mut credentials = auth.split(':');
        let (user, password) = (credentials.next(), credentials.next());
        if credentials.next().is_some() {
            return false;
        }
        if user == Some("") && password == Some("") {
            return false;
        }
    }
    let hostname = pieces.join("@");

    let (host, ipv6, port) = match wrapped_ipv6(&hostname) {
        Some((address, port)) => ("", Some(address), port),
        None => match hostname.split_once(':') {
            Some((host, port)) => (host, None, Some(port)),
            None => (hostname.as_str(), None, None),
        },
    };
    match port.filter(|port| !port.is_empty()) {
        Some(port) => {
            if !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            let digits = port.trim_start_matches('0');
            let number = if digits.len() > 5 {
                u32::MAX
            } else {
                digits.parse().unwrap_or(0)
            };
            if number == 0 || number > 65535 {
                return false;
            }
        }
        None if options.require_port => return false,
        None => {}
    }
    if host.is_empty() && !options.require_host {
        return true;
    }
    let fqdn = FqdnOptions {
        require_tld: options.require_tld,
        allow_underscores: options.allow_underscores,
        allow_trailing_dot: options.allow_trailing_dot,
        ..FqdnOptions::default()
    };
    is_ip(host) || is_fqdn(host, &fqdn) || ipv6.is_some_and(is_ipv6)
}

/// ``/^[a-z\d!#\$%&'\*\+\-\/=\?\^_`{\|}~¡-퟿豈-﷏ﷰ-￯]+$/i``
fn is_local_part_unit(unit: u16) -> bool {
    is_ascii_alphanumeric(unit)
        || u8::try_from(unit).is_ok_and(|byte| b"!#$%&'*+-/=?^_`{|}~".contains(&byte))
        || is_international(unit, false)
}

/// The local part between double quotes: characters of the first class, or
/// a backslash followed by one of the second.
fn is_quoted_local_part(units: &[u16]) -> bool {
    let wide = |unit: u16| matches!(unit, 0x00A0..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFEF);
    let plain = |unit: u16| {
        matches!(unit, 0x01..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F | 0x21 | 0x23..=0x5B | 0x5D..=0x7E)
            || char::from_u32(u32::from(unit)).is_some_and(js::is_whitespace)
            || wide(unit)
    };
    let escaped = |unit: u16| matches!(unit, 0x01..=0x09 | 0x0B | 0x0C | 0x0D..=0x7F) || wide(unit);
    let mut at = 0;
    while at < units.len() {
        if units[at] == u16::from(b'\\') {
            if !units.get(at + 1).copied().is_some_and(escaped) {
                return false;
            }
            at += 2;
        } else if plain(units[at]) {
            at += 1;
        } else {
            return false;
        }
    }
    true
}

/// `isEmail(text)` with validator.js's default options, which is what
/// `vine.string().email()` runs.
pub fn is_email(text: &str) -> bool {
    if js::utf16_len(text) > 254 {
        return false;
    }
    let (user, domain) = match text.rfind('@') {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => ("", text),
    };
    // In bytes of UTF-8, where the checks above count UTF-16 code units.
    if user.len() > 64 || domain.len() > 254 {
        return false;
    }
    let fqdn = FqdnOptions {
        require_tld: true,
        ..FqdnOptions::default()
    };
    if !is_fqdn(domain, &fqdn) {
        return false;
    }
    let units: Vec<u16> = user.encode_utf16().collect();
    let quote = u16::from(b'"');
    if units.first() == Some(&quote) && units.last() == Some(&quote) {
        let inner = if units.len() >= 2 {
            &units[1..units.len() - 1]
        } else {
            &[][..]
        };
        return is_quoted_local_part(inner);
    }
    user.split('.')
        .all(|part| !part.is_empty() && part.encode_utf16().all(is_local_part_unit))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_booleans_the_way_html_forms_write_them() {
        assert_eq!(as_boolean(&json!("on")), Some(true));
        assert_eq!(as_boolean(&json!(1.0)), Some(true));
        assert_eq!(as_boolean(&json!("0")), Some(false));
        assert_eq!(as_boolean(&json!("off")), None);
        assert_eq!(as_boolean(&json!("TRUE")), None);
    }

    #[test]
    fn checks_addresses() {
        assert!(is_ip("127.0.0.1"));
        assert!(is_ip("::1"));
        assert!(!is_ip("256.0.0.1"));
        assert!(is_email("name@example.com"));
        assert!(!is_email("name@localhost"));
        assert!(is_url(
            "https://example.com/path?x=1#y",
            &UrlOptions::default()
        ));
        assert!(!is_url(
            "http://localhost:3333/callback",
            &UrlOptions::default()
        ));
        assert!(is_url(
            "http://localhost:3333/callback",
            &UrlOptions {
                require_tld: false,
                ..UrlOptions::default()
            }
        ));
    }
}
