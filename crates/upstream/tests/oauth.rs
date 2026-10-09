//! `tests/unit/oauth.spec.ts`: the OAuth flow of an HTTP MCP, against
//! providers that answer from a closure.

mod support;

use base64::Engine;
use chrono::Duration;
use mymcps_core::models::{McpAuthType, McpStatus};
use mymcps_core::{TestCore, Timestamp};
use mymcps_net::CannedResponse;
use mymcps_upstream::{clear_oauth_session, read_oauth_session, uses_pasted_oauth_callback};
use serde_json::json;
use support::*;
use url::Url;

fn notion_oauth_server(call: &Call) -> CannedResponse {
    if call.url.contains("/.well-known/oauth-protected-resource") {
        return json_response(json!({
            "resource": "https://mcp.notion.com/mcp",
            "authorization_servers": ["https://auth.example"],
            "scopes_supported": ["notion"],
        }));
    }
    match call.url.as_str() {
        "https://auth.example/.well-known/oauth-authorization-server" => {
            json_response(authorization_server("https://auth.example"))
        }
        "https://auth.example/register" => json_response(json!({
            "client_id": "notion-client-123",
            "redirect_uris": [CALLBACK],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "client_name": "MyMCPs",
        })),
        "https://auth.example/token" if call.body.contains("grant_type=refresh_token") => {
            json_response(json!({
                "access_token": "refreshed-access-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refreshed-refresh-token",
            }))
        }
        "https://auth.example/token" => json_response(json!({
            "access_token": "access-token",
            "token_type": "bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-token",
            "scope": "notion",
        })),
        _ => not_found(),
    }
}

// MCP OAuth

#[tokio::test]
async fn discovers_registers_redirects_exchanges_and_refreshes_a_notion_style_mcp() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, notion_oauth_server);
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Notion".into();
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let session = MemorySession::new();

    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let authorization_url = Url::parse(&redirect).unwrap();
    let state = query(&authorization_url, "state");
    let oauth = read_oauth_session(&session, state.as_deref());

    assert_eq!(origin(&authorization_url), "https://auth.example");
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(
        query(&authorization_url, "client_id").as_deref(),
        Some("notion-client-123")
    );
    assert_eq!(
        query(&authorization_url, "redirect_uri").as_deref(),
        Some(CALLBACK)
    );
    assert_eq!(
        query(&authorization_url, "resource").as_deref(),
        Some("https://mcp.notion.com/mcp")
    );
    assert_eq!(
        query(&authorization_url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    let oauth = oauth.expect("the pending authorization is in the session");
    assert_eq!(oauth.redirect_uri, CALLBACK);
    assert_eq!(oauth.mcp_id, mcp.id);
    assert_eq!(mcp.oauth_issuer.as_deref(), Some("https://auth.example"));
    assert_eq!(
        mcp.oauth_resource.as_deref(),
        Some("https://mcp.notion.com/mcp")
    );
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("notion-client-123"));
    assert_eq!(mcp.oauth_client_auth_method.as_deref(), Some("none"));
    let state = state.unwrap();
    assert!(session.has(&format!("mcp_oauth:{state}")));
    // The state is 24 random bytes, and the session holds what the Node app kept.
    assert_eq!(state.len(), 32);
    let saved = session.value(&format!("mcp_oauth:{state}")).unwrap();
    assert_eq!(
        saved.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "mcpId",
            "redirectUri",
            "authorizationServerUrl",
            "resource",
            "clientId",
            "codeVerifier",
            "state"
        ]
    );

    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "authorization-code", None)
        .await
        .unwrap();

    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("access-token")
    );
    assert_eq!(
        decrypt(&core, &mcp.oauth_refresh_token).as_deref(),
        Some("refresh-token")
    );
    assert_eq!(mcp.oauth_token_type.as_deref(), Some("Bearer"));
    assert_eq!(mcp.oauth_scopes.as_deref(), Some("notion"));
    assert!(!mcp.oauth_required);
    assert!(mcp.oauth_token_expires_at.is_some());
    assert_eq!(mcp.status, McpStatus::Ready);

    mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    mcp.save(&*core.db).await.unwrap();
    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();

    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("refreshed-access-token")
    );
    assert_eq!(
        decrypt(&core, &mcp.oauth_refresh_token).as_deref(),
        Some("refreshed-refresh-token")
    );

    let token_requests = calls.to("https://auth.example/token");
    assert_eq!(token_requests[0].form("code_verifier"), oauth.code_verifier);
    assert_eq!(
        token_requests[0].form("resource").as_deref(),
        Some("https://mcp.notion.com/mcp")
    );
    assert_eq!(
        token_requests[1].form("resource").as_deref(),
        Some("https://mcp.notion.com/mcp")
    );
    let token_bodies: Vec<String> = token_requests.into_iter().map(|call| call.body).collect();
    assert_eq!(token_bodies.len(), 2);
    assert!(
        token_bodies[0]
            .contains("redirect_uri=http%3A%2F%2Flocalhost%3A3333%2Fmcps%2Foauth%2Fcallback")
    );
    assert!(token_bodies[0].contains("grant_type=authorization_code"));
    assert!(token_bodies[0].contains("code=authorization-code"));
    assert!(token_bodies[1].contains("grant_type=refresh_token"));
    assert!(token_bodies[1].contains("refresh_token=refresh-token"));

    let registration = &calls.to("https://auth.example/register")[0];
    assert_eq!(registration.json()["client_name"], "MyMCPs");
    // The registration names what the flow then asks for.
    assert_eq!(
        registration.body,
        r#"{"client_name":"MyMCPs","redirect_uris":["http://localhost:3333/mcps/oauth/callback"],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"none","scope":"notion"}"#
    );

    clear_oauth_session(&session, Some(&state));
    assert_eq!(read_oauth_session(&session, Some(&state)), None);
    assert_eq!(read_oauth_session(&session, None), None);
    assert_eq!(read_oauth_session(&session, Some("")), None);
}

