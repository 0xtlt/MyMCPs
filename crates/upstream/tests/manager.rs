//! Listing, calling and testing HTTP MCPs through the manager, with the
//! credentials saved for them. The cases about what is sent upstream come
//! from `tests/functional/hardening_upstream_mcps.spec.ts` and
//! `tests/unit/security.spec.ts`, here without the pages that edit an MCP.

mod support;

use std::sync::Arc;

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus};
use mymcps_net::CannedResponse;
use mymcps_upstream::approvals::APPROVAL_NOTE;
use mymcps_upstream::{NamespacedTool, Upstream, UpstreamError, UpstreamTool};
use serde_json::{Value, json};
use support::*;

fn tools() -> Value {
    json!([
        {
            "name": "search",
            "title": "Search",
            "description": "Searches the workspace.",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
            "annotations": { "readOnlyHint": true },
        },
        { "name": "delete_page", "inputSchema": { "type": "object" } },
    ])
}

/// An MCP server that lists `tools()` and answers every call with `result`.
fn mcp_server(core: &TestCore, result: Value) -> (Arc<Upstream>, Calls) {
    upstream(core, move |call| mcp_answer(call, &tools(), &result))
}

fn methods(calls: &Calls) -> Vec<String> {
    calls
        .all()
        .iter()
        .map(|call| match call.json()["method"].as_str() {
            Some(method) => format!("{} {method}", call.method),
            None => call.method.clone(),
        })
        .collect()
}

#[tokio::test]
async fn lists_the_tools_of_an_http_mcp_as_name_description_and_input_schema() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core, json!({}));
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into())
    })
    .await;

    let listed = upstream.probe(&mut mcp).await.unwrap();

    assert_eq!(
        listed,
        [
            UpstreamTool {
                name: "search".into(),
                description: Some("Searches the workspace.".into()),
                input_schema: json!({ "type": "object", "properties": { "q": { "type": "string" } } }),
            },
            UpstreamTool {
                name: "delete_page".into(),
                description: None,
                input_schema: json!({ "type": "object" }),
            },
        ]
    );
    assert_eq!(
        serde_json::to_string(&listed[1]).unwrap(),
        r#"{"name":"delete_page","inputSchema":{"type":"object"}}"#
    );
    let sent = methods(&calls);
    assert_eq!(
        sent[..3],
        [
            "POST initialize",
            "POST notifications/initialized",
            "POST tools/list"
        ]
    );
    // Closing sends nothing: the session is not terminated.
    assert!(sent[3..].iter().all(|request| request == "GET"), "{sent:?}");
}

#[tokio::test]
async fn names_every_tool_after_its_mcp_and_skips_an_mcp_that_cannot_be_listed() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |call| {
        if call.hostname() == "broken.example" {
            return status(500).body("upstream exploded");
        }
        mcp_answer(call, &tools(), &json!({}))
    });
    let notes = create_mcp(&core, |mcp| {
        mcp.name = "Team Notes".into();
        mcp.http_url = Some("https://notes.example/mcp".into());
        mcp.tool_approvals = Some(r#"{"delete_page":"ask"}"#.into());
    })
    .await;
    let broken = create_mcp(&core, |mcp| {
        mcp.name = "Broken".into();
        mcp.http_url = Some("https://broken.example/mcp".into());
    })
    .await;
    let wiki = create_mcp(&core, |mcp| {
        mcp.name = "Wiki".into();
        mcp.http_url = Some("https://wiki.example/mcp".into());
    })
    .await;
    let (notes_id, wiki_id) = (notes.id, wiki.id);

    let listed = upstream
        .list_namespaced_tools(&mut [notes, broken, wiki])
        .await;

    assert_eq!(
        listed
            .iter()
            .map(|tool| (tool.namespaced_name.as_str(), tool.mcp_id))
            .collect::<Vec<_>>(),
        [
            ("team-notes__search", notes_id),
            ("team-notes__delete_page", notes_id),
            ("wiki__search", wiki_id),
            ("wiki__delete_page", wiki_id),
        ]
    );
    assert_eq!(
        listed[1],
        NamespacedTool {
            name: "delete_page".into(),
            // Agents are told which tools wait for a person.
            description: Some(APPROVAL_NOTE.into()),
            input_schema: json!({ "type": "object" }),
            mcp_id: notes_id,
            mcp_slug: "team-notes".into(),
            namespaced_name: "team-notes__delete_page".into(),
        }
    );
    assert_eq!(
        listed[0].description.as_deref(),
        Some("Searches the workspace.")
    );
    assert_eq!(listed[3].description, None);
    assert_eq!(
        serde_json::to_value(&listed[2]).unwrap(),
        json!({
            "name": "search",
            "description": "Searches the workspace.",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
            "mcpId": wiki_id,
            "mcpSlug": "wiki",
            "namespacedName": "wiki__search",
        })
    );
}

