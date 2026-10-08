//! Port of `tests/functional/mcp_oauth.spec.ts`: the OAuth connection of an
//! HTTP MCP through the browser, from the start route to the callback.

mod mcps_support;

use http::StatusCode;
use mcps_support::*;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, User};
use mymcps_net::CannedResponse;
use mymcps_web::testing::factories::{create_admin_with, create_mcp};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;
use url::Url;

#[derive(Clone, Copy, Default)]
struct Provider {
    reject_mcp_token: bool,
    reject_token_exchange: bool,
}

fn unauthorized(description: &str, challenge: &str) -> CannedResponse {
    json_status(
        401,
        json!({ "error": "invalid_token", "error_description": description }),
    )
    .header("WWW-Authenticate", challenge)
    .unwrap()
}

/// What Notion's MCP server and its authorization server answer.
fn notion(provider: Provider, call: &Call) -> CannedResponse {
    let url = call.url.as_str();
    if url.contains("/.well-known/oauth-protected-resource") {
        return json_response(json!({
            "resource": "https://mcp.notion.com/mcp",
            "authorization_servers": ["https://auth.example"],
            "scopes_supported": ["notion"],
        }));
    }
    if url == "https://auth.example/.well-known/oauth-authorization-server" {
        return json_response(authorization_server("https://auth.example"));
    }
    if url == "https://auth.example/register" {
        let mut client = registered_client("notion-client-123");
        client["client_name"] = json!("MyMCPs");
        return json_response(client);
    }
    if url == "https://auth.example/token" {
        if provider.reject_token_exchange {
            let code = call.form("code").unwrap_or_default();
            return json_status(
                400,
                json!({
                    "error": "invalid_grant",
                    "error_description": format!("authorization code {code} rejected"),
                }),
            );
        }
        return json_response(json!({
            "access_token": "access-token",
            "token_type": "bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-token",
            "scope": "notion",
        }));
    }
    if !url.starts_with("https://mcp.notion.com/mcp") {
        return status(404).body(format!("not found: {} {url}", call.method));
    }

    if call.header("authorization").as_deref() != Some("Bearer access-token") {
        return unauthorized(
            "Missing or invalid access token",
            "Bearer error=\"invalid_token\"",
        );
    }
    if provider.reject_mcp_token {
        return unauthorized(
            "The access token audience is invalid",
            "Bearer error=\"invalid_token\", error_description=\"The access token audience is invalid\"",
        );
    }
    let message = call.json();
    let session = |response: CannedResponse| {
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
        _ => status(404).body("not found"),
    }
}

struct Flow {
    app: TestApp,
    calls: Calls,
    admin: User,
    mcp: Mcp,
}

/// An MCP that wants an OAuth authorization, behind a provider that behaves as `provider` says.
async fn notion_mcp(provider: Provider, email: &str, name: &str) -> Flow {
    let (app, calls) = app_answering(move |call| notion(provider, call)).await;
    let admin = create_admin_with(&app, email).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = name.to_string();
        mcp.auth_type = McpAuthType::Auto;
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    Flow {
        app,
        calls,
        admin,
        mcp,
    }
}

impl Flow {
    /// Follow the start route to the provider, and say where it sends the browser.
    async fn start(&self) -> (TestResponse, Url) {
        let response = self
            .app
            .get(&format!("/mcps/{}/oauth/start", self.mcp.id))
            .login_as(&self.admin)
            .send()
            .await;
        assert_eq!(response.status, StatusCode::FOUND);
        let authorization_url = Url::parse(response.location().unwrap()).unwrap();
        (response, authorization_url)
    }

    /// Come back from the provider with these parameters, in the session the start left.
    async fn callback(&self, start: &TestResponse, parameters: &[(&str, &str)]) -> TestResponse {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in parameters {
            query.append_pair(name, value);
        }
        self.app
            .get(&format!("/mcps/oauth/callback?{}", query.finish()))
            .session(start.session())
            .send()
            .await
    }
}

