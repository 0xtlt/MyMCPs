//! `tests/functional/hardening_gateway_mcp.spec.ts` for what goes through
//! `/mcp`, and the HTTP side of the endpoint that no spec of the Node app
//! pinned but its stack decided: the body is read before the access token,
//! what a body that is not one message is answered, and the header checks
//! of the MCP server.
//!
//! The Logs page filter of that spec is in `logs_analytics.rs`, the CORS of
//! the other protocol endpoints in `gateway_oauth_server.rs`, and the
//! redaction of a failed log write in the gateway crate, which can read
//! the server log.

#[path = "support/gateway.rs"]
mod support;

use std::collections::BTreeSet;

use axum::body::Body;
use futures::StreamExt;
use http::{Method, StatusCode};
use mymcps_core::models::{CallErrorCategory, ScopeMode};
use mymcps_gateway::rate_limiter::GATEWAY_REQUESTS_PER_MINUTE;
use mymcps_web::testing::factories::create_admin;
use serde_json::{Value, json};

use support::*;

fn rpc(body: Value) -> Value {
    let mut message = json!({ "jsonrpc": "2.0" });
    for (key, value) in body.as_object().unwrap() {
        message[key] = value.clone();
    }
    message
}

fn tool_call(name: &str, arguments: Option<Value>) -> Value {
    let mut params = json!({ "name": name });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    rpc(json!({ "id": 7, "method": "tools/call", "params": params }))
}

fn initialize() -> Value {
    rpc(json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "hardening-test", "version": "1.0.0" },
        },
    }))
}

/// Every MCP has one tool, `echo`, which answers `ok`.
fn echo(message: &UpstreamMessage) -> Reply {
    Reply::Result(match message.method.as_str() {
        "tools/list" => {
            json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] })
        }
        _ => json!({ "content": [{ "type": "text", "text": "ok" }] }),
    })
}

/// A gateway and the token of an agent that may use every MCP.
async fn gateway_with_token() -> (TestGateway, String) {
    let gateway = TestGateway::new(echo).await;
    let admin = create_admin(&gateway).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    (gateway, plaintext)
}

// hardening: gateway requests

#[tokio::test]
async fn contacts_upstreams_in_eager_mode_only_to_list_tools_or_to_call_one() {
    let gateway = TestGateway::new(echo).await;
    let admin = create_admin(&gateway).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    create_http_mcp(&gateway, admin.id, "Calendar", "calendar").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let initialized = gateway.post_message(&plaintext, initialize(), &[]).await;
    assert_eq!(initialized.status, StatusCode::OK);
    let notified = gateway
        .post_message(
            &plaintext,
            rpc(json!({ "method": "notifications/initialized" })),
            &[],
        )
        .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    assert_eq!(notified.text(), "");
    let pinged = gateway
        .post_message(&plaintext, rpc(json!({ "id": 2, "method": "ping" })), &[])
        .await;
    assert_eq!(pinged.status, StatusCode::OK);
    assert_eq!(result_of(&pinged), json!({}));
    assert!(gateway.upstreams.requests().is_empty());

    let called = gateway
        .post_message(&plaintext, tool_call("issues__echo", None), &[])
        .await;
    assert_eq!(called.status, StatusCode::OK);
    assert!(called.text().contains("ok"));
    let methods = gateway.upstreams.methods();
    assert!(methods.contains(&"tools/call".to_owned()));
    assert!(!methods.contains(&"tools/list".to_owned()));
    assert!(
        gateway
            .upstreams
            .hosts()
            .iter()
            .all(|host| host == "issues.example")
    );

    gateway.upstreams.clear();
    let listed = gateway
        .post_message(
            &plaintext,
            rpc(json!({ "id": 3, "method": "tools/list" })),
            &[],
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert!(listed.text().contains("issues__echo"));
    assert!(listed.text().contains("calendar__echo"));
    assert_eq!(
        gateway
            .upstreams
            .hosts()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        ["calendar.example", "issues.example"]
    );
    gateway.flush().await;
}

#[tokio::test]
async fn limits_the_requests_of_one_access_token_without_affecting_another() {
    let gateway = TestGateway::offline().await;
    let admin = create_admin(&gateway).await;
    let busy = create_access_token(&gateway, admin.id, ScopeMode::All, &[]).await;
    let other = create_access_token(&gateway, admin.id, ScopeMode::All, &[]).await;
    for _ in 0..GATEWAY_REQUESTS_PER_MINUTE - 1 {
        gateway
            .state
            .gateway
            .rate_limiter
            .increment(&format!("mcp:{}", busy.token.id))
            .await
            .unwrap();
    }

    let last = gateway
        .post_message(&busy.plaintext, initialize(), &[])
        .await;
    assert_eq!(last.status, StatusCode::OK);

    let refused = gateway
        .post_message(&busy.plaintext, initialize(), &[])
        .await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.json(),
        json!({
            "error": "rate_limited",
            "message": "Too many requests for this access token",
        })
    );
    let retry_after: u64 = refused.header("retry-after").unwrap().parse().unwrap();
    assert!(retry_after > 0 && retry_after <= 60);
    assert_eq!(refused.header("www-authenticate"), None);

    let unaffected = gateway
        .post_message(&other.plaintext, initialize(), &[])
        .await;
    assert_eq!(unaffected.status, StatusCode::OK);
}

