//! The routes and the layers every request goes through.

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::IntoResponse;
use axum::routing::get;
use http::header::CONTENT_TYPE;
use mymcps_core::Core;
use tower::ServiceExt;

use crate::auth::silent_auth_layer;
use crate::cookies::cookies_layer;
use crate::input::{body_layer, csrf_layer};
use crate::security::{cors_layer, override_method, security_headers_layer};
use crate::session::session_layer;
use crate::state::AppState;
use crate::{assets, errors, guards, routes};

/// Public liveness endpoint for reverse proxies and container health checks.
/// It intentionally does not depend on onboarding, auth, or application data.
async fn health() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        "{\"status\":\"ok\"}",
    )
}

/// Layers of the routes a person uses from a browser: a session, a parsed
/// and CSRF-checked body, and who is signed in. Outermost last.
fn browser_layers(router: Router<AppState>, core: &Arc<Core>) -> Router<AppState> {
    router
        .layer(from_fn_with_state(core.clone(), silent_auth_layer))
        .layer(from_fn(csrf_layer))
        .layer(from_fn(body_layer))
        .layer(from_fn_with_state(core.clone(), session_layer))
}

/// Everything the server answers.
pub fn router(state: AppState) -> axum::routing::RouterIntoService<axum::body::Body> {
    let core = state.core.clone();
    let secure_cookies = core.config.is_production();
    let development = core.config.is_development();
    let features = routes::all();

    // App routes require setup to be finished. Unauthenticated visitors
    // never see a public marketing home.
    let pages = Router::new()
        .merge(features.open)
        .merge(features.guest.layer(from_fn(guards::guest)))
        .merge(features.signed_in.layer(from_fn(guards::require_auth)))
        // The admin check runs after the sign-in check: layers added last run first.
        .merge(
            features
                .admin
                .layer(from_fn(guards::require_admin))
                .layer(from_fn(guards::require_auth)),
        )
        .layer(from_fn_with_state(core.clone(), guards::needs_setup));
    let first_run = features
        .first_run
        .layer(from_fn_with_state(core.clone(), guards::setup_complete));
    let browser = browser_layers(pages.merge(first_run), &core);

    let protocol = features
        .protocol
        .layer(from_fn_with_state(core.clone(), guards::needs_setup))
        .layer(from_fn(body_layer));

    Router::new()
        .route("/health", get(health))
        .route("/robots.txt", get(assets::robots))
        .route("/favicon.png", get(assets::favicon))
        .route("/assets/{*path}", get(assets::asset))
        .merge(features.machine)
        .merge(protocol)
        .merge(browser)
        .fallback(errors::not_found)
        // A path that exists under another method is not found either, as
        // in the Node app, whose router matched the method with the path.
        .method_not_allowed_fallback(errors::not_found)
        .layer(from_fn(errors::error_pages_layer))
        .layer(from_fn(move |request, next| {
            cookies_layer(secure_cookies, request, next)
        }))
        .layer(from_fn(security_headers_layer))
        .layer(from_fn(move |request, next| {
            cors_layer(development, request, next)
        }))
        .with_state(state)
        .into_service()
}

/// The router as a service that first applies the method override of HTML
/// forms, which has to happen before a route is chosen.
pub fn service(
    state: AppState,
) -> impl tower::Service<
    Request,
    Response = axum::response::Response,
    Error = std::convert::Infallible,
    Future: Send,
> + Clone
+ Send
+ 'static {
    router(state)
        .map_request(override_method)
        .map_response(not_found_names_no_method)
}

/// The router lists the methods a path has routes for on the answer to any
/// other method. Here that answer is "not found", which says nothing about
/// the path: the list is left out.
fn not_found_names_no_method(mut response: axum::response::Response) -> axum::response::Response {
    if response.status() == http::StatusCode::NOT_FOUND {
        response.headers_mut().remove(http::header::ALLOW);
    }
    response
}
