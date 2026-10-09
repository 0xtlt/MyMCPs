//! The OAuth sign-in of built-in MCPs, against a provider that answers from
//! a script instead of the network.

use std::sync::{Arc, Mutex};

use http::StatusCode;
use mymcps_builtin::oauth::{
    TOKEN_FAILURE_VALIDATOR, builtin_authorization_url, exchange_builtin_authorization_code,
    parse_oauth_scopes, refresh_builtin_tokens, requested_builtin_scopes,
};
use mymcps_builtin::{BuiltinEnv, BuiltinOauthConfig};
use mymcps_core::TestCore;
use mymcps_core::models::Mcp;
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use serde_json::{Value, json};

fn strava() -> BuiltinOauthConfig {
    BuiltinOauthConfig {
        issuer: "https://www.strava.com",
        authorize_url: "https://www.strava.com/oauth/authorize",
        token_url: "https://www.strava.com/api/v3/oauth/token",
        scopes: vec!["read", "read_all", "profile:read_all", "activity:read_all"],
        write_scopes: vec!["activity:write", "profile:write"],
        scope_separator: ",",
        authorize_params: vec![("approval_prompt", "force")],
        sends_redirect_uri_with_code: false,
        client_id_pattern: None,
        client_id_hint: None,
    }
}

struct Provider {
    env: BuiltinEnv,
    mcp: Mcp,
    requests: Arc<Mutex<Vec<SentRequest>>>,
    _core: TestCore,
}

/// A connected MCP whose token endpoint gives this answer.
async fn answering(status: u16, body: Value) -> Provider {
    let core = TestCore::new().await;
    let requests: Arc<Mutex<Vec<SentRequest>>> = Arc::default();
    let fetcher = Fetcher::offline().answering({
        let requests = requests.clone();
        move |request| {
            requests.lock().unwrap().push(request.clone());
            let status = StatusCode::from_u16(status).unwrap();
            Some(match &body {
                Value::String(text) => CannedResponse::new(status).body(text.clone()),
                body => CannedResponse::json(status, body),
            })
        }
    });
    let mcp = Mcp {
        id: 7,
        oauth_client_id: Some("123456".into()),
        oauth_client_secret: core.encrypt_secret(Some("client secret & more")),
        ..Default::default()
    };
    Provider {
        env: BuiltinEnv::new(core.core.clone()).with_fetcher(fetcher),
        mcp,
        requests,
        _core: core,
    }
}

#[test]
fn reads_scopes_whatever_separates_them() {
    assert_eq!(
        parse_oauth_scopes(Some("read,activity:read_all profile:write")),
        ["read", "activity:read_all", "profile:write"]
    );
    assert_eq!(
        parse_oauth_scopes(Some("  read ,\n read_all,,")),
        ["read", "read_all"]
    );
    assert_eq!(
        parse_oauth_scopes(Some("https://www.googleapis.com/auth/adwords")),
        ["https://www.googleapis.com/auth/adwords"]
    );
    // The callback's parameter passes through the browser.
    let long = "s".repeat(65);
    assert_eq!(
        parse_oauth_scopes(Some(&format!("read,<script>,sc\u{e9}ope,{long},a b"))),
        ["read", "a", "b"]
    );
    assert_eq!(parse_oauth_scopes(Some(&vec!["s"; 40].join(","))).len(), 32);
    assert!(parse_oauth_scopes(None).is_empty());
    assert!(parse_oauth_scopes(Some("")).is_empty());
}

