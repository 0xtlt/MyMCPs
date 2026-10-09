//! Turns error text into a short diagnostic that is safe to persist or show
//! in the UI. Upstreams control their error text, so it is treated as
//! untrusted: common credential shapes are redacted before it crosses a
//! logging or presentation boundary.

use std::sync::LazyLock;

use regex::Regex;

use crate::crypto::Encryption;
use crate::models::Mcp;
use crate::secrets::{decrypt_environment, decrypt_secret};

const REDACTED: &str = "[REDACTED]";

/// The length of a diagnostic unless the caller asks for another.
pub const DEFAULT_DIAGNOSTIC_LIMIT: usize = 500;

/// Upstreams choose the size of their error text, so pattern redaction only
/// ever scans this many characters of it.
const PATTERN_SCAN_CHARS: usize = 8 * 1024;

const CREDENTIAL_KEYS: &[&str] = &[
    "authorization",
    "csrftoken",
    "googleaccessid",
    "jsessionid",
    "passphrase",
    "sessionid",
    "sig",
    "xsrftoken",
];

const CREDENTIAL_KEY_SUFFIXES: &[&str] = &[
    "accesskey",
    "accountkey",
    "apikey",
    "credential",
    "password",
    "privatekey",
    "secret",
    "sessionkey",
    "signature",
    "subscriptionkey",
    "token",
];

fn static_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid static pattern")
}

fn static_fancy_regex(pattern: &str) -> fancy_regex::Regex {
    fancy_regex::Regex::new(pattern).expect("a valid static pattern")
}

// The lookbehind covers every key character so that a key is only tried from
// its first character. Retrying from each "." or "-" made long runs quadratic.
static STRUCTURED_KEY_PATTERN: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    static_fancy_regex(r#"(?<![A-Za-z0-9_.-])(["']?([A-Za-z0-9_.-]+)["']?\s*[:=]\s*)"#)
});
static STRUCTURED_VALUE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r#"^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s,;&#}]+)"#));
static QUOTED_VALUE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r#"^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')"#));
static AUTH_SCHEME_PATTERN: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    static_fancy_regex(r#"(?i)(?<![A-Za-z0-9_])(?:Bearer|Basic)\s+(?!error\s*=)[^\s,;"']+"#)
});
static URL_CREDENTIALS_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"://[^\s/:@]+:[^\s/@]+@"));
static CUT_URL_CREDENTIALS_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"://[^\s/:@]+:[^\s/@]*$"));

/// `decodeURIComponent`, which fails on a malformed escape or on bytes that
/// are not UTF-8.
fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn is_credential_key(value: &str) -> bool {
    // Check the raw key when malformed percent escapes cannot be decoded.
    let decoded =
        decode_uri_component(&value.replace('+', " ")).unwrap_or_else(|| value.to_string());
    let normalized: String = decoded
        .to_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    CREDENTIAL_KEYS.contains(&normalized.as_str())
        || CREDENTIAL_KEY_SUFFIXES
            .iter()
            .any(|suffix| normalized.ends_with(suffix))
}

/// `encodeURIComponent`.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// A value as `URLSearchParams` serialises it.
fn form_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"*-._".contains(&byte) {
            encoded.push(byte as char);
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Each secret as it may appear in a diagnostic: raw, inside a JSON string,
/// and URL-encoded both ways. Longest first, so that a secret is not left
/// half redacted by a shorter variant of itself.
fn exact_secret_variants<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut variants: Vec<String> = Vec::new();
    let mut add = |variant: String| {
        if !variants.contains(&variant) {
            variants.push(variant);
        }
    };
    for value in values {
        if value.is_empty() {
            continue;
        }
        add(value.to_string());

        let json = serde_json::Value::String(value.to_string()).to_string();
        add(json[1..json.len() - 1].to_string());
        add(encode_uri_component(value));
        add(form_encode(value));
    }
    variants.sort_by_key(|variant| std::cmp::Reverse(variant.encode_utf16().count()));
    variants
}