#[tokio::test]
async fn registers_the_figma_mcp_with_an_allowlisted_client_name_and_loopback_redirect() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": "https://mcp.figma.com/mcp",
                "authorization_servers": ["https://api.figma.com"],
                "scopes_supported": ["mcp:connect"],
            }));
        }
        match call.url.as_str() {
            "https://api.figma.com/.well-known/oauth-authorization-server" => {
                json_response(json!({
                    "issuer": "https://api.figma.com",
                    "authorization_endpoint": "https://www.figma.com/oauth/mcp",
                    "token_endpoint": "https://api.figma.com/v1/oauth/token",
                    "registration_endpoint": "https://api.figma.com/v1/oauth/mcp/register",
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
                    "code_challenge_methods_supported": ["S256"],
                    "scopes_supported": ["mcp:connect"],
                }))
            }
            "https://api.figma.com/v1/oauth/mcp/register" => json_response(json!({
                "client_id": "figma-client-123",
                "client_secret": "figma-client-secret",
                "redirect_uris": ["http://localhost:45873/callback"],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "client_name": "Codex",
            })),
            "https://api.figma.com/v1/oauth/token" => json_response(json!({
                "access_token": "figma-access-token",
                "token_type": "bearer",
                "expires_in": 3600,
                "refresh_token": "figma-refresh-token",
            })),
            _ => not_found(),
        }
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Figma".into();
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let session = MemorySession::new();

    let redirect =
        Url::parse(&upstream.start_oauth_flow(&session, &mut mcp).await.unwrap()).unwrap();
    let registration = &calls.to("https://api.figma.com/v1/oauth/mcp/register")[0];

    let registered = registration.json();
    assert_eq!(registered["client_name"], "Codex");
    assert_eq!(
        registered["redirect_uris"],
        json!(["http://localhost:45873/callback"])
    );
    assert!(uses_pasted_oauth_callback(&mcp));
    let figma_host_requests: Vec<Call> = calls
        .all()
        .into_iter()
        .filter(|call| call.hostname() == "mcp.figma.com")
        .collect();
    assert!(!figma_host_requests.is_empty());
    for request in &figma_host_requests {
        assert_eq!(
            request.header("user-agent").as_deref(),
            Some("codex-mcp-client/0.0.0")
        );
    }
    // The identity belongs to the MCP host: the provider's own host does not get it.
    assert_eq!(registration.header("user-agent"), None);
    assert_eq!(origin(&redirect), "https://www.figma.com");
    assert_eq!(redirect.path(), "/oauth/mcp");
    assert_eq!(
        query(&redirect, "client_id").as_deref(),
        Some("figma-client-123")
    );
    assert_eq!(
        query(&redirect, "redirect_uri").as_deref(),
        Some("http://localhost:45873/callback")
    );
    assert_eq!(
        mcp.oauth_redirect_uri.as_deref(),
        Some("http://localhost:45873/callback")
    );
    assert_eq!(
        decrypt(&core, &mcp.oauth_client_secret).as_deref(),
        Some("figma-client-secret")
    );

    let oauth = read_oauth_session(&session, query(&redirect, "state").as_deref()).unwrap();
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "authorization-code", None)
        .await
        .unwrap();

    let token_request = &calls.to("https://api.figma.com/v1/oauth/token")[0];
    assert_eq!(
        token_request.header("authorization"),
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode("figma-client-123:figma-client-secret")
        ))
    );
    assert!(
        token_request
            .body
            .contains("redirect_uri=http%3A%2F%2Flocalhost%3A45873%2Fcallback")
    );
    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("figma-access-token")
    );
}

