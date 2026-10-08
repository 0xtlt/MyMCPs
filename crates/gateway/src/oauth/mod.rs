//! The gateway's own OAuth 2.1 authorization server: the metadata documents
//! MCP clients discover it with, client registration (RFC 7591), the
//! authorization request with PKCE, and the token endpoint.
//!
//! Every function takes the parameters of a request as the web layer read
//! them: a JSON object in which a parameter sent once is a string, one sent
//! several times is an array, and one left out is missing. The strings of a
//! body arrive trimmed, and its empty ones as `null`.

pub(crate) mod constants;

mod client_metadata;

use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mymcps_core::crypto::{constant_time_eq, random_base64url};
use mymcps_core::models::{OauthAuthorizationCode, OauthClient};
use mymcps_core::public_url::PublicUrlError;
use mymcps_core::redaction::sanitize_diagnostic;
use mymcps_core::{Config, Core, Timestamp};
use mymcps_vine as vine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Sqlite};
use url::Url;
use url::form_urlencoded;

use crate::access_token::{self, CreatedOauthTokens, NewOauthGrant, OauthGrantRotation};
use crate::db::begin;
use crate::error::{Error, GatewayOauthError, Result};
use crate::js::{decode_base64, decode_hex, decode_uri_component};
use crate::validators::gateway_oauth::{
    AUTHORIZATION_CLIENT_ID, AUTHORIZATION_CODE_GRANT, AUTHORIZATION_REDIRECT_URI,
    AUTHORIZATION_RESPONSE_TYPE, AUTHORIZATION_STATE, AUTHORIZATION_STATE_LENGTH,
    CLIENT_AUTH_METHOD, CLIENT_GRANT_TYPES, CLIENT_NAME, CLIENT_REDIRECT_URIS,
    CLIENT_RESPONSE_TYPES, GATEWAY_RESOURCE, GatewayResource, PKCE_CHALLENGE, PKCE_VERIFIER,
    POSTED_CLIENT_CREDENTIALS, REFRESH_SCOPE, REFRESH_TOKEN_GRANT, REQUESTED_SCOPE,
    REVOCATION_REQUEST, RegisteredRedirectUris, TOKEN_REQUEST,
};

pub use crate::access_token::OAUTH_ACCESS_TOKEN_TTL_SECONDS;
pub use constants::{GATEWAY_OAUTH_SCOPE, LOOPBACK_HOSTS};

const AUTHORIZATION_CODE_TTL_MINUTES: i64 = 5;
const CLIENT_SECRET_TTL_DAYS: i64 = 365;

/// Registration is open to anyone, so the number of stored clients is bounded
/// and clients that nobody has used for the retention period are removed.
pub const MAX_OAUTH_CLIENTS: i64 = 1000;
pub const UNUSED_CLIENT_RETENTION_DAYS: i64 = 90;
const CLIENT_PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// An authorization request that waits for its user to sign in is kept in
/// the session, which is a cookie.
const MAX_SESSION_RETURN_PATH_BYTES: usize = 1536;

pub fn gateway_resource_url(config: &Config) -> Result<String, PublicUrlError> {
    Ok(format!("{}/mcp", config.require_public_app_url()?))
}

pub fn protected_resource_metadata_url(config: &Config) -> Result<String, PublicUrlError> {
    Ok(format!(
        "{}/.well-known/oauth-protected-resource/mcp",
        config.require_public_app_url()?
    ))
}

pub fn protected_resource_metadata(config: &Config) -> Result<Value, PublicUrlError> {
    let issuer = config.require_public_app_url()?;
    Ok(json!({
        "resource": gateway_resource_url(config)?,
        "authorization_servers": [issuer],
        "scopes_supported": [GATEWAY_OAUTH_SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "MyMCPs gateway",
    }))
}

pub fn authorization_server_metadata(config: &Config) -> Result<Value, PublicUrlError> {
    let issuer = config.require_public_app_url()?;
    Ok(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
        "revocation_endpoint": format!("{issuer}/revoke"),
        "scopes_supported": [GATEWAY_OAUTH_SCOPE],
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
        "revocation_endpoint_auth_methods_supported": [
            "none",
            "client_secret_post",
            "client_secret_basic",
        ],
        "code_challenge_methods_supported": ["S256"],
    }))
}

/// True when the redirect URI points at the user's own device.
pub fn is_loopback_redirect_uri(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| constants::has_loopback_host(&url))
}

/// The redirect URI with the parameters of an answer added to its query. A
/// parameter without a value is left out, and one the URI already carries is
/// replaced.
pub fn oauth_redirect(
    redirect_uri: &str,
    params: &[(&str, Option<&str>)],
) -> Result<String, url::ParseError> {
    let mut url = Url::parse(redirect_uri)?;
    let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let mut changed = false;

    for (name, value) in params {
        let Some(value) = value else {
            continue;
        };
        changed = true;
        match pairs.iter().position(|(existing, _)| existing == name) {
            Some(first) => {
                pairs[first].1 = (*value).to_string();
                let mut index = 0;
                pairs.retain(|(existing, _)| {
                    let kept = index <= first || existing != name;
                    index += 1;
                    kept
                });
            }
            None => pairs.push(((*name).to_string(), (*value).to_string())),
        }
    }

    if changed {
        url.query_pairs_mut().clear().extend_pairs(&pairs);
    }
    Ok(url.into())
}

