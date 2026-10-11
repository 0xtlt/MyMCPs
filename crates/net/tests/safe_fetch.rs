//! What `upstream/safe_fetch.ts` does that its TypeScript tests left to the
//! `fetch` of Node or did not reach: the redirect rules one by one, the
//! request as it goes out, and the response as callers read it.

mod support;

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use futures::StreamExt;
use http::{Method, StatusCode};
use mymcps_net::{
    AddressGuard, BodyStream, CannedResponse, DEFAULT_USER_AGENT, DiscoveredEndpointGuard,
    FetchError, FetchRequest, FetchResponse, Fetcher, RestrictedEndpointError, SentRequest,
    UpstreamResponseLimits, fetch_with_same_origin_redirects,
};
use serde_json::{Value, json};
use support::{closed_port, redirect, respond, upstream};
use url::Url;

async fn fetch(request: FetchRequest) -> Result<FetchResponse, FetchError> {
    fetch_with_same_origin_redirects(request, "MCP endpoint", UpstreamResponseLimits::default())
        .await
}

fn targets(server: &support::Upstream) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .map(|request| request.target)
        .collect()
}

#[tokio::test]
async fn sends_the_request_as_it_was_given() {
    let server = upstream(|_, _| async { respond(200, &[], "ok") }).await;

    let request = FetchRequest::new(Method::PUT, server.url("/mcp?key=value&code=fr#section"))
        .header("Content-Type", "application/json")
        .unwrap()
        .header("X-Api-Key", " provider-key\n")
        .unwrap()
        .body(r#"{"jsonrpc":"2.0"}"#);
    let response = fetch(request).await.unwrap();
    assert!(response.ok());
    assert_eq!(response.status_text(), "OK");
    assert_eq!(response.url(), &server.url("/mcp?key=value&code=fr"));

    // A body does not bring a content type with it, and a client that names
    // itself is not renamed.
    let request = FetchRequest::post(server.url("/plain"))
        .header("User-Agent", "claude-code/2.1.89 (cli)")
        .unwrap()
        .body("text");
    fetch(request).await.unwrap();

    let requests = server.requests();
    assert_eq!(requests[0].method, Method::PUT);
    assert_eq!(requests[0].target, "/mcp?key=value&code=fr");
    assert_eq!(requests[0].header("content-type"), Some("application/json"));
    assert_eq!(requests[0].header("x-api-key"), Some("provider-key"));
    assert_eq!(requests[0].header("user-agent"), Some(DEFAULT_USER_AGENT));
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(requests[0].body, r#"{"jsonrpc":"2.0"}"#);
    assert_eq!(requests[1].header("content-type"), None);
    assert_eq!(
        requests[1].header("user-agent"),
        Some("claude-code/2.1.89 (cli)")
    );
    assert_eq!(requests[1].body, "text");
}

#[tokio::test]
async fn follows_five_redirects_and_gives_up_on_the_sixth() {
    let canonical = upstream(|_, call| async move {
        if call <= 5 {
            redirect(302, &format!("/hop-{call}"))
        } else {
            respond(200, &[], "done")
        }
    })
    .await;
    let mut response = fetch(FetchRequest::get(canonical.url("/mcp")))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "done");
    assert_eq!(response.url(), &canonical.url("/hop-5"));
    assert_eq!(canonical.requests().len(), 6);

    let looping = upstream(|_, call| async move { redirect(302, &format!("/hop-{call}")) }).await;
    let failure = fetch(FetchRequest::get(looping.url("/mcp")))
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "MCP endpoint exceeded 5 redirects");
    assert!(matches!(failure, FetchError::TooManyRedirects { .. }));
    assert_eq!(
        targets(&looping),
        ["/mcp", "/hop-1", "/hop-2", "/hop-3", "/hop-4", "/hop-5"],
        "the sixth redirect is not followed"
    );
}

