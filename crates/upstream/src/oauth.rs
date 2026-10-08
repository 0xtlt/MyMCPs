//! OAuth with the provider of an upstream MCP: discovery, dynamic client
//! registration, the authorization redirect with PKCE, the exchange of the
//! code, and the renewal of the tokens.

use std::sync::PoisonError;

use chrono::Duration;
use http::Method;
use mymcps_builtin::oauth::{
    BuiltinOauthTokens, builtin_authorization_url, exchange_builtin_authorization_code,
    parse_oauth_scopes, refresh_builtin_tokens, requested_builtin_scopes,
};
use mymcps_core::Timestamp;
use mymcps_core::crypto::random_base64url;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_mcp_auth::{
    AuthorizationServerMetadata, AuthorizationServerMetadataOptions, ExchangeAuthorizationOptions,
    HttpFetch, HttpFetchError, HttpRequest, HttpResponse, OAuthClientInformationMixed,
    OAuthMetadata, OAuthServerInfo, RefreshAuthorizationOptions, RegisterClientOptions,
    ServerInfoOptions, StartAuthorizationOptions, check_resource_allowed,
    discover_authorization_server_metadata, discover_oauth_server_info, exchange_authorization,
    refresh_authorization, register_client, resource_url_from_server_url, start_authorization,
};
use mymcps_net::{
    DiscoveredEndpointGuard, FetchError, FetchRequest, Fetcher, RestrictedEndpointError,
    UpstreamResponseLimits, parse_http_url,
};
use mymcps_vine::js;
use serde_json::{Map, Value, json};
use url::Url;

use crate::Upstream;
use crate::allowlisted_client::{
    allowlisted_upstream_client, registration_client_name, upstream_identity_headers,
};
use crate::error::UpstreamError;
use crate::session::{OauthSession, OauthSessionStore, OauthStart, start_oauth_session};
use crate::shared::{SharedOutcome, outcome_channel};

/// Metadata documents and token responses are a few KiB.
const MAX_OAUTH_RESPONSE_BYTES: usize = 1024 * 1024;

/// Lifetime assumed for an access token issued without `expires_in`. With no
/// expiry at all the token never counts as fresh, and every connection would
/// refresh it first.
const DEFAULT_ACCESS_TOKEN_LIFETIME_SECONDS: f64 = 60.0 * 60.0;

/// The longest lifetime kept for an access token. A provider that states a
/// longer one, or one that is not a number of seconds at all, must not be
/// able to store a date the database cannot give back.
const MAX_ACCESS_TOKEN_LIFETIME_SECONDS: f64 = 100.0 * 365.0 * 24.0 * 60.0 * 60.0;

/// What an OAuth flow found out about the provider of an MCP.
struct OAuthContext {
    authorization_server_url: String,
    metadata: AuthorizationServerMetadata,
    resource: Option<String>,
    scope: Option<String>,
}

/// The tokens of a token response, from a discovered provider or from the
/// provider of a built-in MCP.
struct IssuedTokens {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<f64>,
    scope: Option<String>,
}

/// What the provider of a built-in MCP is asked to turn into tokens.
enum BuiltinGrant {
    AuthorizationCode { code: String, redirect_uri: String },
    RefreshToken(String),
}

pub(crate) type PendingRefresh = SharedOutcome<()>;

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

fn has_no_search(url: &Url) -> bool {
    url.query().is_none_or(str::is_empty)
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn normalized_http_url(value: &str, label: &str) -> Result<String, UpstreamError> {
    let url = parse_http_url(value, label)?;
    let is_origin = url.path() == "/"
        && has_no_search(&url)
        && url.username().is_empty()
        && url.password().is_none_or(str::is_empty);
    Ok(if is_origin {
        origin(&url)
    } else {
        url.to_string()
    })
}

fn comparable_oauth_issuer(value: &str, label: &str) -> Result<String, UpstreamError> {
    let mut url = parse_http_url(value, label)?;
    // Only a URL without a host refuses these, and an HTTP URL has one.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Ok(if url.path() == "/" && has_no_search(&url) {
        origin(&url)
    } else {
        url.to_string()
    })
}

/// What `fetch` rejected with a `TypeError` in the Node app: discovery reads
/// it as a document that is not there. Every other failure is a refusal or a
/// limit of the gateway's own, which ends the flow.
fn oauth_fetch_error(error: FetchError) -> HttpFetchError {
    match error {
        FetchError::Transport(_)
        | FetchError::BadPort
        | FetchError::Offline
        | FetchError::InvalidRedirectUrl
        | FetchError::BodyNotAllowed
        | FetchError::InvalidHeader { .. } => HttpFetchError::network(error),
        other => HttpFetchError::other(other),
    }
}

/// How the requests of one MCP's OAuth flow reach its provider.
///
/// Every request of an OAuth flow goes through this one fetch, so no call site
/// can reach a discovered endpoint without the address check. Requests to the
/// MCP's own host pass: the operator entered that one.
struct OAuthEndpoints {
    fetcher: Fetcher,
    /// Refuses a discovered URL that leads into a network the MCP is not part of.
    guard: DiscoveredEndpointGuard,
}

impl OAuthEndpoints {
    async fn assert_allowed(&self, target: &Url, label: &str) -> Result<(), UpstreamError> {
        Ok(self.guard.assert_allowed(target, label).await?)
    }
}

