//! Finding the authorization server of an MCP server: RFC 9728 protected
//! resource metadata, then RFC 8414 or OpenID Connect Discovery metadata of
//! the authorization server it names (`client/auth.js`).

use bytes::Bytes;
use http::header::{ACCEPT, HeaderName, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, Method};
use url::Url;

use crate::errors::{Error, MetadataDocument};
use crate::http::{
    HttpFetch, HttpFetchError, HttpRequest, HttpResponse, fetch_within_origin, header_value,
    resolve,
};
use crate::json;
use crate::types::{
    AuthorizationServerMetadata, OAuthMetadata, OAuthProtectedResourceMetadata,
    OpenIdProviderDiscoveryMetadata,
};

/// `LATEST_PROTOCOL_VERSION` of the SDK, sent as `MCP-Protocol-Version` with
/// every discovery request.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

const MCP_PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");

fn protocol_version(requested: Option<HeaderValue>) -> HeaderValue {
    requested.unwrap_or(HeaderValue::from_static(LATEST_PROTOCOL_VERSION))
}

/// What a 401 response says about the authorization it wants.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WwwAuthenticateParams {
    /// Where the protected resource metadata is, when the server says so.
    pub resource_metadata_url: Option<Url>,
    pub scope: Option<String>,
    pub error: Option<String>,
}

/// Extract `resource_metadata`, `scope`, and `error` from the
/// `WWW-Authenticate` header of a response.
pub fn extract_www_authenticate_params(headers: &HeaderMap) -> WwwAuthenticateParams {
    let Some(header) = header_value(headers, WWW_AUTHENTICATE).filter(|header| !header.is_empty())
    else {
        return WwwAuthenticateParams::default();
    };
    let mut words = header.split(' ');
    let scheme = words.next().unwrap_or_default();
    let has_parameters = words.next().is_some_and(|word| !word.is_empty());
    if !scheme.eq_ignore_ascii_case("bearer") || !has_parameters {
        return WwwAuthenticateParams::default();
    }

    WwwAuthenticateParams {
        // An invalid URL is ignored.
        resource_metadata_url: www_authenticate_field(&header, "resource_metadata")
            .and_then(|url| Url::parse(&url).ok()),
        scope: www_authenticate_field(&header, "scope"),
        error: www_authenticate_field(&header, "error"),
    }
}

/// The first `name="value"` or `name=value` of the header, wherever the name
/// appears: the SDK looks for it with the expression
/// `name=(?:"([^"]+)"|([^\s,]+))`.
fn www_authenticate_field(header: &str, name: &str) -> Option<String> {
    let characters: Vec<char> = header.chars().collect();
    let prefix: Vec<char> = name.chars().chain(['=']).collect();
    for start in 0..characters.len() {
        if !characters[start..].starts_with(&prefix) {
            continue;
        }
        let value = &characters[start + prefix.len()..];
        if value.first() == Some(&'"') {
            let quoted = &value[1..];
            if let Some(end) = quoted.iter().position(|&character| character == '"')
                && end > 0
            {
                return Some(quoted[..end].iter().collect());
            }
        }
        let bare: String = value
            .iter()
            .take_while(|&&character| character != ',' && !is_regexp_space(character))
            .collect();
        if !bare.is_empty() {
            return Some(bare);
        }
    }
    None
}

/// `\s` of a JavaScript regular expression, among the characters a header
/// value can hold.
fn is_regexp_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}'
    )
}

