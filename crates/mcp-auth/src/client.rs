//! The steps of the authorization code flow: dynamic client registration
//! (RFC 7591), the authorization request with PKCE, the exchange of the code
//! and the refresh of the tokens (`client/auth.js`).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, Method};
use serde_json::{Map, Value};
use url::Url;
use url::form_urlencoded;

use crate::errors::{Error, OAuthError, OAuthErrorKind};
use crate::http::{HttpFetch, HttpRequest, HttpResponse, fetch_within_origin, unfollowed_redirect};
use crate::js;
use crate::json::{self, Json};
use crate::pkce::pkce_challenge;
use crate::types::{
    AuthorizationServerMetadata, OAuthClientInformationFull, OAuthClientInformationMixed,
    OAuthErrorResponse, OAuthTokens,
};

const AUTHORIZATION_CODE_RESPONSE_TYPE: &str = "code";
const AUTHORIZATION_CODE_CHALLENGE_METHOD: &str = "S256";

/// How a client authenticates at the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientAuthMethod {
    /// HTTP Basic authentication (RFC 6749 Section 2.3.1)
    ClientSecretBasic,
    /// Credentials in the request body (RFC 6749 Section 2.3.1)
    ClientSecretPost,
    /// Public client (RFC 6749 Section 2.1)
    None,
}

impl ClientAuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientSecretBasic => "client_secret_basic",
            Self::ClientSecretPost => "client_secret_post",
            Self::None => "none",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "client_secret_basic" => Some(Self::ClientSecretBasic),
            "client_secret_post" => Some(Self::ClientSecretPost),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// Determines the best client authentication method to use based on server
/// support and client configuration.
///
/// Priority order (highest to lowest):
/// 1. client_secret_basic (if client secret is available)
/// 2. client_secret_post (if client secret is available)
/// 3. none (for public clients)
pub fn select_client_auth_method(
    client_information: &OAuthClientInformationMixed,
    supported_methods: &[String],
) -> ClientAuthMethod {
    let has_client_secret = client_information.client_secret.is_some();
    let supports = |method: ClientAuthMethod| {
        supported_methods
            .iter()
            .any(|supported| supported == method.as_str())
    };

    // Prefer the method returned by the server during client registration, if
    // valid. When server metadata is present we also require the method to be
    // listed as supported; when supportedMethods is empty (metadata omitted
    // the field) the DCR hint stands alone.
    let registered = client_information
        .token_endpoint_auth_method
        .as_deref()
        .and_then(ClientAuthMethod::from_name);
    if let Some(method) = registered
        && (supported_methods.is_empty() || supports(method))
    {
        return method;
    }

    // If server metadata omits token_endpoint_auth_methods_supported, RFC 8414
    // §2 says the default is client_secret_basic. RFC 6749 §2.3.1 also
    // requires servers to support HTTP Basic authentication for clients with
    // a secret, making it the safest default.
    if supported_methods.is_empty() {
        return if has_client_secret {
            ClientAuthMethod::ClientSecretBasic
        } else {
            ClientAuthMethod::None
        };
    }

    // Try methods in priority order (most secure first)
    if has_client_secret && supports(ClientAuthMethod::ClientSecretBasic) {
        return ClientAuthMethod::ClientSecretBasic;
    }
    if has_client_secret && supports(ClientAuthMethod::ClientSecretPost) {
        return ClientAuthMethod::ClientSecretPost;
    }
    if supports(ClientAuthMethod::None) {
        return ClientAuthMethod::None;
    }

    // Fallback: use what we have
    if has_client_secret {
        ClientAuthMethod::ClientSecretPost
    } else {
        ClientAuthMethod::None
    }
}

/// `URLSearchParams`: the parameters of a query or of a form body, in order.
#[derive(Debug, Default)]
struct SearchParams {
    pairs: Vec<(String, String)>,
}

impl SearchParams {
    fn of_query(url: &Url) -> Self {
        Self {
            pairs: url.query_pairs().into_owned().collect(),
        }
    }

