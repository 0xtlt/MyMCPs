//! What the helpers fail with. Each error prints the `message` of the
//! JavaScript error the SDK throws in its place: these messages are shown to
//! the administrator.

use std::fmt;

use url::Url;

use crate::http::HttpFetchError;
use crate::json::JsonSyntaxError;
use crate::schema::SchemaError;

/// A failure of one of the helpers of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The caller's fetch failed, where the SDK lets the error of `fetchFn`
    /// through. It is the error the fetch returned, unchanged: see
    /// [`Error::fetch_error`] to tell which one.
    #[error(transparent)]
    Fetch(HttpFetchError),

    /// No document at the well-known locations of RFC 9728.
    #[error("Resource server does not implement OAuth 2.0 Protected Resource Metadata.")]
    ProtectedResourceMetadataNotImplemented,

    #[error("HTTP {status} trying to load well-known OAuth protected resource metadata.")]
    ProtectedResourceMetadataStatus { status: u16 },

    /// A server error at one of the locations of the authorization server
    /// metadata. A 4xx there only moves discovery on to the next location.
    #[error("HTTP {status} trying to load {document} metadata from {url}")]
    AuthorizationServerMetadataStatus {
        status: u16,
        document: MetadataDocument,
        url: Url,
    },

    #[error("Incompatible auth server: does not support response type code")]
    ResponseTypeNotSupported,

    #[error("Incompatible auth server: does not support code challenge method S256")]
    CodeChallengeMethodNotSupported,

    #[error("Incompatible auth server: does not support dynamic client registration")]
    RegistrationNotSupported,

    /// The client is to authenticate with `client_secret_basic` and has no
    /// secret, or an empty one.
    #[error("client_secret_basic authentication requires a client_secret")]
    ClientSecretRequired,

    /// The `TypeError` of `new URL(...)`: an endpoint of the metadata is not
    /// a URL, or the authorization server URL has no origin to resolve the
    /// well-known paths against.
    #[error("Invalid URL")]
    InvalidUrl,

    /// The `InvalidCharacterError` of `btoa`: a client identifier or secret
    /// sent with `client_secret_basic` holds a character above U+00FF.
    #[error("Invalid character")]
    InvalidCharacter,

    /// A response that should be JSON is not (`SyntaxError`).
    #[error(transparent)]
    Json(#[from] JsonSyntaxError),

    /// A response is JSON but not the document expected (`ZodError`).
    #[error(transparent)]
    Schema(#[from] SchemaError),

    /// The token or registration endpoint answered with an error.
    #[error(transparent)]
    OAuth(#[from] OAuthError),
}

impl Error {
    /// The `name` of the JavaScript error the SDK throws here, `None` for
    /// the error of the caller's own fetch.
    pub fn name(&self) -> Option<&'static str> {
        Some(match self {
            Error::Fetch(_) => return None,
            Error::InvalidUrl => "TypeError",
            Error::InvalidCharacter => "InvalidCharacterError",
            Error::Json(_) => "SyntaxError",
            Error::Schema(_) => "ZodError",
            Error::OAuth(error) => error.name(),
            _ => "Error",
        })
    }

    /// The error of the caller's fetch, when that is what failed.
    pub fn fetch_error(&self) -> Option<&HttpFetchError> {
        match self {
            Error::Fetch(error) => Some(error),
            _ => None,
        }
    }

    /// The error of the caller's fetch, if that is what failed and it is a `T`.
    pub fn downcast_fetch_error<T: std::error::Error + 'static>(&self) -> Option<&T> {
        self.fetch_error()?.downcast_ref()
    }
}

/// Which of the two metadata documents a location serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataDocument {
    /// RFC 8414, `/.well-known/oauth-authorization-server`
    OAuth,
    /// OpenID Connect Discovery, `/.well-known/openid-configuration`
    OpenId,
}

impl fmt::Display for MetadataDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            MetadataDocument::OAuth => "OAuth",
            MetadataDocument::OpenId => "OpenID provider",
        })
    }
}