/// The first `limit` UTF-16 code units of the text, as `String.slice` counts
/// them, without cutting a character in two.
fn truncate(text: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > limit {
            return &text[..index];
        }
    }
    text
}

/// Scan assignments without consuming values for noncredential wrapper
/// labels. This allows nested forms such as `Response: {"token":"..."}` and
/// `Cookie: session_token=...` to be inspected at their inner key.
fn redact_structured_credential_assignments(diagnostic: &str, truncated: bool) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    let mut position = 0;

    while let Ok(Some(captures)) = STRUCTURED_KEY_PATTERN.captures_from_pos(diagnostic, position) {
        let (Some(whole), Some(key)) = (captures.get(0), captures.get(2)) else {
            break;
        };
        position = whole.end();
        if !is_credential_key(key.as_str()) {
            continue;
        }

        let value_start = whole.end();
        let rest = &diagnostic[value_start..];
        let Some(value) = STRUCTURED_VALUE_PATTERN.find(rest) else {
            continue;
        };

        // A quoted value that lost its closing quote to the cut runs to the end.
        let unterminated = truncated
            && (rest.starts_with('"') || rest.starts_with('\''))
            && !QUOTED_VALUE_PATTERN.is_match(rest);
        let value_length = if unterminated {
            rest.len()
        } else {
            value.end()
        };

        output.push_str(&diagnostic[cursor..value_start]);
        output.push_str(REDACTED);
        cursor = value_start + value_length;
        position = cursor;
    }

    if cursor == 0 {
        return diagnostic.to_string();
    }
    output.push_str(&diagnostic[cursor..]);
    output
}

/// `truncated` says the text was cut short, so a credential may be missing
/// the closing delimiter that would otherwise identify it.
fn redact_pattern_credentials(diagnostic: &str, truncated: bool) -> String {
    let mut redacted = String::new();
    let mut cursor = 0;
    for found in AUTH_SCHEME_PATTERN.find_iter(diagnostic).flatten() {
        let scheme = found
            .as_str()
            .split(char::is_whitespace)
            .next()
            .unwrap_or("");
        redacted.push_str(&diagnostic[cursor..found.start()]);
        redacted.push_str(scheme);
        redacted.push(' ');
        redacted.push_str(REDACTED);
        cursor = found.end();
    }
    redacted.push_str(&diagnostic[cursor..]);

    let mut redacted = URL_CREDENTIALS_PATTERN
        .replace_all(&redacted, "://[REDACTED]@")
        .into_owned();
    if truncated {
        redacted = CUT_URL_CREDENTIALS_PATTERN
            .replace(&redacted, "://[REDACTED]")
            .into_owned();
    }
    redact_structured_credential_assignments(&redacted, truncated)
}

/// Redact a diagnostic and cut it to `limit` characters. `sensitive_values`
/// are secrets known to the caller, removed wherever they appear.
pub fn sanitize_diagnostic_with<'a>(
    raw: &str,
    limit: usize,
    sensitive_values: impl IntoIterator<Item = &'a str>,
) -> String {
    // Known secrets are replaced over the whole text, in linear time, so that
    // none is left half-cut at the edge of the part the patterns then scan.
    let mut sanitized = raw.to_string();
    for secret in exact_secret_variants(sensitive_values) {
        if sanitized.contains(&secret) {
            sanitized = sanitized.replace(&secret, REDACTED);
        }
    }

    let scan_length = limit.max(PATTERN_SCAN_CHARS);
    let scanned = truncate(&sanitized, scan_length);
    let was_cut = scanned.len() < sanitized.len();
    truncate(&redact_pattern_credentials(scanned, was_cut), limit).to_string()
}

/// [`sanitize_diagnostic_with`] at the default length, with no known secret.
pub fn sanitize_diagnostic(raw: &str) -> String {
    sanitize_diagnostic_with(raw, DEFAULT_DIAGNOSTIC_LIMIT, [])
}