fn posted(url: Url, method: Method) -> FetchRequest {
    FetchRequest::new(method, url)
        .header("Authorization", "Bearer upstream-token")
        .unwrap()
        .header("Content-Type", "application/json")
        .unwrap()
        .header("Content-Language", "en")
        .unwrap()
        .header("Content-Encoding", "identity")
        .unwrap()
        .header("Content-Location", "/source")
        .unwrap()
        .header("X-Trace", "trace-1")
        .unwrap()
        .body(r#"{"jsonrpc":"2.0"}"#)
}

#[tokio::test]
async fn switches_to_get_where_a_browser_would() {
    for (status, method) in [
        (303, Method::POST),
        (303, Method::PUT),
        (301, Method::POST),
        (302, Method::POST),
    ] {
        let server = upstream(move |_, call| async move {
            if call == 1 {
                redirect(status, "/result")
            } else {
                respond(200, &[], "")
            }
        })
        .await;
        fetch(posted(server.url("/mcp"), method.clone()))
            .await
            .unwrap();

        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{status} {method}");
        let followed = &requests[1];
        assert_eq!(followed.method, Method::GET, "{status} {method}");
        assert_eq!(followed.target, "/result");
        assert!(followed.body.is_empty(), "{status} {method}");
        for dropped in [
            "content-type",
            "content-language",
            "content-encoding",
            "content-location",
            "content-length",
        ] {
            assert_eq!(
                followed.header(dropped),
                None,
                "{status} {method} {dropped}"
            );
        }
        // Same origin: the credentials and the other headers go along.
        assert_eq!(
            followed.header("authorization"),
            Some("Bearer upstream-token")
        );
        assert_eq!(followed.header("x-trace"), Some("trace-1"));
    }
}

#[tokio::test]
async fn repeats_the_request_where_the_method_is_kept() {
    for (status, method) in [
        (307, Method::POST),
        (308, Method::POST),
        (301, Method::PUT),
        (302, Method::DELETE),
        (307, Method::PATCH),
    ] {
        let server = upstream(move |_, call| async move {
            if call == 1 {
                redirect(status, "/canonical")
            } else {
                respond(200, &[], "")
            }
        })
        .await;
        fetch(posted(server.url("/mcp"), method.clone()))
            .await
            .unwrap();

        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{status} {method}");
        let followed = &requests[1];
        assert_eq!(followed.method, method, "{status}");
        assert_eq!(followed.body, r#"{"jsonrpc":"2.0"}"#, "{status} {method}");
        assert_eq!(followed.header("content-type"), Some("application/json"));
        assert_eq!(followed.header("content-language"), Some("en"));
        assert_eq!(
            followed.header("authorization"),
            Some("Bearer upstream-token")
        );
        assert_eq!(followed.header("x-trace"), Some("trace-1"));
    }
}

#[tokio::test]
async fn returns_a_redirect_it_cannot_follow_as_it_is() {
    let server = upstream(|request, _| async move {
        match request.target.as_str() {
            "/nowhere" => respond(302, &[], "moved, but where"),
            "/empty" => respond(301, &[("Location", "")], "moved, but where"),
            "/choices" => respond(300, &[("Location", "/other")], "pick one"),
            _ => respond(304, &[("Location", "/other")], Body::empty()),
        }
    })
    .await;

    for (path, status) in [
        ("/nowhere", 302),
        ("/empty", 301),
        ("/choices", 300),
        ("/same", 304),
    ] {
        let response = fetch(FetchRequest::get(server.url(path))).await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.url(), &server.url(path));
    }
    assert_eq!(server.requests().len(), 4);

    // Not a 2xx answer, so its body is one that gets cut, not one that fails.
    let mut response = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/nowhere")),
        "MCP endpoint",
        UpstreamResponseLimits {
            max_response_bytes: Some(1),
            max_error_response_bytes: Some(5),
        },
    )
    .await
    .unwrap();
    assert_eq!(response.text().await.unwrap(), "moved");
}