impl HttpFetch for OAuthEndpoints {
    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, HttpFetchError> {
        self.guard
            .assert_allowed(&request.url, "OAuth endpoint")
            .await
            .map_err(HttpFetchError::other)?;

        let HttpRequest {
            method,
            url,
            headers,
            body,
        } = request;
        let mut fetch = FetchRequest::new(method, url);
        fetch.headers = headers;
        for (name, value) in upstream_identity_headers(Some(fetch.url.as_str())) {
            if !fetch.headers.contains_key(name) {
                fetch = fetch.header(name, value).map_err(oauth_fetch_error)?;
            }
        }
        if fetch.method != Method::GET && fetch.method != Method::HEAD {
            fetch.body = Some(body);
        }

        let limits = UpstreamResponseLimits {
            max_response_bytes: Some(MAX_OAUTH_RESPONSE_BYTES),
            ..UpstreamResponseLimits::default()
        };
        let mut response = self
            .fetcher
            .fetch_with_same_origin_redirects(fetch, "OAuth endpoint", limits)
            .await
            .map_err(oauth_fetch_error)?;
        let body = match response.bytes().await {
            Ok(body) => body,
            // The SDK read the body of a refusal only to quote it.
            Err(_) if !response.ok() => bytes::Bytes::new(),
            Err(error) => return Err(HttpFetchError::other(error)),
        };
        Ok(HttpResponse {
            status: response.status(),
            headers: response.headers().clone(),
            body,
            url: Some(response.url().clone()),
        })
    }
}

/// The endpoints come from a document the provider or the MCP serves. The
/// authorization endpoint is checked too: the admin's browser is sent there.
async fn validate_oauth_metadata(
    metadata: &AuthorizationServerMetadata,
    endpoints: &OAuthEndpoints,
    expected_issuer: Option<&str>,
) -> Result<(), UpstreamError> {
    let discovered = [
        (
            Some(metadata.authorization_endpoint()),
            "OAuth authorization endpoint",
        ),
        (Some(metadata.token_endpoint()), "OAuth token endpoint"),
        (
            metadata.registration_endpoint(),
            "OAuth registration endpoint",
        ),
    ];
    for (value, label) in discovered {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            endpoints
                .assert_allowed(&parse_http_url(value, label)?, label)
                .await?;
        }
    }
    if !metadata.issuer().is_empty() {
        parse_http_url(metadata.issuer(), "OAuth issuer")?;
        if let Some(expected_issuer) = expected_issuer.filter(|issuer| !issuer.is_empty())
            && comparable_oauth_issuer(metadata.issuer(), "OAuth issuer")?
                != comparable_oauth_issuer(expected_issuer, "OAuth authorization server")?
        {
            return Err(UpstreamError::other(
                "OAuth issuer metadata does not match the authorization server",
            ));
        }
    }
    Ok(())
}

/// RFC 8707 resource indicator for the tokens of an MCP: its URL or a parent of
/// it. A resource server must not name another API as the audience, or the
/// provider would issue tokens for that API and the gateway hand them to this MCP.
fn resource_for_mcp(mcp: &Mcp, resource: Option<&str>) -> Result<Option<String>, UpstreamError> {
    let Some(resource) = resource.filter(|resource| !resource.is_empty()) else {
        return Ok(None);
    };
    let normalized = normalized_http_url(resource, "OAuth resource")?;
    let requested = resource_url_from_server_url(&parse_http_url(
        mcp.http_url.as_deref().unwrap_or_default(),
        "MCP URL",
    )?);
    let matches_mcp = Url::parse(&normalized)
        .is_ok_and(|configured| check_resource_allowed(&requested, &configured));
    if !matches_mcp {
        return Err(UpstreamError::other(
            "OAuth protected resource does not match the MCP URL",
        ));
    }
    Ok(Some(normalized))
}

fn resource_url(resource: Option<&str>) -> Result<Option<Url>, UpstreamError> {
    resource
        .map(|resource| parse_http_url(resource, "OAuth resource"))
        .transpose()
        .map_err(UpstreamError::from)
}

fn authorization_server(authorization_server_url: &str) -> Result<Url, UpstreamError> {
    Ok(parse_http_url(
        authorization_server_url,
        "OAuth authorization server",
    )?)
}

/// The issuer a root metadata document names, when it is a path of the same
/// origin and serves metadata of its own.
async fn metadata_for_same_origin_issuer(
    metadata: &AuthorizationServerMetadata,
    authorization_server_url: &str,
    endpoints: &OAuthEndpoints,
) -> Result<Option<(String, AuthorizationServerMetadata)>, UpstreamError> {
    if metadata.issuer().is_empty() {
        return Ok(None);
    }

    let issuer = normalized_http_url(metadata.issuer(), "OAuth issuer")?;
    if comparable_oauth_issuer(&issuer, "OAuth issuer")?
        == comparable_oauth_issuer(authorization_server_url, "OAuth authorization server")?
    {
        return Ok(None);
    }

    let issuer_url = parse_http_url(&issuer, "OAuth issuer")?;
    if origin(&issuer_url) != origin(&authorization_server(authorization_server_url)?) {
        return Ok(None);
    }

    let Ok(Some(issuer_metadata)) = discover_authorization_server_metadata(
        endpoints,
        &issuer_url,
        AuthorizationServerMetadataOptions::default(),
    )
    .await
    else {
        return Ok(None);
    };
    if validate_oauth_metadata(&issuer_metadata, endpoints, Some(&issuer))
        .await
        .is_err()
    {
        return Ok(None);
    }
    Ok(Some((issuer, issuer_metadata)))
}

