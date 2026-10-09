//! Which address a request came from, behind zero or more reverse proxies.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::IpNet;

/// Returned when not even the socket has a peer address, e.g. after the
/// client disconnected.
const UNKNOWN_IP: &str = "0.0.0.0";

/// The proxies allowed to forward the caller's address, from `TRUST_PROXY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustProxy {
    /// Every hop is believed.
    All,
    /// No forwarded address is believed.
    None,
    /// Only these addresses and ranges are believed.
    Ranges(Vec<IpNet>),
}

impl Default for TrustProxy {
    /// Loopback proxies only.
    fn default() -> Self {
        Self::Ranges(named_ranges("loopback").unwrap_or_default())
    }
}

fn nets(values: &[&str]) -> Vec<IpNet> {
    values
        .iter()
        .filter_map(|value| value.parse().ok())
        .collect()
}

/// The range names proxy-addr understands.
fn named_ranges(name: &str) -> Option<Vec<IpNet>> {
    match name {
        "loopback" => Some(nets(&["127.0.0.1/8", "::1/128"])),
        "linklocal" => Some(nets(&["169.254.0.0/16", "fe80::/10"])),
        "uniquelocal" => Some(nets(&[
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "fc00::/7",
        ])),
        _ => None,
    }
}

impl TrustProxy {
    /// Turn `TRUST_PROXY` into a trust policy: `true`, `false`, or a
    /// comma-separated list of proxy IPs, CIDR ranges, and the names
    /// loopback, linklocal and uniquelocal. Empty means loopback.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let mut entries: Vec<&str> = value
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .collect();
        if entries.is_empty() {
            entries.push("loopback");
        }

        if let [only] = entries.as_slice() {
            match only.to_ascii_lowercase().as_str() {
                "true" => return Ok(Self::All),
                "false" => return Ok(Self::None),
                _ => {}
            }
        }

        let mut ranges = Vec::new();
        for entry in entries {
            let lowered = entry.to_ascii_lowercase();
            if let Some(named) = named_ranges(&lowered) {
                ranges.extend(named);
            } else if let Ok(address) = lowered.parse::<IpAddr>() {
                ranges.push(IpNet::from(address));
            } else if let Ok(range) = lowered.parse::<IpNet>() {
                ranges.push(range.trunc());
            } else {
                return Err(format!(
                    "Invalid TRUST_PROXY entry \"{entry}\". Use true, false, or a comma-separated list of proxy IPs, CIDR ranges, and the names loopback, linklocal, uniquelocal."
                ));
            }
        }
        Ok(Self::Ranges(ranges))
    }

    /// Whether a hop at this address may vouch for the one before it. A
    /// range never matches something that is not an IP address.
    pub fn trusts(&self, address: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Ranges(ranges) => {
                let Ok(address) = address.parse::<IpAddr>() else {
                    return false;
                };
                // An IPv4 proxy seen through a dual-stack socket matches IPv4 ranges.
                let address = address.to_canonical();
                ranges.iter().any(|range| range.contains(&address))
            }
        }
    }

    /// The address proxy-addr resolves: walk back from the socket through
    /// `X-Forwarded-For` while each hop is trusted, and stop at the first
    /// one that is not. The result is copied verbatim from the header, so it
    /// can be any string.
    fn forwarded(
        &self,
        socket_address: Option<&str>,
        x_forwarded_for: Option<&str>,
    ) -> Option<String> {
        let mut hops: Vec<&str> = Vec::new();
        hops.extend(socket_address);
        if let Some(header) = x_forwarded_for {
            hops.extend(header.split(',').map(str::trim).rev());
        }

        let last = hops.len().checked_sub(1)?;
        for (index, hop) in hops.iter().enumerate() {
            if index == last || !self.trusts(hop) {
                return Some((*hop).to_string());
            }
        }
        None
    }

    /// The client address of a request, always an IP address: limiter keys
    /// and call logs rely on it being one.
    pub fn client_ip(
        &self,
        socket_address: Option<IpAddr>,
        x_forwarded_for: Option<&str>,
    ) -> String {
        let socket = socket_address.map(|address| address.to_string());
        let forwarded = self.forwarded(socket.as_deref(), x_forwarded_for);
        resolve_client_ip(forwarded.as_deref(), socket.as_deref())
    }
}

