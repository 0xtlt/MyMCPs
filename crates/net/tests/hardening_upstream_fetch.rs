//! Port of `tests/unit/hardening_upstream_fetch.spec.ts`.
//!
//! An `AbortSignal` became the `timeout` of the request, and dropping the
//! future or the body. The last group went through the MCP client of the SDK;
//! here it reads the same answers straight from the fetch, which is the part
//! this crate owns.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use futures::StreamExt;
use http::Method;
use mymcps_net::{
    FetchError, FetchRequest, MAX_UPSTREAM_ERROR_RESPONSE_BYTES, MAX_UPSTREAM_RESPONSE_BYTES,
    UpstreamResponseLimits, fetch_with_same_origin_redirects,
};
use serde_json::{Value, json};
use support::{OnDrop, eventually, redirect, respond, streamed_body, upstream};
use url::Url;

const NO_LIMITS: UpstreamResponseLimits = UpstreamResponseLimits {
    max_response_bytes: None,
    max_error_response_bytes: None,
};

fn at_most(max_response_bytes: usize) -> UpstreamResponseLimits {
    UpstreamResponseLimits {
        max_response_bytes: Some(max_response_bytes),
        ..NO_LIMITS
    }
}

// Upstream fetch: abort signal

#[tokio::test]
async fn still_cancels_a_request_that_followed_a_same_origin_redirect() {
    let abandoned = Arc::new(AtomicBool::new(false));
    let server = upstream({
        let abandoned = Arc::clone(&abandoned);
        move |_, call| {
            let abandoned = Arc::clone(&abandoned);
            async move {
                if call == 1 {
                    return redirect(307, "/canonical");
                }
                // Like a server that accepted the request and never answers.
                let _abandoned = OnDrop(abandoned);
                std::future::pending().await
            }
        }
    })
    .await;

    let request = FetchRequest::post(server.url("/mcp"))
        .body(r#"{"jsonrpc":"2.0"}"#)
        .timeout(Duration::from_millis(400));
    let failure = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap_err();

    assert!(matches!(failure, FetchError::Timeout), "{failure:?}");
    assert_eq!(
        failure.to_string(),
        "The operation was aborted due to timeout"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].target, "/canonical");
    assert_eq!(requests[1].method, Method::POST);
    assert_eq!(requests[1].body, r#"{"jsonrpc":"2.0"}"#);
    // The request left waiting was dropped, not only given up on.
    eventually(|| abandoned.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn cancels_the_body_read_of_a_response_reached_through_a_redirect() {
    let server = upstream(|request, _| async move {
        if request.target == "/mcp" {
            return redirect(307, "/stalled");
        }
        // Headers and a first chunk, then nothing more.
        let partial = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from("partial"))])
            .chain(futures::stream::pending());
        respond(
            200,
            &[("Content-Type", "text/plain")],
            Body::from_stream(partial),
        )
    })
    .await;

    let request = FetchRequest::get(server.url("/mcp")).timeout(Duration::from_millis(400));
    let mut response = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let failure = response.text().await.unwrap_err();
    assert!(matches!(failure, FetchError::Timeout), "{failure:?}");
    // The failed read is what a second reader gets too.
    assert!(matches!(response.bytes().await, Err(FetchError::Timeout)));
}

#[tokio::test]
async fn cancels_a_stalled_body_when_it_is_dropped() {
    let (body, state) = streamed_body(400, usize::MAX);
    let body = Arc::new(std::sync::Mutex::new(Some(body)));
    let server = upstream(move |_, _| {
        let body = body.lock().unwrap().take().unwrap();
        async move { respond(200, &[], body) }
    })
    .await;

    let response = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")),
        "MCP endpoint",
        NO_LIMITS,
    )
    .await
    .unwrap();
    let mut body = response.into_stream();
    assert!(body.next().await.unwrap().is_ok());
    drop(body);

    state.wait_until_cancelled().await;
}

