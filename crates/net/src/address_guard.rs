use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use async_trait::async_trait;
use tokio::sync::OnceCell;
use url::Url;

/// Networks a URL taken from a remote document must not lead into: unspecified,
/// private, CGNAT, loopback, link-local (cloud metadata lives at
/// 169.254.169.254), and the special-purpose blocks no public OAuth provider is
/// hosted on. Documentation ranges are left out: nothing listens there.
const RESTRICTED_IPV4_SUBNETS: [(Ipv4Addr, u8); 11] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// Unspecified, loopback, local-use NAT64, unique-local, link-local, site-local, multicast.
const RESTRICTED_IPV6_SUBNETS: [(Ipv6Addr, u8); 7] = [
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 128),
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), 128),
    (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

/// Subnets as `(network, mask)` pairs.
#[derive(Default)]
struct BlockList {
    ipv4: Vec<(u32, u32)>,
    ipv6: Vec<(u128, u128)>,
}

impl BlockList {
    fn add_ipv4_subnet(&mut self, network: Ipv4Addr, prefix: u8) {
        let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
        self.ipv4.push((u32::from(network) & mask, mask));
    }

    fn add_ipv6_subnet(&mut self, network: Ipv6Addr, prefix: u8) {
        let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
        self.ipv6.push((u128::from(network) & mask, mask));
    }

    fn contains_ipv4(&self, address: Ipv4Addr) -> bool {
        let address = u32::from(address);
        self.ipv4
            .iter()
            .any(|(network, mask)| address & mask == *network)
    }

    fn contains_ipv6(&self, address: Ipv6Addr) -> bool {
        let address = u128::from(address);
        self.ipv6
            .iter()
            .any(|(network, mask)| address & mask == *network)
    }

    /// An IPv4-mapped address is matched against the IPv4 rules as well.
    fn check(&self, address: IpAddr) -> bool {
        match address {
            IpAddr::V4(address) => self.contains_ipv4(address),
            IpAddr::V6(address) => {
                address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| self.contains_ipv4(mapped))
                    || self.contains_ipv6(address)
            }
        }
    }
}

static RESTRICTED: LazyLock<BlockList> = LazyLock::new(|| {
    let mut restricted = BlockList::default();
    for (network, prefix) in RESTRICTED_IPV4_SUBNETS {
        restricted.add_ipv4_subnet(network, prefix);
        // IPv6 forms that carry an IPv4 destination: IPv4-compatible, NAT64 and
        // 6to4, which a translator or tunnel delivers to the IPv4 address.
        // `check` itself matches IPv4-mapped addresses against the IPv4 rules.
        let ipv4 = u128::from(u32::from(network));
        restricted.add_ipv6_subnet(Ipv6Addr::from(ipv4), 96 + prefix);
        restricted.add_ipv6_subnet(Ipv6Addr::from((0x0064_ff9b << 96) | ipv4), 96 + prefix);
        restricted.add_ipv6_subnet(Ipv6Addr::from((0x2002 << 112) | (ipv4 << 80)), 16 + prefix);
    }
    for (network, prefix) in RESTRICTED_IPV6_SUBNETS {
        restricted.add_ipv6_subnet(network, prefix);
    }
    restricted
});

/// Whether an IP address lies in a network the gateway must not be sent into.
pub fn is_restricted_ip(address: IpAddr) -> bool {
    RESTRICTED.check(address)
}

/// Whether an IP address lies in a network the gateway must not be sent into.
/// Anything that is not an IP address counts as restricted.
pub fn is_restricted_address(address: &str) -> bool {
    // getaddrinfo reports link-local addresses with their zone, `fe80::1%en0`.
    let unscoped = address.split('%').next().unwrap_or(address);
    match unscoped.parse::<IpAddr>() {
        Ok(address) => is_restricted_ip(address),
        Err(_) => true,
    }
}

