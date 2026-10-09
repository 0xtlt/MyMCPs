//! The OAuth sign-in of built-in MCPs: the authorization URL, and the token
//! requests made with the client ID and secret of the API application the
//! admin registered with the provider.

use std::sync::LazyLock;
use std::time::Duration;

use mymcps_core::models::Mcp;
use mymcps_net::{FetchRequest, UpstreamResponseLimits};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::definition::{BuiltinEnv, BuiltinOauthConfig};
use crate::error::{BuiltinError, BuiltinResult};

const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Most characters of a provider's explanation that are passed on.
const MAX_FAILURE_CHARS: usize = 200;

/// Token endpoint JSON (snake_case as returned by OAuth providers).
pub static TOKEN_RESPONSE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "access_token" => vine::string().min_length(1),
        "token_type" => vine::string().optional(),
        "refresh_token" => vine::string().optional(),
        "expires_in" => vine::number().optional(),
        "scope" => vine::string().optional(),
    })
});

/// The reasons a provider gives for refusing a request, in the shape Strava
/// uses: `{ resource: 'Activity', field: 'sport_type', code: 'invalid' }`.
pub fn provider_faults() -> vine::VineArray {
    vine::array(vine::object! {
        "resource" => vine::string().optional(),
        "field" => vine::string().optional(),
        "code" => vine::string().optional(),
    })
}

/// The body of a refused token request: the error of RFC 6749, or the
/// `{ message, errors }` Strava answers with instead.
pub static TOKEN_FAILURE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "error" => vine::string().optional(),
        "error_description" => vine::string().optional(),
        "message" => vine::string().optional(),
        "errors" => provider_faults().optional(),
    })
});

/// What a token endpoint answered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BuiltinOauthTokens {
    pub access_token: String,
    /// `Bearer` when the provider did not say.
    #[serde(default = "bearer")]
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// In seconds.
    #[serde(default)]
    pub expires_in: Option<f64>,
    #[serde(default)]
    pub scope: Option<String>,
}

fn bearer() -> String {
    "Bearer".to_string()
}

fn is_scope_name(scope: &str) -> bool {
    (1..=64).contains(&scope.len())
        && scope.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'/' | b'-')
        })
}

/// Providers disagree on the separator, so store and compare scopes as a
/// list. The callback's `scope` parameter passes through the browser, so
/// anything that is not shaped like a scope name is dropped. Some providers
/// name their scopes with URLs, such as
/// `https://www.googleapis.com/auth/adwords`.
pub fn parse_oauth_scopes(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(|character: char| character == ',' || vine::js::is_whitespace(character))
        .filter(|scope| is_scope_name(scope))
        .take(32)
        .map(str::to_string)
        .collect()
}

/// Write scopes are only requested once the admin allowed write access.
pub fn requested_builtin_scopes(oauth: &BuiltinOauthConfig, mcp: &Mcp) -> Vec<&'static str> {
    let mut scopes = oauth.scopes.clone();
    if mcp.builtin_write_enabled {
        scopes.extend(&oauth.write_scopes);
    }
    scopes
}

pub fn builtin_authorization_url(
    oauth: &BuiltinOauthConfig,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scopes: &[&str],
) -> BuiltinResult<String> {
    let mut url = Url::parse(oauth.authorize_url).map_err(BuiltinError::internal)?;
    let mut parameters: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    // As `searchParams.set`: a parameter the URL already has is replaced in place.
    let mut set = |name: &str, value: String| match parameters
        .iter_mut()
        .find(|(existing, _)| existing == name)
    {
        Some(parameter) => parameter.1 = value,
        None => parameters.push((name.to_string(), value)),
    };
    set("client_id", client_id.to_string());
    set("redirect_uri", redirect_uri.to_string());
    set("response_type", "code".to_string());
    set("scope", scopes.join(oauth.scope_separator));
    set("state", state.to_string());
    for (name, value) in &oauth.authorize_params {
        set(name, (*value).to_string());
    }
    url.query_pairs_mut().clear().extend_pairs(parameters);
    Ok(url.to_string())
}

/// Explain a rejected token request from an RFC 6749 error body or from the
/// `{ message, errors: [{ resource, field, code }] }` shape Strava returns.
fn describe_token_failure(body: Option<&Value>) -> String {
    #[derive(Deserialize, Default)]
    struct Fault {
        resource: Option<String>,
        field: Option<String>,
        code: Option<String>,
    }
    #[derive(Deserialize)]
    struct Failure {
        error: Option<String>,
        error_description: Option<String>,
        message: Option<String>,
        #[serde(default)]
        errors: Vec<Fault>,
    }

    let Ok(failure) = TOKEN_FAILURE_VALIDATOR.validate_as::<Failure>(body) else {
        return String::new();
    };
    let present = |value: Option<String>| value.filter(|value| !value.is_empty());
    let faults: Vec<String> = failure
        .errors
        .into_iter()
        .map(|fault| {
            [fault.resource, fault.field, fault.code]
                .into_iter()
                .filter_map(present)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|fault| !fault.is_empty())
        .collect();
    let summary = [failure.error_description, failure.error, failure.message]
        .into_iter()
        .find_map(present);
    let described: Vec<String> = summary
        .into_iter()
        .chain((!faults.is_empty()).then(|| format!("({})", faults.join("; "))))
        .collect();
    described
        .join(" ")
        .chars()
        .take(MAX_FAILURE_CHARS)
        .collect()
}

enum TokenGrant<'a> {
    AuthorizationCode {
        code: &'a str,
        redirect_uri: Option<&'a str>,
    },
    RefreshToken {
        refresh_token: &'a str,
    },
}

