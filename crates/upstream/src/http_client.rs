//! HTTP MCPs: the connection the gateway opens to one for each request, with
//! the credentials saved for it.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use futures::StreamExt;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use mymcps_core::models::{Mcp, McpAuthType};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_mcp::client::{
    BoxError, Client, HttpRequest, HttpResponse, HttpSend, StreamableHttpClientTransport,
    StreamableHttpClientTransportOptions, Transport,
};
use mymcps_net::{FetchRequest, Fetcher, UpstreamResponseLimits, parse_http_url};
use mymcps_vine::js;
use serde::Serialize;
use serde_json::Value;

use crate::Upstream;
use crate::allowlisted_client::{mcp_client_info_for_url, upstream_identity_headers};
use crate::error::UpstreamError;

/// A tool as an upstream MCP lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpstreamTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

impl From<&mymcps_mcp::Tool> for UpstreamTool {
    fn from(tool: &mymcps_mcp::Tool) -> Self {
        Self {
            name: tool.name().to_owned(),
            description: tool.description().map(str::to_owned),
            input_schema: tool.input_schema().clone(),
        }
    }
}

/// An HTTP MCP that answered the MCP handshake.
pub struct ConnectedHttpUpstream {
    pub client: Client,
    pub transport: StreamableHttpClientTransport,
}

impl ConnectedHttpUpstream {
    /// Connection teardown is best-effort. Nothing is sent: the session is
    /// not terminated.
    pub async fn close(&self) {
        self.client.close().await;
        self.transport.close().await;
    }
}

#[derive(Debug, Clone, Default)]
struct UnauthorizedResponse {
    body: String,
    www_authenticate: String,
}

/// The first `units` UTF-16 code units of a text, as `String.prototype.slice`
/// counts them. A character that would be cut in two is left out.
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut taken = 0;
    for (index, character) in text.char_indices() {
        taken += character.len_utf16();
        if taken > units {
            return &text[..index];
        }
    }
    text
}

fn compact_diagnostic(value: Option<&str>) -> String {
    const LIMIT: usize = 240;
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return String::new();
    };
    let mut compacted = String::with_capacity(value.len());
    let mut in_whitespace = false;
    for character in value.chars() {
        if js::is_whitespace(character) {
            if !in_whitespace {
                compacted.push(' ');
            }
            in_whitespace = true;
        } else {
            compacted.push(character);
            in_whitespace = false;
        }
    }
    let compacted = js::trim(&compacted);
    if js::utf16_len(compacted) > LIMIT {
        format!("{}…", utf16_prefix(compacted, LIMIT - 1))
    } else {
        compacted.to_owned()
    }
}

/// The fetch the MCP transport sends its requests through. It follows
/// redirects within the origin, limits what it reads, and remembers what a
/// server said when it answered 401.
struct DiagnosticFetch {
    fetcher: Fetcher,
    unauthorized: Arc<Mutex<Option<UnauthorizedResponse>>>,
}

#[async_trait]
impl HttpSend for DiagnosticFetch {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut fetch = FetchRequest::new(request.method, request.url);
        fetch.headers = request.headers;
        fetch.body = request.body;
        let mut response = self
            .fetcher
            .fetch_with_same_origin_redirects(
                fetch,
                "MCP endpoint",
                UpstreamResponseLimits::default(),
            )
            .await?;

        if response.status() == http::StatusCode::UNAUTHORIZED {
            // The body is kept once read, so the transport still gets it.
            let body = response.text().await.unwrap_or_default();
            let details = UnauthorizedResponse {
                body: compact_diagnostic(Some(&body)),
                www_authenticate: compact_diagnostic(
                    response.header("WWW-Authenticate").as_deref(),
                ),
            };
            *self
                .unauthorized
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(details);
        }

        Ok(HttpResponse {
            status: response.status(),
            headers: response.headers().clone(),
            body: Box::pin(
                response
                    .into_stream()
                    .map(|chunk| chunk.map_err(BoxError::from)),
            ),
        })
    }
}