#[tokio::test]
async fn refuses_a_location_it_cannot_request() {
    let server = upstream(|request, _| async move {
        match request.target.as_str() {
            "/invalid" => redirect(302, "http://[not-an-address"),
            "/blob" => {
                // Counts as the same origin, and is not something to request.
                let host = request.header("host").unwrap().to_owned();
                redirect(302, &format!("blob:http://{host}/a-blob"))
            }
            "/script" => redirect(302, "javascript:alert(1)"),
            _ => redirect(302, "file:///etc/passwd"),
        }
    })
    .await;

    let failure = fetch(FetchRequest::get(server.url("/invalid")))
        .await
        .unwrap_err();
    assert!(
        matches!(failure, FetchError::InvalidRedirectUrl),
        "{failure:?}"
    );
    assert_eq!(failure.to_string(), "Invalid URL");

    let failure = fetch(FetchRequest::get(server.url("/blob")))
        .await
        .unwrap_err();
    assert!(matches!(failure, FetchError::Transport(_)), "{failure:?}");
    assert_eq!(failure.to_string(), "fetch failed");

    for path in ["/script", "/file"] {
        let failure = fetch(FetchRequest::get(server.url(path)))
            .await
            .unwrap_err();
        assert_eq!(
            failure.to_string(),
            "MCP endpoint redirected to a different origin"
        );
    }
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn takes_credentials_from_a_redirect_only_for_a_request_that_has_none() {
    let server = upstream(|request, call| async move {
        if call == 1 {
            let host = request.header("host").unwrap().to_owned();
            redirect(307, &format!("http://other:secret@{host}/canonical"))
        } else {
            respond(200, &[], "")
        }
    })
    .await;
    let response = fetch(FetchRequest::get(server.url("/mcp"))).await.unwrap();
    assert_eq!(response.url(), &server.url("/canonical"));
    let requests = server.requests();
    assert_eq!(requests[0].header("authorization"), None);
    // base64("other:secret")
    assert_eq!(
        requests[1].header("authorization"),
        Some("Basic b3RoZXI6c2VjcmV0")
    );
    assert_eq!(requests[1].target, "/canonical");

    let server = upstream(|request, call| async move {
        if call == 1 {
            let host = request.header("host").unwrap().to_owned();
            redirect(307, &format!("http://other:secret@{host}/canonical"))
        } else {
            respond(200, &[], "")
        }
    })
    .await;
    let request = FetchRequest::get(server.url("/mcp"))
        .header("Authorization", "Bearer own-token")
        .unwrap();
    fetch(request).await.unwrap();
    for request in server.requests() {
        assert_eq!(request.header("authorization"), Some("Bearer own-token"));
    }
}

#[tokio::test]
async fn fails_without_naming_the_url_when_the_upstream_cannot_be_reached() {
    let port = closed_port().await;
    let url: Url = format!("http://user:url-password@127.0.0.1:{port}/mcp?api_key=query-secret")
        .parse()
        .unwrap();

    let failure = fetch(FetchRequest::get(url)).await.unwrap_err();
    assert!(matches!(failure, FetchError::Transport(_)), "{failure:?}");
    assert_eq!(failure.to_string(), "fetch failed");

    // The cause is there for a log, and gives away neither part of the URL.
    let mut described = format!("{failure:?}");
    let mut source = failure.source();
    assert!(source.is_some());
    while let Some(cause) = source {
        described.push_str(&format!(" {cause} {cause:?}"));
        source = cause.source();
    }
    assert!(!described.contains("query-secret"), "{described}");
    assert!(!described.contains("url-password"), "{described}");
}

#[tokio::test]
async fn never_connects_to_a_port_fetch_refuses() {
    // Port 25 is where mail is taken: a request body could pass for SMTP commands.
    for url in [
        "http://127.0.0.1:25/mcp",
        "https://127.0.0.1:6667/mcp",
        "http://[::1]:22/",
    ] {
        let failure = fetch(FetchRequest::post(url.parse().unwrap()).body("HELO"))
            .await
            .unwrap_err();
        assert!(matches!(failure, FetchError::BadPort), "{url}: {failure:?}");
        assert_eq!(failure.to_string(), "fetch failed");
    }
}

#[tokio::test]
async fn tells_a_response_that_broke_off_from_one_that_never_came() {
    let server = upstream(|_, _| async {
        // Headers and a first chunk, then the connection is cut.
        let start = futures::stream::iter([Ok(Bytes::from("partial"))]);
        let cut = futures::stream::once(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(std::io::Error::other("upstream went away"))
        });
        respond(200, &[], Body::from_stream(start.chain(cut)))
    })
    .await;

    let url = server.url("/mcp?api_key=query-secret");
    let mut response = fetch(FetchRequest::get(url.clone())).await.unwrap();
    let failure = response.text().await.unwrap_err();
    assert!(matches!(failure, FetchError::Terminated(_)), "{failure:?}");
    assert_eq!(failure.to_string(), "terminated");
    assert!(!format!("{failure:?}").contains("query-secret"));

    let mut body = fetch(FetchRequest::get(url)).await.unwrap().into_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), "partial");
    assert!(matches!(
        body.next().await,
        Some(Err(FetchError::Terminated(_)))
    ));
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn refuses_a_body_on_a_get_or_head_request() {
    let server = upstream(|_, _| async { respond(200, &[], "") }).await;

    for method in [Method::GET, Method::HEAD] {
        let request = FetchRequest::new(method, server.url("/mcp")).body("unexpected");
        let failure = fetch(request).await.unwrap_err();
        assert!(matches!(failure, FetchError::BodyNotAllowed), "{failure:?}");
        assert_eq!(
            failure.to_string(),
            "Request with GET/HEAD method cannot have body."
        );
    }
    assert!(server.requests().is_empty());
}