    /// Replaces the value of the first parameter of that name and removes
    /// the others, or adds the parameter at the end.
    fn set(&mut self, name: &str, value: &str) {
        match self.pairs.iter().position(|(key, _)| key == name) {
            Some(first) => {
                self.pairs[first].1 = value.to_owned();
                let mut index = 0;
                self.pairs.retain(|(key, _)| {
                    index += 1;
                    index - 1 <= first || key != name
                });
            }
            None => self.append(name, value),
        }
    }

    fn append(&mut self, name: &str, value: &str) {
        self.pairs.push((name.to_owned(), value.to_owned()));
    }

    /// `application/x-www-form-urlencoded`
    fn serialize(&self) -> String {
        form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&self.pairs)
            .finish()
    }
}

/// Applies client authentication to the request based on the specified
/// method.
fn apply_client_authentication(
    method: ClientAuthMethod,
    client_information: &OAuthClientInformationMixed,
    headers: &mut HeaderMap,
    params: &mut SearchParams,
) -> Result<(), Error> {
    let client_id = client_information.client_id.as_str();
    let client_secret = client_information
        .client_secret
        .as_deref()
        .filter(|secret| !secret.is_empty());
    match method {
        ClientAuthMethod::ClientSecretBasic => {
            let Some(client_secret) = client_secret else {
                return Err(Error::ClientSecretRequired);
            };
            // `btoa`: one byte per character, and no character above U+00FF.
            let credentials = js::latin1_bytes(&format!("{client_id}:{client_secret}"))
                .ok_or(Error::InvalidCharacter)?;
            let value = format!("Basic {}", STANDARD.encode(credentials));
            let value = HeaderValue::from_str(&value).map_err(|_| Error::InvalidCharacter)?;
            headers.insert(AUTHORIZATION, value);
        }
        ClientAuthMethod::ClientSecretPost => {
            params.set("client_id", client_id);
            if let Some(client_secret) = client_secret {
                params.set("client_secret", client_secret);
            }
        }
        ClientAuthMethod::None => params.set("client_id", client_id),
    }
    Ok(())
}

/// Error codes that are names every JavaScript object inherits. The SDK looks
/// the class of an error up in a plain object, finds the inherited member
/// instead of nothing, and fails to construct it: the response is then
/// reported as one that could not be read.
///
/// `constructor` is the one such name left out. With it the SDK throws a
/// value that is not an error at all, which has no counterpart here: it is
/// read as any other code the SDK does not know.
const INHERITED_NAMES: [&str; 11] = [
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// Reads a body as an OAuth error document, or says why it is not one, the
/// way JavaScript prints the error caught.
fn oauth_error_of(body: &str) -> Result<OAuthError, String> {
    let document = json::parse(body).map_err(|error| format!("SyntaxError: {error}"))?;
    let response = OAuthErrorResponse::from_json(&document).map_err(|error| error.to_string())?;
    if INHERITED_NAMES.contains(&response.error.as_str()) {
        return Err("TypeError: errorClass is not a constructor".to_owned());
    }
    let kind = OAuthErrorKind::from_error_code(&response.error).unwrap_or(OAuthErrorKind::Server);
    Ok(OAuthError::new(
        kind,
        response.error_description.unwrap_or_default(),
        response.error_uri,
    ))
}

/// Parses an OAuth error response.
///
/// If the body is a standard OAuth 2.0 error response, the error is of the
/// class its `error` code names, with the `error_description` as its message.
/// Otherwise it is a [`OAuthErrorKind::Server`] error that names the status
/// and quotes the body, or describes the redirect that was not followed.
pub fn parse_error_response(response: &HttpResponse) -> OAuthError {
    let body = json::decode_body(&response.body);
    oauth_error_of(&body).unwrap_or_else(|reason| {
        // Not a valid OAuth error response, but try to inform the user of the
        // raw data anyway.
        let detail = unfollowed_redirect(response)
            .unwrap_or_else(|| format!("Invalid OAuth error response: {reason}. Raw body: {body}"));
        let status = response.status.as_u16();
        OAuthError::new(
            OAuthErrorKind::Server,
            format!("HTTP {status}: {detail}"),
            None,
        )
    })
}

/// [`parse_error_response`] for a body that came without a response.
pub fn parse_error_body(body: &str) -> OAuthError {
    oauth_error_of(body).unwrap_or_else(|reason| {
        OAuthError::new(
            OAuthErrorKind::Server,
            format!("Invalid OAuth error response: {reason}. Raw body: {body}"),
            None,
        )
    })
}

/// `new URL(path, authorizationServerUrl)`
fn at_authorization_server(authorization_server_url: &Url, path: &str) -> Result<Url, Error> {
    authorization_server_url
        .join(path)
        .map_err(|_| Error::InvalidUrl)
}

fn endpoint(url: &str) -> Result<Url, Error> {
    Url::parse(url).map_err(|_| Error::InvalidUrl)
}

#[derive(Debug, Clone, Copy)]
pub struct StartAuthorizationOptions<'a> {
    /// Without metadata, the authorization endpoint is `/authorize` at the
    /// authorization server.
    pub metadata: Option<&'a AuthorizationServerMetadata>,
    pub client_information: &'a OAuthClientInformationMixed,
    /// Sent as written: it has to be the redirect URI the client registered.
    pub redirect_url: &'a str,
    pub scope: Option<&'a str>,
    pub state: Option<&'a str>,
    /// RFC 8707 resource indicator.
    pub resource: Option<&'a Url>,
}