#[tokio::test]
async fn uses_a_full_page_redirect_and_completes_the_browser_callback() {
    let flow = notion_mcp(Provider::default(), "admin@example.com", "Notion").await;

    let (start, authorization_url) = flow.start().await;
    assert_eq!(
        authorization_url.origin().ascii_serialization(),
        "https://auth.example"
    );
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(
        query(&authorization_url, "client_id").as_deref(),
        Some("notion-client-123")
    );
    let redirect_uri = Url::parse(&query(&authorization_url, "redirect_uri").unwrap()).unwrap();
    assert_eq!(redirect_uri.as_str(), CALLBACK);
    assert_eq!(redirect_uri.path(), "/mcps/oauth/callback");

    let state = query(&authorization_url, "state").unwrap();
    let callback = flow
        .callback(&start, &[("code", "authorization-code"), ("state", &state)])
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    // Nothing of the callback is carried over to the registry.
    assert_eq!(callback.location(), Some("/mcps"));
    assert_eq!(callback.flashed("success"), Some(json!("OAuth connected")));
    assert_eq!(callback.flashed("editingMcpId"), Some(json!(flow.mcp.id)));
    let saved = find_mcp(&flow.app, flow.mcp.id).await;
    assert_eq!(
        decrypt(&flow.app, &saved.oauth_access_token).as_deref(),
        Some("access-token")
    );
    assert_eq!(saved.status, McpStatus::Ready);
    assert!(!saved.oauth_required);
    // The pending authorization is used once.
    assert!(
        !callback
            .session()
            .keys()
            .any(|key| key.starts_with("mcp_oauth:"))
    );
    let exchange = &flow.calls.with_path("/token")[0];
    assert_eq!(exchange.form("code").as_deref(), Some("authorization-code"));

    // The dialog then opens on a connected MCP that can be authorized again.
    let page = flow
        .app
        .get("/mcps")
        .session(callback.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">OAuth connected</p>"));
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("Edit Notion"));
    assert!(!dialog.contains("Authorization required"));
    assert!(dialog.contains(&format!(
        "href=\"/mcps/{}/oauth/start\">Re-authorize</a>",
        flow.mcp.id
    )));
    assert!(!page.contains("access-token"));
}

#[tokio::test]
async fn reports_a_rejected_oauth_token_instead_of_claiming_the_mcp_connected() {
    let provider = Provider {
        reject_mcp_token: true,
        ..Provider::default()
    };
    let flow = notion_mcp(provider, "rejected@example.com", "Rejected Notion token").await;

    let (start, authorization_url) = flow.start().await;
    let state = query(&authorization_url, "state").unwrap();
    let callback = flow
        .callback(&start, &[("code", "authorization-code"), ("state", &state)])
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    let error = callback.flashed("error").unwrap();
    let error = error.as_str().unwrap();
    assert!(
        error.contains("The access token audience is invalid"),
        "{error}"
    );
    assert!(!error.contains("access-token"));
    assert_eq!(callback.flashed("success"), None);

    let saved = find_mcp(&flow.app, flow.mcp.id).await;
    assert_eq!(
        decrypt(&flow.app, &saved.oauth_access_token).as_deref(),
        Some("access-token")
    );
    assert_eq!(saved.status, McpStatus::Error);
    assert!(saved.oauth_required);
}

#[tokio::test]
async fn redacts_callback_credentials_echoed_by_a_failed_token_exchange() {
    let provider = Provider {
        reject_token_exchange: true,
        ..Provider::default()
    };
    let flow = notion_mcp(
        provider,
        "exchange-error@example.com",
        "Failed token exchange",
    )
    .await;

    let (start, authorization_url) = flow.start().await;
    let state = query(&authorization_url, "state").unwrap();
    let code = "authorization-code-sensitive-value";
    let callback = flow
        .callback(&start, &[("code", code), ("state", &state)])
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    let error = callback.flashed("error").unwrap();
    let error = error.as_str().unwrap();
    assert!(!error.contains(code), "{error}");
    assert!(error.contains("[REDACTED]"));
    assert_eq!(callback.location(), Some("/mcps"));

    let saved = find_mcp(&flow.app, flow.mcp.id).await;
    let last_error = saved.last_error.unwrap();
    assert!(!last_error.contains(code));
    assert!(last_error.contains("[REDACTED]"));
    assert_eq!(saved.status, McpStatus::Error);
    assert_eq!(callback.flashed("editingMcpId"), Some(json!(flow.mcp.id)));
}