/// Fall back to the socket peer unless the forwarded value is an IP address.
pub fn resolve_client_ip(forwarded: Option<&str>, socket_address: Option<&str>) -> String {
    let is_ip = |value: &&str| value.parse::<IpAddr>().is_ok();
    forwarded
        .filter(is_ip)
        .or(socket_address.filter(is_ip))
        .unwrap_or(UNKNOWN_IP)
        .to_string()
}

/// The key a client address is rate-limited under. An IPv6 client usually
/// holds a whole /64 and can use a new address for every request, so IPv6 is
/// keyed on that prefix; IPv4, including its IPv6-mapped form, on the address.
pub fn rate_limit_client_key(ip: &str) -> String {
    let without_zone = ip.split('%').next().unwrap_or(ip);
    let Ok(address) = without_zone.parse::<Ipv6Addr>() else {
        return ip.to_string();
    };

    let groups = address.segments();
    if groups[..5].iter().all(|group| *group == 0) && groups[5] == 0xffff {
        return Ipv4Addr::from((u32::from(groups[6]) << 16) | u32::from(groups[7])).to_string();
    }
    format!(
        "{:x}:{:x}:{:x}:{:x}::/64",
        groups[0], groups[1], groups[2], groups[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_ip(trust: &TrustProxy, socket: &str, xff: Option<&str>) -> String {
        trust.client_ip(Some(socket.parse().unwrap()), xff)
    }

    #[test]
    fn trusts_only_loopback_proxies_by_default() {
        for value in [
            None,
            Some(""),
            Some("  "),
            Some("loopback"),
            Some("Loopback"),
        ] {
            let trust = TrustProxy::parse(value).unwrap();
            assert!(trust.trusts("127.0.0.1"));
            assert!(trust.trusts("::1"));
            assert!(!trust.trusts("10.0.0.5"));
            assert!(!trust.trusts("203.0.113.7"));
        }
        assert_eq!(TrustProxy::default(), TrustProxy::parse(None).unwrap());
    }

    #[test]
    fn maps_booleans_to_trusting_every_proxy_or_none() {
        assert_eq!(TrustProxy::parse(Some("true")).unwrap(), TrustProxy::All);
        assert_eq!(TrustProxy::parse(Some(" TRUE ")).unwrap(), TrustProxy::All);
        assert_eq!(TrustProxy::parse(Some("false")).unwrap(), TrustProxy::None);
    }

    #[test]
    fn accepts_a_comma_separated_list_of_range_names() {
        let trust = TrustProxy::parse(Some("loopback,uniquelocal")).unwrap();
        for proxy in [
            "127.0.0.1",
            "::1",
            "10.0.1.9",
            "172.18.0.2",
            "192.168.1.1",
            "fd00::7",
        ] {
            assert!(trust.trusts(proxy), "{proxy}");
        }
        assert!(trust.trusts("::ffff:172.18.0.2"));
        for client in [
            "203.0.113.7",
            "172.32.0.1",
            "169.254.1.1",
            "2001:db8::1",
            "traefik",
        ] {
            assert!(!trust.trusts(client), "{client}");
        }
    }

    #[test]
    fn accepts_a_list_mixing_addresses_cidr_ranges_and_names() {
        let trust = TrustProxy::parse(Some(" 203.0.113.7 , 198.51.100.0/24,linklocal, ")).unwrap();
        assert!(trust.trusts("203.0.113.7"));
        assert!(trust.trusts("198.51.100.200"));
        assert!(trust.trusts("169.254.1.1"));
        assert!(!trust.trusts("203.0.113.8"));
        assert!(!trust.trusts("127.0.0.1"));
    }

    #[test]
    fn names_the_entry_it_cannot_use() {
        assert!(
            TrustProxy::parse(Some("loopback,traefik"))
                .unwrap_err()
                .contains("Invalid TRUST_PROXY entry \"traefik\"")
        );
        assert!(
            TrustProxy::parse(Some("true,10.0.0.0/8"))
                .unwrap_err()
                .contains("Invalid TRUST_PROXY entry \"true\"")
        );
        assert!(
            TrustProxy::parse(Some("10.0.0.0/64"))
                .unwrap_err()
                .contains("Invalid TRUST_PROXY entry")
        );
    }

    #[test]
    fn keeps_a_forwarded_value_only_when_it_is_an_ip_address() {
        assert_eq!(
            resolve_client_ip(Some("203.0.113.7"), Some("172.18.0.2")),
            "203.0.113.7"
        );
        assert_eq!(
            resolve_client_ip(Some("2001:db8::1"), Some("172.18.0.2")),
            "2001:db8::1"
        );
        for forged in [
            Some(""),
            Some("unknown"),
            Some("203.0.113.7:4711"),
            Some("203.0.113"),
            Some("<script>"),
            None,
        ] {
            assert_eq!(resolve_client_ip(forged, Some("172.18.0.2")), "172.18.0.2");
        }
    }

    #[test]
    fn still_answers_with_an_address_when_the_socket_has_none() {
        assert!(
            resolve_client_ip(Some("unknown"), None)
                .parse::<Ipv4Addr>()
                .is_ok()
        );
    }

    #[test]
    fn uses_the_nearest_address_a_private_network_proxy_did_not_vouch_for() {
        let trust = TrustProxy::parse(Some("loopback,uniquelocal")).unwrap();
        assert_eq!(
            request_ip(&trust, "172.18.0.2", Some("203.0.113.7")),
            "203.0.113.7"
        );
        assert_eq!(
            request_ip(&trust, "172.18.0.2", Some("198.51.100.1, 203.0.113.7")),
            "203.0.113.7"
        );
        assert_eq!(
            request_ip(&trust, "172.18.0.2", Some("forged, 203.0.113.7")),
            "203.0.113.7"
        );
        assert_eq!(
            request_ip(&trust, "203.0.113.7", Some("198.51.100.1")),
            "203.0.113.7"
        );
    }

    #[test]
    fn falls_back_to_the_socket_address_when_the_forwarded_client_is_not_an_ip_address() {
        for policy in ["true", "loopback,uniquelocal"] {
            let trust = TrustProxy::parse(Some(policy)).unwrap();
            for forged in ["forged", "203.0.113.7:4711", "' OR 1=1 --", "10.0.0.999"] {
                assert_eq!(
                    request_ip(&trust, "172.18.0.2", Some(forged)),
                    "172.18.0.2",
                    "{forged}"
                );
            }
            assert_eq!(request_ip(&trust, "172.18.0.2", None), "172.18.0.2");
        }
    }

    #[test]
    fn the_default_policy_reports_an_address_for_a_forged_forwarded_header() {
        let trust = TrustProxy::default();
        assert_eq!(request_ip(&trust, "127.0.0.1", Some("forged")), "127.0.0.1");
        assert_eq!(
            request_ip(&trust, "127.0.0.1", Some("203.0.113.7")),
            "203.0.113.7"
        );
    }

    #[test]
    fn keys_ipv4_clients_on_their_address() {
        assert_eq!(rate_limit_client_key("203.0.113.7"), "203.0.113.7");
        assert_eq!(rate_limit_client_key("::ffff:203.0.113.7"), "203.0.113.7");
        assert_eq!(rate_limit_client_key("::FFFF:cb00:7107"), "203.0.113.7");
    }

    #[test]
    fn keys_ipv6_clients_on_their_64() {
        let key = rate_limit_client_key("2001:db8:12:34::1");
        assert_eq!(key, "2001:db8:12:34::/64");
        assert_eq!(
            rate_limit_client_key("2001:0DB8:0012:0034:ffff:ffff:ffff:ffff"),
            key
        );
        assert_eq!(rate_limit_client_key("2001:db8:12:34:5:6:7:8%eth0"), key);
        assert_ne!(rate_limit_client_key("2001:db8:12:35::1"), key);
        assert_eq!(rate_limit_client_key("::1"), "0:0:0:0::/64");
        assert_eq!(rate_limit_client_key("fe80::1"), "fe80:0:0:0::/64");
    }
}
