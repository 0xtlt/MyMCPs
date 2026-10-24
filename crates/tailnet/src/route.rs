//! Which hosts a tailnet answers for.
//!
//! A host is routed only when the status of the daemon says it is one of the
//! tailnet's own peers. Nothing is resolved with the DNS of the machine, and a
//! host that is not a peer is never sent to the daemon, which would otherwise
//! be asked to dial anything.

use std::net::IpAddr;

use crate::localapi::{PeerStatus, Status};

/// Addresses a tailnet never routes, whatever a peer announces: a subnet
/// router could advertise `127.0.0.0/8` or the metadata address of a cloud.
pub(crate) fn is_forbidden(address: IpAddr) -> bool {
    let address = address.to_canonical();
    let link_local = match address {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    };
    address.is_loopback() || address.is_unspecified() || address.is_multicast() || link_local
}

/// Whether `host` is the tailnet's to answer for, or nobody's: an IP literal,
/// a name of one label (`nas`, `localhost`), and a name under `.ts.net` or
/// under the MagicDNS suffix of this tailnet, which a Headscale chooses
/// itself. None of these names is a host of the public internet, so a host of
/// this kind that no peer carries must never be sent there instead: an address
/// literal is a machine of some network, which the tailnet's routes withdrawn
/// would turn into one of the network the server runs in, and a name of the
/// tailnet is a device that is gone. Only another DNS name may be left to a
/// network that resolves it.
pub(crate) fn is_tailnet_only(status: &Status, host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty() || !name.contains('.') || under(&name, "ts.net") {
        return true;
    }
    let own = status
        .self_node
        .as_ref()
        .map(PeerStatus::fqdn)
        .and_then(|fqdn| fqdn.split_once('.').map(|(_, suffix)| suffix.to_owned()));
    [Some(status.magic_dns_suffix().to_ascii_lowercase()), own]
        .into_iter()
        .flatten()
        .filter(|suffix| !suffix.is_empty())
        .any(|suffix| under(&name, &suffix))
}

/// Whether `name` is `suffix` or a name below it.
fn under(name: &str, suffix: &str) -> bool {
    name == suffix
        || name
            .strip_suffix(suffix)
            .is_some_and(|label| label.ends_with('.'))
}

/// The Tailscale address to dial for `host`, or `None` when it is not a peer.
///
/// The addresses of this node are never routed, on any path: a subnet router
/// of a hostile control server can advertise a prefix that covers them, and
/// the netstack of the daemon forwards a connection to its own address to the
/// loopback of the host.
pub(crate) fn resolve(status: &Status, host: &str) -> Option<IpAddr> {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    match host.parse::<IpAddr>() {
        Ok(address) => resolve_address(status, address),
        Err(_) => resolve_name(status, host),
    }
}

fn resolve_address(status: &Status, address: IpAddr) -> Option<IpAddr> {
    let address = address.to_canonical();
    if is_forbidden(address) || status.is_own_address(address) {
        return None;
    }
    let reachable = status.peers().any(|peer| {
        peer.addresses()
            .iter()
            .any(|own| own.to_canonical() == address)
            // The default route of an exit node is not a subnet: only a
            // prefix that names some of the network is a route to it.
            || peer
                .routes()
                .iter()
                .any(|route| route.bits() > 0 && route.contains(address))
    });
    reachable.then_some(address)
}

fn resolve_name(status: &Status, host: &str) -> Option<IpAddr> {
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let suffix = status.magic_dns_suffix().to_ascii_lowercase();
    status
        .peers()
        .filter(|peer| names_peer(peer, &name, &suffix))
        .find_map(|peer| preferred_address(status, peer))
}

fn names_peer(peer: &PeerStatus, name: &str, suffix: &str) -> bool {
    let fqdn = peer.fqdn();
    if fqdn.is_empty() {
        return false;
    }
    if fqdn == name {
        return true;
    }
    // The first label alone is the MagicDNS short name, of this tailnet's own
    // nodes: a node shared in from another tailnet has to be named in full.
    let in_tailnet = suffix.is_empty()
        || fqdn
            .strip_suffix(suffix)
            .is_some_and(|label| label.ends_with('.'));
    !name.contains('.') && in_tailnet && fqdn.split('.').next() == Some(name)
}