fn infer_issuer(mcp: &Mcp) -> Result<Option<String>, UpstreamError> {
    let Some(endpoint) = present(&mcp.oauth_authorize_url).or(present(&mcp.oauth_token_url)) else {
        return Ok(None);
    };
    let mut url = parse_http_url(endpoint, "OAuth endpoint")?;
    url.set_path("/");
    url.set_query(None);
    let has_credentials =
        !url.username().is_empty() || url.password().is_some_and(|password| !password.is_empty());
    Ok(Some(if has_credentials {
        url.to_string()
    } else {
        origin(&url)
    }))
}

/// The issuer saved for the MCP, or the one its saved endpoints imply.
fn saved_issuer(mcp: &Mcp) -> Result<Option<String>, UpstreamError> {
    match &mcp.oauth_issuer {
        Some(issuer) => Ok(Some(issuer.clone())),
        None => infer_issuer(mcp),
    }
}

/// Manual endpoint settings, for OAuth providers without discovery.
fn fallback_metadata(mcp: &Mcp, issuer: &str) -> Option<AuthorizationServerMetadata> {
    let authorization_endpoint = present(&mcp.oauth_authorize_url)?;
    let token_endpoint = present(&mcp.oauth_token_url)?;
    Some(
        OAuthMetadata {
            issuer: issuer.to_owned(),
            authorization_endpoint: authorization_endpoint.to_owned(),
            token_endpoint: token_endpoint.to_owned(),
            response_types_supported: vec!["code".to_owned()],
            code_challenge_methods_supported: Some(vec!["S256".to_owned()]),
            token_endpoint_auth_methods_supported: present(&mcp.oauth_client_auth_method)
                .map(|method| vec![method.to_owned()]),
            ..OAuthMetadata::default()
        }
        .into(),
    )
}

/// Whether the provider redirects to a loopback URL the admin must paste back.
/// The loopback URI, when one is required, lives on the allowlisted-client map.
pub fn uses_pasted_oauth_callback(mcp: &Mcp) -> bool {
    mcp.transport == McpTransport::Http
        && allowlisted_upstream_client(mcp.http_url.as_deref())
            .is_some_and(|client| client.loopback_redirect_uri.is_some())
}

/// Tokens belong to the provider and client that issued them. Dropping them
/// also leaves the MCP waiting for its account, so Connect stays available.
fn forget_oauth_tokens(mcp: &mut Mcp) {
    if present(&mcp.oauth_access_token).is_none() && present(&mcp.oauth_refresh_token).is_none() {
        return;
    }
    mcp.oauth_access_token = None;
    mcp.oauth_refresh_token = None;
    mcp.oauth_token_expires_at = None;
    mcp.oauth_token_type = None;
    mcp.oauth_required = true;
}

/// When an access token issued now stops being fresh.
fn token_expiry(expires_in: Option<f64>) -> Timestamp {
    let seconds = expires_in
        .filter(|seconds| !seconds.is_nan())
        .unwrap_or(DEFAULT_ACCESS_TOKEN_LIFETIME_SECONDS)
        .clamp(
            -MAX_ACCESS_TOKEN_LIFETIME_SECONDS,
            MAX_ACCESS_TOKEN_LIFETIME_SECONDS,
        );
    // In range after the clamp: about three thousand billion milliseconds.
    Timestamp::now() + Duration::milliseconds((seconds * 1000.0) as i64)
}

impl Upstream {
    fn oauth_endpoints_for(&self, mcp: &Mcp) -> Result<OAuthEndpoints, UpstreamError> {
        let http_url = present(&mcp.http_url).filter(|_| mcp.transport == McpTransport::Http);
        let Some(http_url) = http_url else {
            return Err(UpstreamError::other(
                "OAuth is supported only for HTTP MCPs",
            ));
        };
        let mcp_url = parse_http_url(http_url, "MCP URL")?;
        Ok(OAuthEndpoints {
            fetcher: self.fetcher.clone(),
            guard: self.addresses.discovered_endpoint_guard(&mcp_url),
        })
    }

    /// Where the provider sends the browser back to once the admin decided.
    pub fn oauth_callback_url(&self) -> Result<String, UpstreamError> {
        Ok(format!(
            "{}/mcps/oauth/callback",
            self.core.config.require_public_app_url()?
        ))
    }

    fn oauth_redirect_uri(&self, mcp: &Mcp) -> Result<String, UpstreamError> {
        match allowlisted_upstream_client(mcp.http_url.as_deref())
            .and_then(|client| client.loopback_redirect_uri)
        {
            Some(loopback) => Ok(loopback.to_owned()),
            None => self.oauth_callback_url(),
        }
    }

    fn client_information_from_mcp(&self, mcp: &Mcp) -> Option<OAuthClientInformationMixed> {
        let client_id = present(&mcp.oauth_client_id)?;
        Some(OAuthClientInformationMixed {
            client_id: client_id.to_owned(),
            client_secret: self.core.decrypt_secret(mcp.oauth_client_secret.as_deref()),
            token_endpoint_auth_method: present(&mcp.oauth_client_auth_method).map(str::to_owned),
        })
    }

