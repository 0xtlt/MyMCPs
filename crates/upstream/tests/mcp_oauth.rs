//! `tests/functional/mcp_oauth.spec.ts` and the OAuth cases of
//! `tests/functional/hardening_upstream_mcps.spec.ts`, as far as they go
//! without the pages: what the routes `/mcps/:id/oauth/start` and
//! `/mcps/oauth/callback` ask of this crate, in the order they ask it.

mod support;

use std::sync::Arc;

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpStatus};
use mymcps_core::redaction::sanitize_mcp_diagnostic_with;
use mymcps_upstream::{OauthSession, Upstream, clear_oauth_session, read_oauth_session};
use serde_json::json;
use support::*;
use url::Url;

#[derive(Clone, Copy, Default)]
struct Options {
    reject_mcp_token: bool,
    reject_token_exchange: bool,
}

/// A Notion-style MCP with its provider: the MCP only answers the access
/// token the provider issues.
fn notion(core: &TestCore, options: Options) -> (Arc<Upstream>, Calls) {
    upstream(core, move |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": "https://mcp.notion.com/mcp",
                "authorization_servers": ["https://auth.example"],
                "scopes_supported": ["notion"],
            }));
        }
        match call.url.as_str() {
            "https://auth.example/.well-known/oauth-authorization-server" => {
                return json_response(authorization_server("https://auth.example"));
            }
            "https://auth.example/register" => {
                return json_response(json!({
                    "client_id": "notion-client-123",
                    "redirect_uris": [CALLBACK],
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"],
                    "token_endpoint_auth_method": "none",
                    "client_name": "MyMCPs",
                }));
            }
            "https://auth.example/token" if options.reject_token_exchange => {
                let code = call.form("code").unwrap_or_default();
                return json_status(
                    400,
                    json!({
                        "error": "invalid_grant",
                        "error_description": format!("authorization code {code} rejected"),
                    }),
                );
            }
            "https://auth.example/token" => {
                return json_response(json!({
                    "access_token": "access-token",
                    "token_type": "bearer",
                    "expires_in": 3600,
                    "refresh_token": "refresh-token",
                    "scope": "notion",
                }));
            }
            _ => {}
        }
        if !call.url.starts_with("https://mcp.notion.com/mcp") {
            return not_found();
        }

        if call.header("authorization").as_deref() != Some("Bearer access-token") {
            return json_status(
                401,
                json!({
                    "error": "invalid_token",
                    "error_description": "Missing or invalid access token",
                }),
            )
            .header("WWW-Authenticate", r#"Bearer error="invalid_token""#)
            .unwrap();
        }
        if options.reject_mcp_token {
            return json_status(
                401,
                json!({
                    "error": "invalid_token",
                    "error_description": "The access token audience is invalid",
                }),
            )
            .header(
                "WWW-Authenticate",
                r#"Bearer error="invalid_token", error_description="The access token audience is invalid""#,
            )
            .unwrap();
        }
        let message = call.json();
        let session = |response: mymcps_net::CannedResponse| {
            response
                .header("Mcp-Session-Id", "notion-test-session")
                .unwrap()
        };
        match message["method"].as_str() {
            Some("initialize") => session(json_response(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "serverInfo": { "name": "Notion mock", "version": "1.0.0" },
                },
            }))),
            Some("tools/list") => session(json_response(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": { "tools": [] },
            }))),
            Some("notifications/initialized") => session(status(202)),
            _ => not_found(),
        }
    })
}