fn same_url(first: &str, second: &str) -> bool {
    match (Url::parse(first), Url::parse(second)) {
        (Ok(first), Ok(second)) => first.as_str() == second.as_str(),
        _ => false,
    }
}

/// Whether a well-formed code verifier is the one behind the stored challenge.
pub fn verify_code_challenge(code_verifier: &str, expected_challenge: &str) -> bool {
    let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    actual.len() == expected_challenge.len()
        && constant_time_eq(actual.as_bytes(), expected_challenge.as_bytes())
}

/// The body of a successful answer of the token endpoint.
pub fn oauth_token_response(created: &CreatedOauthTokens) -> Value {
    let mut response = json!({
        "access_token": created.plaintext,
        "token_type": "Bearer",
        "expires_in": OAUTH_ACCESS_TOKEN_TTL_SECONDS,
        "scope": created.token.oauth_scopes.as_deref().unwrap_or(GATEWAY_OAUTH_SCOPE),
    });
    if let Some(refresh_token) = created
        .refresh_token
        .as_deref()
        .filter(|token| !token.is_empty())
    {
        response["refresh_token"] = json!(refresh_token);
    }
    response
}

/// An authorization request that names a registered client and one of its
/// redirect URIs, and asks for what the gateway grants.
#[derive(Debug, Clone)]
pub struct GatewayAuthorizationRequest {
    pub client: OauthClient,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub scopes: String,
    pub resource: String,
}

impl GatewayAuthorizationRequest {
    /// The path that brings a user back to this request once signed in, to
    /// keep in the session meanwhile. Fails with an error for the client
    /// when the request is too large for that.
    pub fn return_path(&self) -> Result<String, GatewayOauthError> {
        let mut params = form_urlencoded::Serializer::new(String::new());
        params
            .append_pair("client_id", &self.client.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("code_challenge", &self.code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("scope", &self.scopes)
            .append_pair("resource", &self.resource);
        if let Some(state) = self.state.as_deref().filter(|state| !state.is_empty()) {
            params.append_pair("state", state);
        }

        let return_path = format!("/authorize?{}", params.finish());
        if return_path.len() > MAX_SESSION_RETURN_PATH_BYTES {
            return Err(GatewayOauthError {
                redirect_uri: Some(self.redirect_uri.clone()),
                state: self.state.clone(),
                ..GatewayOauthError::new(
                    "invalid_request",
                    "The OAuth authorization request is too large",
                )
            });
        }
        Ok(return_path)
    }
}

/// What the `authorization_code` grant of a token request carries.
#[derive(Debug, Clone, Copy)]
pub struct AuthorizationCodeExchange<'a> {
    pub client: &'a OauthClient,
    pub code: &'a str,
    pub code_verifier: &'a str,
    pub redirect_uri: &'a str,
    pub resource: &'a str,
}

/// What the `refresh_token` grant of a token request carries.
#[derive(Debug, Clone, Copy)]
pub struct RefreshTokenExchange<'a> {
    pub client: &'a OauthClient,
    pub refresh_token: &'a str,
    pub scope: Option<&'a str>,
    pub resource: &'a str,
}

struct ClientCredentials {
    client_id: String,
    client_secret: Option<String>,
    method: &'static str,
}

static BASIC_AUTHORIZATION: LazyLock<vine::JsRegex> =
    LazyLock::new(|| vine::js::regex(r"^Basic\s+(.+)$", "i").expect("a valid static pattern"));

fn basic_client_credentials(header: Option<&str>) -> Option<ClientCredentials> {
    let encoded = BASIC_AUTHORIZATION
        .as_regex()
        .captures(header?)?
        .get(1)?
        .as_str();
    let decoded = decode_base64(encoded);
    let decoded = String::from_utf8_lossy(&decoded);
    let (client_id, client_secret) = decoded.split_once(':')?;
    Some(ClientCredentials {
        client_id: decode_uri_component(client_id)?,
        client_secret: Some(decode_uri_component(client_secret)?),
        method: "client_secret_basic",
    })
}

fn secure_secret_match(plaintext: &str, expected_hash: &str) -> bool {
    let actual = decode_hex(&access_token::hash(plaintext));
    let expected = decode_hex(expected_hash);
    actual.len() == expected.len() && constant_time_eq(&actual, &expected)
}

/// Read the parameters a token or revocation request must carry. The schemas
/// given here ask for nothing more than their presence, so the first field
/// that fails is a missing parameter, which the error names.
fn required_parameters(validator: &vine::Validator, input: &Value) -> Result<Value> {
    validator.validate(input).map_err(|error| {
        let field = error
            .messages
            .first()
            .map(|first| first.field.as_str())
            .unwrap_or_default();
        GatewayOauthError::new("invalid_request", format!("{field} is required")).into()
    })
}

fn parameter<'a>(parameters: &'a Value, name: &str) -> &'a str {
    parameters
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn invalid_client_metadata(message: &str) -> Error {
    GatewayOauthError::new("invalid_client_metadata", message).into()
}