    async fn discover_oauth_context(
        &self,
        mcp: &Mcp,
        endpoints: &OAuthEndpoints,
    ) -> Result<OAuthContext, UpstreamError> {
        let server_url = parse_http_url(mcp.http_url.as_deref().unwrap_or_default(), "MCP URL")?;
        let mut discovery_error: Option<String> = None;

        let server_info: Option<OAuthServerInfo> =
            match discover_oauth_server_info(endpoints, &server_url, ServerInfoOptions::default())
                .await
            {
                Ok(server_info) => Some(server_info),
                Err(error) => {
                    // A refused address is not a provider without discovery: no fallback.
                    if let Some(refused) = error.downcast_fetch_error::<RestrictedEndpointError>() {
                        return Err(refused.clone().into());
                    }
                    discovery_error = Some(error.to_string());
                    None
                }
            };
        let (discovered_url, mut metadata, resource_metadata) = match server_info {
            Some(server_info) => (
                Some(server_info.authorization_server_url),
                server_info.authorization_server_metadata,
                server_info.resource_metadata,
            ),
            None => (None, None, None),
        };

        let discovered_url = match discovered_url {
            Some(url) => Some(url),
            None => saved_issuer(mcp)?,
        };
        let Some(discovered_url) = discovered_url.filter(|url| !url.is_empty()) else {
            return Err(UpstreamError::other(match discovery_error {
                Some(reason) => format!("OAuth discovery failed: {reason}"),
                None => "OAuth provider metadata could not be discovered".to_owned(),
            }));
        };
        let mut authorization_server_url =
            normalized_http_url(&discovered_url, "OAuth authorization server")?;

        if metadata.is_none() {
            match discover_authorization_server_metadata(
                endpoints,
                &authorization_server(&authorization_server_url)?,
                AuthorizationServerMetadataOptions::default(),
            )
            .await
            {
                Ok(discovered) => metadata = discovered,
                Err(error) => {
                    if let Some(refused) = error.downcast_fetch_error::<RestrictedEndpointError>() {
                        return Err(refused.clone().into());
                    }
                    discovery_error = Some(error.to_string());
                }
            }
        }
        if metadata.is_none() {
            metadata = fallback_metadata(mcp, &authorization_server_url);
        }

        // Legacy MCP discovery falls back to the resource server's origin when
        // protected-resource metadata is unavailable. If root metadata advertises a
        // path-based issuer on that same origin, use it only after independently
        // retrieving and validating metadata from the issuer's RFC 8414 location.
        let names_authorization_servers = resource_metadata
            .as_ref()
            .and_then(|resource| resource.authorization_servers.as_ref())
            .is_some_and(|servers| !servers.is_empty());
        if let Some(root_metadata) = &metadata
            && !names_authorization_servers
            && let Some((issuer, issuer_metadata)) =
                metadata_for_same_origin_issuer(root_metadata, &authorization_server_url, endpoints)
                    .await?
        {
            authorization_server_url = issuer;
            metadata = Some(issuer_metadata);
        }

        let Some(metadata) = metadata else {
            return Err(UpstreamError::other(match discovery_error {
                Some(reason) => {
                    format!("OAuth provider metadata could not be discovered: {reason}")
                }
                None => "OAuth provider metadata could not be discovered".to_owned(),
            }));
        };
        validate_oauth_metadata(&metadata, endpoints, Some(&authorization_server_url)).await?;

        let resource = match &resource_metadata {
            Some(resource_metadata) => Some(resource_metadata.resource.as_str()),
            None => mcp.oauth_resource.as_deref(),
        };
        let resource = resource_for_mcp(mcp, resource)?;
        let joined = |scopes: Option<&[String]>| scopes.map(|scopes| scopes.join(" "));
        let scope = [
            mcp.oauth_scopes
                .as_deref()
                .map(|scopes| js::trim(scopes).to_owned()),
            joined(
                resource_metadata
                    .as_ref()
                    .and_then(|resource| resource.scopes_supported.as_deref()),
            ),
            joined(metadata.scopes_supported()),
        ]
        .into_iter()
        .flatten()
        .find(|scope| !scope.is_empty());

        Ok(OAuthContext {
            authorization_server_url,
            metadata,
            resource,
            scope,
        })
    }

    async fn metadata_for_authorization_server(
        &self,
        mcp: &Mcp,
        authorization_server_url: &str,
        endpoints: &OAuthEndpoints,
    ) -> Result<AuthorizationServerMetadata, UpstreamError> {
        let normalized =
            normalized_http_url(authorization_server_url, "OAuth authorization server")?;
        let discovered = match discover_authorization_server_metadata(
            endpoints,
            &authorization_server(&normalized)?,
            AuthorizationServerMetadataOptions::default(),
        )
        .await
        {
            Ok(metadata) => metadata,
            Err(error) => {
                if let Some(refused) = error.downcast_fetch_error::<RestrictedEndpointError>() {
                    return Err(refused.clone().into());
                }
                // Manual endpoint settings remain a fallback for OAuth providers without discovery.
                None
            }
        };
        let Some(metadata) =
            discovered.or_else(|| fallback_metadata(mcp, authorization_server_url))
        else {
            return Err(UpstreamError::other(
                "OAuth provider metadata could not be discovered",
            ));
        };
        validate_oauth_metadata(&metadata, endpoints, Some(&normalized)).await?;
        Ok(metadata)
    }

    fn save_oauth_configuration(
        &self,
        mcp: &mut Mcp,
        context: &OAuthContext,
        client: &OAuthClientInformationMixed,
        redirect_uri: &str,
        registered: bool,
    ) -> Result<(), UpstreamError> {
        let authorize_url = normalized_http_url(
            context.metadata.authorization_endpoint(),
            "OAuth authorization endpoint",
        )?;
        let token_url =
            normalized_http_url(context.metadata.token_endpoint(), "OAuth token endpoint")?;

        mcp.oauth_issuer = Some(context.authorization_server_url.clone());
        mcp.oauth_resource = context.resource.clone();
        mcp.oauth_redirect_uri = Some(redirect_uri.to_owned());
        mcp.oauth_authorize_url = Some(authorize_url);
        mcp.oauth_token_url = Some(token_url);
        if let Some(scope) = &context.scope {
            mcp.oauth_scopes = Some(scope.clone());
        }
        mcp.oauth_client_auth_method = client.token_endpoint_auth_method.clone();
        mcp.oauth_client_id = Some(client.client_id.clone());

        if registered {
            mcp.oauth_client_secret = self.core.encrypt_secret(client.client_secret.as_deref());
        }
        Ok(())
    }