/// `"a".repeat(2000)`, gzipped.
const GZIPPED_2000_BYTES: [u8; 35] = [
    31, 139, 8, 0, 0, 0, 0, 0, 2, 19, 75, 76, 28, 5, 163, 96, 20, 140, 130, 81, 48, 10, 70, 193,
    80, 7, 0, 57, 62, 19, 168, 208, 7, 0, 0,
];

#[tokio::test]
async fn counts_a_body_after_decompression() {
    let server = upstream(|_, _| async {
        respond(
            200,
            &[("Content-Encoding", "gzip")],
            GZIPPED_2000_BYTES.to_vec(),
        )
    })
    .await;

    let mut whole = fetch(FetchRequest::get(server.url("/mcp"))).await.unwrap();
    assert_eq!(whole.text().await.unwrap(), "a".repeat(2000));
    assert!(
        server.requests()[0]
            .header("accept-encoding")
            .unwrap()
            .contains("gzip")
    );

    let mut limited = fetch_with_same_origin_redirects(
        FetchRequest::get(server.url("/mcp")),
        "MCP endpoint",
        UpstreamResponseLimits {
            max_response_bytes: Some(1000),
            max_error_response_bytes: None,
        },
    )
    .await
    .unwrap();
    let failure = limited.bytes().await.unwrap_err();
    assert_eq!(
        failure.to_string(),
        "MCP endpoint response exceeded 1000 bytes"
    );
}