#[test]
fn builds_the_strava_authorization_url_with_comma_separated_scopes() {
    let oauth = strava();
    let url = builtin_authorization_url(
        &oauth,
        "123456",
        "https://mcp.example.com/mcps/oauth/callback",
        "state-value",
        &oauth.scopes,
    )
    .unwrap();
    let url = url::Url::parse(&url).unwrap();

    assert_eq!(
        format!("{}{}", url.origin().ascii_serialization(), url.path()),
        "https://www.strava.com/oauth/authorize"
    );
    let parameters: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(
        parameters,
        [
            ("client_id", "123456"),
            (
                "redirect_uri",
                "https://mcp.example.com/mcps/oauth/callback"
            ),
            ("response_type", "code"),
            ("scope", "read,read_all,profile:read_all,activity:read_all"),
            ("state", "state-value"),
            ("approval_prompt", "force"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
    );
}

#[test]
fn requests_write_scopes_only_once_write_access_is_allowed() {
    let oauth = strava();
    let read_only = Mcp::default();
    let writable = Mcp {
        builtin_write_enabled: true,
        ..Default::default()
    };
    assert_eq!(
        requested_builtin_scopes(&oauth, &read_only),
        ["read", "read_all", "profile:read_all", "activity:read_all"]
    );
    assert_eq!(
        requested_builtin_scopes(&oauth, &writable),
        [
            "read",
            "read_all",
            "profile:read_all",
            "activity:read_all",
            "activity:write",
            "profile:write"
        ]
    );
}

#[tokio::test]
async fn exchanges_a_code_with_the_client_credentials() {
    let provider = answering(200, json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": "21600", "athlete": { "id": 1 } })).await;
    let tokens = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "the code",
        "https://mcp.example.com/mcps/oauth/callback",
    )
    .await
    .unwrap();

    assert_eq!(tokens.access_token, "access");
    assert_eq!(tokens.token_type, "Bearer");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
    assert_eq!(tokens.expires_in, Some(21600.0));
    assert_eq!(tokens.scope, None);

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, http::Method::POST);
    assert_eq!(
        requests[0].url.as_str(),
        "https://www.strava.com/api/v3/oauth/token"
    );
    assert_eq!(
        requests[0].header("accept").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        requests[0].header("content-type").as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    // Strava does not document the redirect URI here, so it is not sent.
    assert_eq!(
        requests[0].text(),
        "client_id=123456&client_secret=client+secret+%26+more&grant_type=authorization_code&code=the+code"
    );
}

#[tokio::test]
async fn sends_the_redirect_uri_again_to_providers_that_want_it() {
    let provider = answering(200, json!({ "access_token": "access", "token_type": "bearer", "scope": "https://www.googleapis.com/auth/adwords" })).await;
    let google = BuiltinOauthConfig {
        sends_redirect_uri_with_code: true,
        token_url: "https://oauth2.googleapis.com/token",
        ..strava()
    };
    let tokens = exchange_builtin_authorization_code(
        &provider.env,
        "Google Ads",
        &google,
        &provider.mcp,
        "c",
        "https://mcp.example.com/mcps/oauth/callback",
    )
    .await
    .unwrap();

    assert_eq!(tokens.token_type, "bearer");
    assert_eq!(
        tokens.scope.as_deref(),
        Some("https://www.googleapis.com/auth/adwords")
    );
    assert!(provider.requests.lock().unwrap()[0].text().ends_with("&grant_type=authorization_code&code=c&redirect_uri=https%3A%2F%2Fmcp.example.com%2Fmcps%2Foauth%2Fcallback"));
}

#[tokio::test]
async fn a_refused_refresh_asks_to_authorize_again() {
    let provider = answering(400, json!({ "message": "Token was revoked" })).await;
    let error = refresh_builtin_tokens(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "old refresh",
    )
    .await
    .unwrap_err();
    assert!(error.is_authorization_error());
    assert_eq!(
        error.to_string(),
        "Strava refused to renew the saved authorization (Token was revoked). Check the Client ID and Client Secret, then re-authorize this MCP in MyMCPs."
    );
    assert!(
        provider.requests.lock().unwrap()[0]
            .text()
            .ends_with("&grant_type=refresh_token&refresh_token=old+refresh")
    );

    let provider = answering(401, json!("not json")).await;
    let error = refresh_builtin_tokens(&provider.env, "Strava", &strava(), &provider.mcp, "r")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Strava refused to renew the saved authorization. Check the Client ID and Client Secret, then re-authorize this MCP in MyMCPs."
    );

    // A provider that is down did not refuse anything.
    let provider = answering(503, json!({ "error": "temporarily_unavailable" })).await;
    let error = refresh_builtin_tokens(&provider.env, "Strava", &strava(), &provider.mcp, "r")
        .await
        .unwrap_err();
    assert!(!error.is_tool_error());
    assert_eq!(
        error.to_string(),
        "Strava rejected the token request (HTTP 503): temporarily_unavailable"
    );
}

#[tokio::test]
async fn explains_a_rejected_code_exchange() {
    let provider = answering(401, json!({ "message": "Authorization Error", "errors": [{ "resource": "Application", "field": "client_secret", "code": "invalid" }, {}] })).await;
    let error = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "c",
        "r",
    )
    .await
    .unwrap_err();
    assert!(!error.is_tool_error());
    assert_eq!(
        error.to_string(),
        "Strava rejected the token request (HTTP 401): Authorization Error (Application client_secret invalid). Check the Client ID and Client Secret."
    );

    let provider = answering(
        400,
        json!({ "error": "invalid_grant", "error_description": "Bad code" }),
    )
    .await;
    let error = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "c",
        "r",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Strava rejected the token request (HTTP 400): Bad code. Check the Client ID and Client Secret."
    );

    // A body in neither shape explains nothing, and a long explanation is cut.
    let provider = answering(500, json!({ "error": { "code": 500 } })).await;
    let error = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "c",
        "r",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Strava rejected the token request (HTTP 500)"
    );
    let provider = answering(500, json!({ "error_description": "x".repeat(500) })).await;
    let error = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "c",
        "r",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Strava rejected the token request (HTTP 500): {}",
            "x".repeat(200)
        )
    );
}

