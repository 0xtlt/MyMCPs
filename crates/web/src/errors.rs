//! The pages and the JSON a failed request is answered with.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use http::header::CONTENT_TYPE;
use http::{Method, StatusCode};
use maud::Markup;

use crate::error::{AppError, ErrorStatus, SERVER_ERROR_MESSAGE, wants_html};
use crate::security::CspNonce;
use crate::views::shell::{PageContext, auth_page, error_card};

/// No route has this address.
pub async fn not_found() -> AppError {
    AppError::NotFound
}

fn error_page(nonce: String, status: StatusCode) -> Markup {
    // Drawn without the session: an error can happen before it is known,
    // and the page must not depend on what failed.
    let context = PageContext {
        user: None,
        path: String::new(),
        csrf: String::new(),
        nonce,
        flash_success: None,
        flash_error: None,
        pending_approvals: 0,
        app_url: None,
        app_url_configured: true,
        sidebar_collapsed: false,
        is_fetch: false,
    };
    if status == StatusCode::NOT_FOUND {
        let card = error_card(
            "search",
            false,
            "Page not found",
            "This route does not exist. Head back home to continue.",
        );
        auth_page(&context, "Page not found", card)
    } else {
        let card = error_card(
            "circle-x",
            true,
            "Something went wrong",
            "An unexpected error occurred. Try again or return home.",
        );
        auth_page(&context, "Something went wrong", card)
    }
}

/// Turns the errors handlers return into what the client can read: a page
/// for a browser, JSON for anything else. An unexpected error never says
/// more than that it happened.
pub async fn error_pages_layer(request: Request, next: Next) -> Response {
    let browser = wants_html(request.headers());
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let nonce = request
        .extensions()
        .get::<CspNonce>()
        .map(|nonce| nonce.0.clone())
        .unwrap_or_default();

    let response = next.run(request).await;
    let Some(ErrorStatus(status)) = response.extensions().get::<ErrorStatus>().copied() else {
        return response;
    };
    if method == Method::HEAD {
        return status.into_response();
    }
    if browser && (status == StatusCode::NOT_FOUND || status.is_server_error()) {
        return (status, Html(error_page(nonce, status).into_string())).into_response();
    }

    let message = if status.is_server_error() {
        SERVER_ERROR_MESSAGE.to_string()
    } else if status == StatusCode::NOT_FOUND {
        format!("Cannot {method}:{path}")
    } else {
        status.canonical_reason().unwrap_or("Error").to_string()
    };
    let body = serde_json::json!({ "message": message }).to_string();
    (
        status,
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response()
}