#[tokio::test]
async fn calls_a_tool_and_returns_what_the_mcp_answered() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(
        &core,
        json!({ "content": [{ "type": "text", "text": "3 pages" }], "structuredContent": { "pages": 3 } }),
    );
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into())
    })
    .await;

    let arguments = json!({ "q": "roadmap", "limit": 3 }).as_object().cloned();
    let result = upstream
        .call_tool(&mut mcp, "search", arguments)
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({ "content": [{ "type": "text", "text": "3 pages" }], "structuredContent": { "pages": 3 } })
    );
    let call = calls
        .all()
        .into_iter()
        .find(|call| call.json()["method"] == "tools/call")
        .unwrap();
    assert_eq!(
        call.json()["params"],
        json!({ "name": "search", "arguments": { "q": "roadmap", "limit": 3 } })
    );

    // A call without arguments is made with an empty object.
    calls.clear();
    upstream.call_tool(&mut mcp, "search", None).await.unwrap();
    let call = calls
        .all()
        .into_iter()
        .find(|call| call.json()["method"] == "tools/call")
        .unwrap();
    assert_eq!(
        call.json()["params"],
        json!({ "name": "search", "arguments": {} })
    );
}

#[tokio::test]
async fn a_tool_that_fails_answers_with_a_result_and_a_refused_call_with_an_error() {
    let core = TestCore::new().await;
    let (upstream, _) = mcp_server(
        &core,
        json!({ "content": [{ "type": "text", "text": "No such page" }], "isError": true }),
    );
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into())
    })
    .await;
    let result = upstream.call_tool(&mut mcp, "search", None).await.unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(result["content"][0]["text"], "No such page");

    let (refusing, _) = support::upstream(&core, |call| {
        let message = call.json();
        if message["method"] == "tools/call" {
            return json_response(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": { "code": -32602, "message": "Unknown tool: nope" },
            }));
        }
        mcp_answer(call, &tools(), &json!({}))
    });
    let error = refusing
        .call_tool(&mut mcp, "nope", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MCP error -32602: Unknown tool: nope");
    assert!(matches!(&error, UpstreamError::Client(client) if client.mcp_code() == Some(-32602)));
    assert!(!error.is_unauthorized());
}

// Re-pointing an MCP from the registry: what reaches the origin it points to.

#[tokio::test]
async fn sends_the_saved_credentials_of_an_mcp_with_every_request_to_it() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core, json!({}));

    let header_value = encrypt(&core, "saved-header-value");
    let mut header = create_mcp(&core, |mcp| {
        mcp.auth_type = McpAuthType::Header;
        mcp.http_url = Some("https://old.example/v2/mcp".into());
        mcp.auth_header_name = Some("X-Api-Key".into());
        mcp.auth_header_value = header_value;
    })
    .await;
    upstream.test_and_update_status(&mut header).await.unwrap();
    assert_eq!(header.status, McpStatus::Ready);
    assert!(calls.len() > 0);
    for request in calls.all() {
        assert_eq!(request.url, "https://old.example/v2/mcp");
        assert_eq!(
            request.header("x-api-key").as_deref(),
            Some("saved-header-value")
        );
        assert_eq!(request.header("authorization"), None);
    }

    calls.clear();
    let bearer_token = encrypt(&core, "saved-bearer");
    let mut bearer = create_mcp(&core, |mcp| {
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_bearer = bearer_token;
        // Left from an earlier configuration: not what this MCP signs in with.
        mcp.auth_header_name = Some("X-Api-Key".into());
        mcp.auth_header_value = encrypt(&core, "saved-header-value");
        mcp.oauth_access_token = encrypt(&core, "saved-oauth-token");
    })
    .await;
    upstream.probe(&mut bearer).await.unwrap();
    for request in calls.all() {
        assert_eq!(
            request.header("authorization").as_deref(),
            Some("Bearer saved-bearer")
        );
        assert_eq!(request.header("x-api-key"), None);
    }

    // An MCP whose saved token was dropped is probed without one.
    calls.clear();
    bearer.auth_bearer = None;
    bearer.http_url = Some("https://attacker.example/mcp".into());
    upstream.probe(&mut bearer).await.unwrap();
    assert!(calls.len() > 0);
    for request in calls.all() {
        assert_eq!(request.hostname(), "attacker.example");
        assert_eq!(request.header("authorization"), None);
    }
}

// From `tests/unit/security.spec.ts`: endpoint query and URL credentials are
// allowed, and credentials are not forwarded to another origin.

