use url::Url;

/// Why a value is not an endpoint the gateway can call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpUrlError {
    #[error("{label} must be a valid URL")]
    Invalid { label: String },
    #[error("{label} must use HTTP or HTTPS")]
    NotHttp { label: String },
    #[error("{label} must not include a fragment")]
    Fragment { label: String },
}

/// Parse an MCP or OAuth endpoint. Query parameters and URL credentials are
/// intentionally preserved because some providers require them.
pub fn parse_http_url(value: &str, label: &str) -> Result<Url, HttpUrlError> {
    let label = || label.to_owned();
    let url = Url::parse(value).map_err(|_| HttpUrlError::Invalid { label: label() })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(HttpUrlError::NotHttp { label: label() });
    }
    // A bare trailing `#` is an empty fragment, which the check has always let through.
    if url.fragment().is_some_and(|fragment| !fragment.is_empty()) {
        return Err(HttpUrlError::Fragment { label: label() });
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_query_and_credentials() {
        let value =
            "https://user:password@example.test/mcp?api_key=provider-required&code=fr&key=primary";
        let url = parse_http_url(value, "MCP URL").unwrap();
        assert_eq!(url.as_str(), value);
        assert_eq!(url.username(), "user");
        assert_eq!(url.password(), Some("password"));
    }

    #[test]
    fn names_the_label_in_every_refusal() {
        let cases = [
            ("not a url", "OAuth issuer must be a valid URL"),
            ("", "OAuth issuer must be a valid URL"),
            ("/relative", "OAuth issuer must be a valid URL"),
            (
                "ws://example.test/mcp",
                "OAuth issuer must use HTTP or HTTPS",
            ),
            ("file:///tmp/socket", "OAuth issuer must use HTTP or HTTPS"),
            (
                "https://example.test/mcp#secret",
                "OAuth issuer must not include a fragment",
            ),
        ];
        for (value, message) in cases {
            let error = parse_http_url(value, "OAuth issuer").unwrap_err();
            assert_eq!(error.to_string(), message, "{value}");
        }
    }

    #[test]
    fn normalizes_like_a_url_parser() {
        let cases = [
            ("HTTPS://Example.TEST", "https://example.test/"),
            (" https://example.test/a ", "https://example.test/a"),
            ("http://example.test:80/a", "http://example.test/a"),
            ("https://example.test\\a", "https://example.test/a"),
            ("http://2130706433/token", "http://127.0.0.1/token"),
            // An empty fragment is not a fragment to refuse.
            ("https://example.test/mcp#", "https://example.test/mcp#"),
        ];
        for (value, expected) in cases {
            assert_eq!(parse_http_url(value, "URL").unwrap().as_str(), expected);
        }
    }
}