#[tokio::test]
async fn hands_every_hop_the_one_timeout_of_the_caller() {
    // The timeout is not started again for each hop: three hops that each
    // answer well within it still run out of it together.
    async fn slow_hops() -> support::Upstream {
        upstream(|_, call| async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if call < 3 {
                redirect(308, &format!("/hop-{call}"))
            } else {
                respond(200, &[], "done")
            }
        })
        .await
    }

    let server = slow_hops().await;
    let mut patient = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")).timeout(Duration::from_secs(30)),
        "MCP endpoint",
        NO_LIMITS,
    )
    .await
    .unwrap();
    assert_eq!(patient.text().await.unwrap(), "done");
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets, ["/mcp", "/hop-1", "/hop-2"]);

    let server = slow_hops().await;
    let failure = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")).timeout(Duration::from_millis(800)),
        "MCP endpoint",
        NO_LIMITS,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, FetchError::Timeout), "{failure:?}");
    assert!(
        server.requests().len() >= 2,
        "the redirects were being followed"
    );
}

// Upstream fetch: response size

#[test]
fn documents_its_limits() {
    assert_eq!(MAX_UPSTREAM_RESPONSE_BYTES, 32 * 1024 * 1024);
    assert_eq!(MAX_UPSTREAM_ERROR_RESPONSE_BYTES, 64 * 1024);
}

#[tokio::test]
async fn fails_the_read_of_a_successful_body_past_its_limit_and_stops_the_download() {
    let (body, endless) = streamed_body(400, usize::MAX);
    let body = Arc::new(std::sync::Mutex::new(Some(body)));
    let server = upstream(move |_, _| {
        let body = body.lock().unwrap().take().unwrap();
        async move { respond(200, &[], body) }
    })
    .await;

    let mut response = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")),
        "MCP endpoint",
        at_most(1000),
    )
    .await
    .unwrap();

    let failure = response.text().await.unwrap_err();
    assert_eq!(
        failure.to_string(),
        "MCP endpoint response exceeded 1000 bytes"
    );
    assert!(matches!(
        failure,
        FetchError::ResponseTooLarge { ref label, max_bytes: 1000 } if label == "MCP endpoint"
    ));
    endless.wait_until_cancelled().await;
}

#[tokio::test]
async fn cuts_an_error_body_short_instead_of_reading_it_whole() {
    let (body, endless) = streamed_body(400, usize::MAX);
    let body = Arc::new(std::sync::Mutex::new(Some(body)));
    let server = upstream(move |_, _| {
        let body = body.lock().unwrap().take().unwrap();
        async move { respond(502, &[], body) }
    })
    .await;

    let mut response = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")),
        "MCP endpoint",
        UpstreamResponseLimits {
            max_error_response_bytes: Some(1000),
            ..NO_LIMITS
        },
    )
    .await
    .unwrap();

    assert_eq!(response.status(), 502);
    assert_eq!(response.status_text(), "Bad Gateway");
    assert!(!response.ok());
    // A second read is what the 401 diagnostic does; both stay bounded.
    assert_eq!(response.text().await.unwrap(), "a".repeat(1000));
    assert_eq!(response.text().await.unwrap(), "a".repeat(1000));
    endless.wait_until_cancelled().await;
}

#[tokio::test]
async fn passes_bodies_within_the_limit_and_bodiless_responses_through_unchanged() {
    let server = upstream(|request, _| async move {
        if request.target.ends_with("/empty") {
            return respond(204, &[], Body::empty());
        }
        let (exact, _) = streamed_body(250, 4);
        respond(
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Mcp-Session-Id", "session-1"),
            ],
            exact,
        )
    })
    .await;

    let mut response = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")),
        "MCP endpoint",
        at_most(1000),
    )
    .await
    .unwrap();
    assert_eq!(
        response.header("Mcp-Session-Id").as_deref(),
        Some("session-1")
    );
    assert_eq!(
        response.header("Content-Type").as_deref(),
        Some("text/plain")
    );
    assert_eq!(response.text().await.unwrap(), "a".repeat(1000));

    let mut empty = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/empty")),
        "MCP endpoint",
        at_most(1),
    )
    .await
    .unwrap();
    assert_eq!(empty.status(), 204);
    assert!(empty.bytes().await.unwrap().is_empty());
}

