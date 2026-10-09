//! Response headers every page gets, CORS for the protocol endpoints, and
//! the method override of HTML forms.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_HEADERS,
    ACCESS_CONTROL_REQUEST_METHOD, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ORIGIN,
    STRICT_TRANSPORT_SECURITY, VARY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use http::{HeaderValue, Method, StatusCode};
use mymcps_core::crypto::random_base64url;

/// The nonce of this response's inline scripts. In the request extensions,
/// for the page shell to put on its `<script>` tags.
#[derive(Debug, Clone)]
pub struct CspNonce(pub String);

/// `180 days`, as the Node app sent.
const HSTS_MAX_AGE_SECONDS: u64 = 180 * 24 * 60 * 60;

/// Paths installed MCP clients may call from any origin. They authenticate
/// with a header the client sets itself, never with cookies, so they answer
/// with a plain wildcard. The session UI stays locked down.
const CORS_PATHS: &[&str] = &[
    "/mcp",
    "/register",
    "/token",
    "/revoke",
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
];

pub fn allows_any_origin(path: &str) -> bool {
    CORS_PATHS.contains(&path)
}

/// Stricter than the Node app's on one point: styles come from the
/// stylesheet only. The React UI needed inline styles; these pages have none.
fn content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; \
         base-uri 'self'; form-action 'self'; frame-ancestors 'none'"
    )
}

/// Content security policy, clickjacking, HSTS and MIME sniffing headers.
/// A handler that sets one of them itself, such as a file download, keeps its own.
pub async fn security_headers_layer(mut request: Request, next: Next) -> Response {
    let nonce = random_base64url(16);
    request.extensions_mut().insert(CspNonce(nonce.clone()));

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let mut set_default = |name, value: String| {
        if !headers.contains_key(&name)
            && let Ok(value) = HeaderValue::from_str(&value)
        {
            headers.insert(name, value);
        }
    };
    set_default(CONTENT_SECURITY_POLICY, content_security_policy(&nonce));
    set_default(X_FRAME_OPTIONS, "DENY".into());
    set_default(
        STRICT_TRANSPORT_SECURITY,
        format!("max-age={HSTS_MAX_AGE_SECONDS}"),
    );
    set_default(X_CONTENT_TYPE_OPTIONS, "nosniff".into());
    // A page is written for one person at one moment: it carries their data
    // and their CSRF token. The browser must ask again rather than show a
    // copy it kept, above all after that person signed out.
    let is_page = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_page && !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// CORS, answered here for every route including preflights.
pub async fn cors_layer(development: bool, request: Request, next: Next) -> Response {
    let origin = request.headers().get(ORIGIN).cloned();
    let Some(origin) = origin else {
        return next.run(request).await;
    };

    let any_origin = allows_any_origin(request.uri().path());
    // In development the caller's origin is echoed, so the response depends on it.
    let allowed = if any_origin {
        Some(HeaderValue::from_static("*"))
    } else if development {
        Some(origin)
    } else {
        None
    };

    let is_preflight = request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(ACCESS_CONTROL_REQUEST_METHOD);
    if is_preflight {
        let requested_headers = request
            .headers()
            .get(ACCESS_CONTROL_REQUEST_HEADERS)
            .cloned();
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(allowed) = allowed {
            let headers = response.headers_mut();
            headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, allowed);
            headers.insert(
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET,HEAD,POST,PUT,DELETE,OPTIONS"),
            );
            // Request headers are reflected.
            if let Some(requested) = requested_headers {
                headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, requested);
            }
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("90"));
            if !any_origin {
                headers.append(VARY, HeaderValue::from_static("Origin"));
            }
        }
        return response;
    }

    let mut response = next.run(request).await;
    if let Some(allowed) = allowed {
        let headers = response.headers_mut();
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, allowed);
        headers.insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("WWW-Authenticate"),
        );
        if !any_origin {
            headers.append(VARY, HeaderValue::from_static("Origin"));
        }
    }
    response
}

/// HTML forms only send GET and POST. A form meant for a PUT, PATCH or
/// DELETE route posts to it with `?_method=` in its action, and is routed as
/// that method. Only ever applied to a POST, which is checked for its CSRF
/// token like the method it stands for.
pub fn override_method<B>(mut request: http::Request<B>) -> http::Request<B> {
    if request.method() != Method::POST {
        return request;
    }
    let overridden = request.uri().query().and_then(|query| {
        query
            .split('&')
            .find_map(|pair| match pair.split_once('=') {
                Some(("_method", value)) => match value.to_ascii_uppercase().as_str() {
                    "PUT" => Some(Method::PUT),
                    "PATCH" => Some(Method::PATCH),
                    "DELETE" => Some(Method::DELETE),
                    _ => None,
                },
                _ => None,
            })
    });
    if let Some(method) = overridden {
        *request.method_mut() = method;
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: Method, uri: &str) -> http::Request<()> {
        http::Request::builder()
            .method(method)
            .uri(uri)
            .body(())
            .unwrap()
    }

    #[test]
    fn overrides_the_method_of_posted_forms_only() {
        assert_eq!(
            override_method(request(Method::POST, "/mcps/3?_method=PUT")).method(),
            Method::PUT
        );
        assert_eq!(
            override_method(request(Method::POST, "/tokens?a=1&_method=delete")).method(),
            Method::DELETE
        );
        assert_eq!(
            override_method(request(Method::POST, "/settings/email?_method=PATCH")).method(),
            Method::PATCH
        );
        assert_eq!(
            override_method(request(Method::POST, "/tokens?_method=GET")).method(),
            Method::POST
        );
        assert_eq!(
            override_method(request(Method::POST, "/tokens?_method=TRACE")).method(),
            Method::POST
        );
        assert_eq!(
            override_method(request(Method::GET, "/tokens?_method=DELETE")).method(),
            Method::GET
        );
        assert_eq!(
            override_method(request(Method::POST, "/tokens")).method(),
            Method::POST
        );
    }

    #[test]
    fn allows_any_origin_only_on_exact_protocol_paths() {
        for path in [
            "/mcp",
            "/token",
            "/register",
            "/revoke",
            "/.well-known/oauth-protected-resource/mcp",
        ] {
            assert!(allows_any_origin(path), "{path}");
        }
        for path in ["/mcps", "/mcps/oauth/callback", "/mcp/", "/authorize", "/"] {
            assert!(!allows_any_origin(path), "{path}");
        }
    }

    #[test]
    fn the_policy_names_the_nonce() {
        let policy = content_security_policy("abc");
        assert!(policy.contains("script-src 'self' 'nonce-abc';"));
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(policy.contains("form-action 'self'"));
        assert!(policy.contains("style-src 'self';"));
        assert!(!policy.contains("unsafe-inline"));
    }
}