async fn notion_mcp(core: &TestCore, name: &str) -> Mcp {
    create_mcp(core, |mcp| {
        mcp.name = name.to_owned();
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await
}

/// What the start route does, then what the browser brings back: the
/// authorization URL and the pending authorization of its state.
async fn start(
    upstream: &Upstream,
    session: &MemorySession,
    mcp: &mut Mcp,
) -> (Url, String, OauthSession) {
    let authorization_url =
        Url::parse(&upstream.start_oauth_flow(session, mcp).await.unwrap()).unwrap();
    let state = query(&authorization_url, "state").unwrap();
    // The callback reads the pending authorization, then clears it.
    let oauth = read_oauth_session(session, Some(&state)).unwrap();
    clear_oauth_session(session, Some(&state));
    assert!(session.pending().is_empty());
    (authorization_url, state, oauth)
}

// MCP OAuth routes

#[tokio::test]
async fn uses_a_full_page_redirect_and_completes_the_browser_callback() {
    let core = TestCore::new().await;
    let (upstream, calls) = notion(&core, Options::default());
    let mut mcp = notion_mcp(&core, "Notion").await;
    let session = MemorySession::new();

    let (authorization_url, state, oauth) = start(&upstream, &session, &mut mcp).await;
    assert_eq!(origin(&authorization_url), "https://auth.example");
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(
        query(&authorization_url, "client_id").as_deref(),
        Some("notion-client-123")
    );
    assert_eq!(
        query(&authorization_url, "redirect_uri").as_deref(),
        Some(upstream.oauth_callback_url().unwrap().as_str())
    );
    assert_eq!(oauth.state, state);
    assert_eq!(oauth.mcp_id, mcp.id);

    // The callback loads the MCP of the pending authorization.
    let mut mcp = find_mcp(&core, oauth.mcp_id).await;
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "authorization-code", None)
        .await
        .unwrap();
    upstream.test_and_update_status(&mut mcp).await.unwrap();

    assert_eq!(mcp.status, McpStatus::Ready);
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-token")
    );
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);
    assert_eq!(saved.last_error, None);
    // The session the MCP opened is repeated on the requests that follow.
    let listing = calls
        .all()
        .into_iter()
        .find(|call| call.json()["method"] == "tools/list")
        .unwrap();
    assert_eq!(
        listing.header("mcp-session-id").as_deref(),
        Some("notion-test-session")
    );
    assert_eq!(
        listing.header("mcp-protocol-version").as_deref(),
        Some("2025-06-18")
    );
}

#[tokio::test]
async fn reports_a_rejected_oauth_token_instead_of_claiming_the_mcp_connected() {
    let core = TestCore::new().await;
    let (upstream, _) = notion(
        &core,
        Options {
            reject_mcp_token: true,
            ..Options::default()
        },
    );
    let mut mcp = notion_mcp(&core, "Rejected Notion token").await;
    let session = MemorySession::new();
    let (_, _, oauth) = start(&upstream, &session, &mut mcp).await;

    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "authorization-code", None)
        .await
        .unwrap();
    upstream.test_and_update_status(&mut mcp).await.unwrap();

    // What the callback shows the administrator.
    let error = mcp.last_error.clone().unwrap();
    assert!(error.contains("The access token audience is invalid"));
    assert!(!error.contains("access-token"));
    assert_ne!(mcp.status, McpStatus::Ready);

    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("access-token")
    );
    assert_eq!(saved.status, McpStatus::Error);
    assert!(saved.oauth_required);
}

#[tokio::test]
async fn redacts_callback_credentials_echoed_by_a_failed_token_exchange() {
    let core = TestCore::new().await;
    let (upstream, _) = notion(
        &core,
        Options {
            reject_token_exchange: true,
            ..Options::default()
        },
    );
    let mut mcp = notion_mcp(&core, "Failed token exchange").await;
    let session = MemorySession::new();
    let (_, state, oauth) = start(&upstream, &session, &mut mcp).await;
    let code = "authorization-code-sensitive-value";

    let error = upstream
        .exchange_authorization_code(&mut mcp, &oauth, code, None)
        .await
        .unwrap_err();

    // The provider echoed the code, and the error says what the provider said.
    assert_eq!(
        error.to_string(),
        "authorization code authorization-code-sensitive-value rejected"
    );
    // The callback stores and shows it without the credentials of the callback.
    let shown = sanitize_mcp_diagnostic_with(
        &core.encryption,
        &error.to_string(),
        &mcp,
        500,
        [code, state.as_str()],
    );
    assert!(!shown.contains(code));
    assert!(shown.contains("[REDACTED]"));
    // Nothing was saved of an exchange that failed.
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.status, McpStatus::Draft);
}

// Starting an upstream OAuth authorization

