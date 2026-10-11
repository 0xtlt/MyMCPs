//! Fixed values of the gateway's OAuth server that its service and its
//! validators both need.

use url::Url;

/// The one scope the gateway grants.
pub const GATEWAY_OAUTH_SCOPE: &str = "mcp:tools";

/// Hosts that name the user's own device, as a parsed URL spells them.
pub const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

pub(crate) fn has_loopback_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| LOOPBACK_HOSTS.contains(&host))
}
