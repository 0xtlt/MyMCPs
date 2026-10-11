//! Port of the address guard groups of `tests/unit/hardening_upstream_address.spec.ts`.
//! Its third group, "OAuth endpoints from remote documents", exercises the
//! OAuth flow and belongs with the port of `upstream/oauth.ts`.

use std::net::IpAddr;
use std::sync::Arc;

use mymcps_net::{
    AddressGuard, RestrictedEndpointError, StaticResolver, is_restricted_address, is_restricted_ip,
};
use url::Url;

fn addresses(values: &[&str]) -> Vec<IpAddr> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

/// Name resolution without the network: only the names a test declares resolve.
fn resolve_names(names: &[(&str, &[&str])]) -> (AddressGuard, Arc<StaticResolver>) {
    let mut resolver = StaticResolver::new();
    for (hostname, values) in names {
        resolver = resolver.with(hostname, &addresses(values));
    }
    let resolver = Arc::new(resolver);
    (AddressGuard::new(resolver.clone()), resolver)
}

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}

fn hostname(value: &str) -> String {
    url(value).host_str().unwrap().to_owned()
}

// Restricted addresses

#[test]
fn recognizes_loopback_private_link_local_cgnat_unique_local_and_unspecified_addresses() {
    let restricted = [
        "127.0.0.1",
        "127.255.255.254",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.127.255.255",
        "0.0.0.0",
        "0.1.2.3",
        "192.0.0.8",
        "198.18.0.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fe80::1",
        "fe80::1%en0",
        "fc00::1",
        "fd12:3456:789a::1",
        "fec0::1",
        "ff02::1",
        // IPv6 spellings of restricted IPv4 addresses.
        "::ffff:127.0.0.1",
        "::ffff:a00:1",
        "::a00:1",
        "64:ff9b::a9fe:a9fe",
        "64:ff9b:1::1",
        "2002:7f00:1::1",
        "not-an-address",
    ];
    for address in restricted {
        assert!(
            is_restricted_address(address),
            "{address} should be restricted"
        );
    }

    let reachable = [
        "8.8.8.8",
        "1.1.1.1",
        "172.15.255.255",
        "172.32.0.1",
        "100.63.255.255",
        "100.128.0.1",
        "169.253.255.255",
        "192.0.2.10",
        "198.51.100.7",
        "203.0.113.10",
        "2606:4700:4700::1111",
        "2001:4860:4860::8888",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "2002:808:808::1",
    ];
    for address in reachable {
        assert!(
            !is_restricted_address(address),
            "{address} should be reachable"
        );
    }
}

/// The verdict of the TypeScript guard on the edges of every restricted
/// network, in each spelling of an address. See `fixtures/restricted_addresses.txt`.
#[test]
fn agrees_with_the_typescript_guard_on_the_edges_of_every_network() {
    let vectors = include_str!("fixtures/restricted_addresses.txt");
    let mut checked = 0;
    for line in vectors.lines().filter(|line| !line.starts_with('#')) {
        let (address, verdict) = line.split_once(' ').unwrap();
        let restricted = match verdict {
            "restricted" => true,
            "reachable" => false,
            other => panic!("unknown verdict {other}"),
        };
        assert_eq!(is_restricted_address(address), restricted, "{address}");
        assert_eq!(
            is_restricted_ip(address.parse().unwrap()),
            restricted,
            "{address}"
        );
        checked += 1;
    }
    assert_eq!(checked, 354);
}

#[tokio::test]
async fn sees_through_every_ip_notation_a_url_accepts_without_a_lookup() {
    let (guard, resolver) = resolve_names(&[]);
    let urls = [
        "http://127.0.0.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://0x7f.1/",
        "http://017700000001/",
        "http://127.1/",
        "http://127.0.0.1./",
        "http://0/",
        "http://0xA9FEA9FE/latest/meta-data/",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/",
        "http://[0:0:0:0:0:0:0:1]/",
        "http://[::]/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:7f00:1]/",
        "http://[fd00::1]/",
    ];
    for url in urls {
        assert!(
            guard.resolves_to_restricted_address(&hostname(url)).await,
            "{url}"
        );
    }
    assert!(
        !guard
            .resolves_to_restricted_address(&hostname("http://203.0.113.10/"))
            .await
    );
    assert!(
        !guard
            .resolves_to_restricted_address(&hostname("http://[2606:4700::1]/"))
            .await
    );
    assert!(resolver.lookups().is_empty());
}

#[tokio::test]
async fn judges_a_hostname_by_every_address_it_resolves_to() {
    let (guard, _) = resolve_names(&[
        ("public.example", &["203.0.113.10", "2606:4700::1"]),
        ("internal.example", &["10.0.0.5"]),
        ("mixed.example", &["203.0.113.10", "127.0.0.1"]),
        ("six.example", &["fd00::5"]),
    ]);

    assert!(!guard.resolves_to_restricted_address("public.example").await);
    assert!(
        guard
            .resolves_to_restricted_address("internal.example")
            .await
    );
    assert!(guard.resolves_to_restricted_address("mixed.example").await);
    assert!(guard.resolves_to_restricted_address("six.example").await);
    // No address, no connection: the request fails by itself.
    assert!(
        !guard
            .resolves_to_restricted_address("unresolvable.example")
            .await
    );
}