#[tokio::test]
async fn asks_the_provider_for_what_the_mcp_names_and_nothing_else() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": "https://mcp.example/mcp",
                "authorization_servers": ["https://auth.example"],
                "scopes_supported": ["read"],
            }));
        }
        match call.url.as_str() {
            "https://auth.example/.well-known/oauth-authorization-server" => {
                // The provider's own endpoint carries a parameter of its own.
                json_response(authorization_server_with(
                    "https://auth.example",
                    json!({ "authorization_endpoint": "https://auth.example/authorize?tenant=one&scope=admin" }),
                ))
            }
            "https://auth.example/register" => {
                json_response(registered_client("registered-client"))
            }
            _ => not_found(),
        }
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "OAuth MCP".into();
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;

    let location = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();

    let authorization = Url::parse(&location).unwrap();
    assert_eq!(origin(&authorization), "https://auth.example");
    let parameters: Vec<(String, String)> = authorization
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let names: Vec<&str> = parameters.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "tenant",
            "scope",
            "response_type",
            "client_id",
            "code_challenge",
            "code_challenge_method",
            "redirect_uri",
            "state",
            "resource"
        ]
    );
    let all = |name: &str| -> Vec<&str> {
        parameters
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    };
    assert_eq!(all("scope"), ["read"]);
    assert_eq!(all("redirect_uri"), [CALLBACK]);
    assert_eq!(all("response_type"), ["code"]);
    assert_eq!(all("client_id"), ["registered-client"]);
    assert_eq!(all("resource"), ["https://mcp.example/mcp"]);
    assert!(all("prompt").is_empty());
    assert_eq!(location.split('?').count(), 2);

    assert_eq!(
        calls
            .all()
            .iter()
            .map(|call| format!("{} {}", call.method, call.url))
            .collect::<Vec<_>>(),
        [
            "GET https://mcp.example/.well-known/oauth-protected-resource/mcp",
            "GET https://auth.example/.well-known/oauth-authorization-server",
            "POST https://auth.example/register",
        ]
    );
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_client_id.as_deref(), Some("registered-client"));
    assert_eq!(saved.oauth_issuer.as_deref(), Some("https://auth.example"));
    assert_eq!(
        saved.oauth_authorize_url.as_deref(),
        Some("https://auth.example/authorize?tenant=one&scope=admin")
    );
    assert_eq!(
        saved.oauth_token_url.as_deref(),
        Some("https://auth.example/token")
    );
    assert_eq!(saved.oauth_scopes.as_deref(), Some("read"));
    assert_eq!(saved.oauth_redirect_uri.as_deref(), Some(CALLBACK));
    assert_eq!(saved.oauth_client_secret, None);
}

#[tokio::test]
async fn needs_the_public_address_of_the_instance_to_tell_the_provider_where_to_return() {
    let core = TestCore::with_config(|config| config.app_url = None).await;
    let (upstream, calls) = upstream(&core, |_| not_found());
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.http_url = Some("https://mcp.example/mcp".into());
    })
    .await;

    assert_eq!(
        upstream.oauth_callback_url().unwrap_err().to_string(),
        "APP_URL is not configured. Set it to the public HTTPS origin and redeploy."
    );
    let error = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "APP_URL is not configured. Set it to the public HTTPS origin and redeploy."
    );
    assert_eq!(calls.len(), 0);

    // An MCP that is not reached over HTTP has no provider to discover.
    let mut npm = create_mcp(&core, |mcp| {
        mcp.transport = mymcps_core::models::McpTransport::Npm;
    })
    .await;
    assert_eq!(
        upstream
            .start_oauth_flow(&MemorySession::new(), &mut npm)
            .await
            .unwrap_err()
            .to_string(),
        "OAuth is supported only for HTTP MCPs"
    );
}

#[tokio::test]
async fn does_not_exchange_a_code_for_a_client_or_a_session_that_is_not_the_one_that_started() {
    let core = TestCore::new().await;
    let (upstream, calls) = notion(&core, Options::default());
    let mut mcp = notion_mcp(&core, "Notion").await;
    let session = MemorySession::new();
    let (_, _, oauth) = start(&upstream, &session, &mut mcp).await;
    calls.clear();

    let mut without_verifier = oauth.clone();
    without_verifier.code_verifier = None;
    assert_eq!(
        upstream
            .exchange_authorization_code(&mut mcp, &without_verifier, "code", None)
            .await
            .unwrap_err()
            .to_string(),
        "OAuth session is missing its PKCE code verifier"
    );

    let mut other_client = oauth.clone();
    other_client.client_id = "another-client".into();
    assert_eq!(
        upstream
            .exchange_authorization_code(&mut mcp, &other_client, "code", None)
            .await
            .unwrap_err()
            .to_string(),
        "OAuth client information is no longer available"
    );

    // A resource that is not the MCP is not sent to the token endpoint.
    let mut other_resource = oauth.clone();
    other_resource.resource = Some("https://api.other.example/".into());
    assert_eq!(
        upstream
            .exchange_authorization_code(&mut mcp, &other_resource, "code", None)
            .await
            .unwrap_err()
            .to_string(),
        "OAuth protected resource does not match the MCP URL"
    );
    assert!(calls.posts().is_empty());
}