/// Where to send the user, and what to keep for the exchange of the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationStart {
    pub authorization_url: Url,
    pub code_verifier: String,
}

/// Begins the authorization flow with the given server, by generating a PKCE
/// challenge and constructing the authorization URL.
///
/// Parameters the authorization endpoint already carries in its query are
/// kept, and replaced when they are one of those set here.
pub fn start_authorization(
    authorization_server_url: &Url,
    options: StartAuthorizationOptions<'_>,
) -> Result<AuthorizationStart, Error> {
    let mut authorization_url = match options.metadata {
        Some(metadata) => {
            let url = endpoint(metadata.authorization_endpoint())?;
            if !metadata
                .response_types_supported()
                .iter()
                .any(|response_type| response_type == AUTHORIZATION_CODE_RESPONSE_TYPE)
            {
                return Err(Error::ResponseTypeNotSupported);
            }
            if let Some(methods) = metadata.code_challenge_methods_supported()
                && !methods
                    .iter()
                    .any(|method| method == AUTHORIZATION_CODE_CHALLENGE_METHOD)
            {
                return Err(Error::CodeChallengeMethodNotSupported);
            }
            url
        }
        None => at_authorization_server(authorization_server_url, "/authorize")?,
    };

    // Generate PKCE challenge
    let challenge = pkce_challenge();

    let mut params = SearchParams::of_query(&authorization_url);
    params.set("response_type", AUTHORIZATION_CODE_RESPONSE_TYPE);
    params.set("client_id", &options.client_information.client_id);
    params.set("code_challenge", &challenge.code_challenge);
    params.set("code_challenge_method", AUTHORIZATION_CODE_CHALLENGE_METHOD);
    params.set("redirect_uri", options.redirect_url);
    if let Some(state) = options.state.filter(|state| !state.is_empty()) {
        params.set("state", state);
    }
    let scope = options.scope.filter(|scope| !scope.is_empty());
    if let Some(scope) = scope {
        params.set("scope", scope);
    }
    if scope.is_some_and(|scope| scope.contains("offline_access")) {
        // if the request includes the OIDC-only "offline_access" scope,
        // we need to set the prompt to "consent" to ensure the user is
        // prompted to grant offline access
        // https://openid.net/specs/openid-connect-core-1_0.html#OfflineAccess
        params.append("prompt", "consent");
    }
    if let Some(resource) = options.resource {
        params.set("resource", resource.as_str());
    }
    authorization_url.set_query(Some(&params.serialize()));

    Ok(AuthorizationStart {
        authorization_url,
        code_verifier: challenge.code_verifier,
    })
}

struct TokenRequest<'a> {
    metadata: Option<&'a AuthorizationServerMetadata>,
    params: SearchParams,
    client_information: &'a OAuthClientInformationMixed,
    resource: Option<&'a Url>,
}

