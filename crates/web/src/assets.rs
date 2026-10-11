//! The stylesheet, script, fonts and icons, compiled into the binary.

use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use http::{HeaderMap, HeaderValue, StatusCode};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

/// Appended to asset URLs, so that a new version is fetched after an upgrade.
pub const ASSET_VERSION: &str = mymcps_core::VERSION;

/// The URL of an asset, such as `css/app.css`.
pub fn asset_url(path: &str) -> String {
    format!("/assets/{path}?v={ASSET_VERSION}")
}

/// The path of every embedded asset.
pub fn asset_names() -> impl Iterator<Item = String> {
    Assets::iter().map(|path| path.into_owned())
}

/// The content of an embedded text asset, such as an icon to inline.
pub fn asset_text(path: &str) -> Option<String> {
    Assets::get(path).and_then(|file| String::from_utf8(file.data.into_owned()).ok())
}

fn serve(path: &str, headers: &HeaderMap, versioned: bool) -> Response {
    // Dot files are never served.
    if path
        .split('/')
        .any(|segment| segment.starts_with('.') || segment.is_empty())
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let hash: String = file
        .metadata
        .sha256_hash()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let etag = format!("\"{hash}\"");
    if headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let cache = if versioned {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=0, must-revalidate"
    };
    let mut response = file.data.into_owned().into_response();
    let response_headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(file.metadata.mimetype()) {
        response_headers.insert(CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response_headers.insert(ETAG, value);
    }
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// `GET /assets/{*path}`
pub async fn asset(Path(path): Path<String>, uri: http::Uri, headers: HeaderMap) -> Response {
    let versioned = uri.query().is_some_and(|query| query.contains("v="));
    serve(&path, &headers, versioned)
}

/// `GET /favicon.png`
pub async fn favicon(headers: HeaderMap) -> Response {
    serve("brand/favicon.png", &headers, false)
}

/// `GET /robots.txt`: the instance is private, nothing is to be indexed.
pub async fn robots() -> Response {
    (
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        "User-agent: *\nDisallow: /\n",
    )
        .into_response()
}
