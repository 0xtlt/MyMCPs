//! The "OAuth endpoints from remote documents" group of
//! `tests/unit/hardening_upstream_address.spec.ts`: the endpoints a remote MCP
//! or its provider names are kept out of the instance's own network.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Duration;
use mymcps_core::models::{Mcp, McpStatus};
use mymcps_core::{TestCore, Timestamp};
use mymcps_upstream::{Upstream, UpstreamError};
use serde_json::{Value, json};
use support::*;
use url::Url;

/// Serves JSON documents by exact URL, 404 for the rest, and records every
/// request. Only the names declared here resolve.
fn documents(core: &TestCore, documents: Vec<(String, Value)>) -> (Arc<Upstream>, Calls) {
    let (addresses, _) = resolve_names(&[
        ("mcp.example", &["203.0.113.10"]),
        ("auth.example", &["203.0.113.11"]),
        ("tokens.internal.example", &["192.168.10.4"]),
        ("mcp.lan.example", &["192.168.1.5"]),
    ]);
    let documents: HashMap<String, Value> = documents.into_iter().collect();
    upstream_with(
        core,
        addresses,
        Default::default(),
        move |call| match documents.get(&call.url) {
            Some(document) => json_response(document.clone()),
            None => not_found(),
        },
    )
}

async fn remote_mcp(core: &TestCore, http_url: &str) -> Mcp {
    create_mcp(core, |mcp| {
        mcp.http_url = Some(http_url.to_owned());
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
    })
    .await
}

async fn connected_mcp(core: &TestCore) -> Mcp {
    let mut mcp = remote_mcp(core, "https://mcp.example/mcp").await;
    mcp.oauth_issuer = Some("https://auth.example".into());
    mcp.oauth_authorize_url = Some("https://auth.example/authorize".into());
    mcp.oauth_token_url = Some("https://auth.example/token".into());
    mcp.oauth_client_id = Some("registered-client".into());
    mcp.oauth_client_auth_method = Some("none".into());
    mcp.oauth_access_token = encrypt(core, "access-old");
    mcp.oauth_refresh_token = encrypt(core, "refresh-old");
    mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    mcp.save(&*core.db).await.unwrap();
    mcp
}

fn protected_resource(provider: &str, resource: &str) -> (String, Value) {
    (
        "https://mcp.example/.well-known/oauth-protected-resource/mcp".to_owned(),
        json!({ "resource": resource, "authorization_servers": [provider] }),
    )
}

fn provider_documents() -> Vec<(String, Value)> {
    vec![
        (
            "https://auth.example/.well-known/oauth-authorization-server".to_owned(),
            authorization_server("https://auth.example"),
        ),
        (
            "https://auth.example/register".to_owned(),
            registered_client("registered-client"),
        ),
    ]
}

fn assert_refused(error: &UpstreamError, label: &str, hostname: &str) {
    assert!(error.is_restricted_endpoint(), "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "{label} host \"{hostname}\" is a loopback, private or link-local address, which a remote MCP may not send MyMCPs to"
        )
    );
}

#[tokio::test]
async fn refuses_an_authorization_server_on_the_metadata_address_before_requesting_it() {
    let core = TestCore::new().await;
    let mut mcp = remote_mcp(&core, "https://mcp.example/mcp").await;
    let (upstream, calls) = documents(
        &core,
        vec![protected_resource(
            "http://169.254.169.254/latest",
            "https://mcp.example/mcp",
        )],
    );

    let error = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap_err();
    assert_refused(&error, "OAuth endpoint", "169.254.169.254");
    assert!(!calls.hosts().contains(&"169.254.169.254".to_owned()));
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_issuer, None);
}

#[tokio::test]
async fn refuses_registration_token_and_authorization_endpoints_inside_the_network() {
    let cases = [
        (
            json!({ "registration_endpoint": "http://10.0.0.5:8080/register" }),
            ("OAuth registration endpoint", "10.0.0.5"),
            "10.0.0.5:8080",
        ),
        (
            json!({ "token_endpoint": "https://tokens.internal.example/token" }),
            ("OAuth token endpoint", "tokens.internal.example"),
            "tokens.internal.example",
        ),
        (
            json!({ "authorization_endpoint": "http://[::1]:9000/authorize" }),
            ("OAuth authorization endpoint", "[::1]"),
            "[::1]:9000",
        ),
        (
            json!({ "token_endpoint": "http://2130706433/token" }),
            ("OAuth token endpoint", "127.0.0.1"),
            "127.0.0.1",
        ),
    ];

    for (overrides, (label, hostname), host) in cases {
        let core = TestCore::new().await;
        let mut mcp = remote_mcp(&core, "https://mcp.example/mcp").await;
        let (upstream, calls) = documents(
            &core,
            vec![
                protected_resource("https://auth.example", "https://mcp.example/mcp"),
                (
                    "https://auth.example/.well-known/oauth-authorization-server".to_owned(),
                    authorization_server_with("https://auth.example", overrides),
                ),
                (
                    "https://auth.example/register".to_owned(),
                    registered_client("registered-client"),
                ),
                (
                    "http://10.0.0.5:8080/register".to_owned(),
                    registered_client("registered-client"),
                ),
            ],
        );

        let error = upstream
            .start_oauth_flow(&MemorySession::new(), &mut mcp)
            .await
            .unwrap_err();
        assert_refused(&error, label, hostname);
        assert!(!calls.hosts().contains(&host.to_owned()), "{host}");
        // Nothing was registered or saved for a provider that was refused.
        assert!(calls.posts().is_empty());
        let saved = find_mcp(&core, mcp.id).await;
        assert_eq!(saved.oauth_client_id, None);
    }
}

