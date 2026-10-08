//! How handlers answer: a full page for a browser navigation, a fragment or
//! an instruction to navigate for the page's script.

use axum::response::{Html, IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode};
use maud::Markup;

use crate::error::is_fetch;
use crate::redirect::{redirect_back, redirect_to};
use crate::session::Session;

/// A full page.
pub fn page(markup: Markup) -> Response {
    Html(markup.into_string()).into_response()
}

/// A full page with a status, such as 422 for a form sent back with its errors.
pub fn page_with_status(status: StatusCode, markup: Markup) -> Response {
    (status, Html(markup.into_string())).into_response()
}

/// A fragment that replaces the closest `[data-fragment]` of the form the
/// script sent. 422 for validation errors.
pub fn fragment(status: StatusCode, markup: Markup) -> Response {
    (status, Html(markup.into_string())).into_response()
}

/// Send the browser to a path of this site: a redirect for a plain form
/// post, and the `X-Location` header for the page's script, which then
/// navigates itself (a redirect it followed would use up the flash message).
pub fn navigate(headers: &HeaderMap, path: &str) -> Response {
    if !is_fetch(headers) {
        return redirect_to(path);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(path) {
        response.headers_mut().insert("x-location", value);
    }
    response
}

/// A form that failed validation. The script gets the form again, with its
/// errors, to put in place. A plain post gets the first error as a flash
/// message on the page it came from: field errors need the script.
pub fn invalid_form(
    headers: &HeaderMap,
    session: &Session,
    form: Markup,
    first_error: &str,
    fallback: &str,
) -> Response {
    if is_fetch(headers) {
        return fragment(StatusCode::UNPROCESSABLE_ENTITY, form);
    }
    session.flash("error", first_error);
    redirect_back(headers, fallback)
}
