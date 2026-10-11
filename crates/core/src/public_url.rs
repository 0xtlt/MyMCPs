//! The public address of the instance, from `APP_URL`.

use url::Url;

use crate::config::Config;

const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "[::1]"];

/// Why `APP_URL` cannot be used for links and OAuth endpoints. The messages
/// are shown to administrators.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicUrlError {
    #[error("APP_URL is not configured. Set it to the public HTTPS origin and redeploy.")]
    NotConfigured,
    #[error(
        "APP_URL must be a public HTTPS origin (HTTP loopback is allowed only in development and tests)."
    )]
    NotAnOrigin,
}

/// Normalize the configured public base URL for links shown in the UI.
pub fn normalize_public_app_url(value: Option<&str>) -> Option<String> {
    let value = value?;
    let trimmed = value.strip_suffix('/').unwrap_or(value);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn validate_public_app_url(
    value: &str,
    allow_insecure_loopback: bool,
) -> Result<String, PublicUrlError> {
    let url = Url::parse(value).map_err(|_| PublicUrlError::NotAnOrigin)?;
    let is_origin_only = url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    let is_secure = url.scheme() == "https";
    let is_allowed_loopback = allow_insecure_loopback
        && url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| LOOPBACK_HOSTS.contains(&host));

    if !is_origin_only || (!is_secure && !is_allowed_loopback) {
        return Err(PublicUrlError::NotAnOrigin);
    }
    Ok(url.origin().ascii_serialization())
}

impl Config {
    /// `APP_URL` without its trailing slash, unchecked. For links shown in the UI.
    pub fn public_app_url(&self) -> Option<String> {
        normalize_public_app_url(self.app_url.as_deref())
    }

    /// The public origin, or why it cannot be used.
    pub fn require_public_app_url(&self) -> Result<String, PublicUrlError> {
        let configured = self.public_app_url().ok_or(PublicUrlError::NotConfigured)?;
        validate_public_app_url(&configured, self.is_development() || self.is_test())
    }

    /// The public origin only when it is safe to use for OAuth endpoints.
    pub fn public_oauth_app_url(&self) -> Option<String> {
        self.require_public_app_url().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_configured_url() {
        assert_eq!(
            normalize_public_app_url(Some("https://mcp.example.com/")).as_deref(),
            Some("https://mcp.example.com")
        );
        assert_eq!(
            normalize_public_app_url(Some("https://mcp.example.com")).as_deref(),
            Some("https://mcp.example.com")
        );
        assert_eq!(normalize_public_app_url(Some("")), None);
        assert_eq!(normalize_public_app_url(None), None);
    }

    #[test]
    fn accepts_https_origins_and_loopback_http_when_allowed() {
        assert_eq!(
            validate_public_app_url("https://mcp.example.com", false).unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            validate_public_app_url("https://mcp.example.com:8443/", false).unwrap(),
            "https://mcp.example.com:8443"
        );
        assert_eq!(
            validate_public_app_url("http://localhost:3333", true).unwrap(),
            "http://localhost:3333"
        );
        assert_eq!(
            validate_public_app_url("http://127.0.0.1:3333", true).unwrap(),
            "http://127.0.0.1:3333"
        );
        assert_eq!(
            validate_public_app_url("http://[::1]:3333", true).unwrap(),
            "http://[::1]:3333"
        );

        for refused in [
            "http://localhost:3333",
            "http://mcp.example.com",
            "https://mcp.example.com/app",
            "https://mcp.example.com?x=1",
            "https://mcp.example.com#top",
            "https://user:pass@mcp.example.com",
            "ftp://mcp.example.com",
            "not a url",
        ] {
            let allow_loopback = !refused.starts_with("http://localhost");
            assert_eq!(
                validate_public_app_url(refused, allow_loopback),
                Err(PublicUrlError::NotAnOrigin),
                "{refused}"
            );
        }
    }

    #[test]
    fn production_requires_https() {
        let mut config = Config::for_tests("tmp");
        assert_eq!(
            config.public_oauth_app_url().as_deref(),
            Some("http://localhost:3333")
        );

        config.environment = crate::Environment::Production;
        assert_eq!(
            config.require_public_app_url(),
            Err(PublicUrlError::NotAnOrigin)
        );
        assert_eq!(
            config.public_app_url().as_deref(),
            Some("http://localhost:3333")
        );

        config.app_url = None;
        assert_eq!(
            config.require_public_app_url(),
            Err(PublicUrlError::NotConfigured)
        );
        assert_eq!(
            PublicUrlError::NotConfigured.to_string(),
            "APP_URL is not configured. Set it to the public HTTPS origin and redeploy."
        );
    }
}
