//! Utilities for handling OAuth resource URIs (`shared/auth-utils.js`).

use url::Url;

/// Converts a server URL to a resource URL by removing the fragment.
/// RFC 8707 section 2 states that resource URIs "MUST NOT include a fragment
/// component". Keeps everything else unchanged (scheme, domain, port, path,
/// query).
pub fn resource_url_from_server_url(url: &Url) -> Url {
    let mut resource_url = url.clone();
    resource_url.set_fragment(None);
    resource_url
}

/// Checks if a requested resource URL matches a configured resource URL.
/// A requested resource matches if it has the same scheme, domain, port,
/// and its path starts with the configured resource's path.
pub fn check_resource_allowed(requested_resource: &Url, configured_resource: &Url) -> bool {
    // Compare the origin (scheme, domain, and port). Two URLs without one,
    // such as two `urn:` names, compare as equal there, as they do in the SDK.
    if requested_resource.origin().ascii_serialization()
        != configured_resource.origin().ascii_serialization()
    {
        return false;
    }

    let requested = requested_resource.path();
    let configured = configured_resource.path();
    // Handle cases like requested=/foo and configured=/foo/
    if requested.len() < configured.len() {
        return false;
    }

    // Check if the requested path starts with the configured path.
    // Ensure both paths end with / for proper comparison: this ensures that
    // if we have paths like "/api" and "/api/users", we properly detect that
    // "/api/users" is a subpath of "/api". By adding a trailing slash if
    // missing, we avoid false positives where paths like "/api123" would
    // incorrectly match "/api".
    let with_slash = |path: &str| {
        if path.ends_with('/') {
            path.to_owned()
        } else {
            format!("{path}/")
        }
    };
    with_slash(requested).starts_with(&with_slash(configured))
}
