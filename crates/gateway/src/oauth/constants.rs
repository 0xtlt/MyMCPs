//! Fixed values of the gateway's OAuth server that its service and its
//! validators both need.

use url::Url;

/// The scope every grant of the gateway has: calling the tools of the MCPs
/// of one team.
pub const GATEWAY_OAUTH_SCOPE: &str = "mcp:tools";

/// The scope a person adds on the consent screen, unchecked by default: the
/// client may also add, change, delete and test the MCPs of the team, and
/// read the links that connect them. Only an OAuth grant carries it.
pub const MCPS_MANAGE_SCOPE: &str = "mcps:manage";

/// Every scope the gateway knows, in the order a grant lists them.
pub const SUPPORTED_OAUTH_SCOPES: [&str; 2] = [GATEWAY_OAUTH_SCOPE, MCPS_MANAGE_SCOPE];

/// The scopes of a space-delimited scope string (RFC 6749, section 3.3).
pub fn scope_names(scopes: &str) -> impl Iterator<Item = &str> {
    scopes.split(' ').filter(|scope| !scope.is_empty())
}

/// Whether a space-delimited scope string holds `scope`.
pub fn has_scope(scopes: &str, scope: &str) -> bool {
    scope_names(scopes).any(|name| name == scope)
}

/// Hosts that name the user's own device, as a parsed URL spells them.
pub const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

pub(crate) fn has_loopback_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| LOOPBACK_HOSTS.contains(&host))
}
