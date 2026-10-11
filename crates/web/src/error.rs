//! Failures of a request, and how they are answered.

use axum::response::{IntoResponse, Response};
use http::header::ACCEPT;
use http::{HeaderMap, StatusCode};

/// What API clients are told about an unexpected error.
pub const SERVER_ERROR_MESSAGE: &str = "Internal server error";

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Not found")]
    NotFound,
    /// Nobody is signed in.
    #[error("Unauthorized access")]
    Unauthorized,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Core(#[from] mymcps_core::Error),
    /// Anything else that should not happen. The message is logged, never shown.
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Database(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            Self::Database(_) | Self::Core(_) | Self::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// Marks a response as an error the outer layer still has to render, as a
/// page for a browser and as JSON for anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorStatus(pub StatusCode);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            // The message of an unexpected error can expose internals, such
            // as the SQL of a failed query: it is logged and nothing more.
            tracing::error!(error = %self, "Request failed");
        }
        let mut response = status.into_response();
        response.extensions_mut().insert(ErrorStatus(status));
        response
    }
}

/// Whether the client is a browser showing a page: it asked for HTML, and is
/// not one of the page's own background requests.
pub fn wants_html(headers: &HeaderMap) -> bool {
    let accepts_html = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    accepts_html && !is_fetch(headers)
}

/// Whether the page's script sent the request, expecting a fragment back.
pub fn is_fetch(headers: &HeaderMap) -> bool {
    headers
        .get("x-requested-with")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("fetch"))
}