/// Narrow a query on `oauth_clients` to the clients nobody is using: no
/// pending authorization code, and no token that still works or that was
/// issued or used since `active_since`. The second part keeps a client known
/// while its user signs in again after a grant expired.
fn push_unused_clients(query: &mut QueryBuilder<Sqlite>, active_since: Timestamp) {
    let now = Timestamp::now();
    query
        .push(
            "not exists (select * from `oauth_authorization_codes` \
             where `oauth_authorization_codes`.`oauth_client_id` = `oauth_clients`.`id` \
             and `oauth_authorization_codes`.`expires_at` > ",
        )
        .push_bind(now)
        .push(
            ") and not exists (select * from `access_tokens` \
             where `access_tokens`.`oauth_client_id` = `oauth_clients`.`id` \
             and (`access_tokens`.`updated_at` >= ",
        )
        .push_bind(active_since)
        .push(" or (`access_tokens`.`revoked_at` is null and (`access_tokens`.`expires_at` > ")
        .push_bind(now)
        .push(" or `access_tokens`.`oauth_refresh_expires_at` > ")
        .push_bind(now)
        .push("))))");
}

/// The OAuth server of one instance.
#[derive(Debug)]
pub struct OauthServer {
    core: Arc<Core>,
    last_client_prune_at: Mutex<Option<Instant>>,
}