#[tokio::test]
async fn reads_a_body_as_text_or_json_more_than_once() {
    let server = upstream(|request, _| async move {
        match request.target.as_str() {
            "/json" => respond(200, &[], r#"{"b":1,"a":{"nested":[true,null]}}"#),
            "/bom" => respond(200, &[], b"\xEF\xBB\xBF{\"ok\":true}".to_vec()),
            "/latin1" => respond(200, &[], b"caf\xE9 \xF0\x9F".to_vec()),
            "/html" => respond(502, &[], "<html>Bad Gateway</html>"),
            _ => respond(200, &[], Body::empty()),
        }
    })
    .await;

    let mut response = fetch(FetchRequest::get(server.url("/json"))).await.unwrap();
    let value: Value = response.json().await.unwrap();
    assert_eq!(value, json!({ "b": 1, "a": { "nested": [true, null] } }));
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["b", "a"]
    );
    // Read again, then streamed: the body is still the one that was read.
    assert_eq!(
        response.text().await.unwrap(),
        r#"{"b":1,"a":{"nested":[true,null]}}"#
    );
    assert_eq!(response.bytes().await.unwrap().len(), 34);
    let mut replayed = response.into_stream();
    assert_eq!(replayed.next().await.unwrap().unwrap().len(), 34);
    assert!(replayed.next().await.is_none());

    let mut response = fetch(FetchRequest::get(server.url("/bom"))).await.unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({ "ok": true })
    );
    assert_eq!(response.text().await.unwrap(), r#"{"ok":true}"#);
    assert_eq!(response.bytes().await.unwrap().len(), 14);

    let mut response = fetch(FetchRequest::get(server.url("/latin1")))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "caf\u{fffd} \u{fffd}");

    // What `response.json().catch(() => null)` relied on.
    let mut response = fetch(FetchRequest::get(server.url("/html"))).await.unwrap();
    let failure = response.json::<Value>().await.unwrap_err();
    assert!(matches!(failure, FetchError::Json(_)), "{failure:?}");
    assert_eq!(response.status(), 502);
    assert_eq!(response.text().await.unwrap(), "<html>Bad Gateway</html>");

    let mut response = fetch(FetchRequest::get(server.url("/nothing")))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "");
    assert!(matches!(
        response.json::<Value>().await,
        Err(FetchError::Json(_))
    ));
    assert!(response.into_stream().next().await.is_none());
}

#[tokio::test]
async fn reports_the_reason_phrase_and_headers_the_server_sent() {
    let server = upstream(|request, _| async move {
        let mut response = respond(
            401,
            &[
                ("WWW-Authenticate", "Bearer error=\"invalid_token\""),
                ("WWW-Authenticate", "Basic realm=\"mcp\""),
            ],
            "denied",
        );
        if request.target == "/custom" {
            response
                .extensions_mut()
                .insert(hyper::ext::ReasonPhrase::from_static(b"Token Expired"));
        }
        response
    })
    .await;

    let response = fetch(FetchRequest::get(server.url("/mcp"))).await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(response.status_text(), "Unauthorized");
    assert_eq!(
        response.header("www-authenticate").as_deref(),
        Some("Bearer error=\"invalid_token\", Basic realm=\"mcp\"")
    );
    assert_eq!(
        response
            .headers()
            .get_all("WWW-Authenticate")
            .iter()
            .count(),
        2
    );
    assert_eq!(response.header("mcp-session-id"), None);

    let response = fetch(FetchRequest::get(server.url("/custom")))
        .await
        .unwrap();
    assert_eq!(response.status_text(), "Token Expired");
}

// The fetcher of a test, which answers in place of the network.

/// Serves JSON documents by exact URL, 404 for the rest, and records every
/// request, like `mockDocuments` of the TypeScript tests.
fn documents(documents: &[(&str, Value)]) -> (Fetcher, Arc<Mutex<Vec<SentRequest>>>) {
    let documents: Vec<(String, Value)> = documents
        .iter()
        .map(|(url, body)| ((*url).to_owned(), body.clone()))
        .collect();
    let calls: Arc<Mutex<Vec<SentRequest>>> = Arc::default();
    let fetcher = Fetcher::offline().answering({
        let calls = Arc::clone(&calls);
        move |request| {
            calls.lock().unwrap().push(request.clone());
            let document = documents
                .iter()
                .find(|(url, _)| url == request.url.as_str());
            Some(match document {
                Some((_, body)) => CannedResponse::json(StatusCode::OK, body),
                None => CannedResponse::new(StatusCode::NOT_FOUND).body("not found"),
            })
        }
    });
    (fetcher, calls)
}