/// A header value as `new Headers()` takes it, or the `TypeError` it throws
/// for one it cannot hold. The error quotes the value as Node does: callers
/// redact the secrets of the MCP from it.
fn header_value(value: &str) -> Result<HeaderValue, UpstreamError> {
    let mut bytes = Vec::with_capacity(value.len());
    for (index, unit) in value.encode_utf16().enumerate() {
        match u8::try_from(unit) {
            Ok(byte) => bytes.push(byte),
            Err(_) => {
                return Err(UpstreamError::other(format!(
                    "Cannot convert argument to a ByteString because the character at index {index} has a value of {unit} which is greater than 255."
                )));
            }
        }
    }
    let is_http_whitespace = |byte: &u8| matches!(byte, b'\t' | b'\n' | b'\r' | b' ');
    let start = bytes
        .iter()
        .position(|byte| !is_http_whitespace(byte))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !is_http_whitespace(byte))
        .map_or(start, |last| last + 1);
    let normalized = &bytes[start..end];
    if normalized
        .iter()
        .any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
    {
        let written: String = normalized.iter().map(|byte| char::from(*byte)).collect();
        return Err(UpstreamError::other(format!(
            "Headers.append: \"{written}\" is an invalid header value."
        )));
    }
    // What is left is a value `Headers` holds and the HTTP client then
    // refuses to send, which `fetch` reports as a request that failed.
    HeaderValue::from_bytes(normalized).map_err(|_| UpstreamError::other("fetch failed"))
}

/// The headers of every request to the MCP, as the `Headers` the Node app
/// built from them: names are case-insensitive, and two values of one name
/// are joined.
fn header_map(entries: &[(String, String)]) -> Result<HeaderMap, UpstreamError> {
    let mut headers = HeaderMap::new();
    for (name, value) in entries {
        let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            UpstreamError::other(format!(
                "Headers.append: \"{name}\" is an invalid header name."
            ))
        })?;
        let value = header_value(value)?;
        let value = match headers.get(&header) {
            Some(existing) => {
                let mut joined = existing.as_bytes().to_vec();
                joined.extend_from_slice(b", ");
                joined.extend_from_slice(value.as_bytes());
                HeaderValue::from_bytes(&joined)
                    .map_err(|_| UpstreamError::other("fetch failed"))?
            }
            None => value,
        };
        headers.insert(header, value);
    }
    Ok(headers)
}

impl Upstream {
    fn build_auth_headers(&self, mcp: &Mcp) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        let decrypt = |column: &Option<String>| self.core.decrypt_secret(column.as_deref());

        match mcp.auth_type {
            McpAuthType::Bearer => {
                if let Some(token) = decrypt(&mcp.auth_bearer) {
                    headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
                }
            }
            McpAuthType::Header => {
                let name = mcp
                    .auth_header_name
                    .as_deref()
                    .filter(|name| !name.is_empty());
                if let (Some(name), Some(value)) = (name, decrypt(&mcp.auth_header_value)) {
                    headers.push((name.to_owned(), value));
                }
            }
            McpAuthType::Auto => {
                if let Some(token) = decrypt(&mcp.oauth_access_token) {
                    // OAuth token type names are case-insensitive, but some upstreams (including
                    // the provider behind Notion MCP) parse the Bearer scheme case-sensitively.
                    headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
                }
            }
        }