#[tokio::test]
async fn registers_the_strava_mcp_as_claude_code_with_the_normal_app_callback() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return json_response(json!({
                "resource": "https://mcp.strava.com/mcp",
                "authorization_servers": ["https://www.strava.com"],
                "scopes_supported": ["activity:read"],
            }));
        }
        match call.url.as_str() {
            "https://www.strava.com/.well-known/oauth-authorization-server" => {
                json_response(json!({
                    "issuer": "https://www.strava.com",
                    "authorization_endpoint": "https://www.strava.com/oauth/authorize",
                    "token_endpoint": "https://www.strava.com/oauth/token",
                    "registration_endpoint": "https://mcp.strava.com/register",
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "token_endpoint_auth_methods_supported": ["none"],
                    "code_challenge_methods_supported": ["S256"],
                    "scopes_supported": ["activity:read"],
                }))
            }
            "https://mcp.strava.com/register" => json_response(json!({
                "client_id": "strava-client-123",
                "redirect_uris": [CALLBACK],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "client_name": "Claude Code",
            })),
            _ => not_found(),
        }
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Strava".into();
        mcp.http_url = Some("https://mcp.strava.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let session = MemorySession::new();

    let redirect =
        Url::parse(&upstream.start_oauth_flow(&session, &mut mcp).await.unwrap()).unwrap();
    let registration = &calls.to("https://mcp.strava.com/register")[0];

    let registered = registration.json();
    assert_eq!(registered["client_name"], "Claude Code");
    assert_eq!(registered["redirect_uris"], json!([CALLBACK]));
    assert!(!uses_pasted_oauth_callback(&mcp));
    assert_eq!(
        registration.header("user-agent").as_deref(),
        Some("claude-code/2.1.89 (cli)")
    );
    let strava_host_requests: Vec<Call> = calls
        .all()
        .into_iter()
        .filter(|call| call.hostname() == "mcp.strava.com")
        .collect();
    assert!(!strava_host_requests.is_empty());
    for request in &strava_host_requests {
        assert_eq!(
            request.header("user-agent").as_deref(),
            Some("claude-code/2.1.89 (cli)")
        );
    }
    assert_eq!(origin(&redirect), "https://www.strava.com");
    assert_eq!(redirect.path(), "/oauth/authorize");
    assert_eq!(
        query(&redirect, "client_id").as_deref(),
        Some("strava-client-123")
    );
    assert_eq!(query(&redirect, "redirect_uri").as_deref(), Some(CALLBACK));
    assert_eq!(mcp.oauth_redirect_uri.as_deref(), Some(CALLBACK));
    assert_eq!(mcp.oauth_client_id.as_deref(), Some("strava-client-123"));
}

#[tokio::test]
async fn verifies_a_same_origin_path_issuer_discovered_through_legacy_root_metadata() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return status(404);
        }
        match call.url.as_str() {
            "https://mcp.example/.well-known/oauth-authorization-server"
            | "https://mcp.example/.well-known/oauth-authorization-server/mcp" => {
                json_response(json!({
                    "issuer": "https://mcp.example/mcp",
                    "authorization_endpoint": "https://mcp.example/mcp/authorize",
                    "token_endpoint": "https://mcp.example/mcp/token",
                    "registration_endpoint": "https://mcp.example/mcp/register",
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "token_endpoint_auth_methods_supported": ["none"],
                    "code_challenge_methods_supported": ["S256"],
                }))
            }
            "https://mcp.example/mcp/register" => json_response(json!({
                "client_id": "path-client-123",
                "redirect_uris": [CALLBACK],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "client_name": "MyMCPs",
            })),
            _ => not_found(),
        }
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Path issuer".into();
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;

    let redirect = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap();
    let redirect = Url::parse(&redirect).unwrap();

    assert_eq!(origin(&redirect), "https://mcp.example");
    assert_eq!(redirect.path(), "/mcp/authorize");
    assert_eq!(mcp.oauth_issuer.as_deref(), Some("https://mcp.example/mcp"));
    assert!(
        calls.all().iter().any(
            |call| call.url == "https://mcp.example/.well-known/oauth-authorization-server/mcp"
        )
    );
}