// hardening: MCP call log

#[tokio::test]
async fn stores_only_a_well_formed_slug_and_bounded_names_for_refused_calls() {
    let (gateway, plaintext) = gateway_with_token().await;
    let long_address = "a".repeat(300);
    let forwarded_for = ("x-forwarded-for", long_address.as_str());

    for name in [
        format!("{}__tool", "x".repeat(4000)),
        "Not A Slug__tool".to_owned(),
        "missing-mcp__tool".to_owned(),
    ] {
        gateway
            .post_message(&plaintext, tool_call(&name, None), &[forwarded_for])
            .await;
    }
    gateway
        .post_message(
            &plaintext,
            tool_call(
                "call_tool",
                Some(json!({ "mcp": "Not A Slug", "tool": "tool" })),
            ),
            &[forwarded_for, ("x-mymcps-tool-mode", "lazy")],
        )
        .await;
    gateway.flush().await;

    let logs = gateway.call_logs().await;
    assert_eq!(
        logs.iter()
            .map(|log| log.error_category)
            .collect::<Vec<_>>(),
        [Some(CallErrorCategory::DisallowedMcp); 4]
    );
    assert_eq!(
        logs.iter()
            .map(|log| log.mcp_slug.as_deref())
            .collect::<Vec<_>>(),
        [None, None, Some("missing-mcp"), None]
    );
    assert_eq!(logs[0].requested_tool_name.len(), 512);
    // A forwarded address that is not one is not believed: the record holds
    // where the connection came from.
    for log in &logs {
        assert_eq!(log.caller_ip.as_deref(), Some("127.0.0.1"));
    }
}