/// An OAuth error response of a token or registration endpoint
/// (`OAuthError` of `server/auth/errors.js` and its subclasses).
///
/// Its text is the JavaScript `message`: the `error_description` of the
/// response, which is empty when the server gave none. A response that is not
/// an OAuth error document becomes a [`OAuthErrorKind::Server`] error whose
/// message quotes the status and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthError {
    kind: OAuthErrorKind,
    message: String,
    error_uri: Option<String>,
}

impl OAuthError {
    pub fn new(
        kind: OAuthErrorKind,
        message: impl Into<String>,
        error_uri: Option<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            error_uri,
        }
    }

    pub fn kind(&self) -> OAuthErrorKind {
        self.kind
    }

    /// The `error` code of RFC 6749, such as `invalid_grant`.
    pub fn error_code(&self) -> &'static str {
        self.kind.error_code()
    }

    /// The `name` of the JavaScript error, such as `InvalidGrantError`.
    pub fn name(&self) -> &'static str {
        self.kind.name()
    }

    /// The `message` of the JavaScript error.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn error_uri(&self) -> Option<&str> {
        self.error_uri.as_deref()
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OAuthError {}

macro_rules! oauth_error_kinds {
    ($( $(#[$attribute:meta])* $kind:ident => $code:literal, $name:literal; )*) => {
        /// The classes of [`OAuthError`], one per error code the SDK knows.
        /// A code it does not know is a [`OAuthErrorKind::Server`] error.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum OAuthErrorKind {
            $( $(#[$attribute])* $kind, )*
        }

        impl OAuthErrorKind {
            pub fn error_code(self) -> &'static str {
                match self { $( Self::$kind => $code, )* }
            }

            /// The name of the JavaScript class.
            pub fn name(self) -> &'static str {
                match self { $( Self::$kind => $name, )* }
            }

            pub fn from_error_code(code: &str) -> Option<Self> {
                match code {
                    $( $code => Some(Self::$kind), )*
                    _ => None,
                }
            }
        }
    };
}

oauth_error_kinds! {
    /// The request is missing a required parameter, includes an invalid
    /// parameter value, includes a parameter more than once, or is otherwise
    /// malformed.
    InvalidRequest => "invalid_request", "InvalidRequestError";
    /// Client authentication failed.
    InvalidClient => "invalid_client", "InvalidClientError";
    /// The authorization grant or refresh token is invalid, expired, revoked,
    /// does not match the redirection URI used in the authorization request,
    /// or was issued to another client.
    InvalidGrant => "invalid_grant", "InvalidGrantError";
    /// The authenticated client is not authorized to use this grant type.
    UnauthorizedClient => "unauthorized_client", "UnauthorizedClientError";
    UnsupportedGrantType => "unsupported_grant_type", "UnsupportedGrantTypeError";
    InvalidScope => "invalid_scope", "InvalidScopeError";
    AccessDenied => "access_denied", "AccessDeniedError";
    /// The authorization server met an unexpected condition. Also what any
    /// response that is not an OAuth error document is reported as.
    Server => "server_error", "ServerError";
    TemporarilyUnavailable => "temporarily_unavailable", "TemporarilyUnavailableError";
    UnsupportedResponseType => "unsupported_response_type", "UnsupportedResponseTypeError";
    UnsupportedTokenType => "unsupported_token_type", "UnsupportedTokenTypeError";
    InvalidToken => "invalid_token", "InvalidTokenError";
    MethodNotAllowed => "method_not_allowed", "MethodNotAllowedError";
    TooManyRequests => "too_many_requests", "TooManyRequestsError";
    InvalidClientMetadata => "invalid_client_metadata", "InvalidClientMetadataError";
    InsufficientScope => "insufficient_scope", "InsufficientScopeError";
    /// The requested resource is invalid, missing, unknown, or malformed
    /// (RFC 8707).
    InvalidTarget => "invalid_target", "InvalidTargetError";
}

/// `UnauthorizedError` of `client/auth.js`: what the transports of the SDK
/// throw when a server keeps answering 401. None of the helpers here does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnauthorizedError {
    message: String,
}

impl UnauthorizedError {
    pub fn new(message: Option<String>) -> Self {
        Self {
            message: message.unwrap_or_else(|| "Unauthorized".to_owned()),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Default for UnauthorizedError {
    fn default() -> Self {
        Self::new(None)
    }
}

impl fmt::Display for UnauthorizedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UnauthorizedError {}