async fn request_tokens(
    env: &BuiltinEnv,
    name: &str,
    oauth: &BuiltinOauthConfig,
    mcp: &Mcp,
    grant: TokenGrant<'_>,
) -> BuiltinResult<BuiltinOauthTokens> {
    let client_secret = env.core.decrypt_secret(mcp.oauth_client_secret.as_deref());
    let (Some(client_id), Some(client_secret)) = (
        mcp.oauth_client_id.as_deref().filter(|id| !id.is_empty()),
        client_secret,
    ) else {
        return Err(BuiltinError::internal(format!(
            "{name} Client ID and Client Secret are required"
        )));
    };

    // The serializer cannot move between threads: it is done with before the
    // request, so that the future of this function can.
    let (form, is_refresh) = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("client_id", client_id)
            .append_pair("client_secret", &client_secret);
        let is_refresh = match grant {
            TokenGrant::AuthorizationCode { code, redirect_uri } => {
                form.append_pair("grant_type", "authorization_code")
                    .append_pair("code", code);
                if let Some(redirect_uri) = redirect_uri {
                    form.append_pair("redirect_uri", redirect_uri);
                }
                false
            }
            TokenGrant::RefreshToken { refresh_token } => {
                form.append_pair("grant_type", "refresh_token")
                    .append_pair("refresh_token", refresh_token);
                true
            }
        };
        (form.finish(), is_refresh)
    };

    let token_url = Url::parse(oauth.token_url).map_err(BuiltinError::internal)?;
    let request = FetchRequest::post(token_url)
        .header("Accept", "application/json")?
        .header("Content-Type", "application/x-www-form-urlencoded")?
        .body(form)
        .timeout(TOKEN_REQUEST_TIMEOUT);
    let mut response = env
        .fetcher
        .fetch_with_same_origin_redirects(
            request,
            &format!("{name} token endpoint"),
            UpstreamResponseLimits::default(),
        )
        .await?;
    let body: Option<Value> = response.json().await.ok();

    if !response.ok() {
        let status = response.status().as_u16();
        let reason = describe_token_failure(body.as_ref());
        let rejected = status == 400 || status == 401;
        if is_refresh && rejected {
            let reason = if reason.is_empty() {
                String::new()
            } else {
                format!(" ({reason})")
            };
            return Err(BuiltinError::authorization(format!(
                "{name} refused to renew the saved authorization{reason}. Check the Client ID and Client Secret, then re-authorize this MCP in MyMCPs."
            )));
        }
        let reason = if reason.is_empty() {
            String::new()
        } else {
            format!(": {reason}")
        };
        let hint = if rejected {
            ". Check the Client ID and Client Secret."
        } else {
            ""
        };
        return Err(BuiltinError::internal(format!(
            "{name} rejected the token request (HTTP {status}){reason}{hint}"
        )));
    }

    TOKEN_RESPONSE_VALIDATOR
        .validate_as::<BuiltinOauthTokens>(body.as_ref())
        .map_err(|_| {
            BuiltinError::internal(format!("{name} returned an unexpected token response"))
        })
}

/// `redirect_uri` is the one the authorization request was sent with.
pub async fn exchange_builtin_authorization_code(
    env: &BuiltinEnv,
    name: &str,
    oauth: &BuiltinOauthConfig,
    mcp: &Mcp,
    code: &str,
    redirect_uri: &str,
) -> BuiltinResult<BuiltinOauthTokens> {
    let redirect_uri = oauth.sends_redirect_uri_with_code.then_some(redirect_uri);
    request_tokens(
        env,
        name,
        oauth,
        mcp,
        TokenGrant::AuthorizationCode { code, redirect_uri },
    )
    .await
}

pub async fn refresh_builtin_tokens(
    env: &BuiltinEnv,
    name: &str,
    oauth: &BuiltinOauthConfig,
    mcp: &Mcp,
    refresh_token: &str,
) -> BuiltinResult<BuiltinOauthTokens> {
    request_tokens(
        env,
        name,
        oauth,
        mcp,
        TokenGrant::RefreshToken { refresh_token },
    )
    .await
}