#[tokio::test]
async fn believes_a_forwarded_address_only_from_a_proxy_it_trusts() {
    let (gateway, plaintext) = gateway_with_token().await;

    // The tests connect from loopback, which `TRUST_PROXY` trusts by default.
    gateway
        .post_message(
            &plaintext,
            tool_call("invalid", None),
            &[("x-forwarded-for", "198.51.100.7, 192.0.2.10")],
        )
        .await;
    gateway.flush().await;
    assert_eq!(
        gateway.call_logs().await[0].caller_ip.as_deref(),
        Some("192.0.2.10")
    );

    let distrustful = TestGateway::with(
        |config| config.trust_proxy = mymcps_core::client_ip::TrustProxy::None,
        |fetcher| fetcher,
        echo,
    )
    .await;
    let admin = create_admin(&distrustful).await;
    let plaintext = create_access_token(&distrustful, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    distrustful
        .post_message(
            &plaintext,
            tool_call("invalid", None),
            &[("x-forwarded-for", "192.0.2.10")],
        )
        .await;
    distrustful.flush().await;
    assert_eq!(
        distrustful.call_logs().await[0].caller_ip.as_deref(),
        Some("127.0.0.1")
    );
}

// hardening: protocol endpoint CORS

#[tokio::test]
async fn answers_other_origins_without_credentials_and_without_echoing_them() {
    let gateway = TestGateway::offline().await;
    create_admin(&gateway).await;
    let origin = "https://agent.example";

    let preflight = gateway
        .mcp_request(
            Method::OPTIONS,
            &[
                ("origin", origin),
                ("access-control-request-method", "POST"),
                (
                    "access-control-request-headers",
                    "authorization,content-type",
                ),
            ],
            None,
        )
        .await;
    assert_eq!(preflight.status, StatusCode::NO_CONTENT);
    assert_eq!(preflight.header("access-control-allow-origin"), Some("*"));
    assert_eq!(preflight.header("access-control-allow-credentials"), None);
    assert!(
        preflight
            .header("access-control-allow-headers")
            .unwrap()
            .contains("authorization")
    );

    let challenged = gateway
        .mcp_request(Method::GET, &[("origin", origin)], None)
        .await;
    assert_eq!(challenged.status, StatusCode::UNAUTHORIZED);
    assert_eq!(challenged.header("access-control-allow-origin"), Some("*"));
    assert_eq!(challenged.header("access-control-allow-credentials"), None);
    // A page of another origin can read where to sign in.
    assert_eq!(
        challenged.header("access-control-expose-headers"),
        Some("WWW-Authenticate")
    );

    let session_page = gateway.get("/login").header("origin", origin).send().await;
    assert_eq!(session_page.header("access-control-allow-origin"), None);
    assert_eq!(
        session_page.header("access-control-allow-credentials"),
        None
    );
}

// The HTTP side of the endpoint

#[tokio::test]
async fn reads_the_body_before_the_access_token() {
    let gateway = TestGateway::offline().await;
    let post = |body: Vec<u8>, content_type: &'static str| {
        gateway
            .mcp(
                Method::POST,
                &[("accept", "application/json, text/event-stream")],
            )
            .raw_body(body, content_type)
            .send()
    };

    let not_json = post(b"{not json".to_vec(), "application/json").await;
    assert_eq!(not_json.status, StatusCode::BAD_REQUEST);
    assert_eq!(not_json.header("www-authenticate"), None);

    for scalar in ["\"text\"", "42", "true"] {
        let not_a_message = post(scalar.as_bytes().to_vec(), "application/json").await;
        assert_eq!(
            not_a_message.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{scalar}"
        );
    }

    let mut large =
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{\"pad\":\"".to_vec();
    large.resize(1024 * 1024 + 1, b'x');
    let too_large = post(large, "application/json").await;
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(too_large.text(), "request entity too large");

    // A body the server can read is then answered for its missing token,
    // whatever it holds.
    for (body, content_type) in [
        (&b"{}"[..], "application/json"),
        (&b"a=1"[..], "application/x-www-form-urlencoded"),
        (&b"{not json"[..], "text/plain"),
    ] {
        let unauthorized = post(body.to_vec(), content_type).await;
        assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
    }
    // A GET has no body to read.
    let get = gateway
        .mcp(Method::GET, &[])
        .raw_body(&b"{not json"[..], "application/json")
        .send()
        .await;
    assert_eq!(get.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn answers_as_the_stateless_server_of_an_agent() {
    let (gateway, plaintext) = gateway_with_token().await;
    let authorization = format!("Bearer {plaintext}");

    let initialized = gateway.post_message(&plaintext, initialize(), &[]).await;
    assert_eq!(
        initialized.header("content-type"),
        Some("text/event-stream")
    );
    assert_eq!(
        initialized.header("cache-control"),
        Some("no-cache, no-transform")
    );
    assert_eq!(initialized.header("mcp-session-id"), None);
    assert_eq!(
        result_of(&initialized),
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mymcps", "version": mymcps_core::VERSION },
        })
    );

    // A method the gateway does not have is a JSON-RPC error, not a failure.
    let unknown = gateway
        .post_message(
            &plaintext,
            rpc(json!({ "id": 4, "method": "resources/list" })),
            &[],
        )
        .await;
    assert_eq!(unknown.status, StatusCode::OK);
    assert_eq!(
        rpc_of(&unknown),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": { "code": -32601, "message": "Method not found" },
        })
    );

    // A batch is not read as one: the Node app handed the MCP server the
    // list as an object, which is no JSON-RPC message.
    let batch = gateway
        .post_message(
            &plaintext,
            json!([initialize(), tool_call("x__y", None)]),
            &[],
        )
        .await;
    assert_eq!(batch.status, StatusCode::BAD_REQUEST);
    assert_eq!(batch.header("content-type"), Some("application/json"));
    assert_eq!(
        batch.json(),
        json!({
            "jsonrpc": "2.0",
            "error": { "code": -32700, "message": "Parse error: Invalid JSON-RPC message" },
            "id": null,
        })
    );
    let empty = gateway.post_message(&plaintext, json!({}), &[]).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let one_type = gateway
        .mcp_request(
            Method::POST,
            &[
                ("authorization", &authorization),
                ("accept", "application/json"),
            ],
            Some(initialize()),
        )
        .await;
    assert_eq!(one_type.status, StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        one_type.json()["error"]["message"],
        "Not Acceptable: Client must accept both application/json and text/event-stream"
    );

    let not_json = gateway
        .mcp(
            Method::POST,
            &[
                ("authorization", &authorization),
                ("accept", "application/json, text/event-stream"),
            ],
        )
        .raw_body(initialize().to_string(), "text/plain")
        .send()
        .await;
    assert_eq!(not_json.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        not_json.json()["error"]["message"],
        "Unsupported Media Type: Content-Type must be application/json"
    );

    let no_stream = gateway
        .mcp_request(
            Method::GET,
            &[("authorization", &authorization), ("accept", "text/html")],
            None,
        )
        .await;
    assert_eq!(no_stream.status, StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        no_stream.json()["error"]["message"],
        "Not Acceptable: Client must accept text/event-stream"
    );

    let other_version = gateway
        .post_message(
            &plaintext,
            rpc(json!({ "id": 5, "method": "ping" })),
            &[("mcp-protocol-version", "1999-01-01")],
        )
        .await;
    assert_eq!(other_version.status, StatusCode::BAD_REQUEST);
    assert!(
        other_version.json()["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Bad Request: Unsupported protocol version: 1999-01-01")
    );
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn reads_the_query_string_over_the_body_as_the_node_app_did() {
    let (gateway, plaintext) = gateway_with_token().await;
    let post = |path: &'static str| {
        gateway.post_to(path, &plaintext, rpc(json!({ "id": 5, "method": "ping" })))
    };

    let renamed = post("/mcp?id=from-the-query").await;
    assert_eq!(renamed.status, StatusCode::OK);
    assert_eq!(rpc_of(&renamed)["id"], "from-the-query");

    // A message has no other member than its own.
    let surplus = post("/mcp?surplus=1").await;
    assert_eq!(surplus.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        surplus.json()["error"]["message"],
        "Parse error: Invalid JSON-RPC message"
    );
}