impl OauthServer {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            last_client_prune_at: Mutex::new(None),
        }
    }

    /// The OAuth server only answers when `APP_URL` is a public HTTPS origin
    /// (plain HTTP on a loopback host will do in development): every one of
    /// its endpoints checks this first.
    pub fn ensure_configured(&self) -> Result<()> {
        self.core.config.require_public_app_url()?;
        Ok(())
    }

    fn gateway_resource(&self) -> Result<GatewayResource, PublicUrlError> {
        gateway_resource_url(&self.core.config).map(GatewayResource)
    }

    async fn find_client(&self, client_id: &str) -> Result<Option<OauthClient>, sqlx::Error> {
        sqlx::query_as("select * from `oauth_clients` where `client_id` = ? limit 1")
            .bind(client_id)
            .fetch_optional(&*self.core.db)
            .await
    }

    /// Remove clients that went unused for the whole retention period.
    /// Nothing else deletes them, so this runs with registration, at most
    /// once an hour unless `force` is set.
    pub async fn prune_unused_clients(&self, force: bool) {
        {
            let mut last_prune_at = self
                .last_client_prune_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !force && last_prune_at.is_some_and(|at| at.elapsed() < CLIENT_PRUNE_INTERVAL) {
                return;
            }
            *last_prune_at = Some(Instant::now());
        }

        let cutoff = Timestamp::now() - chrono::Duration::days(UNUSED_CLIENT_RETENTION_DAYS);
        let mut query = QueryBuilder::<Sqlite>::new("delete from `oauth_clients` where ");
        push_unused_clients(&mut query, cutoff);
        query.push(" and `created_at` < ").push_bind(cutoff);
        if let Err(error) = query.build().execute(&*self.core.db).await {
            tracing::warn!(
                error = %sanitize_diagnostic(&error.to_string()),
                "Unused OAuth clients could not be pruned"
            );
        }
    }

    /// Keep the client table within its limit. At the limit the oldest unused
    /// clients give way to the new one whatever their age, so that filling the
    /// table with throwaway registrations cannot lock real clients out.
    async fn make_room_for_client(&self) -> Result<()> {
        let total: i64 = sqlx::query_scalar("select count(*) from `oauth_clients`")
            .fetch_one(&*self.core.db)
            .await?;
        let excess = total - MAX_OAUTH_CLIENTS + 1;
        if excess <= 0 {
            return Ok(());
        }

        let active_since = Timestamp::now() - chrono::Duration::days(UNUSED_CLIENT_RETENTION_DAYS);
        let mut selection = QueryBuilder::<Sqlite>::new("select `id` from `oauth_clients` where ");
        push_unused_clients(&mut selection, active_since);
        selection
            .push(" order by `id` asc limit ")
            .push_bind(excess);
        let evictable: Vec<i64> = selection
            .build_query_scalar()
            .fetch_all(&*self.core.db)
            .await?;
        if (evictable.len() as i64) < excess {
            return Err(GatewayOauthError::with_status(
                "temporarily_unavailable",
                "Too many OAuth clients are registered",
                503,
            )
            .into());
        }

        let mut removal = QueryBuilder::<Sqlite>::new("delete from `oauth_clients` where ");
        push_unused_clients(&mut removal, active_since);
        removal.push(" and `id` in (");
        let mut ids = removal.separated(", ");
        for id in &evictable {
            ids.push_bind(*id);
        }
        removal.push(")");
        removal.build().execute(&*self.core.db).await?;
        Ok(())
    }

    /// Register a client (RFC 7591) and return the body of the answer: the
    /// metadata as the gateway recorded it, with the identifier and, unless
    /// the client is a public one, the secret it was given.
    pub async fn register_client(&self, input: &Value) -> Result<Value> {
        let Some(metadata) = client_metadata::parse_client_metadata(input) else {
            return Err(invalid_client_metadata("Invalid OAuth client metadata"));
        };

        // The first check that fails decides the error the client is given.
        if CLIENT_REDIRECT_URIS
            .validate(metadata.get("redirect_uris"))
            .is_err()
        {
            return Err(GatewayOauthError::new(
                "invalid_redirect_uri",
                "Redirect URIs must use HTTPS, HTTP on an exact loopback host, or an approved native-app callback",
            )
            .into());
        }

        let Ok(auth_method) =
            CLIENT_AUTH_METHOD.validate(metadata.get("token_endpoint_auth_method"))
        else {
            return Err(invalid_client_metadata(
                "Unsupported token endpoint authentication method",
            ));
        };
        let auth_method = auth_method.as_str().unwrap_or_default();

        let Ok(grant_types) = CLIENT_GRANT_TYPES.validate(metadata.get("grant_types")) else {
            return Err(invalid_client_metadata("Unsupported OAuth grant type"));
        };

        let Ok(response_types) = CLIENT_RESPONSE_TYPES.validate(metadata.get("response_types"))
        else {
            return Err(invalid_client_metadata(
                "Only the code response type is supported",
            ));
        };

        if REQUESTED_SCOPE.validate(metadata.get("scope")).is_err() {
            return Err(invalid_client_metadata("Unsupported OAuth scope"));
        }

        let Ok(name) = CLIENT_NAME.validate_opt(metadata.get("client_name")) else {
            return Err(invalid_client_metadata("Client name is too long"));
        };
        let client_name = name
            .as_ref()
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("MCP client");

        self.prune_unused_clients(false).await;
        self.make_room_for_client().await?;

        let client_id = format!("mcp_client_{}", random_base64url(24));
        let client_secret =
            (auth_method != "none").then(|| format!("mcp_secret_{}", random_base64url(32)));
        let secret_expires_at = client_secret
            .as_ref()
            .map(|_| Timestamp::now() + chrono::Duration::days(CLIENT_SECRET_TTL_DAYS));
        let mut client = OauthClient {
            client_id: client_id.clone(),
            client_secret_hash: client_secret.as_deref().map(access_token::hash),
            client_secret_prefix: client_secret
                .as_deref()
                .map(|secret| access_token::prefix(secret).to_string()),
            client_secret_expires_at: secret_expires_at,
            client_name: client_name.to_string(),
            redirect_uris: metadata
                .get("redirect_uris")
                .map(Value::to_string)
                .unwrap_or_default(),
            token_endpoint_auth_method: auth_method.to_string(),
            grant_types: grant_types.to_string(),
            response_types: response_types.to_string(),
            scope: GATEWAY_OAUTH_SCOPE.to_string(),
            ..Default::default()
        };
        client.insert(&*self.core.db).await?;

        let mut response = metadata;
        response.insert("client_name".into(), json!(client_name));
        response.insert("redirect_uris".into(), json!(client.redirect_uri_list()));
        response.insert("token_endpoint_auth_method".into(), json!(auth_method));
        response.insert("grant_types".into(), grant_types);
        response.insert("response_types".into(), response_types);
        response.insert("scope".into(), json!(GATEWAY_OAUTH_SCOPE));
        response.insert("client_id".into(), json!(client_id));
        response.insert(
            "client_id_issued_at".into(),
            json!(client.created_at.as_datetime().timestamp()),
        );
        if let (Some(client_secret), Some(secret_expires_at)) = (client_secret, secret_expires_at) {
            response.insert("client_secret".into(), json!(client_secret));
            response.insert(
                "client_secret_expires_at".into(),
                json!(secret_expires_at.as_datetime().timestamp()),
            );
        }
        Ok(Value::Object(response))
    }

    /// Read the parameters of an authorization request: the query string of
    /// `GET /authorize`, or the body of the consent form.
    ///
    /// An error that has a `redirect_uri` was found after the client and its
    /// redirect URI were established. Whether to send it there is for the
    /// caller to decide: see [`is_loopback_redirect_uri`].
    pub async fn parse_authorization_request(
        &self,
        input: &Value,
    ) -> Result<GatewayAuthorizationRequest> {
        let Ok(client_id) = AUTHORIZATION_CLIENT_ID.validate(input.get("client_id")) else {
            return Err(GatewayOauthError::new("invalid_request", "client_id is required").into());
        };

        let Some(client) = self
            .find_client(client_id.as_str().unwrap_or_default())
            .await?
        else {
            return Err(GatewayOauthError::new("invalid_client", "Unknown OAuth client").into());
        };

        let registered = RegisteredRedirectUris(client.redirect_uri_list());
        let Ok(Value::String(redirect_uri)) =
            AUTHORIZATION_REDIRECT_URI.validate_with(input.get("redirect_uri"), &registered)
        else {
            return Err(
                GatewayOauthError::new("invalid_request", "Unregistered redirect_uri").into(),
            );
        };

        // From here on an error can be returned to the client, and carries its
        // state. The state is therefore read now, and its length is checked last.
        let state = match AUTHORIZATION_STATE.validate_opt(input.get("state")) {
            Ok(Some(Value::String(state))) => Some(state),
            _ => None,
        };
        let redirect_error = |code: &'static str, message: &str| -> Error {
            GatewayOauthError {
                redirect_uri: Some(redirect_uri.clone()),
                state: state.clone(),
                ..GatewayOauthError::new(code, message)
            }
            .into()
        };

        if AUTHORIZATION_RESPONSE_TYPE
            .validate(input.get("response_type"))
            .is_err()
        {
            return Err(redirect_error(
                "unsupported_response_type",
                "Only the code response type is supported",
            ));
        }

        let Ok(pkce) = PKCE_CHALLENGE.validate(input) else {
            return Err(redirect_error(
                "invalid_request",
                "PKCE with the S256 method is required",
            ));
        };

        if REQUESTED_SCOPE.validate(input.get("scope")).is_err() {
            return Err(redirect_error("invalid_scope", "Unsupported OAuth scope"));
        }

        let gateway_resource = self.gateway_resource()?;
        if GATEWAY_RESOURCE
            .validate_with(input.get("resource"), &gateway_resource)
            .is_err()
        {
            return Err(redirect_error(
                "invalid_target",
                "The OAuth resource must be the MyMCPs gateway",
            ));
        }

        let sent_state = state.clone().map_or(Value::Null, Value::String);
        if AUTHORIZATION_STATE_LENGTH.validate(&sent_state).is_err() {
            return Err(redirect_error("invalid_request", "OAuth state is too long"));
        }

        let code_challenge = parameter(&pkce, "code_challenge").to_string();
        Ok(GatewayAuthorizationRequest {
            client,
            redirect_uri,
            state,
            code_challenge,
            scopes: GATEWAY_OAUTH_SCOPE.to_string(),
            resource: gateway_resource.0,
        })
    }

    /// Record that the user granted the request, and return the code its
    /// client exchanges for tokens. Only the hash of the code is stored.
    pub async fn create_authorization_code(
        &self,
        request: &GatewayAuthorizationRequest,
        user_id: i64,
    ) -> Result<String> {
        let plaintext = random_base64url(32);
        sqlx::query("delete from `oauth_authorization_codes` where `expires_at` < ?")
            .bind(Timestamp::now())
            .execute(&*self.core.db)
            .await?;
        let mut code = OauthAuthorizationCode {
            code_hash: access_token::hash(&plaintext),
            oauth_client_id: request.client.id,
            user_id,
            redirect_uri: request.redirect_uri.clone(),
            code_challenge: request.code_challenge.clone(),
            scopes: request.scopes.clone(),
            resource: request.resource.clone(),
            expires_at: Timestamp::now()
                + chrono::Duration::minutes(AUTHORIZATION_CODE_TTL_MINUTES),
            ..Default::default()
        };
        code.insert(&*self.core.db).await?;
        Ok(plaintext)
    }

    /// Authenticate the client of a token or revocation request, from its
    /// `Authorization` header or from the parameters of the request. A client
    /// may only use the method it registered.
    pub async fn authenticate_client(
        &self,
        authorization_header: Option<&str>,
        input: &Value,
    ) -> Result<OauthClient> {
        let basic = basic_client_credentials(authorization_header);
        let credentials = basic.or_else(|| {
            let posted = POSTED_CLIENT_CREDENTIALS.validate(input).ok()?;
            let client_secret = posted
                .get("client_secret")
                .and_then(Value::as_str)
                .filter(|secret| !secret.is_empty())
                .map(str::to_string);
            Some(ClientCredentials {
                client_id: parameter(&posted, "client_id").to_string(),
                method: if client_secret.is_some() {
                    "client_secret_post"
                } else {
                    "none"
                },
                client_secret,
            })
        });

        let Some(credentials) = credentials else {
            return Err(GatewayOauthError::with_status(
                "invalid_client",
                "OAuth client authentication is required",
                401,
            )
            .into());
        };

        let invalid_credentials = || -> Error {
            GatewayOauthError::with_status(
                "invalid_client",
                "Invalid OAuth client credentials",
                401,
            )
            .into()
        };
        let Some(client) = self
            .find_client(&credentials.client_id)
            .await?
            .filter(|client| client.token_endpoint_auth_method == credentials.method)
        else {
            return Err(invalid_credentials());
        };

        if let Some(secret_hash) = client
            .client_secret_hash
            .as_deref()
            .filter(|secret_hash| !secret_hash.is_empty())
        {
            let secret_matches = credentials
                .client_secret
                .as_deref()
                .filter(|secret| !secret.is_empty())
                .is_some_and(|secret| secure_secret_match(secret, secret_hash));
            let secret_expired = client
                .client_secret_expires_at
                .is_some_and(|expires_at| expires_at <= Timestamp::now());
            if !secret_matches || secret_expired {
                return Err(invalid_credentials());
            }
        }

        Ok(client)
    }

    /// Exchange an authorization code for the tokens of a new grant. A code
    /// works once: of two requests that present it, one gets the grant.
    pub async fn exchange_authorization_code(
        &self,
        params: AuthorizationCodeExchange<'_>,
    ) -> Result<CreatedOauthTokens> {
        let authorization_code: Option<OauthAuthorizationCode> = sqlx::query_as(
            "select * from `oauth_authorization_codes` where `code_hash` = ? limit 1",
        )
        .bind(access_token::hash(params.code))
        .fetch_optional(&*self.core.db)
        .await?;
        // A malformed verifier or a resource other than the gateway is answered
        // like a code that does not match.
        let malformed_verifier = PKCE_VERIFIER
            .validate(&Value::from(params.code_verifier))
            .is_err();
        let foreign_resource = GATEWAY_RESOURCE
            .validate_with(&Value::from(params.resource), &self.gateway_resource()?)
            .is_err();

        let Some(authorization_code) = authorization_code.filter(|authorization_code| {
            !malformed_verifier
                && !foreign_resource
                && authorization_code.oauth_client_id == params.client.id
                && authorization_code.expires_at > Timestamp::now()
                && authorization_code.redirect_uri == params.redirect_uri
                && same_url(&authorization_code.resource, params.resource)
                && verify_code_challenge(params.code_verifier, &authorization_code.code_challenge)
        }) else {
            return Err(GatewayOauthError::new(
                "invalid_grant",
                "Invalid or expired authorization code",
            )
            .into());
        };

        let client_supports_refresh = params
            .client
            .grant_type_list()
            .iter()
            .any(|grant_type| grant_type == "refresh_token");
        let mut transaction = begin(&self.core.db).await?;
        let deleted = sqlx::query("delete from `oauth_authorization_codes` where `id` = ?")
            .bind(authorization_code.id)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        if deleted != 1 {
            return Err(GatewayOauthError::new(
                "invalid_grant",
                "Authorization code was already used",
            )
            .into());
        }

        let created = access_token::create_oauth_grant(
            &mut *transaction,
            NewOauthGrant {
                name: &params.client.client_name,
                client_id: params.client.id,
                client_supports_refresh,
                scopes: &authorization_code.scopes,
                resource: &authorization_code.resource,
                created_by: authorization_code.user_id,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(created)
    }

    /// Exchange a refresh token for new tokens of the same grant.
    pub async fn exchange_refresh_token(
        &self,
        params: RefreshTokenExchange<'_>,
    ) -> Result<CreatedOauthTokens> {
        if !params
            .client
            .grant_type_list()
            .iter()
            .any(|grant_type| grant_type == "refresh_token")
        {
            return Err(GatewayOauthError::new(
                "unauthorized_client",
                "This OAuth client cannot refresh tokens",
            )
            .into());
        }

        let scope = params.scope.map_or(Value::Null, Value::from);
        if REFRESH_SCOPE.validate(&scope).is_err() {
            return Err(GatewayOauthError::new("invalid_scope", "Unsupported OAuth scope").into());
        }

        let gateway_resource = self.gateway_resource()?;
        if GATEWAY_RESOURCE
            .validate_with(&Value::from(params.resource), &gateway_resource)
            .is_err()
        {
            return Err(GatewayOauthError::new(
                "invalid_target",
                "The OAuth resource must be the MyMCPs gateway",
            )
            .into());
        }

        let rotation = access_token::rotate_oauth_grant(
            &self.core.db,
            params.refresh_token,
            params.client.id,
            &gateway_resource.0,
        )
        .await?;
        match rotation {
            OauthGrantRotation::Rotated(tokens) => Ok(*tokens),
            OauthGrantRotation::Invalid | OauthGrantRotation::Reused => {
                Err(GatewayOauthError::new(
                    "invalid_grant",
                    "Invalid, expired, or revoked refresh token",
                )
                .into())
            }
        }
    }

    /// Answer a request to the token endpoint: authenticate its client, then
    /// run the grant it names. `input` holds the parameters of the request,
    /// and [`oauth_token_response`] makes the body of the answer.
    pub async fn issue_tokens(
        &self,
        authorization_header: Option<&str>,
        input: &Value,
    ) -> Result<CreatedOauthTokens> {
        let client = self
            .authenticate_client(authorization_header, input)
            .await?;
        let request = required_parameters(&TOKEN_REQUEST, input)?;

        match parameter(&request, "grant_type") {
            "authorization_code" => {
                let grant = required_parameters(&AUTHORIZATION_CODE_GRANT, input)?;
                self.exchange_authorization_code(AuthorizationCodeExchange {
                    client: &client,
                    code: parameter(&grant, "code"),
                    code_verifier: parameter(&grant, "code_verifier"),
                    redirect_uri: parameter(&grant, "redirect_uri"),
                    resource: parameter(&grant, "resource"),
                })
                .await
            }
            "refresh_token" => {
                let grant = required_parameters(&REFRESH_TOKEN_GRANT, input)?;
                self.exchange_refresh_token(RefreshTokenExchange {
                    client: &client,
                    refresh_token: parameter(&grant, "refresh_token"),
                    scope: grant.get("scope").and_then(Value::as_str),
                    resource: parameter(&grant, "resource"),
                })
                .await
            }
            _ => Err(GatewayOauthError::new(
                "unsupported_grant_type",
                "Only authorization_code and refresh_token grants are supported",
            )
            .into()),
        }
    }

    /// Answer a request to the revocation endpoint (RFC 7009): authenticate
    /// its client, then revoke the grant its token belongs to. A token the
    /// client does not hold is not an error.
    pub async fn revoke_token(
        &self,
        authorization_header: Option<&str>,
        input: &Value,
    ) -> Result<()> {
        let client = self
            .authenticate_client(authorization_header, input)
            .await?;
        let request = required_parameters(&REVOCATION_REQUEST, input)?;
        access_token::revoke_oauth_token(&self.core.db, client.id, parameter(&request, "token"))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(header: &str) -> Option<(String, String)> {
        basic_client_credentials(Some(header)).map(|credentials| {
            assert_eq!(credentials.method, "client_secret_basic");
            (
                credentials.client_id,
                credentials.client_secret.unwrap_or_default(),
            )
        })
    }

    /// What the Node app read from these `Authorization` headers.
    #[test]
    fn reads_basic_credentials_as_the_node_app_did() {
        let pair = |id: &str, secret: &str| Some((id.to_owned(), secret.to_owned()));
        for (header, credentials) in [
            ("Basic YTpi", pair("a", "b")),
            ("basic   YTpi", pair("a", "b")),
            ("BASIC\tYTpi", pair("a", "b")),
            ("Basic\u{a0}YTpi", pair("a", "b")),
            ("Basic YT pi", pair("a", "b")),
            ("Basic YTpi\n", None),
            ("Basic", None),
            ("Basic ", None),
            ("Basic  ", None),
            ("BasicYTpi", None),
            (" Basic YTpi", None),
            ("Bearer YTpi", None),
            ("Basic !!!", None),
            // No colon.
            ("Basic YQ==", None),
            ("Basic Og==", pair("", "")),
            ("Basic YTo=", pair("a", "")),
            ("Basic OmI=", pair("", "b")),
            ("Basic YTpiOmM=", pair("a", "b:c")),
            // Both halves are percent-encoded: `a%20b:c%3Ad`, then `a%:b`.
            ("Basic YSUyMGI6YyUzQWQ=", pair("a b", "c:d")),
            ("Basic YSU6Yg==", None),
            ("Basic YStiOmMrZA==", pair("a+b", "c+d")),
            ("Basic JUMzJUE5OiVGMCU5RiU5OCU4MA==", pair("é", "😀")),
            ("Basic w6k6w7w=", pair("é", "ü")),
            (
                "Basic bWNwX2NsaWVudF94LV86bWNwX3NlY3JldF95LV8=",
                pair("mcp_client_x-_", "mcp_secret_y-_"),
            ),
        ] {
            assert_eq!(basic(header), credentials, "{header:?}");
        }
        assert!(basic_client_credentials(None).is_none());
    }

    #[test]
    fn matches_a_secret_against_its_stored_hash() {
        let stored = access_token::hash("mcp_secret_right");
        assert!(secure_secret_match("mcp_secret_right", &stored));
        assert!(secure_secret_match(
            "mcp_secret_right",
            &stored.to_uppercase()
        ));
        assert!(!secure_secret_match("mcp_secret_wrong", &stored));
        assert!(!secure_secret_match("mcp_secret_right", &stored[..62]));
        assert!(!secure_secret_match("mcp_secret_right", ""));
        assert!(!secure_secret_match("mcp_secret_right", "not hex"));
    }

    /// The example of RFC 7636, appendix B.
    #[test]
    fn verifies_a_code_verifier_against_its_s256_challenge() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_code_challenge(verifier, challenge));
        assert!(!verify_code_challenge(verifier, &challenge[..42]));
        assert!(!verify_code_challenge(verifier, &format!("{challenge}=")));
        assert!(!verify_code_challenge(verifier, &challenge.to_lowercase()));
        assert!(!verify_code_challenge(&verifier[1..], challenge));
        // A challenge is never its own verifier, as it would be with the `plain` method.
        assert!(!verify_code_challenge(challenge, challenge));
        assert!(!verify_code_challenge("", ""));
    }

    /// What `new URL(value).hostname` of Node 24 makes of these.
    #[test]
    fn tells_a_redirect_uri_on_the_users_own_device() {
        for loopback in [
            "http://127.0.0.1:49152/callback",
            "http://127.1/cb",
            "http://127.0.0.1./cb",
            "http://2130706433/cb",
            "http://[::1]:80/cb",
            "http://[0:0:0:0:0:0:0:1]/cb",
            "http://LOCALHOST/cb",
            "https://localhost/cb",
            "cursor://localhost/x",
            "http://localhost\\@evil.example/",
        ] {
            assert!(is_loopback_redirect_uri(loopback), "{loopback}");
        }
        for remote in [
            "https://client.example/callback",
            "http://localhost./cb",
            "http://sub.localhost/cb",
            "http://127.0.0.2/cb",
            "http://0/cb",
            "http://[::ffff:127.0.0.1]/cb",
            "http://localhost@evil.example/cb",
            "http://evil.example#@localhost/",
            "http://evil.example\\@localhost/",
            "cursor://anysphere.cursor-mcp/oauth/callback",
            "CURSOR://LOCALHOST/x",
            "file:///localhost",
            "localhost",
            "not a url",
            "",
        ] {
            assert!(!is_loopback_redirect_uri(remote), "{remote}");
        }
    }

    /// What `url.searchParams.set` and `url.href` of Node 24 give.
    #[test]
    fn adds_the_parameters_of_an_answer_to_a_redirect_uri() {
        let redirect =
            |uri: &str, params: &[(&str, Option<&str>)]| oauth_redirect(uri, params).unwrap();

        assert_eq!(
            redirect(
                "http://127.0.0.1:49152/callback",
                &[
                    ("error", Some("access_denied")),
                    (
                        "error_description",
                        Some("The user denied the authorization request")
                    ),
                    ("state", Some("a b&c=d#e%20")),
                ]
            ),
            "http://127.0.0.1:49152/callback?error=access_denied&error_description=The+user+denied+the+authorization+request&state=a+b%26c%3Dd%23e%2520"
        );
        // A parameter the URI already has is replaced where it stands, and
        // the rest of its query is written out again.
        assert_eq!(
            redirect(
                "https://client.example/cb?x=1&state=old&y=%20a+b&z=~!*()'",
                &[("code", Some("abc-_")), ("state", Some("né w/?:@"))]
            ),
            "https://client.example/cb?x=1&state=n%C3%A9+w%2F%3F%3A%40&y=+a+b&z=%7E%21*%28%29%27&code=abc-_"
        );
        assert_eq!(
            redirect(
                "https://client.example/cb?state=1&state=2&q",
                &[("state", Some("x"))]
            ),
            "https://client.example/cb?state=x&q="
        );
        assert_eq!(
            redirect(
                "https://client.example/cb?code=old",
                &[("code", Some("new")), ("error", None), ("state", Some("s"))]
            ),
            "https://client.example/cb?code=new&state=s"
        );
        assert_eq!(
            redirect(
                "https://client.example/cb?a=%41%zz&b=c%2Bd&e=é",
                &[("state", Some("x"))]
            ),
            "https://client.example/cb?a=A%25zz&b=c%2Bd&e=%C3%A9&state=x"
        );
        assert_eq!(
            redirect("https://client.example/cb?&&a=1&&", &[("code", Some("c"))]),
            "https://client.example/cb?a=1&code=c"
        );
        assert_eq!(
            redirect("https://client.example/cb?a=1;b=2", &[("code", Some("c"))]),
            "https://client.example/cb?a=1%3Bb%3D2&code=c"
        );
        assert_eq!(
            redirect("https://client.example/cb?=v&k", &[("code", Some("c"))]),
            "https://client.example/cb?=v&k=&code=c"
        );
        assert_eq!(
            redirect("https://client.example/cb?", &[("code", Some("c"))]),
            "https://client.example/cb?code=c"
        );
        // An empty value is a value, and the fragment stays last.
        assert_eq!(
            redirect(
                "cursor://anysphere.cursor-mcp/oauth/callback",
                &[("code", Some("abc")), ("state", Some(""))]
            ),
            "cursor://anysphere.cursor-mcp/oauth/callback?code=abc&state="
        );
        assert_eq!(
            redirect(
                "https://client.example/cb?a=b#frag",
                &[("code", Some("abc")), ("state", None)]
            ),
            "https://client.example/cb?a=b&code=abc#frag"
        );
        assert_eq!(
            redirect(
                "https://client.example",
                &[("state", Some("x")), ("error", Some("it's (a) *test* ~ok"))]
            ),
            "https://client.example/?state=x&error=it%27s+%28a%29+*test*+%7Eok"
        );
        // Without a parameter to add, the query is left as it was written.
        assert_eq!(
            redirect("https://client.example/cb?y=%20a", &[("state", None)]),
            "https://client.example/cb?y=%20a"
        );
        assert_eq!(
            redirect("HTTPS://Client.Example:443/cb/../x", &[]),
            "https://client.example/x"
        );
        assert!(oauth_redirect("not a url", &[("code", Some("c"))]).is_err());
    }

    /// What `` `/authorize?${new URLSearchParams(...)}` `` of Node 24 gives.
    #[test]
    fn writes_the_path_that_resumes_an_authorization_request() {
        let request = |state: Option<&str>| GatewayAuthorizationRequest {
            client: OauthClient {
                client_id: "mcp_client_a-b_c".into(),
                ..Default::default()
            },
            redirect_uri: "http://127.0.0.1:49152/callback".into(),
            state: state.map(str::to_owned),
            code_challenge: "x".repeat(43),
            scopes: "mcp:tools".into(),
            resource: "http://localhost:3333/mcp".into(),
        };
        let without_state = "/authorize?client_id=mcp_client_a-b_c&redirect_uri=http%3A%2F%2F127.0.0.1%3A49152%2Fcallback&response_type=code&code_challenge=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx&code_challenge_method=S256&scope=mcp%3Atools&resource=http%3A%2F%2Flocalhost%3A3333%2Fmcp";

        assert_eq!(
            request(Some("a b&c=d#e%20é~!*()'")).return_path().unwrap(),
            format!("{without_state}&state=a+b%26c%3Dd%23e%2520%C3%A9%7E%21*%28%29%27")
        );
        assert_eq!(request(None).return_path().unwrap(), without_state);
        assert_eq!(request(Some("")).return_path().unwrap(), without_state);

        // The limit is on the bytes of the path, state included.
        let room = MAX_SESSION_RETURN_PATH_BYTES - without_state.len() - "&state=".len();
        assert_eq!(
            request(Some(&"s".repeat(room)))
                .return_path()
                .unwrap()
                .len(),
            MAX_SESSION_RETURN_PATH_BYTES
        );
        let too_large = request(Some(&"s".repeat(room + 1)))
            .return_path()
            .unwrap_err();
        assert_eq!(too_large.code, "invalid_request");
        assert_eq!(
            too_large.redirect_uri.as_deref(),
            Some("http://127.0.0.1:49152/callback")
        );
        assert_eq!(too_large.state, Some("s".repeat(room + 1)));
    }
}
