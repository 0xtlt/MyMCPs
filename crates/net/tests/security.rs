//! Port of the cases of `tests/unit/security.spec.ts` that concern the URL
//! check and the fetch. The others (CORS, redaction, Deno) are tested with the
//! code they cover.

mod support;

use mymcps_net::{
    FetchRequest, UpstreamResponseLimits, fetch_with_same_origin_redirects, parse_http_url,
};
use support::{redirect, respond, upstream};

#[test]
fn allows_endpoint_query_and_url_credentials_while_requiring_an_http_url() {
    for http_url in ["file:///tmp/socket", "https://example.test/mcp#secret"] {
        assert!(parse_http_url(http_url, "MCP URL").is_err(), "{http_url}");
    }

    let http_url =
        "https://user:password@example.test/mcp?api_key=provider-required&code=fr&key=primary";
    assert_eq!(
        parse_http_url(http_url, "MCP URL").unwrap().to_string(),
        http_url
    );
}

#[tokio::test]
async fn follows_same_origin_redirects_but_blocks_cross_origin_credential_forwarding() {
    let trusted = upstream(|_, call| async move {
        if call == 1 {
            redirect(307, "/canonical")
        } else {
            respond(200, &[], "")
        }
    })
    .await;

    let mut with_credentials = trusted.url("/mcp");
    with_credentials.set_username("user").unwrap();
    with_credentials.set_password(Some("p%40ss")).unwrap();
    assert!(
        with_credentials
            .as_str()
            .starts_with("http://user:p%40ss@127.0.0.1:")
    );

    let response = fetch_with_same_origin_redirects(
        FetchRequest::get(with_credentials),
        "MCP endpoint",
        UpstreamResponseLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.url(), &trusted.url("/canonical"));
    let requests = trusted.requests();
    for request in &requests {
        assert_eq!(request.header("authorization"), Some("Basic dXNlcjpwQHNz"));
    }
    let targets: Vec<&str> = requests
        .iter()
        .map(|request| request.target.as_str())
        .collect();
    assert_eq!(targets, ["/mcp", "/canonical"]);

    for elsewhere in [
        "https://attacker.example/collect",
        "//attacker.example/collect",
    ] {
        let leaving = upstream(move |_, _| async move { redirect(307, elsewhere) }).await;
        let failure = fetch_with_same_origin_redirects(
            FetchRequest::get(leaving.url("/mcp")),
            "MCP endpoint",
            UpstreamResponseLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            failure.to_string(),
            "MCP endpoint redirected to a different origin"
        );
        assert_eq!(leaving.requests().len(), 1);
    }

    // Another port of the same host is another origin: it is never contacted.
    let collector = upstream(|_, _| async move { respond(200, &[], "collected") }).await;
    let collect = collector.url("/collect").to_string();
    let leaving = upstream(move |_, _| {
        let collect = collect.clone();
        async move { redirect(307, &collect) }
    })
    .await;
    let mut with_credentials = leaving.url("/mcp");
    with_credentials.set_username("user").unwrap();
    with_credentials.set_password(Some("secret")).unwrap();
    let failure = fetch_with_same_origin_redirects(
        FetchRequest::get(with_credentials),
        "MCP endpoint",
        UpstreamResponseLimits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "MCP endpoint redirected to a different origin"
    );
    assert!(collector.requests().is_empty());
}