    fn save_oauth_tokens(&self, mcp: &mut Mcp, tokens: IssuedTokens) {
        mcp.oauth_access_token = self.core.encrypt_secret(Some(&tokens.access_token));
        mcp.oauth_refresh_token = self.core.encrypt_secret(tokens.refresh_token.as_deref());
        mcp.oauth_token_type = Some(
            match tokens
                .token_type
                .filter(|token_type| !token_type.is_empty())
            {
                Some(token_type) if token_type.to_lowercase() != "bearer" => token_type,
                _ => "Bearer".to_owned(),
            },
        );
        mcp.oauth_token_expires_at = Some(token_expiry(tokens.expires_in));
        if let Some(scope) = tokens.scope.filter(|scope| !scope.is_empty()) {
            mcp.oauth_scopes = Some(scope);
        }
        mcp.oauth_required = false;
    }

    /// Ask the provider of a built-in MCP for tokens.
    async fn request_builtin_tokens(
        &self,
        mcp: &Mcp,
        grant: BuiltinGrant,
    ) -> Result<BuiltinOauthTokens, UpstreamError> {
        let (name, oauth) = self.require_builtin_oauth_mcp(mcp)?;
        let tokens = match &grant {
            BuiltinGrant::AuthorizationCode { code, redirect_uri } => {
                exchange_builtin_authorization_code(
                    &self.builtin_env,
                    name,
                    oauth,
                    mcp,
                    code,
                    redirect_uri,
                )
                .await
            }
            BuiltinGrant::RefreshToken(refresh_token) => {
                refresh_builtin_tokens(&self.builtin_env, name, oauth, mcp, refresh_token).await
            }
        };
        Ok(tokens?)
    }

    /// Built-in MCPs use the API application the admin registered with the
    /// provider, so there is nothing to discover or register.
    fn start_builtin_oauth_flow(
        &self,
        session: &dyn OauthSessionStore,
        mcp: &Mcp,
    ) -> Result<String, UpstreamError> {
        let (name, oauth) = self.require_builtin_oauth_mcp(mcp)?;
        let client_id = present(&mcp.oauth_client_id).filter(|_| {
            self.core
                .decrypt_secret(mcp.oauth_client_secret.as_deref())
                .is_some()
        });
        let Some(client_id) = client_id else {
            return Err(UpstreamError::other(format!(
                "Add the {name} Client ID and Client Secret before connecting"
            )));
        };

        let redirect_uri = self.oauth_callback_url()?;
        let state = random_base64url(24);
        start_oauth_session(
            session,
            mcp,
            OauthStart {
                redirect_uri: redirect_uri.clone(),
                authorization_server_url: oauth.issuer.to_owned(),
                resource: None,
                client_id: client_id.to_owned(),
                code_verifier: None,
                state: state.clone(),
            },
        );

        Ok(builtin_authorization_url(
            oauth,
            client_id,
            &redirect_uri,
            &state,
            &requested_builtin_scopes(oauth, mcp),
        )?)
    }

