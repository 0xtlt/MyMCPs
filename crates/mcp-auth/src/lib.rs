//! OAuth for the client side of MCP: what the gateway needs to be authorized
//! by an upstream MCP server.
//!
//! This is a port of the OAuth helpers of `@modelcontextprotocol/sdk` 1.32
//! (`client/auth.js`, `shared/auth.js`, `shared/auth-utils.js` and the errors
//! of `server/auth/errors.js`), kept to what goes on the wire and to how
//! responses are read:
//!
//! - [`discover_oauth_server_info`] finds the authorization server of an MCP
//!   server (RFC 9728, then RFC 8414 or OpenID Connect Discovery), and
//!   [`discover_authorization_server_metadata`] reads the metadata of one;
//! - [`register_client`] registers a client (RFC 7591);
//! - [`start_authorization`] builds the authorization URL, with PKCE;
//! - [`exchange_authorization`] and [`refresh_authorization`] obtain tokens;
//! - [`check_resource_allowed`] and [`resource_url_from_server_url`] handle
//!   the resource indicator of RFC 8707.
//!
//! It is a protocol library: it opens no socket. Every function that needs
//! the network takes an [`HttpFetch`], which is where the caller applies its
//! own rules (which addresses may be reached, redirects, size limits,
//! identity headers). An error of that fetch comes back as [`Error::Fetch`]
//! with the error the fetch returned inside.
//!
//! The messages of the errors are those of the SDK, which the administrator
//! reads when a connection fails.
//!
//! # A whole flow
//!
//! ```
//! use mymcps_mcp_auth::{
//!     AuthorizationServerMetadata, Error, ExchangeAuthorizationOptions, HttpFetch,
//!     OAuthClientInformationMixed, OAuthTokens, RefreshAuthorizationOptions,
//!     RegisterClientOptions, ServerInfoOptions, StartAuthorizationOptions,
//!     discover_oauth_server_info, exchange_authorization, refresh_authorization,
//!     register_client, start_authorization,
//! };
//! use url::Url;
//!
//! async fn connect(fetch: &impl HttpFetch, code: &str) -> Result<OAuthTokens, Error> {
//!     let server_url = Url::parse("https://mcp.example/mcp").map_err(|_| Error::InvalidUrl)?;
//!     let redirect_uri = "https://gateway.example/mcps/oauth/callback";
//!
//!     // Discovery. Only the metadata of the authorization server can fail
//!     // it: a server without protected resource metadata is its own
//!     // authorization server.
//!     let info = discover_oauth_server_info(fetch, &server_url, ServerInfoOptions::default()).await?;
//!     let issuer = Url::parse(&info.authorization_server_url).map_err(|_| Error::InvalidUrl)?;
//!     let metadata: Option<&AuthorizationServerMetadata> =
//!         info.authorization_server_metadata.as_ref();
//!     let resource = info
//!         .resource_metadata
//!         .as_ref()
//!         .and_then(|metadata| Url::parse(&metadata.resource).ok());
//!     let scope = "read write";
//!
//!     // Registration. The members are sent in the order written here.
//!     let registered = register_client(
//!         fetch,
//!         &issuer,
//!         RegisterClientOptions {
//!             metadata,
//!             client_metadata: &serde_json::json!({
//!                 "client_name": "MyMCPs",
//!                 "redirect_uris": [redirect_uri],
//!                 "grant_types": ["authorization_code", "refresh_token"],
//!                 "response_types": ["code"],
//!                 "token_endpoint_auth_method": "none",
//!             }),
//!             scope: Some(scope),
//!         },
//!     )
//!     .await?;
//!     let client = OAuthClientInformationMixed::from(&registered);
//!
//!     // The browser goes to `authorization_url`, and comes back with a code.
//!     let start = start_authorization(
//!         &issuer,
//!         StartAuthorizationOptions {
//!             metadata,
//!             client_information: &client,
//!             redirect_url: redirect_uri,
//!             scope: Some(scope),
//!             state: Some("state-kept-in-the-session"),
//!             resource: resource.as_ref(),
//!         },
//!     )?;
//!     let _send_the_browser_to = start.authorization_url.as_str();
//!
//!     let tokens = exchange_authorization(
//!         fetch,
//!         &issuer,
//!         ExchangeAuthorizationOptions {
//!             metadata,
//!             client_information: &client,
//!             authorization_code: code,
//!             code_verifier: &start.code_verifier,
//!             redirect_uri,
//!             resource: resource.as_ref(),
//!         },
//!     )
//!     .await?;
//!
//!     // Later, with the refresh token that was kept. The tokens returned
//!     // carry it again when the server did not rotate it.
//!     let Some(refresh_token) = tokens.refresh_token.as_deref() else {
//!         return Ok(tokens);
//!     };
//!     refresh_authorization(
//!         fetch,
//!         &issuer,
//!         RefreshAuthorizationOptions {
//!             metadata,
//!             client_information: &client,
//!             refresh_token,
//!             resource: resource.as_ref(),
//!         },
//!     )
//!     .await
//! }
//! ```
//!
//! # Telling errors apart
//!
//! ```
//! use mymcps_mcp_auth::{Error, OAuthErrorKind};
//!
//! # #[derive(Debug)] struct RestrictedEndpointError;
//! # impl std::fmt::Display for RestrictedEndpointError {
//! #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("refused") }
//! # }
//! # impl std::error::Error for RestrictedEndpointError {}
//! fn describe(error: &Error) -> String {
//!     // An error of the fetch is the one the fetch returned.
//!     if error.downcast_fetch_error::<RestrictedEndpointError>().is_some() {
//!         return "the provider points into a private network".to_owned();
//!     }
//!     match error {
//!         Error::OAuth(error) if error.kind() == OAuthErrorKind::InvalidGrant => {
//!             "the authorization is no longer valid".to_owned()
//!         }
//!         // What `error.message` is in JavaScript.
//!         other => format!("OAuth discovery failed: {other}"),
//!     }
//! }
//! ```

mod client;
mod discovery;
mod errors;
mod http;
mod js;
mod json;
mod pkce;
mod resource;
mod schema;
mod types;

pub use client::{
    AuthorizationStart, ClientAuthMethod, ExchangeAuthorizationOptions,
    RefreshAuthorizationOptions, RegisterClientOptions, StartAuthorizationOptions,
    exchange_authorization, parse_error_body, parse_error_response, refresh_authorization,
    register_client, select_client_auth_method, start_authorization,
};
pub use discovery::{
    AuthorizationServerMetadataOptions, DiscoveryUrl, LATEST_PROTOCOL_VERSION, OAuthServerInfo,
    ProtectedResourceMetadataOptions, ServerInfoOptions, WwwAuthenticateParams,
    build_discovery_urls, discover_authorization_server_metadata,
    discover_oauth_protected_resource_metadata, discover_oauth_server_info,
    extract_www_authenticate_params,
};
pub use errors::{Error, MetadataDocument, OAuthError, OAuthErrorKind, UnauthorizedError};
pub use http::{BoxError, HttpFetch, HttpFetchError, HttpRequest, HttpResponse};
pub use json::JsonSyntaxError;
pub use pkce::{PkceChallenge, generate_challenge, pkce_challenge};
pub use resource::{check_resource_allowed, resource_url_from_server_url};
pub use schema::SchemaError;
pub use types::{
    AuthorizationServerMetadata, OAuthClientInformationFull, OAuthClientInformationMixed,
    OAuthClientMetadata, OAuthErrorResponse, OAuthMetadata, OAuthProtectedResourceMetadata,
    OAuthTokens, OpenIdProviderDiscoveryMetadata,
};