#[tokio::test]
async fn answers_in_place_of_an_upstream_for_a_test() {
    let (fetcher, calls) = documents(&[(
        "https://auth.example/.well-known/oauth-authorization-server",
        json!({ "issuer": "https://auth.example" }),
    )]);
    let limits = UpstreamResponseLimits::default();

    let metadata: Url = "https://auth.example/.well-known/oauth-authorization-server"
        .parse()
        .unwrap();
    let mut response = fetcher
        .fetch_with_same_origin_redirects(
            FetchRequest::get(metadata.clone()),
            "OAuth endpoint",
            limits,
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.status_text(), "OK");
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(response.url(), &metadata);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({ "issuer": "https://auth.example" })
    );

    let request = FetchRequest::post("https://user:secret@auth.example/token".parse().unwrap())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .unwrap()
        .body("grant_type=refresh_token");
    let mut response = fetcher
        .fetch_with_same_origin_redirects(request, "OAuth endpoint", limits)
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.status_text(), "Not Found");
    assert_eq!(response.text().await.unwrap(), "not found");

    // The answer sees a request as it would go out.
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, Method::GET);
    assert_eq!(calls[0].text(), "");
    assert_eq!(calls[1].method, Method::POST);
    assert_eq!(calls[1].url.as_str(), "https://auth.example/token");
    assert_eq!(
        calls[1].header("authorization").as_deref(),
        Some("Basic dXNlcjpzZWNyZXQ=")
    );
    assert_eq!(
        calls[1].header("Content-Type").as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(calls[1].text(), "grant_type=refresh_token");
}

#[tokio::test]
async fn treats_a_canned_response_like_one_from_the_network() {
    let fetcher = Fetcher::offline().answering(|request| {
        Some(match request.url.path() {
            "/mcp" => CannedResponse::new(StatusCode::TEMPORARY_REDIRECT)
                .header("Location", "/canonical")
                .unwrap(),
            "/leaving" => CannedResponse::new(StatusCode::FOUND)
                .header("Location", "https://attacker.example/collect")
                .unwrap(),
            "/large" => CannedResponse::new(StatusCode::OK).body("a".repeat(2000)),
            "/failing" => CannedResponse::new(StatusCode::BAD_GATEWAY).body("e".repeat(2000)),
            _ => CannedResponse::new(StatusCode::OK).body(request.text()),
        })
    });
    let limits = UpstreamResponseLimits {
        max_response_bytes: Some(1000),
        max_error_response_bytes: Some(100),
    };
    let url = |path: &str| -> Url { format!("https://mcp.example{path}").parse().unwrap() };

    let request = FetchRequest::post(url("/mcp")).body("ping");
    let mut response = fetcher
        .fetch_with_same_origin_redirects(request, "MCP endpoint", limits)
        .await
        .unwrap();
    assert_eq!(response.url(), &url("/canonical"));
    assert_eq!(response.text().await.unwrap(), "ping");

    let failure = fetcher
        .fetch_with_same_origin_redirects(
            FetchRequest::get(url("/leaving")),
            "MCP endpoint",
            limits,
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "MCP endpoint redirected to a different origin"
    );

    let mut large = fetcher
        .fetch_with_same_origin_redirects(FetchRequest::get(url("/large")), "MCP endpoint", limits)
        .await
        .unwrap();
    assert_eq!(
        large.text().await.unwrap_err().to_string(),
        "MCP endpoint response exceeded 1000 bytes"
    );

    let mut failing = fetcher
        .fetch_with_same_origin_redirects(
            FetchRequest::get(url("/failing")),
            "MCP endpoint",
            limits,
        )
        .await
        .unwrap();
    assert_eq!(failing.status_text(), "Bad Gateway");
    assert_eq!(failing.text().await.unwrap(), "e".repeat(100));
}