        headers
    }

    /// Auth headers plus the spoofed client identity for allowlisted MCP
    /// hosts, by name, in the order they are set.
    pub fn build_upstream_headers(&self, mcp: &Mcp) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = upstream_identity_headers(mcp.http_url.as_deref())
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        for (name, value) in self.build_auth_headers(mcp) {
            // The same name, written the same way, replaces the identity header.
            match headers.iter_mut().find(|(existing, _)| *existing == name) {
                Some(header) => header.1 = value,
                None => headers.push((name, value)),
            }
        }
        headers
    }

    /// Open a connection to an HTTP MCP and run the MCP handshake, after
    /// renewing its OAuth access token if it is about to expire.
    ///
    /// `mcp` is the row as it is after that renewal.
    pub async fn connect_http_upstream(
        &self,
        mcp: &mut Mcp,
    ) -> Result<ConnectedHttpUpstream, UpstreamError> {
        let has_access_token = mcp
            .oauth_access_token
            .as_deref()
            .is_some_and(|token| !token.is_empty());
        if mcp.auth_type == McpAuthType::Auto && has_access_token {
            self.refresh_oauth_access_token(mcp).await?;
        }

        // Refresh reloads the model. Resolve the destination and credentials from the
        // same current configuration, never pair fresh credentials with an old URL.
        let Some(http_url) = mcp.http_url.as_deref().filter(|url| !url.is_empty()) else {
            return Err(UpstreamError::other("HTTP MCP is missing a URL"));
        };
        let endpoint = parse_http_url(http_url, "MCP URL")?;
        let headers = header_map(&self.build_upstream_headers(mcp))?;
        let unauthorized: Arc<Mutex<Option<UnauthorizedResponse>>> = Arc::default();
        let transport = StreamableHttpClientTransport::with_options(
            endpoint,
            Arc::new(DiagnosticFetch {
                fetcher: self.fetcher.clone(),
                unauthorized: Arc::clone(&unauthorized),
            }),
            StreamableHttpClientTransportOptions {
                headers,
                ..Default::default()
            },
        );
        let client = Client::new(mcp_client_info_for_url(Some(http_url)));

        if let Err(error) = client.connect(transport.clone()).await {
            let details = unauthorized
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let Some(details) = details else {
                return Err(error.into());
            };
            let context: Vec<String> = [
                (!details.body.is_empty()).then(|| format!("Response: {}", details.body)),
                (!details.www_authenticate.is_empty())
                    .then(|| format!("WWW-Authenticate: {}", details.www_authenticate)),
            ]
            .into_iter()
            .flatten()
            .collect();
            let diagnostic = if context.is_empty() {
                "MCP server returned HTTP 401 Unauthorized.".to_owned()
            } else {
                format!("MCP server returned HTTP 401. {}", context.join(" | "))
            };
            return Err(UpstreamError::Unauthorized(sanitize_mcp_diagnostic(
                &self.core.encryption,
                &diagnostic,
                mcp,
            )));
        }

        Ok(ConnectedHttpUpstream { client, transport })
    }

    /// The tools of an HTTP MCP: connect, ask, close.
    pub async fn list_http_tools(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
        let connected = self.connect_http_upstream(mcp).await?;
        let result = connected.client.list_tools().await;
        connected.close().await;
        Ok(result?.tools.iter().map(UpstreamTool::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_what_a_server_said_into_one_short_line() {
        assert_eq!(compact_diagnostic(None), "");
        assert_eq!(compact_diagnostic(Some("")), "");
        assert_eq!(compact_diagnostic(Some(" \n\t ")), "");
        assert_eq!(
            compact_diagnostic(Some(
                "  {\n  \"error\":\t\"invalid_token\"\u{a0}\u{feff}}\r\n"
            )),
            "{ \"error\": \"invalid_token\" }"
        );

        let exact = "x".repeat(240);
        assert_eq!(compact_diagnostic(Some(&exact)), exact);
        let long = "x".repeat(241);
        assert_eq!(
            compact_diagnostic(Some(&long)),
            format!("{}…", "x".repeat(239))
        );
        // Lengths are counted as JavaScript counts them.
        let astral = "😀".repeat(121);
        assert_eq!(
            compact_diagnostic(Some(&astral)),
            format!("{}…", "😀".repeat(119))
        );
    }

    #[test]
    fn builds_headers_as_the_headers_class_does() {
        let entry = |name: &str, value: &str| (name.to_owned(), value.to_owned());

        let headers = header_map(&[
            entry("User-Agent", "codex-mcp-client/0.0.0"),
            entry("user-agent", "custom"),
            entry("X-Api-Key", " padded\r\n"),
            entry("X-Latin", "caf\u{e9}"),
        ])
        .unwrap();
        assert_eq!(headers["user-agent"], "codex-mcp-client/0.0.0, custom");
        assert_eq!(headers["x-api-key"], "padded");
        assert_eq!(headers["x-latin"].as_bytes(), b"caf\xe9");

        let message =
            |name: &str, value: &str| header_map(&[entry(name, value)]).unwrap_err().to_string();
        assert_eq!(
            message("bad name", "x"),
            "Headers.append: \"bad name\" is an invalid header name."
        );
        assert_eq!(
            message("", "x"),
            "Headers.append: \"\" is an invalid header name."
        );
        assert_eq!(
            message("Authorization", "Bearer a\nb"),
            "Headers.append: \"Bearer a\nb\" is an invalid header value."
        );
        // The value is quoted as it was normalized.
        assert_eq!(
            message("X-Api-Key", " \n a\0b \r"),
            "Headers.append: \"a\0b\" is an invalid header value."
        );
        assert_eq!(
            message("Authorization", "Bearer \u{20ac}"),
            "Cannot convert argument to a ByteString because the character at index 7 has a value of 8364 which is greater than 255."
        );
        assert_eq!(message("Authorization", "Bearer a\u{1}b"), "fetch failed");
    }
}