#[tokio::test]
async fn resolves_names_like_the_http_client_by_default() {
    // The hosts file answers this one, without the network.
    assert!(
        AddressGuard::system()
            .resolves_to_restricted_address("localhost")
            .await
    );
    assert!(
        AddressGuard::default()
            .resolves_to_restricted_address("localhost")
            .await
    );
}

// Discovered endpoint guard

#[tokio::test]
async fn keeps_a_public_mcp_out_of_restricted_networks() {
    let (guard, resolver) = resolve_names(&[
        ("mcp.example", &["203.0.113.10"]),
        ("auth.example", &["203.0.113.11"]),
        ("internal.example", &["192.168.10.4"]),
    ]);
    let endpoints = guard.discovered_endpoint_guard(&url("https://mcp.example/mcp"));

    endpoints
        .assert_allowed(
            &url("https://mcp.example/.well-known/oauth-protected-resource"),
            "OAuth endpoint",
        )
        .await
        .unwrap();
    assert!(
        resolver.lookups().is_empty(),
        "the MCP host itself needs no check"
    );

    endpoints
        .assert_allowed(&url("https://auth.example/token"), "OAuth token endpoint")
        .await
        .unwrap();
    endpoints
        .assert_allowed(
            &url("https://auth.example/register"),
            "OAuth registration endpoint",
        )
        .await
        .unwrap();
    assert_eq!(
        resolver.lookups(),
        ["mcp.example", "auth.example"],
        "each name is resolved once"
    );

    for target in [
        "http://169.254.169.254/latest/meta-data/",
        "http://127.0.0.1:8080/token",
        "http://[::1]:8080/token",
        "https://internal.example/token",
    ] {
        let failure: RestrictedEndpointError = endpoints
            .assert_allowed(&url(target), "OAuth token endpoint")
            .await
            .unwrap_err();
        assert!(
            failure.to_string().contains(&format!(
                "OAuth token endpoint host \"{}\"",
                hostname(target)
            )),
            "{target}: {failure}"
        );
    }
}

#[tokio::test]
async fn words_the_refusal_for_an_administrator() {
    let (guard, _) = resolve_names(&[("mcp.example", &["203.0.113.10"])]);
    let endpoints = guard.discovered_endpoint_guard(&url("https://mcp.example/mcp"));

    let failure = endpoints
        .assert_allowed(
            &url("http://[::1]:9000/authorize"),
            "OAuth authorization endpoint",
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "OAuth authorization endpoint host \"[::1]\" is a loopback, private or link-local address, which a remote MCP may not send MyMCPs to"
    );
    assert_eq!(failure.label, "OAuth authorization endpoint");
    assert_eq!(failure.hostname, "[::1]");

    // The URL parser has already turned the decimal notation into an address.
    let failure = endpoints
        .assert_allowed(&url("http://2130706433/token"), "OAuth token endpoint")
        .await
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .starts_with("OAuth token endpoint host \"127.0.0.1\" is")
    );
}

#[tokio::test]
async fn leaves_an_mcp_on_a_private_address_free_to_use_its_own_network() {
    let (guard, _) = resolve_names(&[("mcp.lan.example", &["192.168.1.5"])]);

    for mcp_url in [
        "http://127.0.0.1:9999/mcp",
        "http://192.168.1.20/mcp",
        "http://[::1]/mcp",
    ] {
        let endpoints = guard.discovered_endpoint_guard(&url(mcp_url));
        endpoints
            .assert_allowed(&url("http://10.0.0.5/token"), "OAuth token endpoint")
            .await
            .unwrap();
    }

    let by_name = guard.discovered_endpoint_guard(&url("http://mcp.lan.example/mcp"));
    by_name
        .assert_allowed(
            &url("http://192.168.1.6:8080/token"),
            "OAuth token endpoint",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn resolves_each_name_once_however_many_checks_run_together() {
    let (guard, resolver) = resolve_names(&[
        ("mcp.example", &["203.0.113.10"]),
        ("auth.example", &["203.0.113.11"]),
        ("internal.example", &["10.0.0.5"]),
    ]);
    let endpoints = guard.discovered_endpoint_guard(&url("https://mcp.example/mcp"));
    let shared = endpoints.clone();

    let token = url("https://auth.example/token");
    let register = url("https://auth.example/register");
    let internal = url("https://internal.example/token");
    let (first, second, third, fourth) = tokio::join!(
        endpoints.assert_allowed(&token, "OAuth token endpoint"),
        shared.assert_allowed(&register, "OAuth registration endpoint"),
        endpoints.assert_allowed(&internal, "OAuth token endpoint"),
        shared.assert_allowed(&internal, "OAuth token endpoint"),
    );
    assert!(first.is_ok() && second.is_ok());
    assert!(third.is_err() && fourth.is_err());

    let mut lookups = resolver.lookups();
    lookups.sort();
    assert_eq!(lookups, ["auth.example", "internal.example", "mcp.example"]);
}