/// Sends a discovery request, once more without its headers when the first
/// attempt got no response, and reports `None` when the second got none
/// either.
///
/// The SDK does this for browsers, where a request with a header the server
/// does not allow fails in the CORS preflight with a `TypeError`. `fetch`
/// rejects with a `TypeError` for every network failure in Node too, so the
/// retry and the silence are what a server that is down or does not resolve
/// gets there as well.
async fn fetch_with_cors_retry<F: HttpFetch + ?Sized>(
    fetch: &F,
    url: &Url,
    headers: HeaderMap,
) -> Result<Option<HttpResponse>, HttpFetchError> {
    let request = |headers: HeaderMap| HttpRequest {
        method: Method::GET,
        url: url.clone(),
        headers,
        body: Bytes::new(),
    };
    match fetch_within_origin(fetch, request(headers)).await {
        Ok(response) => return Ok(Some(response)),
        Err(HttpFetchError::Network(_)) => {}
        Err(error) => return Err(error),
    }
    match fetch_within_origin(fetch, request(HeaderMap::new())).await {
        Ok(response) => Ok(Some(response)),
        Err(HttpFetchError::Network(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Where and how to look for protected resource metadata.
#[derive(Debug, Clone, Default)]
pub struct ProtectedResourceMetadataOptions {
    /// Defaults to [`LATEST_PROTOCOL_VERSION`].
    pub protocol_version: Option<HeaderValue>,
    /// The URL a `WWW-Authenticate` header gave. It is then the only one
    /// tried.
    pub resource_metadata_url: Option<Url>,
}

/// Looks up RFC 9728 OAuth 2.0 Protected Resource Metadata.
///
/// The well-known URL that carries the path of the server is tried first,
/// then the one at the root when that answered with a 4xx or not at all.
pub async fn discover_oauth_protected_resource_metadata<F: HttpFetch + ?Sized>(
    fetch: &F,
    server_url: &Url,
    options: ProtectedResourceMetadataOptions,
) -> Result<OAuthProtectedResourceMetadata, Error> {
    const WELL_KNOWN: &str = "/.well-known/oauth-protected-resource";
    let mut headers = HeaderMap::new();
    headers.insert(
        MCP_PROTOCOL_VERSION,
        protocol_version(options.protocol_version),
    );

    let url = match &options.resource_metadata_url {
        Some(url) => url.clone(),
        None => {
            // Path-aware discovery first. A trailing slash is dropped to
            // avoid a double one.
            let path = server_url.path();
            let path = path.strip_suffix('/').unwrap_or(path);
            let mut url = server_url
                .join(&format!("{WELL_KNOWN}{path}"))
                .map_err(|_| Error::InvalidUrl)?;
            url.set_query(server_url.query().filter(|query| !query.is_empty()));
            url
        }
    };
    let mut response = fetch_with_cors_retry(fetch, &url, headers.clone())
        .await
        .map_err(Error::Fetch)?;

    // If path-aware discovery fails with a 4xx and we're not already at the
    // root, fall back to root discovery. No response at all falls back even
    // from the root.
    let falls_back = match &response {
        None => true,
        Some(response) => {
            !response.ok() && response.status.as_u16() < 500 && server_url.path() != "/"
        }
    };
    if options.resource_metadata_url.is_none() && falls_back {
        let root_url = server_url.join(WELL_KNOWN).map_err(|_| Error::InvalidUrl)?;
        response = fetch_with_cors_retry(fetch, &root_url, headers)
            .await
            .map_err(Error::Fetch)?;
    }

    let Some(response) = response.filter(|response| response.status.as_u16() != 404) else {
        return Err(Error::ProtectedResourceMetadataNotImplemented);
    };
    if !response.ok() {
        return Err(Error::ProtectedResourceMetadataStatus {
            status: response.status.as_u16(),
        });
    }
    let document = json::parse(&json::decode_body(&response.body))?;
    Ok(OAuthProtectedResourceMetadata::from_json(&document)?)
}

/// One location authorization server metadata may be at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryUrl {
    pub url: Url,
    pub document: MetadataDocument,
}

/// Builds a list of discovery URLs to try for authorization server metadata.
/// URLs are returned in priority order:
/// 1. OAuth metadata at the given URL
/// 2. OIDC metadata endpoints at the given URL
///
/// Only the origin and the path of the URL count. Fails with
/// [`Error::InvalidUrl`] for a URL that has no origin to resolve against.
pub fn build_discovery_urls(authorization_server_url: &Url) -> Result<Vec<DiscoveryUrl>, Error> {
    let origin = authorization_server_url.origin();
    if !origin.is_tuple() {
        return Err(Error::InvalidUrl);
    }
    let origin = Url::parse(&origin.ascii_serialization()).map_err(|_| Error::InvalidUrl)?;
    let location = |path: String, document: MetadataDocument| {
        resolve(&origin, &path)
            .map(|url| DiscoveryUrl { url, document })
            .ok_or(Error::InvalidUrl)
    };

    let path = authorization_server_url.path();
    if path == "/" {
        return Ok(vec![
            // Root path: https://example.com/.well-known/oauth-authorization-server
            location(
                "/.well-known/oauth-authorization-server".to_owned(),
                MetadataDocument::OAuth,
            )?,
            // OIDC: https://example.com/.well-known/openid-configuration
            location(
                "/.well-known/openid-configuration".to_owned(),
                MetadataDocument::OpenId,
            )?,
        ]);
    }

    // Strip trailing slash from pathname to avoid double slashes
    let path = path.strip_suffix('/').unwrap_or(path);
    Ok(vec![
        // 1. OAuth metadata at the given URL. Insert well-known before the
        // path: https://example.com/.well-known/oauth-authorization-server/tenant1
        location(
            format!("/.well-known/oauth-authorization-server{path}"),
            MetadataDocument::OAuth,
        )?,
        // 2. OIDC metadata endpoints. RFC 8414 style: insert
        // /.well-known/openid-configuration before the path
        location(
            format!("/.well-known/openid-configuration{path}"),
            MetadataDocument::OpenId,
        )?,
        // OIDC Discovery 1.0 style: append /.well-known/openid-configuration
        // after the path
        location(
            format!("{path}/.well-known/openid-configuration"),
            MetadataDocument::OpenId,
        )?,
    ])
}

#[derive(Debug, Clone, Default)]
pub struct AuthorizationServerMetadataOptions {
    /// Defaults to [`LATEST_PROTOCOL_VERSION`].
    pub protocol_version: Option<HeaderValue>,
}

/// Discovers authorization server metadata with support for RFC 8414 OAuth
/// 2.0 Authorization Server Metadata and OpenID Connect Discovery 1.0.
///
/// The locations of [`build_discovery_urls`] are tried in order. A location
/// that answers with a 4xx, a redirect that was not followed, or not at all
/// is passed over. A 5xx is an error, and so is a document that is not valid
/// metadata: the next location is not tried after either. `None` means no
/// location had a document.
///
/// The metadata is returned as served: nothing checks that its `issuer` is
/// the URL it was fetched for.
pub async fn discover_authorization_server_metadata<F: HttpFetch + ?Sized>(
    fetch: &F,
    authorization_server_url: &Url,
    options: AuthorizationServerMetadataOptions,
) -> Result<Option<AuthorizationServerMetadata>, Error> {
    let mut headers = HeaderMap::new();
    headers.insert(
        MCP_PROTOCOL_VERSION,
        protocol_version(options.protocol_version),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    for DiscoveryUrl { url, document } in build_discovery_urls(authorization_server_url)? {
        let response = fetch_with_cors_retry(fetch, &url, headers.clone())
            .await
            .map_err(Error::Fetch)?;
        let Some(response) = response else {
            continue;
        };
        if !response.ok() {
            // Continue looking for any 4xx response code, or a redirect that
            // was not followed.
            if response.status.as_u16() < 500 {
                continue;
            }
            return Err(Error::AuthorizationServerMetadataStatus {
                status: response.status.as_u16(),
                document,
                url,
            });
        }

        let body = json::parse(&json::decode_body(&response.body))?;
        return Ok(Some(match document {
            MetadataDocument::OAuth => OAuthMetadata::from_json(&body)?.into(),
            MetadataDocument::OpenId => OpenIdProviderDiscoveryMetadata::from_json(&body)?.into(),
        }));
    }
    Ok(None)
}

#[derive(Debug, Clone, Default)]
pub struct ServerInfoOptions {
    /// Override URL for the protected resource metadata endpoint.
    pub resource_metadata_url: Option<Url>,
}

/// The authorization server of an MCP server, as far as it was discovered.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthServerInfo {
    /// The first authorization server the protected resource metadata names,
    /// as written there, or else the root of the server URL.
    pub authorization_server_url: String,
    /// `None` when no location of the authorization server had metadata.
    pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
    /// `None` when the server has no usable protected resource metadata.
    pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
}

/// Discovers the authorization server for an MCP server following RFC 9728
/// (OAuth 2.0 Protected Resource Metadata), with fallback to treating the
/// server URL as the authorization server.
///
/// Every failure of the first step falls back, as in the SDK: a missing
/// document, an invalid one, and an error of the fetch alike. Only the second
/// step, the metadata of the authorization server, can fail this call.
pub async fn discover_oauth_server_info<F: HttpFetch + ?Sized>(
    fetch: &F,
    server_url: &Url,
    options: ServerInfoOptions,
) -> Result<OAuthServerInfo, Error> {
    // RFC 9728 not supported -- fall back to treating the server URL as the
    // authorization server.
    let resource_metadata = discover_oauth_protected_resource_metadata(
        fetch,
        server_url,
        ProtectedResourceMetadataOptions {
            protocol_version: None,
            resource_metadata_url: options.resource_metadata_url,
        },
    )
    .await
    .ok();

    let advertised = resource_metadata
        .as_ref()
        .and_then(|metadata| metadata.authorization_servers.as_ref())
        .and_then(|servers| servers.first())
        .filter(|server| !server.is_empty());
    let (authorization_server_url, parsed) = match advertised {
        Some(server) => (
            server.clone(),
            Url::parse(server).map_err(|_| Error::InvalidUrl)?,
        ),
        // The legacy MCP spec behavior: the MCP server base URL acts as the
        // authorization server.
        None => {
            let root = server_url.join("/").map_err(|_| Error::InvalidUrl)?;
            (root.to_string(), root)
        }
    };

    let authorization_server_metadata = discover_authorization_server_metadata(
        fetch,
        &parsed,
        AuthorizationServerMetadataOptions::default(),
    )
    .await?;
    Ok(OAuthServerInfo {
        authorization_server_url,
        authorization_server_metadata,
        resource_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(WWW_AUTHENTICATE, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn reads_the_resource_metadata_url_of_a_401() {
        let params = extract_www_authenticate_params(&header(
            r#"Bearer error="invalid_token", resource_metadata="https://mcp.example/.well-known/oauth-protected-resource", scope="read write""#,
        ));
        assert_eq!(
            params.resource_metadata_url.unwrap().as_str(),
            "https://mcp.example/.well-known/oauth-protected-resource"
        );
        assert_eq!(params.scope.as_deref(), Some("read write"));
        assert_eq!(params.error.as_deref(), Some("invalid_token"));
        assert_eq!(
            extract_www_authenticate_params(&header("Basic realm=\"x\"")),
            WwwAuthenticateParams::default()
        );
    }
}
