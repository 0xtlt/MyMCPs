//! `tests/unit/upstream_client_identity.spec.ts`: the identity the gateway
//! shows to the hosts that only let known clients in.

mod support;

use mymcps_core::TestCore;
use mymcps_core::models::McpAuthType;
use serde_json::{Value, json};
use support::*;

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_owned(), value.to_owned())
}

/// The `initialize` request among these, the one that carries `clientInfo`.
fn initialize(calls: &Calls) -> Call {
    calls
        .all()
        .into_iter()
        .find(|call| call.json()["method"] == "initialize")
        .expect("the handshake was made")
}

fn client_info(call: &Call) -> Value {
    call.json()["params"]["clientInfo"].clone()
}

#[tokio::test]
async fn sends_codex_on_figma_requests_and_claude_code_on_strava_requests() {
    let core = TestCore::new().await;
    let figma_token = encrypt(&core, "figma-token");
    let mut figma = create_mcp(&core, |mcp| {
        mcp.name = "Figma".into();
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = figma_token;
    })
    .await;
    let mut strava = create_mcp(&core, |mcp| {
        mcp.name = "Strava".into();
        mcp.http_url = Some("https://mcp.strava.com/mcp".into());
    })
    .await;
    let mut other = create_mcp(&core, |mcp| {
        mcp.name = "Other".into();
        mcp.http_url = Some("https://mcp.example/mcp".into());
    })
    .await;
    let mcp_server = |call: &Call| mcp_answer(call, &json!([]), &json!({}));

    let (figma_upstream, figma_server) = upstream(&core, mcp_server);
    assert_eq!(
        figma_upstream.build_upstream_headers(&figma),
        [
            pair("User-Agent", "codex-mcp-client/0.0.0"),
            pair("Authorization", "Bearer figma-token"),
        ]
    );
    assert_eq!(
        figma_upstream.build_upstream_headers(&strava),
        [pair("User-Agent", "claude-code/2.1.89 (cli)")]
    );
    assert!(figma_upstream.build_upstream_headers(&other).is_empty());

    let connected = figma_upstream
        .connect_http_upstream(&mut figma)
        .await
        .unwrap();
    connected.close().await;
    let figma_initialize = initialize(&figma_server);
    assert_eq!(
        figma_initialize.header("user-agent").as_deref(),
        Some("codex-mcp-client/0.0.0")
    );
    assert_eq!(
        figma_initialize.header("authorization").as_deref(),
        Some("Bearer figma-token")
    );
    assert_eq!(
        serde_json::to_string(&client_info(&figma_initialize)).unwrap(),
        r#"{"name":"codex-mcp-client","version":"0.0.0","title":"Codex"}"#
    );
    assert!(figma_server.len() > 0);
    for request in figma_server.all() {
        assert_eq!(
            request.header("user-agent").as_deref(),
            Some("codex-mcp-client/0.0.0")
        );
    }

    let (strava_upstream, strava_server) = upstream(&core, mcp_server);
    let connected = strava_upstream
        .connect_http_upstream(&mut strava)
        .await
        .unwrap();
    connected.close().await;
    let strava_initialize = initialize(&strava_server);
    assert_eq!(
        strava_initialize.header("user-agent").as_deref(),
        Some("claude-code/2.1.89 (cli)")
    );
    assert_eq!(strava_initialize.header("authorization"), None);
    assert_eq!(
        client_info(&strava_initialize),
        json!({ "name": "claude-code", "version": "2.1.89", "title": "Claude Code" })
    );

    let (other_upstream, other_server) = upstream(&core, mcp_server);
    let connected = other_upstream
        .connect_http_upstream(&mut other)
        .await
        .unwrap();
    connected.close().await;
    let other_initialize = initialize(&other_server);
    // No identity is borrowed: the HTTP client names the gateway itself.
    assert_eq!(other_initialize.header("user-agent"), None);
    assert_eq!(
        client_info(&other_initialize),
        json!({ "name": "mymcps-gateway", "version": mymcps_core::VERSION })
    );
}

#[tokio::test]
async fn a_custom_header_named_like_the_identity_header_replaces_or_joins_it() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |call| mcp_answer(call, &json!([]), &json!({})));
    let value = encrypt(&core, "custom-agent/1.0");
    let mut same_spelling = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.auth_type = McpAuthType::Header;
        mcp.auth_header_name = Some("User-Agent".into());
        mcp.auth_header_value = value.clone();
    })
    .await;
    assert_eq!(
        upstream.build_upstream_headers(&same_spelling),
        [pair("User-Agent", "custom-agent/1.0")]
    );
    // Nothing is sent of a header whose value cannot be read any more.
    same_spelling.auth_header_value = Some("not ciphertext".into());
    assert_eq!(
        upstream.build_upstream_headers(&same_spelling),
        [pair("User-Agent", "codex-mcp-client/0.0.0")]
    );

    let other_spelling = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.auth_type = McpAuthType::Header;
        mcp.auth_header_name = Some("user-agent".into());
        mcp.auth_header_value = value;
    })
    .await;
    assert_eq!(
        upstream.build_upstream_headers(&other_spelling),
        [
            pair("User-Agent", "codex-mcp-client/0.0.0"),
            pair("user-agent", "custom-agent/1.0"),
        ]
    );
}