// Upstream fetch: MCP client through the limited body

fn tools_list(url: Url) -> FetchRequest {
    FetchRequest::post(url)
        .header("Content-Type", "application/json")
        .unwrap()
        .header("Accept", "application/json, text/event-stream")
        .unwrap()
        .body(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string())
}

fn tools_answer(message: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": message["id"],
        "result": { "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] },
    })
}

fn tool_names(answer: &Value) -> Vec<&str> {
    answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn reads_json_and_event_stream_answers_as_before() {
    let as_json = upstream(|request, _| async move {
        let message: Value = serde_json::from_slice(&request.body).unwrap();
        respond(
            200,
            &[("Content-Type", "application/json")],
            tools_answer(&message).to_string(),
        )
    })
    .await;
    let as_stream = upstream(|request, _| async move {
        let message: Value = serde_json::from_slice(&request.body).unwrap();
        // Split mid-event, as a network would.
        let event = format!("event: message\ndata: {}\n\n", tools_answer(&message));
        let (start, end) = (event[..20].to_owned(), event[20..].to_owned());
        let chunks = futures::stream::once(async move { Ok::<_, std::io::Error>(start) }).chain(
            futures::stream::once(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(end)
            }),
        );
        respond(
            200,
            &[("Content-Type", "text/event-stream")],
            Body::from_stream(chunks),
        )
    })
    .await;

    let request = tools_list(as_json.url("/mcp"));
    let mut response = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap();
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("application/json")
    );
    let answer: Value = response.json().await.unwrap();
    assert_eq!(tool_names(&answer), ["echo"]);
    let sent = &as_json.requests()[0];
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(
        sent.header("accept"),
        Some("application/json, text/event-stream")
    );

    let request = tools_list(as_stream.url("/mcp"));
    let response = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap();
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("text/event-stream")
    );
    let mut body = response.into_stream();
    let mut chunks = Vec::new();
    while let Some(chunk) = body.next().await {
        chunks.push(chunk.unwrap());
    }
    assert!(
        chunks.len() >= 2,
        "the event arrives as the server writes it"
    );
    let event = String::from_utf8(chunks.concat()).unwrap();
    let data = event
        .strip_prefix("event: message\ndata: ")
        .unwrap()
        .strip_suffix("\n\n")
        .unwrap();
    assert_eq!(tool_names(&serde_json::from_str(data).unwrap()), ["echo"]);
}

#[tokio::test]
async fn gives_up_on_an_answer_larger_than_the_limit() {
    // Twice the limit, sent only as fast as the client reads it.
    let (body, oversized) = streamed_body(1024 * 1024, 64);
    let body = Arc::new(std::sync::Mutex::new(Some(body)));
    let server = upstream(move |_, _| {
        let body = body.lock().unwrap().take().unwrap();
        async move { respond(200, &[("Content-Type", "application/json")], body) }
    })
    .await;

    let request = tools_list(server.url("/mcp"));
    let mut response = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap();

    let failure = response.json::<Value>().await.unwrap_err();
    assert_eq!(
        failure.to_string(),
        "MCP endpoint response exceeded 33554432 bytes"
    );
    oversized.wait_until_cancelled().await;
    assert!(
        oversized.sent() < 64,
        "{} chunks were written",
        oversized.sent()
    );
}

#[tokio::test]
async fn quotes_only_the_start_of_an_oversized_error_page() {
    let server = upstream(|_, _| async move {
        respond(
            500,
            &[("Content-Type", "text/html")],
            vec![b'e'; 4 * 1024 * 1024],
        )
    })
    .await;

    let request = tools_list(server.url("/mcp"));
    let mut response = fetch_with_same_origin_redirects(request, "MCP endpoint", NO_LIMITS)
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    let page = response.text().await.unwrap();
    assert_eq!(page.len(), MAX_UPSTREAM_ERROR_RESPONSE_BYTES);
    assert!(page.bytes().all(|byte| byte == b'e'));
}
