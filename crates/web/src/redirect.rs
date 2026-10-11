//! Redirects, which the pages answer most form posts with.

use axum::response::{IntoResponse, Response};
use http::header::{HOST, LOCATION, REFERER};
use http::{HeaderMap, HeaderValue, StatusCode};
use url::Url;

/// `302 Found` to a path of this site, as the Node app answered.
pub fn redirect_to(path: &str) -> Response {
    let location = HeaderValue::from_str(path).unwrap_or(HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(LOCATION, location)]).into_response()
}

/// Back to the page the request came from, when the browser named one on
/// this host. Otherwise to `fallback`.
pub fn redirect_back(headers: &HeaderMap, fallback: &str) -> Response {
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    let referer = headers
        .get(REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| {
            let authority = match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap_or("")),
                None => url.host_str().unwrap_or("").to_string(),
            };
            host.is_some_and(|host| host.eq_ignore_ascii_case(&authority))
        });
    match referer {
        Some(url) => {
            let mut path = url.path().to_string();
            if let Some(query) = url.query() {
                path.push('?');
                path.push_str(query);
            }
            redirect_to(&path)
        }
        None => redirect_to(fallback),
    }
}

/// `path`, followed by the query string of the request being answered. The
/// Node app forwarded it on its redirects (`forwardQueryString: true`). The
/// method override of an HTML form (`_method`) is how that request was sent,
/// not something the next page asked for: it is left out.
pub fn with_query(path: &str, query: Option<&str>) -> String {
    let kept: Vec<&str> = query
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some("_method"))
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(response: Response) -> String {
        response.headers()[LOCATION].to_str().unwrap().to_string()
    }

    #[test]
    fn goes_back_only_within_the_site() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("mcp.example.com"));
        assert_eq!(location(redirect_back(&headers, "/mcps")), "/mcps");

        headers.insert(
            REFERER,
            HeaderValue::from_static("https://mcp.example.com/tokens?page=2"),
        );
        assert_eq!(location(redirect_back(&headers, "/")), "/tokens?page=2");

        headers.insert(
            REFERER,
            HeaderValue::from_static("https://evil.example/phish"),
        );
        assert_eq!(location(redirect_back(&headers, "/")), "/");

        headers.insert(HOST, HeaderValue::from_static("localhost:3333"));
        headers.insert(
            REFERER,
            HeaderValue::from_static("http://localhost:3333/settings"),
        );
        assert_eq!(location(redirect_back(&headers, "/")), "/settings");
        assert_eq!(with_query("/login", Some("a=1")), "/login?a=1");
        assert_eq!(with_query("/login", Some("")), "/login");
        assert_eq!(with_query("/", Some("_method=PATCH")), "/");
        assert_eq!(
            with_query("/invites", Some("_method=DELETE&page=2")),
            "/invites?page=2"
        );
    }
}