#[tokio::test]
async fn refuses_token_responses_it_cannot_use_and_missing_credentials() {
    for unusable in [
        json!({ "token_type": "Bearer" }),
        json!({ "access_token": "" }),
        json!({ "access_token": 42 }),
        json!([]),
        json!("<html>"),
    ] {
        let provider = answering(200, unusable).await;
        let error = exchange_builtin_authorization_code(
            &provider.env,
            "Strava",
            &strava(),
            &provider.mcp,
            "c",
            "r",
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Strava returned an unexpected token response"
        );
    }

    let mut provider = answering(200, json!({ "access_token": "a" })).await;
    provider.mcp.oauth_client_secret = None;
    let error = exchange_builtin_authorization_code(
        &provider.env,
        "Strava",
        &strava(),
        &provider.mcp,
        "c",
        "r",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Strava Client ID and Client Secret are required"
    );
    provider.mcp.oauth_client_secret = Some("not ciphertext".into());
    assert!(
        refresh_builtin_tokens(&provider.env, "Strava", &strava(), &provider.mcp, "r")
            .await
            .is_err()
    );
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "nothing was sent"
    );
}

#[test]
fn reads_why_a_token_request_was_refused_in_either_of_the_two_shapes() {
    assert_eq!(
        TOKEN_FAILURE_VALIDATOR
            .validate(&json!({ "error": "invalid_grant", "error_description": "The refresh token was revoked", "error_uri": "https://example.com" }))
            .unwrap(),
        json!({ "error": "invalid_grant", "error_description": "The refresh token was revoked" })
    );
    let strava_shape = json!({ "message": "Bad Request", "errors": [{ "resource": "RefreshToken", "field": "refresh_token", "code": "invalid" }] });
    assert_eq!(
        TOKEN_FAILURE_VALIDATOR.validate(&strava_shape).unwrap(),
        strava_shape
    );
    assert!(
        TOKEN_FAILURE_VALIDATOR
            .validate(&json!({ "error": { "code": 400 } }))
            .is_err()
    );
}
