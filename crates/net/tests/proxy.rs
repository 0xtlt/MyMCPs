//! A fetcher that sends its requests through a SOCKS5 proxy, as the fetcher of
//! an MCP on a tailnet does, and the guard of the endpoints such an MCP names.

mod support;

use std::error::Error;
use std::sync::{Arc, Mutex};

use http::StatusCode;
use mymcps_net::{
    AddressGuard, CannedResponse, FetchError, FetchRequest, Fetcher, StaticResolver,
    UpstreamResponseLimits,
};
use support::socks::socks5;
use support::{respond, upstream};
use url::Url;

async fn get(fetcher: &Fetcher, url: &str) -> Result<String, FetchError> {
    let request = FetchRequest::get(Url::parse(url).unwrap());
    let mut response = fetcher
        .fetch_with_same_origin_redirects(
            request,
            "MCP endpoint",
            UpstreamResponseLimits::default(),
        )
        .await?;
    response.text().await
}

/// Every message of an error and of its sources.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        text.push_str(&format!(" / {error} / {error:?}"));
        source = error.source();
    }
    text
}

#[tokio::test]
async fn sends_an_http_request_through_the_proxy_which_resolves_the_name() {
    let server = upstream(|_, _| async { respond(200, &[], "from the tailnet") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let fetcher = Fetcher::shared().through_proxy(&proxy.url()).unwrap();

    // The name resolves nowhere: only the proxy can connect to it.
    let url = format!("http://box.tailnet.invalid:{}/mcp", server.port());
    assert_eq!(get(&fetcher, &url).await.unwrap(), "from the tailnet");

    assert_eq!(
        proxy.connects(),
        [format!("box.tailnet.invalid:{}", server.port())]
    );
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        server.requests()[0].header("host"),
        Some(format!("box.tailnet.invalid:{}", server.port()).as_str())
    );

    // The same request without the proxy cannot find the machine.
    assert!(get(&Fetcher::shared(), &url).await.is_err());
}

#[tokio::test]
async fn sends_an_https_request_through_the_proxy_too() {
    // The proxy ends the connection after the greeting of the TLS handshake:
    // what counts is that the connection was asked of the proxy, by name.
    let server = upstream(|_, _| async { respond(200, &[], "plain") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let fetcher = Fetcher::shared().through_proxy(&proxy.url()).unwrap();

    let result = get(&fetcher, "https://box.tailnet.invalid/mcp").await;

    assert!(result.is_err());
    assert_eq!(proxy.connects(), ["box.tailnet.invalid:443"]);
}

#[tokio::test]
async fn keeps_the_credentials_out_of_debug_and_errors() {
    let server = upstream(|_, _| async { respond(200, &[], "ok") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let url = format!("http://box.tailnet.invalid:{}/mcp", server.port());

    let fetcher = Fetcher::shared().through_proxy(&proxy.url()).unwrap();
    let debug = format!("{fetcher:?}");
    assert!(!debug.contains("s3cret-token"), "{debug}");
    assert!(!debug.contains("127.0.0.1"), "{debug}");

    // The wrong password: the proxy refuses, and the error does not repeat it.
    let wrong = Fetcher::shared()
        .through_proxy(&proxy.url_with("7", "wrong-password"))
        .unwrap();
    let error = get(&wrong, &url).await.unwrap_err();
    let text = chain(&error);
    assert!(!text.contains("wrong-password"), "{text}");
    assert!(!text.contains("s3cret-token"), "{text}");
    assert!(proxy.connects().is_empty());

    // A proxy nothing listens on.
    let closed = support::closed_port().await;
    let down = Fetcher::shared()
        .through_proxy(&Url::parse(&format!("socks5h://7:down-secret@127.0.0.1:{closed}")).unwrap())
        .unwrap();
    let error = get(&down, &url).await.unwrap_err();
    let text = chain(&error);
    assert!(!text.contains("down-secret"), "{text}");
    assert!(!format!("{down:?}").contains("down-secret"));
}

#[tokio::test]
async fn is_refused_by_a_proxy_that_wants_credentials_when_it_is_given_none() {
    let server = upstream(|_, _| async { respond(200, &[], "ok") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let fetcher = Fetcher::shared()
        .through_proxy(&proxy.url_without_credentials())
        .unwrap();

    let url = format!("http://box.tailnet.invalid:{}/mcp", server.port());
    assert!(get(&fetcher, &url).await.is_err());
    assert!(proxy.connects().is_empty());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn refuses_a_proxy_url_that_is_not_one() {
    let error = Fetcher::shared()
        .through_proxy(&Url::parse("mailto:someone@example.com").unwrap())
        .unwrap_err();
    assert!(!chain(&error).contains("someone@example.com"));
}

#[tokio::test]
async fn keeps_answering_in_front_of_the_proxy() {
    let server = upstream(|_, _| async { respond(200, &[], "through the proxy") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let answered: Arc<Mutex<Vec<String>>> = Arc::default();
    let fetcher = Fetcher::shared()
        .answering(|_| None)
        .answering({
            let answered = Arc::clone(&answered);
            move |request| {
                (request.url.path() == "/canned").then(|| {
                    answered.lock().unwrap().push(request.url.path().to_owned());
                    CannedResponse::new(StatusCode::OK).body("canned")
                })
            }
        })
        .through_proxy(&proxy.url())
        .unwrap();

    let origin = format!("http://box.tailnet.invalid:{}", server.port());
    assert_eq!(
        get(&fetcher, &format!("{origin}/canned")).await.unwrap(),
        "canned"
    );
    assert!(proxy.connects().is_empty());

    // Whatever nothing answers goes through the proxy, not to the network.
    assert_eq!(
        get(&fetcher, &format!("{origin}/other")).await.unwrap(),
        "through the proxy"
    );
    assert_eq!(proxy.connects().len(), 1);
    assert_eq!(*answered.lock().unwrap(), ["/canned"]);
}

#[tokio::test]
async fn stays_offline_when_it_was_offline() {
    let proxy = socks5("7", "s3cret-token", None).await;
    let fetcher = Fetcher::offline().through_proxy(&proxy.url()).unwrap();

    assert!(
        get(&fetcher, "http://box.tailnet.invalid/mcp")
            .await
            .is_err()
    );
    assert!(proxy.connects().is_empty());
}

#[tokio::test]
async fn stays_proxied_when_it_must_be_public_only() {
    let server = upstream(|_, _| async { respond(200, &[], "from the tailnet") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let fetcher = Fetcher::shared()
        .through_proxy(&proxy.url())
        .unwrap()
        .public_only();

    let url = format!("http://box.tailnet.invalid:{}/mcp", server.port());
    assert_eq!(get(&fetcher, &url).await.unwrap(), "from the tailnet");
    assert_eq!(proxy.connects().len(), 1);

    // The other way round, a fetcher that must be public only goes through the proxy too.
    let fetcher = Fetcher::shared()
        .public_only()
        .through_proxy(&proxy.url())
        .unwrap();
    assert_eq!(get(&fetcher, &url).await.unwrap(), "from the tailnet");
    assert_eq!(proxy.connects().len(), 2);
}

#[tokio::test]
async fn leaves_the_fetcher_it_was_made_from_direct() {
    let server = upstream(|_, _| async { respond(200, &[], "direct") }).await;
    let proxy = socks5("7", "s3cret-token", Some(server.address())).await;
    let direct = Fetcher::shared();
    let _proxied = direct.through_proxy(&proxy.url()).unwrap();

    assert_eq!(
        get(&direct, server.url("/mcp").as_str()).await.unwrap(),
        "direct"
    );
    assert!(proxy.connects().is_empty());
}

#[tokio::test]
async fn allows_every_endpoint_of_an_mcp_reached_through_a_proxy() {
    let resolver = Arc::new(
        StaticResolver::new()
            .with("box.tailnet.invalid", &["203.0.113.10".parse().unwrap()])
            .with("internal.example", &["10.0.0.5".parse().unwrap()])
            .with("metadata.example", &["169.254.169.254".parse().unwrap()]),
    );
    let addresses = AddressGuard::new(resolver);
    let mcp_url: Url = "https://box.tailnet.invalid/mcp".parse().unwrap();

    // The ordinary guard of an MCP at a public address refuses them.
    let ordinary = addresses.discovered_endpoint_guard(&mcp_url);
    let internal: Url = "https://internal.example/token".parse().unwrap();
    assert!(ordinary.must_be_public(&internal).await);
    assert!(
        ordinary
            .assert_allowed(&internal, "OAuth token endpoint")
            .await
            .is_err()
    );

    let trusted = addresses.trusted_network_guard(&mcp_url);
    for endpoint in [
        "https://internal.example/token",
        "https://metadata.example/register",
        "https://box.tailnet.invalid/authorize",
        "https://other.example/token",
    ] {
        let endpoint: Url = endpoint.parse().unwrap();
        assert!(!trusted.must_be_public(&endpoint).await, "{endpoint}");
        assert!(
            trusted
                .assert_allowed(&endpoint, "OAuth token endpoint")
                .await
                .is_ok(),
            "{endpoint}"
        );
    }
}