    /// Discover an upstream's OAuth provider, register a public client when needed,
    /// and create the browser authorization redirect.
    ///
    /// Returns the URL to send the browser to. The pending authorization is
    /// kept in `session` until the callback.
    pub async fn start_oauth_flow(
        &self,
        session: &dyn OauthSessionStore,
        mcp: &mut Mcp,
    ) -> Result<String, UpstreamError> {
        if mcp.transport == McpTransport::Builtin {
            return self.start_builtin_oauth_flow(session, mcp);
        }

        let endpoints = self.oauth_endpoints_for(mcp)?;
        let redirect_uri = self.oauth_redirect_uri(mcp)?;
        let context = self.discover_oauth_context(mcp, &endpoints).await?;
        let issuer = authorization_server(&context.authorization_server_url)?;
        let existing_client = self.client_information_from_mcp(mcp);
        let can_reuse_existing = present(&mcp.oauth_redirect_uri)
            .is_none_or(|saved| saved == redirect_uri)
            && present(&mcp.oauth_issuer)
                .is_none_or(|saved| saved == context.authorization_server_url);

        let (client, registered) = match existing_client {
            Some(client) if can_reuse_existing => (client, false),
            existing_client => {
                if context
                    .metadata
                    .registration_endpoint()
                    .is_none_or(str::is_empty)
                {
                    return Err(UpstreamError::other(if existing_client.is_some() {
                        "The OAuth redirect origin changed, but this provider does not support automatic client registration"
                    } else {
                        "This OAuth provider does not support automatic client registration"
                    }));
                }

                let mut client_metadata = Map::new();
                client_metadata.insert(
                    "client_name".to_owned(),
                    json!(registration_client_name(mcp.http_url.as_deref())),
                );
                client_metadata.insert("redirect_uris".to_owned(), json!([redirect_uri]));
                client_metadata.insert(
                    "grant_types".to_owned(),
                    json!(["authorization_code", "refresh_token"]),
                );
                client_metadata.insert("response_types".to_owned(), json!(["code"]));
                client_metadata.insert("token_endpoint_auth_method".to_owned(), json!("none"));
                if let Some(scope) = &context.scope {
                    client_metadata.insert("scope".to_owned(), json!(scope));
                }
                let registration = register_client(
                    &endpoints,
                    &issuer,
                    RegisterClientOptions {
                        metadata: Some(&context.metadata),
                        client_metadata: &Value::Object(client_metadata),
                        scope: context.scope.as_deref(),
                    },
                )
                .await?;
                (OAuthClientInformationMixed::from(registration), true)
            }
        };

        let state = random_base64url(24);
        let resource = resource_url(context.resource.as_deref())?;
        let authorization = start_authorization(
            &issuer,
            StartAuthorizationOptions {
                metadata: Some(&context.metadata),
                client_information: &client,
                redirect_url: &redirect_uri,
                scope: context.scope.as_deref(),
                state: Some(&state),
                resource: resource.as_ref(),
            },
        )?;

        // The provider is discovered again from metadata the MCP serves. A refresh
        // token kept across a change of provider or client would later be posted to
        // whichever token endpoint the MCP pointed this flow at.
        let issuer_changed = match saved_issuer(mcp)? {
            Some(previous_issuer) => {
                comparable_oauth_issuer(&previous_issuer, "OAuth issuer")?
                    != comparable_oauth_issuer(
                        &context.authorization_server_url,
                        "OAuth authorization server",
                    )?
            }
            None => false,
        };
        if registered || issuer_changed {
            forget_oauth_tokens(mcp);
        }

        self.save_oauth_configuration(mcp, &context, &client, &redirect_uri, registered)?;
        mcp.save(&*self.core.db).await?;
        start_oauth_session(
            session,
            mcp,
            OauthStart {
                redirect_uri,
                authorization_server_url: context.authorization_server_url,
                resource: context.resource,
                client_id: client.client_id,
                code_verifier: Some(authorization.code_verifier),
                state,
            },
        );

        Ok(parse_http_url(
            authorization.authorization_url.as_str(),
            "OAuth authorization URL",
        )?
        .to_string())
    }

    /// Exchange the code of a callback for tokens and save them.
    ///
    /// `granted_scope` is the callback's `scope` parameter, for providers that let
    /// the user uncheck permissions and report what is left there.
    pub async fn exchange_authorization_code(
        &self,
        mcp: &mut Mcp,
        oauth: &OauthSession,
        code: &str,
        granted_scope: Option<&str>,
    ) -> Result<(), UpstreamError> {
        let client = self
            .client_information_from_mcp(mcp)
            .filter(|client| client.client_id == oauth.client_id);
        let Some(client) = client else {
            return Err(UpstreamError::other(
                "OAuth client information is no longer available",
            ));
        };

        if mcp.transport == McpTransport::Builtin {
            let tokens = self
                .request_builtin_tokens(
                    mcp,
                    BuiltinGrant::AuthorizationCode {
                        code: code.to_owned(),
                        redirect_uri: oauth.redirect_uri.clone(),
                    },
                )
                .await?;
            let scopes = parse_oauth_scopes(tokens.scope.as_deref().or(granted_scope));
            self.save_oauth_tokens(
                mcp,
                IssuedTokens {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                    token_type: Some(tokens.token_type),
                    expires_in: tokens.expires_in,
                    scope: None,
                },
            );
            // Never keep the scopes of an earlier authorization for these tokens.
            mcp.oauth_scopes = (!scopes.is_empty()).then(|| scopes.join(" "));
            mcp.status = McpStatus::Ready;
            mcp.last_error = None;
            mcp.save(&*self.core.db).await?;
            return Ok(());
        }

        let Some(code_verifier) = present(&oauth.code_verifier) else {
            return Err(UpstreamError::other(
                "OAuth session is missing its PKCE code verifier",
            ));
        };

        let endpoints = self.oauth_endpoints_for(mcp)?;
        let metadata = self
            .metadata_for_authorization_server(mcp, &oauth.authorization_server_url, &endpoints)
            .await?;
        let resource = resource_for_mcp(mcp, oauth.resource.as_deref())?;
        let resource = resource_url(resource.as_deref())?;
        let tokens = exchange_authorization(
            &endpoints,
            &authorization_server(&oauth.authorization_server_url)?,
            ExchangeAuthorizationOptions {
                metadata: Some(&metadata),
                client_information: &client,
                authorization_code: code,
                code_verifier,
                redirect_uri: &oauth.redirect_uri,
                resource: resource.as_ref(),
            },
        )
        .await?;

        self.save_oauth_tokens(
            mcp,
            IssuedTokens {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                token_type: Some(tokens.token_type),
                expires_in: tokens.expires_in,
                scope: tokens.scope,
            },
        );
        mcp.status = McpStatus::Ready;
        mcp.last_error = None;
        mcp.save(&*self.core.db).await?;
        Ok(())
    }

    /// Read the row again, dropping changes that were not saved.
    pub(crate) async fn reload(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        match mcp.refresh(&*self.core.db).await {
            Err(sqlx::Error::RowNotFound) => Err(UpstreamError::other(format!(
                "\"Model.refresh\" failed. Unable to lookup \"mcps\" table where \"id\" = {}",
                mcp.id
            ))),
            outcome => Ok(outcome?),
        }
    }