/// Decrypt the credentials associated with one MCP only long enough to
/// redact their exact values from diagnostics.
pub fn mcp_sensitive_values(encryption: &Encryption, mcp: &Mcp) -> Vec<String> {
    let mut values: Vec<String> = [
        &mcp.auth_bearer,
        &mcp.auth_header_value,
        &mcp.oauth_client_secret,
        &mcp.oauth_access_token,
        &mcp.oauth_refresh_token,
        &mcp.builtin_password,
    ]
    .into_iter()
    .filter_map(|column| decrypt_secret(encryption, column.as_deref()))
    .collect();

    // A corrupt encrypted environment is already reported by the caller. Do
    // not let diagnostic sanitization replace that error with another one.
    if let Ok(environment) = decrypt_environment(encryption, mcp.npm_env.as_deref()) {
        values.extend(environment.into_iter().map(|(_, value)| value));
    }
    values
}

/// Redact a diagnostic about one MCP, including the exact values of its
/// saved credentials.
pub fn sanitize_mcp_diagnostic(encryption: &Encryption, raw: &str, mcp: &Mcp) -> String {
    sanitize_mcp_diagnostic_with(encryption, raw, mcp, DEFAULT_DIAGNOSTIC_LIMIT, [])
}

pub fn sanitize_mcp_diagnostic_with<'a>(
    encryption: &Encryption,
    raw: &str,
    mcp: &Mcp,
    limit: usize,
    additional_sensitive_values: impl IntoIterator<Item = &'a str>,
) -> String {
    let known = mcp_sensitive_values(encryption, mcp);
    let additional: Vec<&str> = additional_sensitive_values.into_iter().collect();
    let sensitive: Vec<&str> = known
        .iter()
        .map(String::as_str)
        .chain(additional.iter().copied())
        .collect();
    sanitize_diagnostic_with(raw, limit, sensitive)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::config::TEST_APP_KEY;
    use crate::secrets::{EnvironmentInput, encrypt_secret, merge_environment};

    const ONE_MEGABYTE: usize = 1024 * 1024;

    #[test]
    fn sanitizes_a_one_megabyte_hostile_diagnostic_without_blocking_the_process() {
        // Shapes that keep a pattern scanning without ever completing a match.
        let hostile_units = [
            "a.",
            "a-",
            "a ",
            "\"",
            "\"a",
            "Bearer ",
            "Bearer error ",
            "://a:",
            "://a:b",
            "token=\"",
            "token='x ",
            "token=\"a\\\" ",
        ];
        for unit in hostile_units {
            let hostile = unit.repeat(ONE_MEGABYTE.div_ceil(unit.len()));
            let started_at = Instant::now();
            let sanitized = sanitize_diagnostic_with(&hostile, 500, ["configured-secret"]);
            let elapsed = started_at.elapsed();

            assert!(sanitized.encode_utf16().count() <= 500);
            // The Node app allowed 250 ms; a debug build gets more room.
            assert!(
                elapsed.as_millis() < 2_000,
                "sanitizing {unit:?} runs took {elapsed:?}"
            );
        }
    }

    #[test]
    fn redacts_a_known_secret_wherever_it_appears_before_the_text_is_cut() {
        let secret = "opaque-value-without-a-credential-shape";
        let filler = "x".repeat(480);

        let sanitized =
            sanitize_diagnostic_with(&format!("{filler} {secret} trailing text"), 500, [secret]);

        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains(&secret[..8]));
    }

    #[test]
    fn redacts_credentials_that_the_scan_limit_cut_short() {
        // A long redacted value pulls the end of the scanned text into the result.
        let padding = format!("token={}", "t".repeat(8 * 1024 - 60));
        let overflow = "y".repeat(1024);

        let quoted = sanitize_diagnostic(&format!(
            "{padding} \"password\":\"correct horse battery staple and more words {overflow}\""
        ));
        assert!(!quoted.contains("horse"));
        assert!(!quoted.contains("battery"));

        let url = sanitize_diagnostic(&format!(
            "{padding} https://user:url-password-{}{overflow}@example.test/mcp",
            "z".repeat(60)
        ));
        assert!(!url.contains("url-password"));
        assert!(url.contains("https://[REDACTED]"));
    }

    #[test]
    fn keeps_host_and_port_readable_at_the_end_of_an_untruncated_diagnostic() {
        assert_eq!(
            sanitize_diagnostic("connect ECONNREFUSED http://127.0.0.1:9999"),
            "connect ECONNREFUSED http://127.0.0.1:9999"
        );
        assert_eq!(
            sanitize_diagnostic("Unexpected token: \"abc def"),
            "Unexpected token: [REDACTED] def"
        );
    }

    #[test]
    fn redacts_structured_and_header_shaped_credentials_from_diagnostics() {
        let stringified = serde_json::json!({ "client_secret": "stringified-secret" }).to_string();
        let sanitized = sanitize_diagnostic(&format!(
            "Authorization: Bearer abc123 {{\"client_secret\":\"oauth-secret\",\"access_token\":\"access-secret\"}} https://example.test?X-Amz-Signature=aws-secret&code=useful-code session_id=session-secret Set-Cookie: jsessionid=session-cookie Stringified: {stringified} WWW-Authenticate: Bearer error=\"invalid_token\""
        ));
        for secret in [
            "abc123",
            "oauth-secret",
            "access-secret",
            "aws-secret",
            "session-secret",
            "session-cookie",
            "stringified-secret",
        ] {
            assert!(!sanitized.contains(secret), "{secret} in {sanitized}");
        }
        assert!(sanitized.contains("code=useful-code"));
        assert!(sanitized.contains("Bearer error=\"invalid_token\""));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_exact_custom_header_and_npm_environment_values() {
        let encryption = Encryption::new(TEST_APP_KEY);
        let header_secret = "p\"ass word";
        let environment_secret = "line\\break\nnext value";
        let mcp = Mcp {
            auth_header_value: encrypt_secret(&encryption, Some(header_secret)),
            npm_env: merge_environment(
                &encryption,
                None,
                &[EnvironmentInput {
                    name: "CUSTOM_CREDENTIAL".into(),
                    value: Some(environment_secret.into()),
                }],
            ),
            ..Default::default()
        };

        let json_diagnostic =
            serde_json::json!({ "arbitrary": header_secret, "another": environment_secret })
                .to_string();
        let form_encoded_header = form_encode(header_secret);
        let form_encoded_environment = form_encode(environment_secret);
        assert_eq!(form_encoded_header, "p%22ass+word");

        let sanitized = sanitize_mcp_diagnostic(
            &encryption,
            &format!(
                "{json_diagnostic} header={form_encoded_header} env={form_encoded_environment}"
            ),
            &mcp,
        );
        for leaked in [
            "p\\\"ass word",
            "line\\\\break\\nnext value",
            header_secret,
            environment_secret,
            form_encoded_header.as_str(),
            form_encoded_environment.as_str(),
        ] {
            assert!(!sanitized.contains(leaked), "{leaked} in {sanitized}");
        }
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_long_secrets_before_truncating() {
        let encryption = Encryption::new(TEST_APP_KEY);
        let secret = format!("opaque-{}-tail", "x".repeat(400));
        let mcp = Mcp {
            npm_env: merge_environment(
                &encryption,
                None,
                &[EnvironmentInput {
                    name: "OPAQUE_VALUE".into(),
                    value: Some(secret.clone()),
                }],
            ),
            ..Default::default()
        };

        let sanitized =
            sanitize_mcp_diagnostic(&encryption, &format!("startup echoed {secret}"), &mcp);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains(&secret[..300]));
    }

    #[test]
    fn recognises_credential_keys() {
        for key in [
            "Authorization",
            "api_key",
            "X-Amz-Signature",
            "client%5Fsecret",
            "session+token",
            "SIG",
        ] {
            assert!(is_credential_key(key), "{key}");
        }
        for key in ["code", "status", "signal", "%zz"] {
            assert!(!is_credential_key(key), "{key}");
        }
    }
}
