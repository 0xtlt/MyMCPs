//! How a request to `/mcp` is let in: a Bearer access token that is usable,
//! that was issued for this gateway when it comes from OAuth, and that is
//! within its request allowance.

use mymcps_core::Config;
use mymcps_core::limiter::LimiterError;
use mymcps_core::models::{AccessToken, Mcp, TokenSource};
use mymcps_vine as vine;
use serde_json::{Value, json};
use url::Url;

use crate::Gateway;
use crate::access_token;
use crate::oauth::{GATEWAY_OAUTH_SCOPE, gateway_resource_url, protected_resource_metadata_url};

/// What an authenticated request may reach.
#[derive(Debug, Clone)]
pub struct BearerAccess {
    pub access_token: AccessToken,
    /// The enabled MCPs the token may use, by name.
    pub allowed_mcps: Vec<Mcp>,
}

/// Why a request is not let in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BearerRejection {
    #[error("Missing Bearer access token")]
    Missing,
    #[error("Empty Bearer access token")]
    Empty,
    #[error("Invalid, expired, or revoked access token")]
    Invalid,
    /// `retry_after` is in seconds.
    #[error("Too many requests for this access token")]
    RateLimited { retry_after: u64 },
}

impl BearerRejection {
    pub fn status(&self) -> u16 {
        match self {
            Self::RateLimited { .. } => 429,
            _ => 401,
        }
    }

    /// The JSON body of the answer.
    pub fn body(&self) -> Value {
        let error = match self {
            Self::RateLimited { .. } => "rate_limited",
            _ => "unauthorized",
        };
        json!({ "error": error, "message": self.to_string() })
    }

    /// The `WWW-Authenticate` header of the answer, which tells an MCP
    /// client where to find the OAuth server.
    pub fn www_authenticate(&self, config: &Config) -> Option<String> {
        let error = match self {
            Self::Missing | Self::Empty => None,
            Self::Invalid => Some("invalid_token"),
            Self::RateLimited { .. } => return None,
        };
        let mut values = Vec::new();
        if let Some(error) = error {
            values.push(format!("error=\"{error}\""));
        }
        // Keep bearer authentication available while refusing to publish insecure OAuth metadata.
        if let Ok(resource_metadata) = protected_resource_metadata_url(config) {
            values.push(format!("resource_metadata=\"{resource_metadata}\""));
        }
        values.push(format!("scope=\"{GATEWAY_OAUTH_SCOPE}\""));
        Some(format!("Bearer {}", values.join(", ")))
    }

    /// The `Retry-After` header of the answer, in seconds.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BearerError {
    #[error(transparent)]
    Rejected(#[from] BearerRejection),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn has_valid_oauth_audience_and_scope(config: &Config, token: &AccessToken) -> bool {
    if token.source != TokenSource::Oauth {
        return true;
    }
    let (Some(resource), Ok(gateway_resource)) = (
        token.oauth_resource.as_deref(),
        gateway_resource_url(config),
    ) else {
        return false;
    };
    Url::parse(resource).is_ok_and(|resource| resource.as_str() == gateway_resource)
        && token.oauth_scopes.as_deref() == Some(GATEWAY_OAUTH_SCOPE)
}

impl Gateway {
    /// Authenticate a request to `/mcp` from its `Authorization` header and
    /// hold its token to the request allowance.
    pub async fn authenticate_bearer(
        &self,
        authorization_header: Option<&str>,
    ) -> Result<BearerAccess, BearerError> {
        let Some(credentials) = authorization_header.and_then(|header| {
            header
                .split_at_checked(7)
                .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer "))
                .map(|(_, credentials)| credentials)
        }) else {
            return Err(BearerRejection::Missing.into());
        };

        let plaintext = vine::js::trim(credentials);
        if plaintext.is_empty() {
            return Err(BearerRejection::Empty.into());
        }

        let db = &self.core.db;
        let Some(mut token) = access_token::find_usable_by_plaintext(db, plaintext)
            .await?
            .filter(|token| has_valid_oauth_audience_and_scope(&self.core.config, token))
        else {
            return Err(BearerRejection::Invalid.into());
        };

        match self
            .rate_limiter
            .consume(&format!("mcp:{}", token.id))
            .await
        {
            Ok(_) => {}
            Err(LimiterError::TooManyRequests(response)) => {
                return Err(BearerRejection::RateLimited {
                    retry_after: response.available_in,
                }
                .into());
            }
            Err(LimiterError::Store(error)) => return Err(error.into()),
        }

        access_token::touch_last_used(db, &mut token).await?;
        let allowed_mcps = access_token::resolve_allowed_mcps(db, &token).await?;

        Ok(BearerAccess {
            access_token: token,
            allowed_mcps,
        })
    }
}