    /// The renewal under way for this MCP, started now if there is none.
    /// The second value says whether this call started it.
    ///
    /// The gateway serves parallel requests in one process. All callers that
    /// hold the same MCP must share the rotation, including the save of the new
    /// pair. Do not key this by model identity or plaintext credentials.
    ///
    /// The renewal runs as a task of its own: a rotation that reached the
    /// provider is saved even when every request that waited for it is gone.
    fn pending_refresh(&self, mcp_id: i64) -> (PendingRefresh, bool) {
        /// Lets the next caller start a renewal, however this one ended.
        struct Release {
            upstream: Upstream,
            mcp_id: i64,
        }

        impl Drop for Release {
            fn drop(&mut self) {
                self.upstream
                    .pending_refreshes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&self.mcp_id);
            }
        }

        let (outcome, refresh) = {
            let mut pending = self
                .pending_refreshes
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(refresh) = pending.get(&mcp_id) {
                return (refresh.clone(), false);
            }
            let (outcome, refresh) = outcome_channel("The OAuth token refresh was interrupted");
            pending.insert(mcp_id, refresh.clone());
            (outcome, refresh)
        };

        let release = Release {
            upstream: self.clone(),
            mcp_id,
        };
        tokio::spawn(async move {
            let refreshed = release
                .upstream
                .refresh_current_oauth_access_token(mcp_id)
                .await;
            drop(release);
            // Nobody may be waiting any more.
            let _ = outcome.send(refreshed);
        });
        (refresh, true)
    }

    /// Refresh once per MCP; callers never proceed with a stale token on failure.
    ///
    /// On success `mcp` is the row as the renewal left it, whoever ran it.
    pub async fn refresh_oauth_access_token(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        let (refresh, started) = self.pending_refresh(mcp.id);
        if let Err(error) = refresh.await {
            // The caller that starts a renewal has its row read again first,
            // and goes on with that one when the renewal then fails.
            if started {
                let _ = self.reload(mcp).await;
            }
            return Err(error);
        }

        // Waiting callers have their own rows. They must use the saved
        // access token, not the old token from before they joined the shared refresh.
        self.reload(mcp).await
    }

    async fn refresh_current_oauth_access_token(&self, mcp_id: i64) -> Result<(), UpstreamError> {
        // A caller can hold an old row even after an earlier rotation has completed.
        let mut mcp = Mcp {
            id: mcp_id,
            ..Default::default()
        };
        self.reload(&mut mcp).await?;
        if mcp.auth_type != McpAuthType::Auto {
            return Ok(());
        }

        let refresh = self.core.decrypt_secret(mcp.oauth_refresh_token.as_deref());
        let is_fresh = mcp
            .oauth_token_expires_at
            .is_some_and(|expires_at| expires_at > Timestamp::now() + Duration::minutes(2));

        if mcp.transport == McpTransport::Builtin {
            let Some(refresh) = refresh.filter(|_| !is_fresh) else {
                return Ok(());
            };
            let tokens = self
                .request_builtin_tokens(&mcp, BuiltinGrant::RefreshToken(refresh.clone()))
                .await?;
            self.save_oauth_tokens(
                &mut mcp,
                IssuedTokens {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token.or(Some(refresh)),
                    token_type: Some(tokens.token_type),
                    expires_in: tokens.expires_in,
                    scope: None,
                },
            );
            mcp.save(&*self.core.db).await?;
            return Ok(());
        }

        let client = self.client_information_from_mcp(&mcp);
        let authorization_server_url = saved_issuer(&mcp)?.filter(|issuer| !issuer.is_empty());
        let (Some(refresh), Some(client), Some(authorization_server_url)) =
            (refresh, client, authorization_server_url)
        else {
            return Ok(());
        };

        if is_fresh {
            return Ok(());
        }

        // The token endpoint is discovered again for every refresh, so it is checked
        // again as well, as is the saved resource indicator.
        let endpoints = self.oauth_endpoints_for(&mcp)?;
        let metadata = self
            .metadata_for_authorization_server(&mcp, &authorization_server_url, &endpoints)
            .await?;
        let resource = resource_for_mcp(&mcp, mcp.oauth_resource.as_deref())?;
        let resource = resource_url(resource.as_deref())?;
        let tokens = refresh_authorization(
            &endpoints,
            &authorization_server(&authorization_server_url)?,
            RefreshAuthorizationOptions {
                metadata: Some(&metadata),
                client_information: &client,
                refresh_token: &refresh,
                resource: resource.as_ref(),
            },
        )
        .await?;

        self.save_oauth_tokens(
            &mut mcp,
            IssuedTokens {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                token_type: Some(tokens.token_type),
                expires_in: tokens.expires_in,
                scope: tokens.scope,
            },
        );
        mcp.save(&*self.core.db).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_an_origin_without_its_trailing_slash() {
        let normalized = |value: &str| normalized_http_url(value, "OAuth issuer").unwrap();
        assert_eq!(normalized("https://auth.example"), "https://auth.example");
        assert_eq!(
            normalized("HTTPS://Auth.Example:443/"),
            "https://auth.example"
        );
        assert_eq!(normalized("https://auth.example/?"), "https://auth.example");
        assert_eq!(normalized("https://auth.example/#"), "https://auth.example");
        assert_eq!(normalized("http://127.0.0.1:9998"), "http://127.0.0.1:9998");
        assert_eq!(
            normalized("https://auth.example/tenant"),
            "https://auth.example/tenant"
        );
        assert_eq!(
            normalized("https://auth.example/?a=1"),
            "https://auth.example/?a=1"
        );
        assert_eq!(
            normalized("https://user:pw@auth.example"),
            "https://user:pw@auth.example/"
        );
        assert_eq!(
            normalized("https://user@auth.example"),
            "https://user@auth.example/"
        );
        assert_eq!(
            normalized_http_url("ftp://auth.example", "OAuth issuer")
                .unwrap_err()
                .to_string(),
            "OAuth issuer must use HTTP or HTTPS"
        );
    }

    #[test]
    fn compares_issuers_without_their_credentials() {
        let comparable = |value: &str| comparable_oauth_issuer(value, "OAuth issuer").unwrap();
        assert_eq!(
            comparable("https://user:pw@auth.example/"),
            "https://auth.example"
        );
        assert_eq!(comparable("https://auth.example"), "https://auth.example");
        assert_eq!(
            comparable("https://user:pw@auth.example/tenant?x=1"),
            "https://auth.example/tenant?x=1"
        );
        assert_ne!(
            comparable("https://auth.example/a"),
            comparable("https://auth.example/a/")
        );
    }

    #[test]
    fn infers_the_issuer_of_rows_saved_before_it_was_stored() {
        let mcp = |authorize: Option<&str>, token: Option<&str>| Mcp {
            oauth_authorize_url: authorize.map(str::to_owned),
            oauth_token_url: token.map(str::to_owned),
            ..Default::default()
        };
        assert_eq!(infer_issuer(&mcp(None, None)).unwrap(), None);
        assert_eq!(infer_issuer(&mcp(Some(""), Some(""))).unwrap(), None);
        assert_eq!(
            infer_issuer(&mcp(
                Some("https://legacy.example/oauth/authorize?x=1"),
                None
            ))
            .unwrap(),
            Some("https://legacy.example".to_owned())
        );
        assert_eq!(
            infer_issuer(&mcp(Some(""), Some("https://legacy.example:8443/token"))).unwrap(),
            Some("https://legacy.example:8443".to_owned())
        );
        assert_eq!(
            infer_issuer(&mcp(Some("https://user:pw@legacy.example/authorize"), None)).unwrap(),
            Some("https://user:pw@legacy.example/".to_owned())
        );
        assert_eq!(
            infer_issuer(&mcp(Some("not a url"), None))
                .unwrap_err()
                .to_string(),
            "OAuth endpoint must be a valid URL"
        );
    }

    #[test]
    fn requires_the_resource_to_be_the_mcp_url_or_a_parent_of_it() {
        let mcp = Mcp {
            http_url: Some("https://mcp.example/mcp".to_owned()),
            ..Default::default()
        };
        assert_eq!(resource_for_mcp(&mcp, None).unwrap(), None);
        assert_eq!(resource_for_mcp(&mcp, Some("")).unwrap(), None);
        assert_eq!(
            resource_for_mcp(&mcp, Some("https://mcp.example/mcp"))
                .unwrap()
                .as_deref(),
            Some("https://mcp.example/mcp")
        );
        assert_eq!(
            resource_for_mcp(&mcp, Some("https://mcp.example/"))
                .unwrap()
                .as_deref(),
            Some("https://mcp.example")
        );
        for other in [
            "https://api.other.example/",
            "https://mcp.example/other",
            "https://mcp.example/mcp/deeper",
            "http://mcp.example/mcp",
            "https://mcp.example:8443/mcp",
        ] {
            assert_eq!(
                resource_for_mcp(&mcp, Some(other)).unwrap_err().to_string(),
                "OAuth protected resource does not match the MCP URL",
                "{other}"
            );
        }
        assert_eq!(
            resource_for_mcp(&mcp, Some("urn:example:mcp"))
                .unwrap_err()
                .to_string(),
            "OAuth resource must use HTTP or HTTPS"
        );
    }

    #[test]
    fn keeps_a_stated_lifetime_within_what_the_database_can_hold() {
        let seconds_away =
            |expires_in: Option<f64>| (token_expiry(expires_in) - Timestamp::now()).num_seconds();
        let about = |seconds: i64, expected: i64| (expected - 1..=expected).contains(&seconds);

        assert!(about(seconds_away(None), 3600));
        assert!(about(seconds_away(Some(600.0)), 600));
        assert!(about(seconds_away(Some(0.0)), 0));
        assert!(about(seconds_away(Some(f64::NAN)), 3600));
        let century = 100 * 365 * 24 * 60 * 60;
        assert!(about(seconds_away(Some(1e300)), century));
        assert!(about(seconds_away(Some(f64::INFINITY)), century));
        assert!(seconds_away(Some(-1e300)) < 0);
        // What is stored can be read back.
        let stored = token_expiry(Some(f64::INFINITY)).to_sql();
        assert_eq!(
            Timestamp::parse_sql(&stored),
            Some(token_expiry(Some(f64::INFINITY)))
        );
    }

    #[test]
    fn only_allowlisted_http_hosts_with_a_loopback_redirect_use_a_pasted_callback() {
        let mcp = |transport: McpTransport, url: &str| Mcp {
            transport,
            http_url: Some(url.to_owned()),
            ..Default::default()
        };
        assert!(uses_pasted_oauth_callback(&mcp(
            McpTransport::Http,
            "https://mcp.figma.com/mcp"
        )));
        assert!(!uses_pasted_oauth_callback(&mcp(
            McpTransport::Http,
            "https://mcp.strava.com/mcp"
        )));
        assert!(!uses_pasted_oauth_callback(&mcp(
            McpTransport::Http,
            "https://mcp.example/mcp"
        )));
        assert!(!uses_pasted_oauth_callback(&mcp(
            McpTransport::Npm,
            "https://mcp.figma.com/mcp"
        )));
    }
}