/// IPv4 first: it is the one every service listens on. An address of this
/// node, that a peer lists as its own, is not one.
fn preferred_address(status: &Status, peer: &PeerStatus) -> Option<IpAddr> {
    let usable: Vec<IpAddr> = peer
        .addresses()
        .into_iter()
        .map(|address| address.to_canonical())
        .filter(|address| !is_forbidden(*address) && !status.is_own_address(*address))
        .collect();
    usable
        .iter()
        .find(|address| address.is_ipv4())
        .or(usable.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The shape of `tailscale status --json`, trimmed to what is read.
    fn status() -> Status {
        serde_json::from_value(json!({
            "Version": "1.104.2-t1234",
            "BackendState": "Running",
            "AuthURL": "",
            "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"],
            "Self": {
                "DNSName": "mymcps.tail1234.ts.net.",
                "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"]
            },
            "MagicDNSSuffix": "tail1234.ts.net",
            "CurrentTailnet": {"Name": "me@example.com", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true},
            "Peer": {
                "nodekey:aaaa": {
                    "HostName": "NAS",
                    "DNSName": "nas.tail1234.ts.net.",
                    "Online": true,
                    "TailscaleIPs": ["100.64.0.2", "fd7a:115c:a1e0::2"]
                },
                "nodekey:bbbb": {
                    "DNSName": "Router.tail1234.ts.net.",
                    "TailscaleIPs": ["fd7a:115c:a1e0::3", "100.64.0.3"],
                    "PrimaryRoutes": ["192.168.1.0/24", "10.1.0.0/16", "fd00::/64"]
                },
                "nodekey:cccc": {
                    "DNSName": "evil.tail1234.ts.net.",
                    "TailscaleIPs": ["100.64.0.4"],
                    "PrimaryRoutes": ["127.0.0.0/8", "169.254.0.0/16", "224.0.0.0/4", "0.0.0.0/0", "fe80::/10"]
                },
                "nodekey:dddd": {
                    "DNSName": "shared.other-tailnet.ts.net.",
                    "TailscaleIPs": ["100.64.0.5"]
                },
                "nodekey:eeee": {
                    "DNSName": "nas.other-tailnet.ts.net.",
                    "TailscaleIPs": ["100.64.0.6"]
                },
                "nodekey:ffff": {
                    "DNSName": "bare.tail1234.ts.net.",
                    "TailscaleIPs": null,
                    "PrimaryRoutes": null
                }
            }
        }))
        .unwrap()
    }

    fn ip(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    #[test]
    fn a_peer_is_found_by_its_full_magic_dns_name() {
        let status = status();
        assert_eq!(resolve(&status, "nas.tail1234.ts.net"), ip("100.64.0.2"));
        assert_eq!(resolve(&status, "nas.tail1234.ts.net."), ip("100.64.0.2"));
    }

    #[test]
    fn a_peer_is_found_by_its_first_label_in_any_case() {
        let status = status();
        assert_eq!(resolve(&status, "nas"), ip("100.64.0.2"));
        assert_eq!(resolve(&status, "NAS"), ip("100.64.0.2"));
        assert_eq!(resolve(&status, "ROUTER.Tail1234.TS.net"), ip("100.64.0.3"));
        assert_eq!(resolve(&status, "router"), ip("100.64.0.3"));
    }

    #[test]
    fn the_first_label_only_names_a_node_of_this_tailnet() {
        let status = status();
        // `nas` exists in two tailnets: the one of this tailnet wins, and the
        // shared node answers to its full name only.
        assert_eq!(resolve(&status, "nas"), ip("100.64.0.2"));
        assert_eq!(
            resolve(&status, "nas.other-tailnet.ts.net"),
            ip("100.64.0.6")
        );
        assert_eq!(resolve(&status, "shared"), None);
        assert_eq!(
            resolve(&status, "shared.other-tailnet.ts.net"),
            ip("100.64.0.5")
        );
    }

    #[test]
    fn other_hosts_are_not_peers() {
        let status = status();
        for host in [
            "example.com",
            "nas.example.com",
            "nas.tail1234.ts.net.example.com",
            "nas.tail1234",
            "mymcps",
            "mymcps.tail1234.ts.net",
            "",
            ".",
            "localhost",
            "bare",
        ] {
            assert_eq!(resolve(&status, host), None, "{host:?}");
        }
    }

    #[test]
    fn a_peer_is_found_by_one_of_its_tailscale_addresses() {
        let status = status();
        assert_eq!(resolve(&status, "100.64.0.2"), ip("100.64.0.2"));
        assert_eq!(
            resolve(&status, "fd7a:115c:a1e0::2"),
            ip("fd7a:115c:a1e0::2")
        );
        assert_eq!(
            resolve(&status, "[fd7a:115c:a1e0::2]"),
            ip("fd7a:115c:a1e0::2")
        );
        assert_eq!(resolve(&status, "::ffff:100.64.0.2"), ip("100.64.0.2"));
        assert_eq!(resolve(&status, "100.64.0.99"), None);
        // This node is not a peer of itself.
        assert_eq!(resolve(&status, "100.64.0.1"), None);
    }

    #[test]
    fn a_name_gets_the_ipv4_address_first() {
        let status = status();
        // `Router` lists its IPv6 address first.
        assert_eq!(resolve(&status, "router"), ip("100.64.0.3"));
    }

    #[test]
    fn an_address_inside_the_primary_routes_of_a_peer_is_routed() {
        let status = status();
        assert_eq!(resolve(&status, "192.168.1.50"), ip("192.168.1.50"));
        assert_eq!(resolve(&status, "10.1.200.3"), ip("10.1.200.3"));
        assert_eq!(resolve(&status, "fd00::1234"), ip("fd00::1234"));
        assert_eq!(resolve(&status, "192.168.2.50"), None);
        assert_eq!(resolve(&status, "10.2.0.1"), None);
        assert_eq!(resolve(&status, "8.8.8.8"), None);
        assert_eq!(resolve(&status, "fd01::1"), None);
    }

    #[test]
    fn a_route_never_reaches_loopback_link_local_unspecified_or_multicast() {
        let status = status();
        for host in [
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "169.254.169.254",
            "fe80::1",
            "0.0.0.0",
            "::",
            "224.0.0.1",
            "239.255.255.250",
            "ff02::1",
            "::ffff:127.0.0.1",
            "255.255.255.255",
        ] {
            assert_eq!(resolve(&status, host), None, "{host}");
        }
    }

    #[test]
    fn the_hosts_only_the_tailnet_answers_for() {
        let status = status();
        for host in [
            "127.0.0.1",
            "8.8.8.8",
            "192.168.2.50",
            "::1",
            "[fd01::1]",
            "::ffff:8.8.8.8",
            "nas",
            "localhost",
            "NAS",
            "gone.tail1234.ts.net",
            "gone.tail1234.ts.net.",
            "mymcps.tail1234.ts.net",
            "x.other-tailnet.ts.net",
            "TS.NET",
            "ts.net",
            "",
            ".",
        ] {
            assert!(is_tailnet_only(&status, host), "{host:?}");
        }
        for host in [
            "example.com",
            "auth.example.com",
            "nas.tail1234.ts.net.example.com",
            "tail1234.ts.net.evil.io",
            "notts.net",
            "evil-tail1234.ts.net.example",
        ] {
            assert!(!is_tailnet_only(&status, host), "{host:?}");
        }
    }

    #[test]
    fn the_magic_dns_suffix_of_a_headscale_is_the_tailnets_too() {
        let status: Status = serde_json::from_value(json!({
            "MagicDNSSuffix": "tailnet.corp.example",
            "Self": {"DNSName": "mymcps.tailnet.corp.example."},
            "Peer": {}
        }))
        .unwrap();
        assert!(is_tailnet_only(&status, "gone.tailnet.corp.example"));
        assert!(is_tailnet_only(&status, "tailnet.corp.example"));
        assert!(!is_tailnet_only(&status, "corp.example"));
        assert!(!is_tailnet_only(&status, "auth.example.com"));
        assert!(!is_tailnet_only(&status, "xtailnet.corp.example"));
        // Without a suffix in the status, the one of this node's own name.
        let status: Status = serde_json::from_value(json!({
            "Self": {"DNSName": "mymcps.hs.example."}
        }))
        .unwrap();
        assert!(is_tailnet_only(&status, "gone.hs.example"));
        assert!(!is_tailnet_only(&status, "auth.example.com"));
    }

    #[test]
    fn the_default_route_of_an_exit_node_routes_nothing() {
        // `evil` announces 0.0.0.0/0: that does not make 8.8.8.8 a peer.
        assert_eq!(resolve(&status(), "8.8.8.8"), None);
    }

    #[test]
    fn a_status_without_peers_routes_nothing() {
        let status: Status = serde_json::from_value(json!({
            "BackendState": "Running",
            "Peer": null
        }))
        .unwrap();
        assert_eq!(resolve(&status, "nas"), None);
        assert_eq!(resolve(&status, "100.64.0.2"), None);
        let status: Status = serde_json::from_value(json!({})).unwrap();
        assert_eq!(resolve(&status, "nas"), None);
    }

    #[test]
    fn the_legacy_suffix_is_used_when_the_tailnet_has_none() {
        let status: Status = serde_json::from_value(json!({
            "MagicDNSSuffix": "tail1234.ts.net",
            "Peer": {"k": {"DNSName": "nas.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.2"]}}
        }))
        .unwrap();
        assert_eq!(status.magic_dns_suffix(), "tail1234.ts.net");
        assert_eq!(resolve(&status, "nas"), ip("100.64.0.2"));
    }

    /// A hostile control server: a peer routes the prefixes that hold the
    /// addresses of this node, and another lists one of them as its own.
    fn status_with_a_subnet_over_this_node() -> Status {
        serde_json::from_value(json!({
            "BackendState": "Running",
            "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"],
            "Self": {"DNSName": "mymcps.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"]},
            "MagicDNSSuffix": "tail1234.ts.net",
            "Peer": {
                "nodekey:aaaa": {
                    "DNSName": "router.tail1234.ts.net.",
                    "TailscaleIPs": ["100.64.0.3"],
                    "PrimaryRoutes": ["100.64.0.0/10", "fd7a:115c:a1e0::/48", "192.168.1.0/24"]
                },
                "nodekey:bbbb": {
                    "DNSName": "twin.tail1234.ts.net.",
                    "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"]
                },
                "nodekey:cccc": {
                    "DNSName": "half.tail1234.ts.net.",
                    "TailscaleIPs": ["100.64.0.1", "100.64.0.9"]
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_subnet_route_never_reaches_the_addresses_of_this_node() {
        let status = status_with_a_subnet_over_this_node();
        for host in [
            "100.64.0.1",
            "fd7a:115c:a1e0::1",
            "[fd7a:115c:a1e0::1]",
            "::ffff:100.64.0.1",
            "FD7A:115C:A1E0:0:0:0:0:1",
        ] {
            assert_eq!(resolve(&status, host), None, "{host}");
        }
        // The rest of the prefix is still the router's.
        assert_eq!(resolve(&status, "100.64.0.77"), ip("100.64.0.77"));
        assert_eq!(
            resolve(&status, "fd7a:115c:a1e0::77"),
            ip("fd7a:115c:a1e0::77")
        );
        assert_eq!(resolve(&status, "192.168.1.5"), ip("192.168.1.5"));
    }

    #[test]
    fn a_peer_that_lists_an_address_of_this_node_is_not_dialled_there() {
        let status = status_with_a_subnet_over_this_node();
        // `twin` has nothing but this node's addresses: nothing to dial.
        assert_eq!(resolve(&status, "twin"), None);
        assert_eq!(resolve(&status, "twin.tail1234.ts.net"), None);
        // `half` keeps its own address.
        assert_eq!(resolve(&status, "half"), ip("100.64.0.9"));
        // The other peers are not affected.
        assert_eq!(resolve(&status, "router"), ip("100.64.0.3"));
    }

    #[test]
    fn the_addresses_of_this_node_are_read_from_both_parts_of_the_status() {
        let top: Status = serde_json::from_value(json!({"TailscaleIPs": ["100.64.0.1"]})).unwrap();
        let node: Status =
            serde_json::from_value(json!({"Self": {"TailscaleIPs": ["fd7a:115c:a1e0::1"]}}))
                .unwrap();
        assert!(top.is_own_address("100.64.0.1".parse().unwrap()));
        assert!(node.is_own_address("fd7a:115c:a1e0::1".parse().unwrap()));
        assert!(!top.is_own_address("100.64.0.2".parse().unwrap()));
        assert!(!node.is_own_address("100.64.0.1".parse().unwrap()));
    }

    #[test]
    fn prefixes_are_matched_bit_by_bit() {
        // /15 is the second octet 2 and 3.
        let prefix: crate::localapi::Prefix = "10.2.0.0/15".parse().unwrap();
        assert!(prefix.contains("10.2.0.0".parse().unwrap()));
        assert!(prefix.contains("10.3.255.255".parse().unwrap()));
        assert!(!prefix.contains("10.1.255.255".parse().unwrap()));
        assert!(!prefix.contains("10.4.0.0".parse().unwrap()));
        let host: crate::localapi::Prefix = "100.64.0.9/32".parse().unwrap();
        assert!(host.contains("100.64.0.9".parse().unwrap()));
        assert!(!host.contains("100.64.0.8".parse().unwrap()));
        assert!("10.0.0.0/33".parse::<crate::localapi::Prefix>().is_err());
        assert!("10.0.0.0".parse::<crate::localapi::Prefix>().is_err());
        assert!("::/129".parse::<crate::localapi::Prefix>().is_err());
    }
}
