//! Route guards: who may reach a group of routes, and where the others go.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use mymcps_core::Core;
use mymcps_core::models::User;

use crate::auth::Auth;
use crate::error::{AppError, wants_html};
use crate::redirect::{redirect_to, with_query};
use crate::session::Session;

fn user_of(request: &Request) -> Option<&User> {
    request
        .extensions()
        .get::<Auth>()
        .and_then(|auth| auth.user.as_ref())
}

/// When the instance has no users yet, force every app route to /onboarding.
pub async fn needs_setup(State(core): State<Arc<Core>>, request: Request, next: Next) -> Response {
    match User::setup_complete(&*core.db).await {
        Ok(true) => next.run(request).await,
        Ok(false) => redirect_to(&with_query("/onboarding", request.uri().query())),
        Err(error) => AppError::from(error).into_response(),
    }
}

/// Onboarding is only available before the first admin exists. Afterwards,
/// send people to the app (or login).
pub async fn setup_complete(
    State(core): State<Arc<Core>>,
    request: Request,
    next: Next,
) -> Response {
    match User::setup_complete(&*core.db).await {
        Ok(false) => next.run(request).await,
        Ok(true) => {
            let target = if user_of(&request).is_some() {
                "/"
            } else {
                "/login"
            };
            redirect_to(&with_query(target, request.uri().query()))
        }
        Err(error) => AppError::from(error).into_response(),
    }
}

/// Deny access to unauthenticated users: a browser is sent to sign in, any
/// other client is told it is not authorized.
pub async fn require_auth(request: Request, next: Next) -> Response {
    if user_of(&request).is_some() {
        return next.run(request).await;
    }
    if wants_html(request.headers()) {
        // A message left for the page this visitor was sent to (an invite
        // that is no longer valid) is shown on the sign-in page instead.
        if let Some(session) = request.extensions().get::<Session>() {
            session.reflash();
        }
        return redirect_to("/login");
    }
    (StatusCode::UNAUTHORIZED, "Unauthorized access").into_response()
}

/// Routes that are for visitors who are not signed in, such as the login page.
pub async fn guest(request: Request, next: Next) -> Response {
    if user_of(&request).is_some() {
        if let Some(session) = request.extensions().get::<Session>() {
            session.reflash();
        }
        return redirect_to("/");
    }
    next.run(request).await
}

/// Ensures the authenticated user is an admin.
pub async fn require_admin(request: Request, next: Next) -> Response {
    if user_of(&request).is_some_and(User::is_admin) {
        return next.run(request).await;
    }
    if let Some(session) = request.extensions().get::<Session>() {
        session.flash("error", "Admin access required");
    }
    redirect_to(&with_query("/", request.uri().query()))
}