#[tokio::test]
async fn hands_the_mcp_the_arguments_as_the_body_parser_left_them() {
    let gateway = TestGateway::new(echo).await;
    let admin = create_admin(&gateway).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    // Every string of a body is trimmed and an empty one is null, for
    // `/mcp` as for the pages.
    let called = gateway
        .post_message(
            &plaintext,
            tool_call(
                " issues__echo ",
                Some(json!({
                    "title": "  padded  ",
                    "note": "",
                    "lines": ["", " a ", { "deep": "   " }],
                    "count": 0,
                    "big": 9007199254740993_i64,
                })),
            ),
            &[],
        )
        .await;
    assert_eq!(called.status, StatusCode::OK, "{}", called.text());
    gateway.flush().await;

    assert_eq!(
        gateway.upstreams.calls(),
        [json!({
            "name": "echo",
            "arguments": {
                "title": "padded",
                "note": null,
                "lines": [null, "a", { "deep": null }],
                "count": 0,
                "big": 9007199254740993_i64,
            },
        })]
    );
}

#[tokio::test]
async fn sends_a_whole_answer_with_its_length_and_a_pending_one_as_it_comes() {
    let gateway = TestGateway::new(echo).await;
    let admin = create_admin(&gateway).await;
    create_http_mcp(&gateway, admin.id, "Issues", "issues").await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let refused = gateway.mcp_request(Method::GET, &[], None).await;
    assert_eq!(
        refused.header("content-length"),
        Some(refused.body.len().to_string().as_str())
    );
    let initialized = gateway.post_message(&plaintext, initialize(), &[]).await;
    assert_eq!(
        initialized.header("content-length"),
        Some(initialized.body.len().to_string().as_str())
    );
    let notified = gateway
        .post_message(
            &plaintext,
            rpc(json!({ "method": "notifications/initialized" })),
            &[],
        )
        .await;
    assert_eq!(notified.header("content-length"), Some("0"));

    // The answer of a tool call arrives on the stream when the MCP gave it.
    let called = gateway
        .post_message(&plaintext, tool_call("issues__echo", None), &[])
        .await;
    assert_eq!(called.header("content-length"), None);
    assert_eq!(
        result_of(&called),
        json!({ "content": [{ "type": "text", "text": "ok" }] })
    );
    gateway.flush().await;
}

