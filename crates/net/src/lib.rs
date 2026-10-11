//! Outbound HTTP of the gateway: the URL check every endpoint goes through
//! ([`parse_http_url`]), the guard that keeps discovered endpoints out of the
//! instance's own network ([`AddressGuard`]), and the fetch that follows
//! redirects without leaving the origin and limits what it reads
//! ([`Fetcher::fetch_with_same_origin_redirects`]).
//!
//! The fetch does not call the address guard. An MCP URL is entered by an
//! operator, who may point it at a private address; only the endpoints a remote
//! document names are checked, by the caller, before they are fetched.
//!
//! Code that fetches takes a [`Fetcher`], and code that checks discovered
//! endpoints an [`AddressGuard`], rather than reaching for a global: the
//! application passes [`Fetcher::shared`] and [`AddressGuard::system`], and a
//! test passes its own, see the last section.
//!
//! # A JSON API
//!
//! ```no_run
//! # async fn athlete(fetcher: &mymcps_net::Fetcher, access_token: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
//! use std::time::Duration;
//!
//! use mymcps_net::{FetchError, FetchRequest, UpstreamResponseLimits};
//!
//! let request = FetchRequest::get("https://www.strava.com/api/v3/athlete".parse()?)
//!     .header("Accept", "application/json")?
//!     .header("Authorization", &format!("Bearer {access_token}"))?
//!     .timeout(Duration::from_secs(30));
//! let limits = UpstreamResponseLimits::default();
//! let mut response =
//!     match fetcher.fetch_with_same_origin_redirects(request, "Strava API", limits).await {
//!         // The timeout also covers the read of the body below, which fails the same way.
//!         Err(FetchError::Timeout) => return Err("Strava did not respond in time. Try again.".into()),
//!         response => response?,
//!     };
//!
//! if !response.ok() {
//!     // The body of an error is cut at 64 KiB and may not be JSON at all.
//!     let failure: Option<serde_json::Value> = response.json().await.ok();
//!     return Err(format!("HTTP {}: {failure:?}", response.status().as_u16()).into());
//! }
//! Ok(response.json().await?)
//! # }
//! ```
//!
//! # An MCP endpoint, answering with JSON or an event stream
//!
//! ```no_run
//! # async fn post(fetcher: &mymcps_net::Fetcher, endpoint: url::Url, message: serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
//! use futures::StreamExt;
//! use mymcps_net::{FetchRequest, UpstreamResponseLimits};
//!
//! let request = FetchRequest::post(endpoint)
//!     .header("Content-Type", "application/json")?
//!     .header("Accept", "application/json, text/event-stream")?
//!     .body(message.to_string());
//! let limits = UpstreamResponseLimits::default();
//! let mut response =
//!     fetcher.fetch_with_same_origin_redirects(request, "MCP endpoint", limits).await?;
//!
//! if response.status() == 401 {
//!     // A body that was read is kept, so the diagnostic does not take it away
//!     // from whoever handles the response next.
//!     let body = response.text().await.unwrap_or_default();
//!     let challenge = response.header("WWW-Authenticate");
//! #   let _ = (body, challenge);
//! }
//!
//! let content_type = response.header("Content-Type").unwrap_or_default();
//! if content_type.starts_with("text/event-stream") {
//!     // No timeout was set, so the stream stays open as long as the server
//!     // keeps sending. Dropping it closes the connection.
//!     let mut events = response.into_stream();
//!     while let Some(chunk) = events.next().await {
//!         let chunk: bytes::Bytes = chunk?;
//! #       let _ = chunk;
//!     }
//! } else {
//!     let answer: serde_json::Value = response.json().await?;
//! #   let _ = answer;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # A file
//!
//! ```no_run
//! # async fn download(fetcher: &mymcps_net::Fetcher, link: url::Url) -> Result<bytes::Bytes, Box<dyn std::error::Error>> {
//! use std::time::Duration;
//!
//! use mymcps_net::{FetchRequest, UpstreamResponseLimits};
//!
//! let request = FetchRequest::get(link).timeout(Duration::from_secs(60));
//! let limits = UpstreamResponseLimits {
//!     max_response_bytes: Some(10 * 1024 * 1024),
//!     ..UpstreamResponseLimits::default()
//! };
//! let mut response = fetcher.fetch_with_same_origin_redirects(request, "File link", limits).await?;
//! if !response.ok() {
//!     return Err(format!("File link answered HTTP {}", response.status().as_u16()).into());
//! }
//! // Fails with "File link response exceeded 10485760 bytes" past the limit.
//! Ok(response.bytes().await?)
//! # }
//! ```
//!
//! # An endpoint named by a remote document
//!
//! ```no_run
//! # async fn check(addresses: &mymcps_net::AddressGuard, discovered: &str) -> Result<(), Box<dyn std::error::Error>> {
//! use mymcps_net::parse_http_url;
//!
//! let mcp_url = parse_http_url("https://mcp.example/mcp", "MCP URL")?;
//! // One guard per MCP and per flow: it remembers what each name resolved to.
//! let endpoints = addresses.discovered_endpoint_guard(&mcp_url);
//!
//! let token_endpoint = parse_http_url(discovered, "OAuth token endpoint")?;
//! endpoints.assert_allowed(&token_endpoint, "OAuth token endpoint").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # In a test
//!
//! Where the TypeScript tests replaced `fetch`, a test hands the code a fetcher
//! that answers by itself. Redirects, credentials, limits and timeouts are
//! handled as for a response from the network. Where they started a server, a
//! test starts one on 127.0.0.1 and uses [`Fetcher::shared`] with its URL.
//! Where they replaced the name resolution of the guard, a test builds the
//! guard on a [`StaticResolver`].
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
//! use std::sync::{Arc, Mutex};
//!
//! use http::StatusCode;
//! use mymcps_net::{
//!     AddressGuard, CannedResponse, FetchRequest, Fetcher, SentRequest, StaticResolver,
//!     UpstreamResponseLimits,
//! };
//! use serde_json::json;
//!
//! let requests: Arc<Mutex<Vec<SentRequest>>> = Arc::default();
//! // `offline`: a request nothing answers fails instead of leaving the machine.
//! let fetcher = Fetcher::offline().answering({
//!     let requests = Arc::clone(&requests);
//!     move |request| {
//!         requests.lock().unwrap().push(request.clone());
//!         Some(match request.url.path() {
//!             "/api/v3/athlete" => CannedResponse::json(StatusCode::OK, &json!({ "id": 4242 })),
//!             _ => CannedResponse::new(StatusCode::NOT_FOUND).body("Record Not Found"),
//!         })
//!     }
//! });
//!
//! let request = FetchRequest::get("https://www.strava.com/api/v3/athlete".parse()?)
//!     .header("Authorization", "Bearer access-token")?;
//! let mut response = fetcher
//!     .fetch_with_same_origin_redirects(request, "Strava API", UpstreamResponseLimits::default())
//!     .await?;
//! assert_eq!(response.json::<serde_json::Value>().await?, json!({ "id": 4242 }));
//! let sent = requests.lock().unwrap();
//! assert_eq!(sent[0].header("authorization").as_deref(), Some("Bearer access-token"));
//!
//! // Only the names declared here resolve, and nothing is asked of the system.
//! let resolver = Arc::new(
//!     StaticResolver::new()
//!         .with("mcp.example", &["203.0.113.10".parse()?])
//!         .with("internal.example", &["10.0.0.5".parse()?]),
//! );
//! let addresses = AddressGuard::new(resolver.clone());
//! let endpoints = addresses.discovered_endpoint_guard(&"https://mcp.example/mcp".parse()?);
//! let refused = endpoints
//!     .assert_allowed(&"https://internal.example/token".parse()?, "OAuth token endpoint")
//!     .await;
//! assert!(refused.is_err());
//! assert_eq!(resolver.lookups(), ["mcp.example", "internal.example"]);
//! # Ok(())
//! # })
//! # }
//! ```

mod address_guard;
mod http_url;
mod safe_fetch;

pub use address_guard::{
    AddressGuard, DiscoveredEndpointGuard, Resolver, RestrictedEndpointError, StaticResolver,
    SystemResolver, is_restricted_address, is_restricted_ip,
};
pub use http_url::{HttpUrlError, parse_http_url};
pub use safe_fetch::{
    BodyStream, CannedResponse, DEFAULT_USER_AGENT, FetchError, FetchRequest, FetchResponse,
    Fetcher, MAX_UPSTREAM_ERROR_RESPONSE_BYTES, MAX_UPSTREAM_RESPONSE_BYTES, SentRequest,
    UpstreamResponseLimits, fetch_with_same_origin_redirects, shared_client,
};