#[tokio::test]
async fn stays_off_the_network_unless_a_test_lets_a_request_through() {
    let limits = UpstreamResponseLimits::default();
    let strava: Url = "https://www.strava.com/api/v3/athlete".parse().unwrap();

    // Nothing answers: the request fails like one the network could not carry.
    let failure = Fetcher::offline()
        .fetch_with_same_origin_redirects(FetchRequest::get(strava.clone()), "Strava API", limits)
        .await
        .unwrap_err();
    assert!(matches!(failure, FetchError::Offline), "{failure:?}");
    assert_eq!(failure.to_string(), "fetch failed");

    // Each answer takes the requests of its own origin, as the TypeScript
    // helpers passed the others on to the `fetch` they had replaced.
    let server = upstream(|_, _| async { respond(200, &[], "from the local server") }).await;
    let fetcher = Fetcher::shared()
        .answering(|request| {
            (request.url.host_str() == Some("www.strava.com"))
                .then(|| CannedResponse::json(StatusCode::OK, &json!({ "id": 4242 })))
        })
        .answering(|request| {
            (request.url.host_str() == Some("mcp.example"))
                .then(|| CannedResponse::new(StatusCode::ACCEPTED))
        })
        // The answer added last is asked first.
        .answering(|request| {
            (request.url.path() == "/api/v3/athlete/zones")
                .then(|| CannedResponse::new(StatusCode::TOO_MANY_REQUESTS))
        });

    let mut response = fetcher
        .fetch_with_same_origin_redirects(FetchRequest::get(strava), "Strava API", limits)
        .await
        .unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({ "id": 4242 })
    );

    let zones = "https://www.strava.com/api/v3/athlete/zones"
        .parse()
        .unwrap();
    let response = fetcher
        .fetch_with_same_origin_redirects(FetchRequest::get(zones), "Strava API", limits)
        .await
        .unwrap();
    assert_eq!(response.status(), 429);

    let mcp = "https://mcp.example/mcp".parse().unwrap();
    let response = fetcher
        .fetch_with_same_origin_redirects(FetchRequest::post(mcp), "MCP endpoint", limits)
        .await
        .unwrap();
    assert_eq!(response.status(), 202);

    let mut response = fetcher
        .fetch_with_same_origin_redirects(
            FetchRequest::get(server.url("/mcp")),
            "MCP endpoint",
            limits,
        )
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "from the local server");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn can_be_shared_between_tasks() {
    fn send(_: &impl Send) {}
    fn send_and_sync<T: Send + Sync>() {}

    send_and_sync::<Fetcher>();
    send_and_sync::<SentRequest>();
    send_and_sync::<CannedResponse>();
    send_and_sync::<FetchRequest>();
    send_and_sync::<FetchResponse>();
    send_and_sync::<BodyStream>();
    send_and_sync::<FetchError>();
    send_and_sync::<AddressGuard>();
    send_and_sync::<DiscoveredEndpointGuard>();
    send_and_sync::<RestrictedEndpointError>();

    // Neither future is polled: nothing is resolved or requested.
    let url: Url = "http://127.0.0.1/mcp".parse().unwrap();
    send(&fetch(FetchRequest::get(url.clone())));
    let fetcher = Fetcher::offline().answering(|_| None);
    send(&fetcher.fetch_with_same_origin_redirects(
        FetchRequest::get(url.clone()),
        "MCP endpoint",
        UpstreamResponseLimits::default(),
    ));
    let guard = AddressGuard::system();
    let endpoints = guard.discovered_endpoint_guard(&url);
    send(&endpoints.assert_allowed(&url, "OAuth endpoint"));
    send(&guard.resolves_to_restricted_address("localhost"));
}