#[tokio::test]
async fn opens_a_stream_on_get_that_only_the_agent_ends() {
    let (gateway, plaintext) = gateway_with_token().await;
    let request = http::Request::builder()
        .method(Method::GET)
        .uri("/mcp")
        .header("authorization", format!("Bearer {plaintext}"))
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();

    let response = send_unread(&gateway, request).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    // A stateless server has nothing to say on it, and does not end it.
    let mut body = response.into_body().into_data_stream();
    let silent = tokio::time::timeout(std::time::Duration::from_millis(100), body.next()).await;
    assert!(silent.is_err());
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn routes_get_head_and_post_and_no_other_method() {
    let (gateway, plaintext) = gateway_with_token().await;
    let authorization = format!("Bearer {plaintext}");

    // HEAD goes where GET goes, as with the router of the Node app: the
    // token is asked for first, and the MCP server has no answer to it.
    let anonymous = gateway.mcp_request(Method::HEAD, &[], None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert!(anonymous.header("www-authenticate").is_some());
    assert!(anonymous.body.is_empty());
    let head = gateway
        .mcp_request(Method::HEAD, &[("authorization", &authorization)], None)
        .await;
    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(head.header("allow"), Some("GET, POST, DELETE"));

    for method in [Method::DELETE, Method::PUT, Method::PATCH] {
        let response = gateway
            .mcp_request(method.clone(), &[("authorization", &authorization)], None)
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(response.header("allow"), None);
        assert_eq!(
            response.json(),
            json!({ "message": format!("Cannot {method}:/mcp") })
        );
    }
    let nested = gateway
        .app
        .get("/mcp/")
        .api()
        .header("authorization", &authorization)
        .send()
        .await;
    assert_eq!(nested.status, StatusCode::NOT_FOUND);
}