#[tokio::test]
async fn rejects_a_cross_origin_issuer_from_legacy_root_metadata_without_requesting_it() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |call| {
        if call.url.contains("/.well-known/oauth-protected-resource") {
            return status(404);
        }
        if call.url == "https://mcp.example/.well-known/oauth-authorization-server" {
            return json_response(json!({
                "issuer": "https://untrusted.example/oauth",
                "authorization_endpoint": "https://untrusted.example/oauth/authorize",
                "token_endpoint": "https://untrusted.example/oauth/token",
                "registration_endpoint": "https://untrusted.example/oauth/register",
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
            }));
        }
        not_found()
    });
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Cross-origin issuer".into();
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;

    let error = upstream
        .start_oauth_flow(&MemorySession::new(), &mut mcp)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "OAuth issuer metadata does not match the authorization server"
    );
    assert!(
        !calls
            .all()
            .iter()
            .any(|call| call.url.starts_with("https://untrusted.example/"))
    );
}

#[tokio::test]
async fn reports_an_actionable_error_when_an_mcp_has_no_oauth_metadata_or_client_registration() {
    let core = TestCore::new().await;
    let (upstream, _) = upstream(&core, |_| status(404));
    let mut mcp = create_mcp(&core, |mcp| {
        mcp.name = "Undiscoverable".into();
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let session = MemorySession::new();

    let error = upstream
        .start_oauth_flow(&session, &mut mcp)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "OAuth provider metadata could not be discovered"
    );
    assert!(session.pending().is_empty());
    assert_eq!(find_mcp(&core, mcp.id).await.oauth_issuer, None);
}

// MCP automatic authentication

#[tokio::test]
async fn marks_only_auto_http_mcps_as_requiring_oauth_after_an_unauthorized_response() {
    let core = TestCore::new().await;
    let (upstream, calls) = upstream(&core, |_| {
        json_status(
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
        .unwrap()
    });
    let mut automatic = create_mcp(&core, |mcp| {
        mcp.name = "Automatic auth".into();
        mcp.status = McpStatus::Draft;
    })
    .await;
    let mut rejected_token = create_mcp(&core, |mcp| {
        mcp.name = "Rejected OAuth token".into();
        mcp.oauth_access_token = encrypt(&core, "rejected-access-token");
        mcp.oauth_token_type = Some("bearer".into());
    })
    .await;
    let mut manual = create_mcp(&core, |mcp| {
        mcp.name = "Manual bearer".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.status = McpStatus::Draft;
    })
    .await;

    upstream
        .test_and_update_status(&mut automatic)
        .await
        .unwrap();
    upstream
        .test_and_update_status(&mut rejected_token)
        .await
        .unwrap();
    upstream.test_and_update_status(&mut manual).await.unwrap();

    assert_eq!(automatic.status, McpStatus::Draft);
    assert_eq!(
        automatic.last_error.as_deref(),
        Some("OAuth authorization required")
    );
    assert!(automatic.oauth_required);

    assert_eq!(rejected_token.status, McpStatus::Error);
    let last_error = rejected_token.last_error.clone().unwrap();
    assert!(last_error.contains("The access token audience is invalid"));
    assert!(!last_error.contains("rejected-access-token"));
    assert!(
        last_error.starts_with("OAuth token rejected. MCP server returned HTTP 401. Response: ")
    );
    assert!(last_error.contains(" | WWW-Authenticate: Bearer error=\"invalid_token\""));
    let authorization_headers: Vec<Option<String>> = calls
        .all()
        .iter()
        .map(|call| call.header("authorization"))
        .collect();
    assert!(authorization_headers.contains(&Some("Bearer rejected-access-token".to_owned())));
    assert!(!authorization_headers.contains(&Some("bearer rejected-access-token".to_owned())));
    assert!(rejected_token.oauth_required);

    assert_eq!(manual.status, McpStatus::Error);
    assert!(!manual.oauth_required);
    assert_eq!(
        manual.last_error.as_deref(),
        Some(
            r#"MCP server returned HTTP 401. Response: {"error":"invalid_token","error_description":"The access token audience is invalid"} | WWW-Authenticate: Bearer error="invalid_token", error_description="The access token audience is invalid""#
        )
    );

    // What was found is saved, and nothing else of the row is touched.
    let saved = find_mcp(&core, rejected_token.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert_eq!(saved.last_error, rejected_token.last_error);
    assert!(saved.oauth_required);
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("rejected-access-token")
    );
}
