use mymcps_core::public_url::PublicUrlError;
use mymcps_core::redaction::sanitize_diagnostic;
use serde_json::{Value, json};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An answer the OAuth protocol has a name for: what the gateway tells a
/// client whose request it refuses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GatewayOauthError {
    /// The `error` of the answer, such as `invalid_grant`.
    pub code: &'static str,
    /// The `error_description` of the answer.
    pub message: String,
    pub status: u16,
    /// Where an authorization request asked to be answered, once that is
    /// known to be a redirect URI its client registered.
    pub redirect_uri: Option<String>,
    /// The `state` to send back with the error.
    pub state: Option<String>,
}

impl GatewayOauthError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(code, message, 400)
    }

    pub fn with_status(code: &'static str, message: impl Into<String>, status: u16) -> Self {
        Self {
            code,
            message: message.into(),
            status,
            redirect_uri: None,
            state: None,
        }
    }

    /// The JSON body of the answer.
    pub fn body(&self) -> Value {
        json!({ "error": self.code, "error_description": self.message })
    }

    /// The `WWW-Authenticate` header of the answer, which a client that
    /// failed to authenticate is owed.
    pub fn www_authenticate(&self) -> Option<&'static str> {
        (self.code == "invalid_client").then_some("Basic realm=\"MyMCPs OAuth\"")
    }
}

/// Why a request to the OAuth server did not go through.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request was refused: the error is the answer.
    #[error(transparent)]
    Oauth(#[from] GatewayOauthError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// `APP_URL` cannot serve as the address of the OAuth server.
    #[error(transparent)]
    PublicUrl(#[from] PublicUrlError),
    /// A redirect URI that was stored is not a URL.
    #[error("invalid redirect URI: {0}")]
    RedirectUri(#[from] url::ParseError),
}

impl Error {
    /// What to answer the client with. A failure of the server says nothing
    /// of its cause: see [`Error::diagnostic`] for what to log.
    pub fn to_oauth(&self) -> GatewayOauthError {
        match self {
            Self::Oauth(error) => error.clone(),
            Self::PublicUrl(_) => GatewayOauthError::with_status(
                "temporarily_unavailable",
                "OAuth requires APP_URL to be a public HTTPS origin",
                503,
            ),
            Self::Database(_) | Self::RedirectUri(_) => GatewayOauthError::with_status(
                "server_error",
                "The OAuth request could not be completed",
                500,
            ),
        }
    }

    /// The HTTP status of the answer.
    pub fn status(&self) -> u16 {
        self.to_oauth().status
    }

    /// The JSON body of the answer.
    pub fn body(&self) -> Value {
        self.to_oauth().body()
    }

    /// Whether the server failed, rather than refused the request: an error
    /// to log.
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Database(_) | Self::RedirectUri(_))
    }

    /// The error in a form that is safe to log.
    pub fn diagnostic(&self) -> String {
        sanitize_diagnostic(&self.to_string())
    }
}
