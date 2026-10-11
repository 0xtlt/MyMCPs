//! The identity the gateway shows to remote MCP hosts that only let known
//! clients in.

use mymcps_mcp::Implementation;
use mymcps_net::parse_http_url;

/// OAuth dynamic-registration name used for hosts that do not allowlist a client.
pub const DEFAULT_OAUTH_CLIENT_NAME: &str = "MyMCPs";

/// MCP initialize identity used for hosts that do not allowlist a client.
pub const DEFAULT_MCP_CLIENT_NAME: &str = "mymcps-gateway";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamMcpClientInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub title: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowlistedUpstreamClient {
    /// Compared to the MCP URL hostname.
    pub hostname: &'static str,
    /// RFC 7591 `client_name`. These hosts allowlist the exact string.
    pub oauth_client_name: &'static str,
    /// MCP `initialize` `clientInfo`, matching that client's handshake.
    pub mcp_client_info: UpstreamMcpClientInfo,
    /// `User-Agent` on HTTP requests whose host is `hostname`.
    pub user_agent: &'static str,
    /// Fixed loopback redirect. Nothing listens on it, so the admin pastes the
    /// browser address back into MyMCPs. `None` to use the normal app callback.
    pub loopback_redirect_uri: Option<&'static str>,
}

/// Remote MCP hosts that reject a generic client.
///
/// OAuth registration is the check these providers document. Figma's register
/// endpoint accepts an exact `client_name` such as `Codex` and rejects others
/// (anthropics/claude-code#74768); the initialize HTTP 403 in that report was
/// the failed registration, not a separate `clientInfo` check. Strava Help
/// documents Claude Code as the HTTP client, and generic dynamic registration
/// is rejected at registration or token issuance rather than by User-Agent
/// (millerchou/strava-mcp-bridge). The runtime fields below still match the
/// clients those hosts allow, so a later header or handshake check sees the
/// same identity as OAuth.
///
/// Codex (openai/codex `rmcp-client`, workspace package version 0.0.0) sends
/// `clientInfo.name` `codex-mcp-client`, title `Codex`, and
/// `User-Agent: codex-mcp-client/0.0.0`. It only registers that name with a
/// localhost redirect, which is why Figma keeps the pasted loopback callback.
/// Claude Code sends `clientInfo.name` `claude-code`, title `Claude Code`, and
/// `User-Agent: claude-code/<version> (cli)` (observed as 2.1.89). Strava does
/// not document a fixed loopback redirect, so it uses the normal app callback.
const ALLOWLISTED_UPSTREAM_CLIENTS: [AllowlistedUpstreamClient; 2] = [
    AllowlistedUpstreamClient {
        hostname: "mcp.figma.com",
        oauth_client_name: "Codex",
        mcp_client_info: UpstreamMcpClientInfo {
            name: "codex-mcp-client",
            version: "0.0.0",
            title: Some("Codex"),
        },
        user_agent: "codex-mcp-client/0.0.0",
        loopback_redirect_uri: Some("http://localhost:45873/callback"),
    },
    AllowlistedUpstreamClient {
        hostname: "mcp.strava.com",
        oauth_client_name: "Claude Code",
        mcp_client_info: UpstreamMcpClientInfo {
            name: "claude-code",
            version: "2.1.89",
            title: Some("Claude Code"),
        },
        user_agent: "claude-code/2.1.89 (cli)",
        loopback_redirect_uri: None,
    },
];

pub fn allowlisted_upstream_client(
    http_url: Option<&str>,
) -> Option<&'static AllowlistedUpstreamClient> {
    let http_url = http_url.filter(|url| !url.is_empty())?;
    let url = parse_http_url(http_url, "MCP URL").ok()?;
    let hostname = url.host_str()?;
    ALLOWLISTED_UPSTREAM_CLIENTS
        .iter()
        .find(|client| client.hostname == hostname)
}

/// `client_name` for dynamic client registration.
pub fn registration_client_name(http_url: Option<&str>) -> &'static str {
    allowlisted_upstream_client(http_url)
        .map_or(DEFAULT_OAUTH_CLIENT_NAME, |client| client.oauth_client_name)
}

/// The `clientInfo` of the MCP handshake with this URL.
pub fn mcp_client_info_for_url(http_url: Option<&str>) -> Implementation {
    match allowlisted_upstream_client(http_url) {
        Some(client) => {
            let info = client.mcp_client_info;
            let implementation = Implementation::new(info.name, info.version);
            match info.title {
                Some(title) => implementation.with_title(title),
                None => implementation,
            }
        }
        None => Implementation::new(DEFAULT_MCP_CLIENT_NAME, mymcps_core::VERSION),
    }
}

/// Identity headers for an allowlisted URL. Empty for every other host.
pub fn upstream_identity_headers(http_url: Option<&str>) -> Vec<(&'static str, &'static str)> {
    allowlisted_upstream_client(http_url)
        .map(|client| ("User-Agent", client.user_agent))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_figma_and_strava_by_the_hostname_of_the_mcp_url() {
        let figma = allowlisted_upstream_client(Some("https://mcp.figma.com/mcp")).unwrap();
        assert_eq!(figma.oauth_client_name, "Codex");
        assert_eq!(
            figma.loopback_redirect_uri,
            Some("http://localhost:45873/callback")
        );
        // The hostname is compared as a URL parser reads it.
        assert!(allowlisted_upstream_client(Some("HTTPS://MCP.FIGMA.COM:8443/x?y=1")).is_some());
        assert!(allowlisted_upstream_client(Some("https://user:pw@mcp.strava.com/mcp")).is_some());

        for other in [
            "https://figma.com/mcp",
            "https://mcp.figma.com.attacker.example/mcp",
            "https://attacker.example/mcp.figma.com",
            "ftp://mcp.figma.com/mcp",
            "not a url",
            "",
        ] {
            assert!(
                allowlisted_upstream_client(Some(other)).is_none(),
                "{other}"
            );
        }
        assert!(allowlisted_upstream_client(None).is_none());
    }

    #[test]
    fn falls_back_to_the_identity_of_the_gateway() {
        assert_eq!(
            registration_client_name(Some("https://mcp.figma.com/mcp")),
            "Codex"
        );
        assert_eq!(
            registration_client_name(Some("https://mcp.strava.com/mcp")),
            "Claude Code"
        );
        assert_eq!(
            registration_client_name(Some("https://mcp.example/mcp")),
            "MyMCPs"
        );
        assert_eq!(registration_client_name(None), "MyMCPs");

        assert_eq!(
            mcp_client_info_for_url(Some("https://mcp.strava.com/mcp")),
            Implementation::new("claude-code", "2.1.89").with_title("Claude Code")
        );
        assert_eq!(
            mcp_client_info_for_url(Some("https://mcp.example/mcp")),
            Implementation::new("mymcps-gateway", mymcps_core::VERSION)
        );

        assert_eq!(
            upstream_identity_headers(Some("https://mcp.figma.com/mcp")),
            [("User-Agent", "codex-mcp-client/0.0.0")]
        );
        assert!(upstream_identity_headers(Some("https://mcp.example/mcp")).is_empty());
        assert!(upstream_identity_headers(None).is_empty());
    }
}