/// Whether a host is written as an IP address, an IPv6 one possibly with its zone.
fn is_ip_literal(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    value.split_once('%').is_some_and(|(address, zone)| {
        address.parse::<Ipv6Addr>().is_ok()
            && !zone.is_empty()
            && zone
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':'))
    })
}

/// Name resolution used by the guard. Replaceable in tests.
#[async_trait]
pub trait Resolver: Send + Sync {
    /// Every address a hostname resolves to, or an error when it does not resolve.
    async fn lookup(&self, hostname: &str) -> io::Result<Vec<IpAddr>>;
}

/// Goes through getaddrinfo like the HTTP client, so the hosts file and
/// `localhost` names count.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn lookup(&self, hostname: &str) -> io::Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((hostname, 0)).await?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

/// Name resolution without the network, for tests: only the names it was given
/// resolve, and every lookup is recorded.
#[derive(Debug, Default)]
pub struct StaticResolver {
    names: HashMap<String, Vec<IpAddr>>,
    lookups: Mutex<Vec<String>>,
}

impl StaticResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `hostname` resolve to `addresses`.
    pub fn with(mut self, hostname: &str, addresses: &[IpAddr]) -> Self {
        self.names.insert(hostname.to_owned(), addresses.to_vec());
        self
    }

    /// The hostnames looked up so far, in order.
    pub fn lookups(&self) -> Vec<String> {
        self.lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Resolver for StaticResolver {
    async fn lookup(&self, hostname: &str) -> io::Result<Vec<IpAddr>> {
        self.lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(hostname.to_owned());
        self.names.get(hostname).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("getaddrinfo ENOTFOUND {hostname}"),
            )
        })
    }
}

/// The address check, with the name resolution it relies on.
#[derive(Clone)]
pub struct AddressGuard {
    resolver: Arc<dyn Resolver>,
}

impl AddressGuard {
    pub fn new(resolver: Arc<dyn Resolver>) -> Self {
        Self { resolver }
    }

    /// The guard used outside tests, which resolves names like the HTTP client does.
    pub fn system() -> Self {
        Self::new(Arc::new(SystemResolver))
    }

    /// Whether the host of a parsed URL is, or resolves to, a restricted address.
    /// `Url` has already turned every IPv4 notation (decimal, octal, hex, short
    /// forms) into dotted decimal and compressed IPv6 literals inside brackets.
    /// `hostname` is what `Url::host_str` returns.
    pub async fn resolves_to_restricted_address(&self, hostname: &str) -> bool {
        let literal = match hostname.strip_prefix('[') {
            Some(rest) => {
                let mut characters = rest.chars();
                characters.next_back();
                characters.as_str()
            }
            None => hostname,
        };
        if is_ip_literal(literal) {
            return is_restricted_address(literal);
        }

        match self.resolver.lookup(hostname).await {
            Ok(addresses) => addresses.into_iter().any(is_restricted_ip),
            // A name that does not resolve cannot be connected to either.
            Err(_) => false,
        }
    }

    /// Check for URLs that come from documents the MCP or its OAuth provider serves
    /// (authorization server, token, registration and authorization endpoints),
    /// which would otherwise let a remote MCP aim gateway requests at the
    /// instance's own network.
    ///
    /// Operators may point an MCP at a private address. When the MCP's own host is
    /// one, its provider may be too, and nothing is refused.
    ///
    /// The check resolves the name before the request is made and the HTTP client
    /// resolves it again to connect, so a DNS answer that changes in between (DNS
    /// rebinding) is not caught. Closing that window needs the check at connect
    /// time, in the name resolution of a dedicated HTTP client.
    pub fn discovered_endpoint_guard(&self, mcp_url: &Url) -> DiscoveredEndpointGuard {
        DiscoveredEndpointGuard {
            state: Arc::new(GuardState {
                addresses: self.clone(),
                mcp_hostname: mcp_url.host_str().unwrap_or_default().to_owned(),
                mcp_is_restricted: OnceCell::new(),
                verdicts: Mutex::new(HashMap::new()),
            }),
        }
    }
}