/// Executes a token request with the given parameters.
async fn execute_token_request<F: HttpFetch + ?Sized>(
    fetch: &F,
    authorization_server_url: &Url,
    request: TokenRequest<'_>,
) -> Result<OAuthTokens, Error> {
    let TokenRequest {
        metadata,
        mut params,
        client_information,
        resource,
    } = request;
    let token_url = match metadata.map(AuthorizationServerMetadata::token_endpoint) {
        Some(token_endpoint) if !token_endpoint.is_empty() => endpoint(token_endpoint)?,
        _ => at_authorization_server(authorization_server_url, "/token")?,
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    if let Some(resource) = resource {
        params.set("resource", resource.as_str());
    }

    let supported_methods = metadata
        .and_then(AuthorizationServerMetadata::token_endpoint_auth_methods_supported)
        .unwrap_or_default();
    let auth_method = select_client_auth_method(client_information, supported_methods);
    apply_client_authentication(auth_method, client_information, &mut headers, &mut params)?;

    let response = fetch_within_origin(
        fetch,
        HttpRequest {
            method: Method::POST,
            url: token_url,
            headers,
            body: Bytes::from(params.serialize()),
        },
    )
    .await
    .map_err(Error::Fetch)?;
    if !response.ok() {
        return Err(parse_error_response(&response).into());
    }
    let document = json::parse(&json::decode_body(&response.body))?;
    Ok(OAuthTokens::from_json(&document)?)
}

#[derive(Debug, Clone, Copy)]
pub struct ExchangeAuthorizationOptions<'a> {
    /// Without metadata, the token endpoint is `/token` at the authorization
    /// server.
    pub metadata: Option<&'a AuthorizationServerMetadata>,
    pub client_information: &'a OAuthClientInformationMixed,
    pub authorization_code: &'a str,
    pub code_verifier: &'a str,
    /// Sent as written: it has to be the one of the authorization request.
    pub redirect_uri: &'a str,
    /// RFC 8707 resource indicator.
    pub resource: Option<&'a Url>,
}

/// Exchanges an authorization code for an access token with the given server.
///
/// Supports multiple client authentication methods as specified in OAuth 2.1:
/// - Automatically selects the best authentication method based on server
///   support
/// - Falls back to appropriate defaults when server metadata is unavailable
pub async fn exchange_authorization<F: HttpFetch + ?Sized>(
    fetch: &F,
    authorization_server_url: &Url,
    options: ExchangeAuthorizationOptions<'_>,
) -> Result<OAuthTokens, Error> {
    let mut params = SearchParams::default();
    params.append("grant_type", "authorization_code");
    params.append("code", options.authorization_code);
    params.append("code_verifier", options.code_verifier);
    params.append("redirect_uri", options.redirect_uri);
    execute_token_request(
        fetch,
        authorization_server_url,
        TokenRequest {
            metadata: options.metadata,
            params,
            client_information: options.client_information,
            resource: options.resource,
        },
    )
    .await
}

#[derive(Debug, Clone, Copy)]
pub struct RefreshAuthorizationOptions<'a> {
    /// Without metadata, the token endpoint is `/token` at the authorization
    /// server.
    pub metadata: Option<&'a AuthorizationServerMetadata>,
    pub client_information: &'a OAuthClientInformationMixed,
    pub refresh_token: &'a str,
    /// RFC 8707 resource indicator.
    pub resource: Option<&'a Url>,
}

/// Exchange a refresh token for an updated access token.
///
/// The tokens returned keep the original refresh token if the server did not
/// return a new one.
pub async fn refresh_authorization<F: HttpFetch + ?Sized>(
    fetch: &F,
    authorization_server_url: &Url,
    options: RefreshAuthorizationOptions<'_>,
) -> Result<OAuthTokens, Error> {
    let mut params = SearchParams::default();
    params.append("grant_type", "refresh_token");
    params.append("refresh_token", options.refresh_token);
    let mut tokens = execute_token_request(
        fetch,
        authorization_server_url,
        TokenRequest {
            metadata: options.metadata,
            params,
            client_information: options.client_information,
            resource: options.resource,
        },
    )
    .await?;
    // Preserve original refresh token if server didn't return a new one
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(options.refresh_token.to_owned());
    }
    Ok(tokens)
}

