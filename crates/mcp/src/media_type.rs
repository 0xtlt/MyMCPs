//! `Content-Type` comparison as the SDK does it (`shared/mediaType.js`), and
//! header reading as a `fetch` `Headers` object does it.

use std::sync::LazyLock;

use http::HeaderMap;
use regex::Regex;

/// A header as `headers.get(name)` returns it: the values of repeated
/// headers joined with `, `, bytes read as Latin-1.
pub(crate) fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter().peekable();
    values.peek()?;
    let mut joined = String::new();
    for (index, value) in values.enumerate() {
        if index > 0 {
            joined.push_str(", ");
        }
        joined.extend(value.as_bytes().iter().map(|byte| char::from(*byte)));
    }
    Some(joined)
}

static TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+/[!#$%&'*+.^_`|~0-9A-Za-z-]+$")
        .expect("the media type pattern is a valid regex")
});

/// One `; name=value` parameter, anchored where the previous one ended.
static PARAMETER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^; *([!#$%&'*+.^_`|~0-9A-Za-z-]+) *= *(\"(?:[\\x0b\\x20\\x21\\x23-\\x5b\\x5d-\\x7e\\u{80}-\\u{ff}]|\\\\[\\x0b\\x20-\\u{ff}])*\"|[!#$%&'*+.^_`|~0-9A-Za-z-]+) *",
    )
    .expect("the media type parameter pattern is a valid regex")
});

/// The lowercased `type/subtype` of a `Content-Type` header, without its
/// parameters. `None` when the header is missing, empty or ambiguous.
///
/// Never a substring search: `text/plain; a=application/json` is `text/plain`.
pub(crate) fn media_type_essence(header: Option<&str>) -> Option<String> {
    let header = header.filter(|header| !header.is_empty())?;
    if let Some(essence) = parse_strictly(header) {
        return Some(essence);
    }

    // A header whose parameters are malformed still names its media type
    // before the first `;`, as browsers and most HTTP stacks read it.
    let essence = header
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    // A comma after the media type means duplicate headers were joined: ambiguous.
    let rest: String = header.chars().skip(essence.chars().count()).collect();
    if essence.is_empty() || rest.contains(',') {
        return None;
    }
    Some(essence)
}

/// The `content-type` package: RFC 9110, parameters included.
fn parse_strictly(header: &str) -> Option<String> {
    let (media_type, mut parameters) = match header.find(';') {
        Some(index) => (header[..index].trim(), &header[index..]),
        None => (header.trim(), ""),
    };
    if !TYPE.is_match(media_type) {
        return None;
    }
    while !parameters.is_empty() {
        let matched = PARAMETER.find(parameters)?;
        parameters = &parameters[matched.end()..];
    }
    Some(media_type.to_lowercase())
}

/// Whether a `Content-Type` header denotes `application/json`, whatever its parameters.
pub(crate) fn is_json_content_type(header: Option<&str>) -> bool {
    media_type_essence(header).as_deref() == Some("application/json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_media_type_and_never_a_substring() {
        for (header, expected) in [
            ("application/json", Some("application/json")),
            ("Application/JSON; charset=utf-8", Some("application/json")),
            (
                "application/json;charset=\"utf-8\"",
                Some("application/json"),
            ),
            ("  application/json  ", Some("application/json")),
            (
                "text/event-stream; charset=utf-8",
                Some("text/event-stream"),
            ),
            ("text/plain; a=application/json", Some("text/plain")),
            // Malformed parameters do not hide an unambiguous media type.
            ("application/json;", Some("application/json")),
            ("application/json; charset=", Some("application/json")),
            // Joined duplicate headers.
            ("application/json; charset=, text/plain", None),
            (
                "application/json, text/plain",
                Some("application/json, text/plain"),
            ),
            ("", None),
            (";", None),
        ] {
            assert_eq!(
                media_type_essence(Some(header)).as_deref(),
                expected,
                "{header:?}"
            );
        }
        assert_eq!(media_type_essence(None), None);
        assert!(is_json_content_type(Some(
            "application/json; charset=utf-8"
        )));
        assert!(!is_json_content_type(Some("application/json-patch+json")));
        assert!(!is_json_content_type(Some(
            "application/json, application/json"
        )));
    }

    #[test]
    fn joins_repeated_headers_like_fetch() {
        let mut headers = HeaderMap::new();
        assert_eq!(header_value(&headers, "accept"), None);
        headers.append("accept", "application/json".parse().unwrap());
        headers.append("accept", "text/event-stream".parse().unwrap());
        assert_eq!(
            header_value(&headers, "Accept").as_deref(),
            Some("application/json, text/event-stream")
        );
    }
}