#[tokio::test]
async fn redacts_a_configured_secret_from_a_provider_controlled_oauth_callback_error() {
    let flow = notion_mcp(
        Provider::default(),
        "callback-error@example.com",
        "OAuth callback error",
    )
    .await;
    let secret = "exact-callback-secret-value";
    let mut mcp = find_mcp(&flow.app, flow.mcp.id).await;
    mcp.auth_bearer = encrypt(&flow.app, secret);
    mcp.save(&*flow.app.core.db).await.unwrap();

    let (start, authorization_url) = flow.start().await;
    let callback_state = query(&authorization_url, "state").unwrap();
    let callback_code = "provider-error-code";
    let callback = flow
        .callback(
            &start,
            &[
                ("state", &callback_state),
                ("code", callback_code),
                (
                    "error",
                    &format!("provider echoed {secret} {callback_code} {callback_state}"),
                ),
            ],
        )
        .await;

    assert_eq!(callback.status, StatusCode::FOUND);
    assert_eq!(
        callback.flashed("error"),
        Some(json!(
            "OAuth error: provider echoed [REDACTED] [REDACTED] [REDACTED]"
        ))
    );
    assert_eq!(callback.location(), Some("/mcps"));
    assert_eq!(callback.flashed("editingMcpId"), Some(json!(flow.mcp.id)));
    // The provider refused: nothing was exchanged.
    assert!(flow.calls.with_path("/token").is_empty());
    assert_eq!(
        find_mcp(&flow.app, flow.mcp.id).await.oauth_access_token,
        None
    );
}

#[tokio::test]
async fn refuses_a_callback_that_does_not_answer_an_authorization_of_this_session() {
    let flow = notion_mcp(Provider::default(), "forged@example.com", "Forged callback").await;
    let (start, authorization_url) = flow.start().await;
    let state = query(&authorization_url, "state").unwrap();
    let invalid = |response: &TestResponse| {
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(response.location(), Some("/mcps"));
        assert_eq!(
            response.flashed("error"),
            Some(json!("Invalid OAuth callback"))
        );
        assert_eq!(response.flashed("editingMcpId"), None);
    };

    // A state this session did not start.
    invalid(
        &flow
            .callback(
                &start,
                &[("code", "authorization-code"), ("state", "forged-state")],
            )
            .await,
    );
    // The state without the code it answers with.
    invalid(&flow.callback(&start, &[("state", &state)]).await);
    // Another session: the state means nothing there.
    let elsewhere = flow
        .app
        .get(&format!(
            "/mcps/oauth/callback?code=authorization-code&state={state}"
        ))
        .login_as(&flow.admin)
        .send()
        .await;
    invalid(&elsewhere);
    // A comma makes the parameter a list, which is not a state.
    let listed = flow
        .app
        .get(&format!(
            "/mcps/oauth/callback?state={state},{state}&code=authorization-code"
        ))
        .session(start.session())
        .send()
        .await;
    invalid(&listed);
    // Longer than a provider answers with.
    invalid(
        &flow
            .callback(&start, &[("code", &"c".repeat(8193)), ("state", &state)])
            .await,
    );

    assert!(flow.calls.with_path("/token").is_empty());
    assert_eq!(
        find_mcp(&flow.app, flow.mcp.id).await.oauth_access_token,
        None
    );

    // A visitor is sent to sign in before anything is read.
    let visitor = flow
        .app
        .get(&format!(
            "/mcps/oauth/callback?code=authorization-code&state={state}"
        ))
        .send()
        .await;
    assert_redirect(&visitor, "/login");
}

#[tokio::test]
async fn says_so_when_the_mcp_of_a_callback_is_gone() {
    let flow = notion_mcp(Provider::default(), "gone@example.com", "Deleted meanwhile").await;
    let (start, authorization_url) = flow.start().await;
    let state = query(&authorization_url, "state").unwrap();
    find_mcp(&flow.app, flow.mcp.id)
        .await
        .delete(&*flow.app.core.db)
        .await
        .unwrap();

    let callback = flow
        .callback(&start, &[("code", "authorization-code"), ("state", &state)])
        .await;

    assert_eq!(callback.location(), Some("/mcps"));
    assert_eq!(callback.flashed("error"), Some(json!("MCP not found")));
    assert!(flow.calls.with_path("/token").is_empty());
}