#[tokio::test]
async fn checks_the_token_endpoint_again_on_every_refresh() {
    let core = TestCore::new().await;

    // The provider's metadata is fetched again for each refresh.
    let mut moved = connected_mcp(&core).await;
    let (upstream, moved_calls) = documents(
        &core,
        vec![(
            "https://auth.example/.well-known/oauth-authorization-server".to_owned(),
            authorization_server_with(
                "https://auth.example",
                json!({ "token_endpoint": "http://127.0.0.1:8080/token" }),
            ),
        )],
    );
    let error = upstream
        .refresh_oauth_access_token(&mut moved)
        .await
        .unwrap_err();
    assert_refused(&error, "OAuth token endpoint", "127.0.0.1");
    assert!(moved_calls.posts().is_empty());

    // Without discovery the endpoints saved on the row are used.
    let mut saved = connected_mcp(&core).await;
    saved.oauth_token_url = Some("http://10.0.0.9/token".into());
    saved.save(&*core.db).await.unwrap();
    let (upstream, saved_calls) = documents(&core, vec![]);
    let error = upstream
        .refresh_oauth_access_token(&mut saved)
        .await
        .unwrap_err();
    assert_refused(&error, "OAuth token endpoint", "10.0.0.9");
    assert!(saved_calls.posts().is_empty());
    let row = find_mcp(&core, saved.id).await;
    assert_eq!(
        decrypt(&core, &row.oauth_refresh_token).as_deref(),
        Some("refresh-old")
    );
}

#[tokio::test]
async fn lets_a_self_hosted_mcp_use_a_provider_on_its_own_network() {
    for (mcp_url, provider) in [
        ("http://127.0.0.1:9999/mcp", "http://127.0.0.1:9998"),
        ("http://mcp.lan.example/mcp", "http://192.168.1.6:8080"),
    ] {
        let core = TestCore::new().await;
        let mut mcp = remote_mcp(&core, mcp_url).await;
        let mcp_origin = origin(&Url::parse(mcp_url).unwrap());
        let (upstream, _) = documents(
            &core,
            vec![
                (
                    format!("{mcp_origin}/.well-known/oauth-protected-resource/mcp"),
                    json!({ "resource": mcp_url, "authorization_servers": [provider] }),
                ),
                (
                    format!("{provider}/.well-known/oauth-authorization-server"),
                    authorization_server(provider),
                ),
                (
                    format!("{provider}/register"),
                    registered_client("registered-client"),
                ),
            ],
        );

        let redirect = upstream
            .start_oauth_flow(&MemorySession::new(), &mut mcp)
            .await
            .unwrap();
        assert_eq!(origin(&Url::parse(&redirect).unwrap()), provider);
        assert_eq!(mcp.oauth_client_id.as_deref(), Some("registered-client"));
    }
}

#[tokio::test]
async fn requires_the_resource_indicator_to_be_the_mcp_url_or_a_parent_of_it() {
    for resource in ["https://api.other.example/", "https://mcp.example/other"] {
        let core = TestCore::new().await;
        let mut mcp = remote_mcp(&core, "https://mcp.example/mcp").await;
        let mut served = vec![protected_resource("https://auth.example", resource)];
        served.extend(provider_documents());
        let (upstream, calls) = documents(&core, served);

        let error = upstream
            .start_oauth_flow(&MemorySession::new(), &mut mcp)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "OAuth protected resource does not match the MCP URL"
        );
        assert!(calls.posts().is_empty());
    }

    let core = TestCore::new().await;
    let mut mcp = remote_mcp(&core, "https://mcp.example/mcp").await;
    let mut served = vec![protected_resource(
        "https://auth.example",
        "https://mcp.example",
    )];
    served.extend(provider_documents());
    let (upstream, _) = documents(&core, served);

    let redirect = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();
    let redirect = Url::parse(&redirect).unwrap();
    assert_eq!(
        query(&redirect, "resource").as_deref(),
        Some("https://mcp.example/")
    );
    assert_eq!(mcp.oauth_resource.as_deref(), Some("https://mcp.example"));
}

#[tokio::test]
async fn does_not_refresh_tokens_for_a_saved_resource_that_is_not_the_mcp() {
    let core = TestCore::new().await;
    let mut mcp = connected_mcp(&core).await;
    mcp.oauth_resource = Some("https://api.other.example/".into());
    mcp.save(&*core.db).await.unwrap();
    let (upstream, calls) = documents(
        &core,
        vec![(
            "https://auth.example/.well-known/oauth-authorization-server".to_owned(),
            authorization_server("https://auth.example"),
        )],
    );

    let error = upstream
        .refresh_oauth_access_token(&mut mcp)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "OAuth protected resource does not match the MCP URL"
    );
    assert!(calls.posts().is_empty());
}

#[tokio::test]
async fn resolves_each_discovered_name_once_for_a_flow_and_never_the_mcp_host_alone() {
    let core = TestCore::new().await;
    let mut mcp = remote_mcp(&core, "https://mcp.example/mcp").await;
    let (addresses, resolver) = resolve_names(&[
        ("mcp.example", &["203.0.113.10"]),
        ("auth.example", &["203.0.113.11"]),
    ]);
    let mut served: HashMap<String, Value> = provider_documents().into_iter().collect();
    let (url, document) = protected_resource("https://auth.example", "https://mcp.example/mcp");
    served.insert(url, document);
    let (upstream, _) = upstream_with(
        &core,
        addresses,
        Default::default(),
        move |call| match served.get(&call.url) {
            Some(document) => json_response(document.clone()),
            None => not_found(),
        },
    );

    upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();
    // The MCP host is only resolved to learn whether the MCP is itself on a
    // private network, when the first other host is met.
    assert_eq!(resolver.lookups(), ["mcp.example", "auth.example"]);
}