#[derive(Debug, Clone, Copy)]
pub struct RegisterClientOptions<'a> {
    /// Without metadata, the registration endpoint is `/register` at the
    /// authorization server.
    pub metadata: Option<&'a AuthorizationServerMetadata>,
    /// The client metadata to register, a JSON object. Its members are sent
    /// in the order they have here.
    pub client_metadata: &'a Value,
    /// Overrides the `scope` of `client_metadata`.
    pub scope: Option<&'a str>,
}

/// Performs OAuth 2.0 Dynamic Client Registration according to RFC 7591.
///
/// If `scope` is provided, it overrides `client_metadata.scope` in the
/// registration request body. This allows callers to apply the Scope
/// Selection Strategy (SEP-835) consistently across both DCR and the
/// subsequent authorization request.
pub async fn register_client<F: HttpFetch + ?Sized>(
    fetch: &F,
    authorization_server_url: &Url,
    options: RegisterClientOptions<'_>,
) -> Result<OAuthClientInformationFull, Error> {
    let registration_url = match options.metadata {
        Some(metadata) => match metadata.registration_endpoint() {
            Some(registration_endpoint) if !registration_endpoint.is_empty() => {
                endpoint(registration_endpoint)?
            }
            _ => return Err(Error::RegistrationNotSupported),
        },
        None => at_authorization_server(authorization_server_url, "/register")?,
    };

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let response = fetch_within_origin(
        fetch,
        HttpRequest {
            method: Method::POST,
            url: registration_url,
            headers,
            body: Bytes::from(registration_body(options.client_metadata, options.scope)),
        },
    )
    .await
    .map_err(Error::Fetch)?;
    if !response.ok() {
        return Err(parse_error_response(&response).into());
    }
    let document = json::parse(&json::decode_body(&response.body))?;
    Ok(OAuthClientInformationFull::from_json(&document)?)
}

/// `JSON.stringify({ ...clientMetadata, ...(scope !== undefined ? { scope } : {}) })`
fn registration_body(client_metadata: &Value, scope: Option<&str>) -> String {
    // Through the JavaScript view of the value, so that members come in the
    // order JavaScript writes them and numbers the way it prints them.
    let mut body: Map<String, Value> = match Json::from_value(client_metadata).to_value() {
        Value::Object(members) => members,
        // Spreading anything else: an array gives its indices, a string its
        // characters, and the rest nothing.
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item))
            .collect(),
        Value::String(text) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                (
                    index.to_string(),
                    Value::String(String::from_utf16_lossy(&[unit])),
                )
            })
            .collect(),
        _ => Map::new(),
    };
    if let Some(scope) = scope {
        body.insert("scope".to_owned(), Value::String(scope.to_owned()));
    }
    Value::Object(body).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_the_first_parameter_and_drops_its_repeats() {
        let url = Url::parse("https://auth.example/authorize?a=1&b=2&a=3&c=4&a=5").unwrap();
        let mut params = SearchParams::of_query(&url);
        params.set("a", "x y");
        params.set("d", "~*'");
        assert_eq!(params.serialize(), "a=x+y&b=2&c=4&d=%7E*%27");
    }

    #[test]
    fn the_registration_body_keeps_the_members_in_the_order_given() {
        let metadata = serde_json::json!({
            "client_name": "MyMCPs",
            "redirect_uris": ["http://localhost:3333/mcps/oauth/callback"],
            "scope": "a",
            "token_endpoint_auth_method": "none",
        });
        assert_eq!(
            registration_body(&metadata, Some("b c")),
            r#"{"client_name":"MyMCPs","redirect_uris":["http://localhost:3333/mcps/oauth/callback"],"scope":"b c","token_endpoint_auth_method":"none"}"#
        );
        assert_eq!(registration_body(&serde_json::json!(null), None), "{}");
    }
}