#[tokio::test]
async fn keeps_the_query_of_an_endpoint_and_sends_its_url_credentials_as_basic_authentication() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core, json!({}));
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some(
            "https://user:p%40ss@example.test/mcp?api_key=provider-required&code=fr&key=primary"
                .into(),
        );
    })
    .await;

    upstream.probe(&mut mcp).await.unwrap();

    assert!(calls.len() > 0);
    for request in calls.all() {
        assert_eq!(
            request.url,
            "https://example.test/mcp?api_key=provider-required&code=fr&key=primary"
        );
        assert_eq!(
            request.header("authorization").as_deref(),
            Some("Basic dXNlcjpwQHNz")
        );
    }
}

#[tokio::test]
async fn follows_a_redirect_of_the_endpoint_within_its_origin_and_stops_at_another_origin() {
    let core = TestCore::new().await;
    let token = encrypt(&core, "saved-bearer");
    let (upstream, calls) = upstream(&core, |call| match call.path().as_str() {
        "/mcp" => status(307).header("Location", "/canonical").unwrap(),
        "/canonical" => mcp_answer(call, &tools(), &json!({})),
        "/moved" => status(307)
            .header("Location", "https://attacker.example/collect")
            .unwrap(),
        _ => not_found(),
    });

    let mut canonical = create_mcp(&core, |mcp| {
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://trusted.example/mcp".into());
        mcp.auth_bearer = token.clone();
    })
    .await;
    assert_eq!(upstream.probe(&mut canonical).await.unwrap().len(), 2);

    calls.clear();
    let mut moved = create_mcp(&core, |mcp| {
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://trusted.example/moved".into());
        mcp.auth_bearer = token;
    })
    .await;
    upstream.test_and_update_status(&mut moved).await.unwrap();

    assert_eq!(moved.status, McpStatus::Error);
    assert_eq!(
        moved.last_error.as_deref(),
        Some("MCP endpoint redirected to a different origin")
    );
    assert!(!moved.oauth_required);
    assert!(calls.len() > 0);
    assert!(
        calls
            .all()
            .iter()
            .all(|call| call.hostname() == "trusted.example")
    );
}

// Testing a connection

#[tokio::test]
async fn saves_a_healthy_connection_without_undoing_what_changed_meanwhile() {
    let core = TestCore::new().await;
    let (upstream, _) = mcp_server(&core, json!({}));
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("stale".into());
        mcp.oauth_required = true;
    })
    .await;
    // Another request renamed the MCP after this one read it.
    let mut other = find_mcp(&core, mcp.id).await;
    other.name = "Renamed".into();
    other.save(&*core.db).await.unwrap();

    upstream.test_and_update_status(&mut mcp).await.unwrap();

    assert_eq!(mcp.status, McpStatus::Ready);
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
    assert!(!saved.oauth_required);
    assert_eq!(saved.name, "Renamed");
}

fn unauthorized_after_the_handshake(call: &Call) -> CannedResponse {
    if call.json()["method"] == "tools/list" {
        return status(401).body("token expired");
    }
    mcp_answer(call, &tools(), &json!({}))
}

#[tokio::test]
async fn tells_a_token_refused_after_the_handshake_from_a_missing_authorization() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, unauthorized_after_the_handshake);

    let mut connected = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.oauth_access_token = encrypt(&core, "access-token");
    })
    .await;
    let error = upstream.probe(&mut connected).await.unwrap_err();
    assert!(error.is_unauthorized());
    assert!(matches!(error, UpstreamError::Client(_)));
    assert_eq!(
        error.to_string(),
        "Streamable HTTP error: Error POSTing to endpoint: token expired"
    );
    upstream
        .test_and_update_status(&mut connected)
        .await
        .unwrap();
    assert_eq!(connected.status, McpStatus::Error);
    assert_eq!(
        connected.last_error.as_deref(),
        Some("OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP.")
    );
    assert!(connected.oauth_required);

    let mut waiting = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
    })
    .await;
    upstream.test_and_update_status(&mut waiting).await.unwrap();
    assert_eq!(waiting.status, McpStatus::Draft);
    assert_eq!(
        waiting.last_error.as_deref(),
        Some("OAuth authorization required")
    );
    assert!(waiting.oauth_required);

    // A static credential that is refused is an error to fix in the form.
    let mut header = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.auth_type = McpAuthType::Header;
    })
    .await;
    upstream.test_and_update_status(&mut header).await.unwrap();
    assert_eq!(header.status, McpStatus::Error);
    assert_eq!(
        header.last_error.as_deref(),
        Some("Streamable HTTP error: Error POSTing to endpoint: token expired")
    );
    assert!(!header.oauth_required);
}