impl Default for AddressGuard {
    fn default() -> Self {
        Self::system()
    }
}

impl fmt::Debug for AddressGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AddressGuard")
            .finish_non_exhaustive()
    }
}

/// A discovered endpoint leads into a network the MCP itself is not part of.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{label} host \"{hostname}\" is a loopback, private or link-local address, which a remote MCP may not send MyMCPs to"
)]
pub struct RestrictedEndpointError {
    pub label: String,
    pub hostname: String,
}

struct GuardState {
    addresses: AddressGuard,
    mcp_hostname: String,
    mcp_is_restricted: OnceCell<bool>,
    verdicts: Mutex<HashMap<String, Arc<OnceCell<bool>>>>,
}

/// The check of one MCP's discovered endpoints, see
/// [`AddressGuard::discovered_endpoint_guard`]. Each name is resolved once for
/// the life of the guard, and clones share what they have resolved.
#[derive(Clone)]
pub struct DiscoveredEndpointGuard {
    state: Arc<GuardState>,
}

impl DiscoveredEndpointGuard {
    /// Refuse `target` when it leads into a restricted network the MCP is not in.
    /// `label` names the endpoint in the error.
    pub async fn assert_allowed(
        &self,
        target: &Url,
        label: &str,
    ) -> Result<(), RestrictedEndpointError> {
        let state = &*self.state;
        let hostname = target.host_str().unwrap_or_default();
        if hostname == state.mcp_hostname {
            return Ok(());
        }

        let mcp_is_restricted = state
            .mcp_is_restricted
            .get_or_init(|| {
                state
                    .addresses
                    .resolves_to_restricted_address(&state.mcp_hostname)
            })
            .await;
        if *mcp_is_restricted {
            return Ok(());
        }

        let verdict = {
            let mut verdicts = state
                .verdicts
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Arc::clone(verdicts.entry(hostname.to_owned()).or_default())
        };
        let restricted = verdict
            .get_or_init(|| state.addresses.resolves_to_restricted_address(hostname))
            .await;
        if *restricted {
            return Err(RestrictedEndpointError {
                label: label.to_owned(),
                hostname: hostname.to_owned(),
            });
        }
        Ok(())
    }
}

impl fmt::Debug for DiscoveredEndpointGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiscoveredEndpointGuard")
            .field("mcp_hostname", &self.state.mcp_hostname)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_an_ip_literal_from_a_name() {
        for literal in [
            "127.0.0.1",
            "::1",
            "::ffff:1.2.3.4",
            "fe80::1%en0",
            "::%1",
            "::1%-.:",
        ] {
            assert!(is_ip_literal(literal), "{literal}");
        }
        for name in [
            "",
            "localhost",
            "127.1",
            "0x7f.1",
            "1.2.3.04",
            "1.2.3.4.5",
            "256.1.1.1",
            "[::1]",
            "fe80::1%",
            "fe80::1%a%b",
            "fe80::1%a b",
            "1.2.3.4%1",
            " 127.0.0.1",
        ] {
            assert!(!is_ip_literal(name), "{name}");
        }
    }

    #[test]
    fn ignores_the_zone_and_refuses_what_is_not_an_address() {
        assert!(is_restricted_address("fe80::1%"));
        assert!(is_restricted_address("%"));
        assert!(is_restricted_address(""));
        assert!(is_restricted_address(" 127.0.0.1"));
        assert!(is_restricted_address("[::1]"));
        assert!(is_restricted_address("127.1"));
        assert!(is_restricted_address("::01.2.3.4"));
        assert!(!is_restricted_address("1.2.3.4%eth0"));
        assert!(!is_restricted_address("::ffff:1.2.3.4%x%y"));
        assert!(!is_restricted_address("2606:4700::1%en0"));
    }
}