#[tokio::test]
async fn says_what_a_server_answered_with_its_401_without_the_secrets_of_the_mcp() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |call| {
        let echoed = call.header("authorization").unwrap_or_default();
        status(401)
            .header("WWW-Authenticate", "Digest   realm=\"mcp\",\tqop=\"auth\"")
            .unwrap()
            .body(format!(
                "<html>\n  <body>refused   the\trequest</body>\n</html>\n<!-- {echoed} -->"
            ))
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.auth_type = McpAuthType::Header;
        mcp.auth_header_name = Some("X-Api-Key".into());
    })
    .await;

    // What the server said, on one line.
    let error = upstream.probe(&mut mcp).await.unwrap_err();
    assert!(error.is_unauthorized());
    assert!(matches!(error, UpstreamError::Unauthorized(_)));
    assert_eq!(
        error.to_string(),
        "MCP server returned HTTP 401. Response: <html> <body>refused the request</body> </html> <!-- --> | WWW-Authenticate: Digest realm=\"mcp\", qop=\"auth\""
    );

    // A server that echoes the credential it refused.
    mcp.auth_type = McpAuthType::Bearer;
    mcp.auth_bearer = encrypt(&core, "sk-live-very-secret");
    let error = upstream.probe(&mut mcp).await.unwrap_err();
    assert!(matches!(error, UpstreamError::Unauthorized(_)));
    let message = error.to_string();
    assert!(
        message.starts_with(
            "MCP server returned HTTP 401. Response: <html> <body>refused the request</body>"
        ),
        "{message}"
    );
    assert!(message.contains("[REDACTED]"), "{message}");
    assert!(!message.contains("sk-live-very-secret"), "{message}");

    // A 401 that says nothing.
    let (silent, _) = support::upstream(&core, |_| status(401));
    let error = silent.probe(&mut mcp).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP server returned HTTP 401 Unauthorized."
    );

    // A long answer is cut.
    let (verbose, _) = support::upstream(&core, |_| status(401).body("x".repeat(5000)));
    let error = verbose.probe(&mut mcp).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "MCP server returned HTTP 401. Response: {}…",
            "x".repeat(239)
        )
    );
}

#[tokio::test]
async fn fails_before_connecting_when_the_mcp_cannot_be_reached_as_configured() {
    let core = TestCore::new().await;
    let (upstream, calls) = mcp_server(&core, json!({}));

    let mut without_url = create_mcp(&core, |_| {}).await;
    without_url.http_url = None;
    assert_eq!(
        upstream
            .probe(&mut without_url)
            .await
            .unwrap_err()
            .to_string(),
        "HTTP MCP is missing a URL"
    );
    without_url.http_url = Some("ftp://files.example/mcp".into());
    assert_eq!(
        upstream
            .probe(&mut without_url)
            .await
            .unwrap_err()
            .to_string(),
        "MCP URL must use HTTP or HTTPS"
    );

    // A saved token no header can carry: the error names it, and what is
    // saved for the administrator does not.
    let mut unusable = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = encrypt(&core, "line\nbreak");
    })
    .await;
    assert_eq!(
        upstream.probe(&mut unusable).await.unwrap_err().to_string(),
        "Headers.append: \"Bearer line\nbreak\" is an invalid header value."
    );
    upstream
        .test_and_update_status(&mut unusable)
        .await
        .unwrap();
    assert_eq!(unusable.status, McpStatus::Error);
    assert_eq!(
        unusable.last_error.as_deref(),
        Some("Headers.append: \"Bearer [REDACTED]\" is an invalid header value.")
    );
    assert_eq!(calls.len(), 0);
}

/// The futures of the upstream are awaited by request handlers, which may
/// move between threads. Compiling this is the test.
#[allow(dead_code)]
fn every_operation_can_be_awaited_from_a_request_handler(
    upstream: &Upstream,
    mcp: &mut Mcp,
    session: &MemorySession,
    oauth: &mymcps_upstream::OauthSession,
) {
    fn assert_send<T: Send>(_: T) {}

    assert_send(upstream.probe(mcp));
    assert_send(upstream.call_tool(mcp, "tool", None));
    assert_send(upstream.list_namespaced_tools(std::slice::from_mut(mcp)));
    assert_send(upstream.test_and_update_status(mcp));
    assert_send(upstream.connect_http_upstream(mcp));
    assert_send(upstream.start_oauth_flow(session, mcp));
    assert_send(upstream.exchange_authorization_code(mcp, oauth, "code", None));
    assert_send(upstream.refresh_oauth_access_token(mcp));
    assert_send(upstream.call_builtin_tool(mcp, "tool", None));
    assert_send(upstream.describe_builtin_call(mcp, "tool", None));
    assert_send(upstream.download_builtin_file(mcp, Value::Null));
    assert_send(upstream.builtin_upload_target(mcp, Value::Null));
    assert_send(upstream.verify_builtin(mcp));
    assert_send(upstream.update_mcp_to_latest(mcp));
    assert_send(upstream.update_latest_tracking_mcps());
}
