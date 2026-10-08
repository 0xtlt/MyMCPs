//! The runtime of built-in MCPs (`app/services/builtin/runtime.ts`), with
//! providers of the tests' own: what the saved sign-in may do, how a call
//! is refused before it reaches the provider, and how the OAuth sign-in of a
//! built-in MCP is started, completed and renewed.
//!
//! The cases follow the ones `tests/unit/builtin_strava.spec.ts` has about
//! the runtime, which the providers' own tests leave to this crate.

mod support;

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Duration;
use futures::future::join_all;
use mymcps_core::models::{Mcp, McpStatus, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_core::{TestCore, Timestamp};
use mymcps_net::CannedResponse;
use mymcps_upstream::{Upstream, read_oauth_session};
use serde_json::{Value, json};
use support::*;
use url::Url;

type Answer = Arc<dyn Fn(&Call) -> Option<CannedResponse> + Send + Sync>;

/// The example provider: its token endpoint and its API, unless `special`
/// answers first.
fn example_provider(core: &TestCore, special: Option<Answer>) -> (Arc<Upstream>, Calls) {
    upstream_with(core, public_names(), providers::registry(), move |call| {
        if let Some(answer) = special.as_ref().and_then(|special| special(call)) {
            return answer;
        }
        match call.url.as_str() {
            "https://www.example.test/oauth/token" => json_response(json!({
                "token_type": "Bearer",
                "access_token": "rotated-access-token",
                "refresh_token": "rotated-refresh-token",
                "expires_in": 21600,
            })),
            "https://api.example.test/profile" => json_response(json!({ "id": 42, "name": "Ada" })),
            "https://api.example.test/items" => json_response(json!([{ "id": 1 }])),
            _ => not_found(),
        }
    })
}

fn answering(
    answer: impl Fn(&Call) -> Option<CannedResponse> + Send + Sync + 'static,
) -> Option<Answer> {
    Some(Arc::new(answer))
}

fn token_requests(calls: &Calls) -> Vec<Call> {
    calls.to("https://www.example.test/oauth/token")
}

fn api_requests(calls: &Calls) -> Vec<Call> {
    calls
        .all()
        .into_iter()
        .filter(|call| call.hostname() == "api.example.test")
        .collect()
}

fn settings(core: &TestCore, entries: &[(&str, &str)]) -> Option<String> {
    let entries: Vec<EnvironmentInput> = entries
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: (*name).to_owned(),
            value: Some((*value).to_owned()),
        })
        .collect();
    merge_environment(&core.encryption, None, &entries)
}

/// A connected MCP of the example provider.
async fn connected_example(core: &TestCore, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    let client_secret = encrypt(core, "example-client-secret");
    let access_token = encrypt(core, "example-access-token");
    let refresh_token = encrypt(core, "example-refresh-token");
    let builtin_settings = settings(core, &[("account", "123")]);
    create_mcp(core, |mcp| {
        mcp.name = "Example".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("example".into());
        mcp.oauth_client_id = Some("123456".into());
        mcp.oauth_client_secret = client_secret;
        mcp.oauth_access_token = access_token;
        mcp.oauth_refresh_token = refresh_token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + Duration::hours(1));
        mcp.oauth_scopes = Some("read items:read_all".into());
        mcp.builtin_settings = builtin_settings;
        adjust(mcp);
    })
    .await
}

fn disconnect(mcp: &mut Mcp) {
    mcp.oauth_access_token = None;
    mcp.oauth_refresh_token = None;
    mcp.oauth_token_expires_at = None;
    mcp.oauth_scopes = None;
    mcp.status = McpStatus::Draft;
}

/// An MCP of the provider that signs in with a password.
async fn mailbox(core: &TestCore, adjust: impl FnOnce(&mut Mcp)) -> Mcp {
    let password = encrypt(core, "app-password");
    create_mcp(core, |mcp| {
        mcp.name = "Mailbox".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("mailbox".into());
        mcp.builtin_username = Some("ada@example.com".into());
        mcp.builtin_password = password;
        mcp.builtin_permissions = Some("read".into());
        mcp.builtin_aliases = Some("ada@example.org countess@example.com".into());
        adjust(mcp);
    })
    .await
}

fn arguments(value: Value) -> Option<serde_json::Map<String, Value>> {
    value.as_object().cloned()
}

fn is_error(result: &Value) -> bool {
    result.get("isError") == Some(&json!(true))
}

fn result_text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

async fn tool_names(upstream: &Upstream, mcp: &mut Mcp) -> Vec<String> {
    upstream
        .probe(mcp)
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}

// Tools

#[tokio::test]
async fn lists_tools_without_calling_the_provider_and_hides_tools_whose_scope_was_unchecked() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);

    let mut full = connected_example(&core, |_| {}).await;
    let tools = upstream.probe(&mut full).await.unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["get_profile", "list_items"]
    );
    assert_eq!(upstream.list_builtin_tools(&full).unwrap(), tools);
    assert_eq!(
        serde_json::to_value(&tools[0]).unwrap(),
        json!({
            "name": "get_profile",
            "description": "Returns the profile of the connected account.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        })
    );

    let mut reduced = connected_example(&core, |mcp| {
        mcp.name = "Example reduced".into();
        mcp.oauth_scopes = Some("read".into());
    })
    .await;
    assert_eq!(tool_names(&upstream, &mut reduced).await, ["get_profile"]);

    // A provider that does not report the scopes it granted allows every tool.
    let mut unknown_scopes = connected_example(&core, |mcp| {
        mcp.name = "Example unknown".into();
        mcp.oauth_scopes = None;
    })
    .await;
    assert_eq!(
        tool_names(&upstream, &mut unknown_scopes).await,
        ["get_profile", "list_items"]
    );

    assert_eq!(calls.len(), 0);
}

#[tokio::test]
async fn exposes_write_tools_only_when_allowed_here_and_granted_by_the_provider() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);
    let write_tools = ["create_item", "set_budget"];
    let has_write_tools = |names: &[String]| {
        write_tools
            .iter()
            .all(|tool| names.iter().any(|name| name == tool))
    };
    let has_no_write_tool = |names: &[String]| {
        !write_tools
            .iter()
            .any(|tool| names.iter().any(|name| name == tool))
    };

    let mut read_only = connected_example(&core, |mcp| mcp.name = "Example read".into()).await;
    let names = tool_names(&upstream, &mut read_only).await;
    assert_eq!(names.len(), 2);
    assert!(has_no_write_tool(&names));

    let mut writable = connected_example(&core, |mcp| {
        mcp.name = "Example write".into();
        mcp.builtin_write_enabled = true;
        mcp.oauth_scopes = Some("read items:read_all items:write".into());
    })
    .await;
    let names = tool_names(&upstream, &mut writable).await;
    assert_eq!(names.len(), 4);
    assert!(has_write_tools(&names));

    // Allowed in MyMCPs after connecting: the saved authorization is still read-only.
    let mut awaiting = connected_example(&core, |mcp| {
        mcp.name = "Example awaiting".into();
        mcp.builtin_write_enabled = true;
    })
    .await;
    assert!(has_no_write_tool(
        &tool_names(&upstream, &mut awaiting).await
    ));
    assert!(!upstream.builtin_write_granted(&awaiting).unwrap());
    assert!(upstream.builtin_write_granted(&writable).unwrap());

    // Unknown scopes count as granted, so nobody is asked to re-authorize on a guess.
    let mut unknown = connected_example(&core, |mcp| {
        mcp.name = "Example unknown".into();
        mcp.builtin_write_enabled = true;
        mcp.oauth_scopes = None;
    })
    .await;
    assert!(upstream.builtin_write_granted(&unknown).unwrap());
    assert!(has_write_tools(&tool_names(&upstream, &mut unknown).await));

    // Turned off in MyMCPs while the authorization still carries the scopes.
    let mut turned_off = connected_example(&core, |mcp| {
        mcp.name = "Example off".into();
        mcp.oauth_scopes = Some("read items:read_all items:write".into());
    })
    .await;
    assert!(has_no_write_tool(
        &tool_names(&upstream, &mut turned_off).await
    ));

    // A password sign-in has nothing to re-authorize, and neither has a
    // provider whose one scope both reads and writes.
    let mailbox = mailbox(&core, |_| {}).await;
    assert!(upstream.builtin_write_granted(&mailbox).unwrap());
    let google_ads = connected_example(&core, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.builtin_key = Some("google-ads".into());
        mcp.oauth_scopes = Some("unrelated".into());
    })
    .await;
    assert!(upstream.builtin_write_granted(&google_ads).unwrap());
}

#[tokio::test]
async fn refuses_to_list_or_call_tools_before_an_account_is_connected() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, disconnect).await;

    let error = upstream.probe(&mut mcp).await.unwrap_err();
    assert!(error.is_builtin_tool_error());
    assert!(error.is_builtin_authorization_error());
    assert_eq!(
        error.to_string(),
        "Example is not connected. Connect it from the MCPs page in MyMCPs."
    );
    let result = upstream
        .call_tool(&mut mcp, "get_profile", arguments(json!({})))
        .await
        .unwrap();
    assert!(is_error(&result));
    assert!(result_text(&result).contains("Example is not connected"));
    assert_eq!(
        result,
        json!({
            "content": [{
                "type": "text",
                "text": "Example is not connected. Connect it from the MCPs page in MyMCPs.",
            }],
            "isError": true,
        })
    );

    // The gateway skips an MCP that is not connected yet.
    assert!(
        upstream
            .list_namespaced_tools(std::slice::from_mut(&mut mcp))
            .await
            .is_empty()
    );

    // A saved token that can no longer be decrypted still lists the tools,
    // which is local, and connects no call.
    mcp.oauth_access_token = Some("not ciphertext".into());
    mcp.save(&*core.db).await.unwrap();
    assert_eq!(tool_names(&upstream, &mut mcp).await.len(), 2);
    let result = upstream
        .call_tool(&mut mcp, "get_profile", None)
        .await
        .unwrap();
    assert!(is_error(&result));
    assert!(result_text(&result).contains("Example is not connected"));
    assert_eq!(calls.len(), 0);
}

#[tokio::test]
async fn runs_a_tool_with_the_saved_sign_in_and_settings() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |mcp| {
        // A setting the provider does not have is not handed to its tools.
        mcp.builtin_settings = settings(&core, &[("legacy", "x"), ("account", "123")]);
    })
    .await;

    let result = upstream
        .call_tool(&mut mcp, "get_profile", None)
        .await
        .unwrap();

    assert!(!is_error(&result));
    assert_eq!(
        result,
        json!({
            "content": [{
                "type": "text",
                "text": format!(
                    r#"{{"profile":{{"id":42,"name":"Ada"}},"account":"123","scopes":["read","items:read_all"],"mcpId":{}}}"#,
                    mcp.id
                ),
            }],
        })
    );
    let requests = api_requests(&calls);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].header("authorization").as_deref(),
        Some("Bearer example-access-token")
    );
    assert!(token_requests(&calls).is_empty());

    let definition = upstream.require_builtin_mcp(&mcp).unwrap().clone();
    assert_eq!(
        upstream
            .builtin_settings(&definition, &mcp)
            .into_iter()
            .collect::<Vec<_>>(),
        [("account".to_owned(), "123".to_owned())]
    );
    // Settings that can no longer be decrypted are left out.
    mcp.builtin_settings = Some(r#"{"account":"garbage"}"#.into());
    assert!(upstream.builtin_settings(&definition, &mcp).is_empty());
    mcp.builtin_settings = None;
    assert!(upstream.builtin_settings(&definition, &mcp).is_empty());
}

#[tokio::test]
async fn refuses_a_tool_whose_permission_was_not_granted() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |mcp| mcp.oauth_scopes = Some("read".into())).await;

    let result = upstream
        .call_tool(&mut mcp, "list_items", None)
        .await
        .unwrap();

    assert!(is_error(&result));
    assert_eq!(
        result_text(&result),
        "list_items needs the Example permission \"items:read\" or \"items:read_all\", which was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked."
    );
    assert_eq!(calls.len(), 0);
}

#[tokio::test]
async fn refuses_write_tools_while_write_access_is_turned_off() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |mcp| {
        mcp.oauth_scopes = Some("read items:read_all items:write".into());
    })
    .await;

    for tool in ["create_item", "set_budget"] {
        let result = upstream
            .call_tool(&mut mcp, tool, arguments(json!({ "amount": 5 })))
            .await
            .unwrap();
        assert!(is_error(&result));
        assert_eq!(
            result_text(&result),
            format!(
                "{tool} changes Example data, and write access is turned off for this MCP. An administrator can allow it from the MCPs page in MyMCPs."
            )
        );
    }
    assert_eq!(calls.len(), 0);

    mcp.builtin_write_enabled = true;
    let result = upstream
        .call_tool(&mut mcp, "create_item", arguments(json!({ "amount": 5 })))
        .await
        .unwrap();
    assert_eq!(result_text(&result), r#"{"created":5}"#);
}

#[tokio::test]
async fn tells_the_agent_about_an_unknown_tool_and_about_arguments_that_are_wrong() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |mcp| {
        mcp.builtin_write_enabled = true;
        mcp.oauth_scopes = None;
    })
    .await;

    let result = upstream.call_tool(&mut mcp, "nope", None).await.unwrap();
    assert!(is_error(&result));
    assert_eq!(result_text(&result), "Unknown Example tool: nope");

    for wrong in [
        json!({}),
        json!({ "amount": 0 }),
        json!({ "amount": "many" }),
    ] {
        let result = upstream
            .call_tool(&mut mcp, "create_item", arguments(wrong.clone()))
            .await
            .unwrap();
        assert!(is_error(&result), "{wrong}");
        assert!(result_text(&result).starts_with("amount "), "{wrong}");
    }
    // No arguments at all are an empty object.
    let result = upstream
        .call_tool(&mut mcp, "create_item", None)
        .await
        .unwrap();
    assert_eq!(result_text(&result), "amount is required");
}

#[tokio::test]
async fn fails_for_a_built_in_mcp_this_instance_does_not_have() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);
    let mut unknown =
        connected_example(&core, |mcp| mcp.builtin_key = Some("retired".into())).await;

    let error = upstream.probe(&mut unknown).await.unwrap_err();
    assert_eq!(error.to_string(), "Unknown built-in MCP: retired");
    assert!(!error.is_builtin_tool_error());
    // Not a failure the agent can act on: the call fails instead of answering.
    let error = upstream
        .call_tool(&mut unknown, "get_profile", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Unknown built-in MCP: retired");

    unknown.builtin_key = None;
    assert_eq!(
        upstream
            .builtin_write_granted(&unknown)
            .unwrap_err()
            .to_string(),
        "Unknown built-in MCP: none"
    );

    upstream.test_and_update_status(&mut unknown).await.unwrap();
    assert_eq!(unknown.status, McpStatus::Error);
    assert_eq!(
        unknown.last_error.as_deref(),
        Some("Unknown built-in MCP: none")
    );
    assert!(!unknown.oauth_required);
}

// Authorization lifecycle

#[tokio::test]
async fn renews_an_expired_token_once_for_concurrent_calls_and_stores_the_rotated_pair() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mcp = connected_example(&core, |mcp| {
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    let mut callers = Vec::new();
    for _ in 0..4 {
        callers.push(find_mcp(&core, mcp.id).await);
    }

    let results = join_all(
        callers
            .iter_mut()
            .map(|caller| upstream.call_tool(caller, "get_profile", arguments(json!({})))),
    )
    .await;

    for result in results {
        assert!(!is_error(&result.unwrap()));
    }
    let renewals = token_requests(&calls);
    assert_eq!(renewals.len(), 1);
    assert_eq!(
        renewals[0].body,
        "client_id=123456&client_secret=example-client-secret&grant_type=refresh_token&refresh_token=example-refresh-token"
    );
    let requests = api_requests(&calls);
    assert_eq!(requests.len(), 4);
    for request in requests {
        assert_eq!(
            request.header("authorization").as_deref(),
            Some("Bearer rotated-access-token")
        );
    }

    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("rotated-access-token")
    );
    assert_eq!(
        decrypt(&core, &saved.oauth_refresh_token).as_deref(),
        Some("rotated-refresh-token")
    );
    // The scopes of the authorization are not those of a renewal.
    assert_eq!(saved.oauth_scopes.as_deref(), Some("read items:read_all"));
    assert!(saved.oauth_token_expires_at.unwrap() > Timestamp::now() + Duration::hours(5));
    for caller in &callers {
        assert_eq!(
            decrypt(&core, &caller.oauth_access_token).as_deref(),
            Some("rotated-access-token")
        );
    }
}

#[tokio::test]
async fn keeps_the_refresh_token_a_provider_does_not_rotate() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(
        &core,
        answering(|call| {
            (call.path() == "/oauth/token")
                .then(|| json_response(json!({ "access_token": "renewed-access-token" })))
        }),
    );
    let mut mcp = connected_example(&core, |mcp| {
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;

    upstream.refresh_oauth_access_token(&mut mcp).await.unwrap();

    assert_eq!(
        decrypt(&core, &mcp.oauth_access_token).as_deref(),
        Some("renewed-access-token")
    );
    assert_eq!(
        decrypt(&core, &mcp.oauth_refresh_token).as_deref(),
        Some("example-refresh-token")
    );
    assert_eq!(mcp.oauth_token_type.as_deref(), Some("Bearer"));
    // Without a stated lifetime the token is given an hour.
    let minutes = (mcp.oauth_token_expires_at.unwrap() - Timestamp::now()).num_minutes();
    assert!((58..=60).contains(&minutes), "{minutes} minutes");
}

#[tokio::test]
async fn does_not_renew_a_token_that_is_still_valid() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |_| {}).await;

    upstream
        .call_tool(&mut mcp, "get_profile", None)
        .await
        .unwrap();

    assert!(token_requests(&calls).is_empty());
}

#[tokio::test]
async fn asks_to_re_authorize_when_the_provider_refuses_to_renew_the_authorization() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(
        &core,
        answering(|call| {
            (call.path() == "/oauth/token").then(|| {
                json_status(
                    400,
                    json!({
                        "message": "Bad Request",
                        "errors": [{ "resource": "RefreshToken", "field": "refresh_token", "code": "invalid" }],
                    }),
                )
            })
        }),
    );
    let mut mcp = connected_example(&core, |mcp| {
        mcp.oauth_token_expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;

    let result = upstream
        .call_tool(&mut mcp, "get_profile", None)
        .await
        .unwrap();
    assert!(is_error(&result));
    assert_eq!(
        result_text(&result),
        "Example refused to renew the saved authorization (Bad Request (RefreshToken refresh_token invalid)). Check the Client ID and Client Secret, then re-authorize this MCP in MyMCPs."
    );
    assert!(!result_text(&result).contains("example-client-secret"));
    assert!(api_requests(&calls).is_empty());

    upstream.test_and_update_status(&mut mcp).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Error);
    assert!(mcp.oauth_required);
    assert!(mcp.last_error.as_deref().unwrap().contains("RefreshToken"));
}

#[tokio::test]
async fn reports_connection_health_from_one_authenticated_provider_request() {
    let core = TestCore::new().await;
    let profile_status = Arc::new(AtomicU16::new(200));
    let (upstream, calls) = example_provider(
        &core,
        answering({
            let profile_status = profile_status.clone();
            move |call| {
                let status = profile_status.load(Ordering::SeqCst);
                (call.path() == "/profile" && status != 200)
                    .then(|| json_status(status, json!({ "message": "Authorization Error" })))
            }
        }),
    );

    let mut pending = connected_example(&core, |mcp| {
        mcp.name = "Example pending".into();
        disconnect(mcp);
    })
    .await;
    upstream.test_and_update_status(&mut pending).await.unwrap();
    assert_eq!(pending.status, McpStatus::Draft);
    assert_eq!(
        pending.last_error.as_deref(),
        Some("OAuth authorization required")
    );
    assert!(pending.oauth_required);
    assert_eq!(calls.len(), 0);

    let mut mcp = connected_example(&core, |mcp| {
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("stale".into());
    })
    .await;
    upstream.test_and_update_status(&mut mcp).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Ready);
    assert_eq!(mcp.last_error, None);
    assert!(!mcp.oauth_required);
    assert_eq!(
        api_requests(&calls)
            .iter()
            .map(Call::path)
            .collect::<Vec<_>>(),
        ["/profile"]
    );
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!((saved.status, saved.last_error), (McpStatus::Ready, None));

    profile_status.store(401, Ordering::SeqCst);
    upstream.test_and_update_status(&mut mcp).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Error);
    assert!(mcp.oauth_required);
    assert_eq!(
        mcp.last_error.as_deref(),
        Some("Example rejected the saved authorization. Re-authorize this MCP in MyMCPs.")
    );

    profile_status.store(503, Ordering::SeqCst);
    upstream.test_and_update_status(&mut mcp).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Error);
    assert!(!mcp.oauth_required);
    assert!(
        mcp.last_error
            .as_deref()
            .unwrap()
            .contains("Example API returned HTTP 503")
    );
}

// A password sign-in

#[tokio::test]
async fn a_password_sign_in_may_do_what_the_admin_allowed_and_nothing_else() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = mailbox(&core, |_| {}).await;

    assert_eq!(tool_names(&upstream, &mut mcp).await, ["list_messages"]);
    let result = upstream
        .call_tool(&mut mcp, "list_messages", None)
        .await
        .unwrap();
    assert_eq!(
        result_text(&result),
        format!(
            r#"{{"username":"ada@example.com","permissions":["read"],"aliases":["ada@example.org","countess@example.com"],"mcpId":{}}}"#,
            mcp.id
        )
    );

    let result = upstream
        .call_tool(&mut mcp, "send_message", None)
        .await
        .unwrap();
    assert!(is_error(&result));
    assert_eq!(
        result_text(&result),
        "send_message needs the \"send\" permission, which is not allowed for this Mailbox MCP. An administrator can allow it from the MCPs page in MyMCPs."
    );

    mcp.builtin_permissions = Some("read send".into());
    assert_eq!(
        tool_names(&upstream, &mut mcp).await,
        ["list_messages", "send_message"]
    );
    let result = upstream
        .call_tool(&mut mcp, "send_message", None)
        .await
        .unwrap();
    assert_eq!(result_text(&result), r#"{"sent":true}"#);

    // Nothing allowed is nothing listed, where no reported scope allows every tool.
    mcp.builtin_permissions = None;
    assert!(tool_names(&upstream, &mut mcp).await.is_empty());
    assert_eq!(calls.len(), 0);
}

#[tokio::test]
async fn a_password_sign_in_is_not_repaired_by_connecting() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);

    let mut missing = mailbox(&core, |mcp| {
        mcp.name = "Mailbox missing".into();
        mcp.builtin_password = None;
    })
    .await;
    let error = upstream.probe(&mut missing).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Mailbox is not connected. Connect it from the MCPs page in MyMCPs."
    );
    upstream.test_and_update_status(&mut missing).await.unwrap();
    assert_eq!(missing.status, McpStatus::Error);
    assert!(!missing.oauth_required);
    assert_eq!(
        missing.last_error.as_deref(),
        Some("Mailbox is not connected. Connect it from the MCPs page in MyMCPs.")
    );

    let mut healthy = mailbox(&core, |mcp| mcp.name = "Mailbox healthy".into()).await;
    upstream.test_and_update_status(&mut healthy).await.unwrap();
    assert_eq!(healthy.status, McpStatus::Ready);

    let wrong_password = encrypt(&core, "main-password");
    let mut refused = mailbox(&core, |mcp| {
        mcp.name = "Mailbox refused".into();
        mcp.builtin_password = wrong_password;
    })
    .await;
    upstream.test_and_update_status(&mut refused).await.unwrap();
    assert_eq!(refused.status, McpStatus::Error);
    assert!(!refused.oauth_required);
    assert_eq!(
        refused.last_error.as_deref(),
        Some("Mailbox refused the app password. Enter a new one in MyMCPs.")
    );
}

// Approvals, files

#[tokio::test]
async fn describes_a_call_for_the_person_asked_to_approve_it() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let mut mcp = connected_example(&core, |mcp| {
        mcp.builtin_write_enabled = true;
        mcp.oauth_scopes = Some("read items:write".into());
    })
    .await;

    let summary = upstream
        .describe_builtin_call(&mut mcp, "set_budget", arguments(json!({ "amount": 250 })))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({
            "title": "Change the daily budget of Ada",
            "details": [{ "label": "Daily budget", "value": "€250", "before": "€2" }],
        })
    );
    // Nothing was changed at the provider: its profile was read, and that is all.
    assert_eq!(
        api_requests(&calls)
            .iter()
            .map(|call| format!("{} {}", call.method, call.path()))
            .collect::<Vec<_>>(),
        ["GET /profile"]
    );

    // A tool that only has its arguments to show.
    assert_eq!(
        upstream
            .describe_builtin_call(&mut mcp, "create_item", arguments(json!({ "amount": 1 })))
            .await
            .unwrap(),
        None
    );

    // Nobody is asked to approve a call that would be refused.
    let refused = upstream
        .describe_builtin_call(&mut mcp, "set_budget", arguments(json!({ "amount": 0 })))
        .await
        .unwrap_err();
    assert!(refused.is_tool_error());
    assert!(refused.to_string().starts_with("amount "));
    let refused = upstream
        .describe_builtin_call(&mut mcp, "nope", None)
        .await
        .unwrap_err();
    assert!(refused.is_tool_error());
    assert_eq!(refused.to_string(), "Unknown Example tool: nope");

    mcp.builtin_write_enabled = false;
    let refused = upstream
        .describe_builtin_call(&mut mcp, "set_budget", arguments(json!({ "amount": 250 })))
        .await
        .unwrap_err();
    assert!(refused.is_tool_error());
    assert!(
        refused
            .to_string()
            .contains("write access is turned off for this MCP")
    );

    // The scopes are read from the row the sign-in loaded, not from the
    // copy the caller holds.
    mcp.builtin_write_enabled = true;
    mcp.oauth_scopes = Some("read".into());
    mcp.save(&*core.db).await.unwrap();
    let refused = upstream
        .describe_builtin_call(&mut mcp, "set_budget", arguments(json!({ "amount": 250 })))
        .await
        .unwrap_err();
    assert!(refused.is_tool_error());
    assert!(
        refused
            .to_string()
            .contains("needs the Example permission \"items:write\"")
    );
}

#[tokio::test]
async fn serves_the_files_behind_the_links_its_tools_hand_out() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);
    let mut example = connected_example(&core, |_| {}).await;
    let mut mailbox = mailbox(&core, |_| {}).await;

    let file = upstream
        .download_builtin_file(&mut example, json!({ "name": "report.txt" }))
        .await
        .unwrap();
    assert_eq!(file.filename, "report.txt");
    assert_eq!(file.content_type, "text/plain");
    assert_eq!(file.content.concat(), b"file of example-access-token");

    // The provider says when a link can no longer be served.
    let gone = upstream
        .download_builtin_file(&mut example, json!({}))
        .await
        .unwrap_err();
    assert!(gone.is_tool_error());
    assert_eq!(gone.to_string(), "This link is no longer valid");

    let none = upstream
        .download_builtin_file(&mut mailbox, json!({ "name": "report.txt" }))
        .await
        .unwrap_err();
    assert!(none.is_tool_error());
    assert_eq!(none.to_string(), "Mailbox has no files to download");

    let mut disconnected = connected_example(&core, |mcp| {
        mcp.name = "Example disconnected".into();
        disconnect(mcp);
    })
    .await;
    let not_connected = upstream
        .download_builtin_file(&mut disconnected, json!({ "name": "report.txt" }))
        .await
        .unwrap_err();
    assert!(not_connected.is_authorization_error());
}

#[tokio::test]
async fn takes_a_file_only_while_the_link_may_still_be_used() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(&core, None);
    let mut example = connected_example(&core, |_| {}).await;
    let reference = json!({ "id": "3f2c1c1e-9d57-4a0b-8f3e-0d5e6f7a8b9c" });

    // The link outlives the call that made it. Where write access is one
    // switch, a file is only taken while it is on.
    let off = upstream
        .builtin_upload_target(&mut example, reference.clone())
        .await
        .unwrap_err();
    assert!(off.is_tool_error());
    assert_eq!(
        off.to_string(),
        "Write access is turned off for this Example MCP"
    );

    example.builtin_write_enabled = true;
    let target = upstream
        .builtin_upload_target(&mut example, reference.clone())
        .await
        .unwrap();
    assert_eq!(target.id, "3f2c1c1e-9d57-4a0b-8f3e-0d5e6f7a8b9c");
    assert_eq!(
        (target.filename.as_str(), target.max_bytes),
        ("upload.bin", 1024)
    );

    // A password sign-in has no such switch: its permissions decide.
    let mut mailbox = mailbox(&core, |_| {}).await;
    let target = upstream
        .builtin_upload_target(&mut mailbox, reference.clone())
        .await
        .unwrap();
    assert_eq!(target.filename, "attachment.bin");

    let mut google_ads = connected_example(&core, |mcp| {
        mcp.name = "Google Ads".into();
        mcp.builtin_key = Some("google-ads".into());
        mcp.builtin_write_enabled = true;
    })
    .await;
    let none = upstream
        .builtin_upload_target(&mut google_ads, reference)
        .await
        .unwrap_err();
    assert!(none.is_tool_error());
    assert_eq!(none.to_string(), "Google Ads takes no files");
}

// The OAuth sign-in of a built-in MCP

#[tokio::test]
async fn sends_the_admin_to_the_provider_and_stores_the_tokens_and_granted_scopes_on_return() {
    let core = TestCore::new().await;
    let exchanged: Arc<Mutex<Value>> = Arc::new(Mutex::new(json!({
        "token_type": "Bearer",
        "access_token": "first-access-token",
        "refresh_token": "first-refresh-token",
        "expires_in": 21600,
    })));
    let (upstream, calls) = example_provider(
        &core,
        answering({
            let exchanged = exchanged.clone();
            move |call| {
                (call.path() == "/oauth/token"
                    && call.body.contains("grant_type=authorization_code"))
                .then(|| json_response(exchanged.lock().unwrap().clone()))
            }
        }),
    );
    let mut mcp = connected_example(&core, |mcp| {
        disconnect(mcp);
        // The scopes of an earlier authorization.
        mcp.oauth_scopes = Some("read items:read_all items:write".into());
        mcp.oauth_required = true;
        mcp.last_error = Some("OAuth authorization required".into());
    })
    .await;
    let session = MemorySession::new();

    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let redirect = Url::parse(&redirect).unwrap();
    let state = query(&redirect, "state").unwrap();
    assert_eq!(
        redirect.as_str(),
        format!(
            "https://www.example.test/oauth/authorize?client_id=123456&redirect_uri=http%3A%2F%2Flocalhost%3A3333%2Fmcps%2Foauth%2Fcallback&response_type=code&scope=read%2Citems%3Aread_all&state={state}&approval_prompt=force"
        )
    );
    assert_eq!(calls.len(), 0, "there is nothing to discover or register");
    assert_eq!(
        serde_json::to_string(&session.value(&format!("mcp_oauth:{state}")).unwrap()).unwrap(),
        format!(
            r#"{{"mcpId":{},"redirectUri":"{CALLBACK}","authorizationServerUrl":"https://www.example.test","clientId":"123456","state":"{state}"}}"#,
            mcp.id
        )
    );
    let oauth = read_oauth_session(&session, Some(&state)).unwrap();
    assert_eq!(oauth.code_verifier, None);

    // The provider lets the user uncheck permissions, and says what is left
    // in the callback.
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "the-code", Some("read,items:read_all"))
        .await
        .unwrap();

    assert_eq!(
        token_requests(&calls)[0].body,
        "client_id=123456&client_secret=example-client-secret&grant_type=authorization_code&code=the-code&redirect_uri=http%3A%2F%2Flocalhost%3A3333%2Fmcps%2Foauth%2Fcallback"
    );
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(
        decrypt(&core, &saved.oauth_access_token).as_deref(),
        Some("first-access-token")
    );
    assert_eq!(
        decrypt(&core, &saved.oauth_refresh_token).as_deref(),
        Some("first-refresh-token")
    );
    assert_eq!(saved.oauth_scopes.as_deref(), Some("read items:read_all"));
    assert_eq!(saved.oauth_token_type.as_deref(), Some("Bearer"));
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
    assert!(!saved.oauth_required);
    assert!(saved.oauth_token_expires_at.unwrap() > Timestamp::now() + Duration::hours(5));

    // The scopes the token response states win over those of the callback,
    // and write scopes are requested once write access is allowed.
    *exchanged.lock().unwrap() =
        json!({ "access_token": "second-access-token", "scope": "read items:write" });
    mcp.builtin_write_enabled = true;
    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let redirect = Url::parse(&redirect).unwrap();
    assert_eq!(
        query(&redirect, "scope").as_deref(),
        Some("read,items:read_all,items:write")
    );
    let oauth = read_oauth_session(&session, query(&redirect, "state").as_deref()).unwrap();
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "another-code", Some("read"))
        .await
        .unwrap();
    assert_eq!(mcp.oauth_scopes.as_deref(), Some("read items:write"));
    assert_eq!(mcp.oauth_refresh_token, None);

    // Neither says anything: no scope is kept from an earlier authorization.
    *exchanged.lock().unwrap() = json!({ "access_token": "third-access-token" });
    upstream
        .exchange_authorization_code(&mut mcp, &oauth, "a-third-code", None)
        .await
        .unwrap();
    assert_eq!(mcp.oauth_scopes, None);
}

#[tokio::test]
async fn does_not_start_a_sign_in_it_could_not_complete() {
    let core = TestCore::new().await;
    let (upstream, calls) = example_provider(&core, None);
    let session = MemorySession::new();

    let mut without_secret = connected_example(&core, |mcp| {
        mcp.name = "Example without secret".into();
        mcp.oauth_client_secret = None;
    })
    .await;
    let error = upstream
        .start_oauth_flow(&session, &mut without_secret)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Add the Example Client ID and Client Secret before connecting"
    );

    let mut mailbox = mailbox(&core, |_| {}).await;
    let error = upstream
        .start_oauth_flow(&session, &mut mailbox)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Mailbox does not sign in with OAuth");
    assert!(session.pending().is_empty());

    // The application was replaced while the browser was at the provider.
    let mut mcp = connected_example(&core, |mcp| mcp.name = "Example replaced".into()).await;
    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let state = query(&Url::parse(&redirect).unwrap(), "state");
    let oauth = read_oauth_session(&session, state.as_deref()).unwrap();
    mcp.oauth_client_id = Some("654321".into());
    let error = upstream
        .exchange_authorization_code(&mut mcp, &oauth, "the-code", None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "OAuth client information is no longer available"
    );
    assert_eq!(calls.len(), 0);
}

#[tokio::test]
async fn explains_rejected_application_credentials_without_echoing_them() {
    let core = TestCore::new().await;
    let (upstream, _) = example_provider(
        &core,
        answering(|call| {
            (call.path() == "/oauth/token").then(|| {
                json_status(
                    401,
                    json!({
                        "message": "Authorization Error",
                        "errors": [{ "resource": "Application", "field": "client_secret", "code": "invalid" }],
                    }),
                )
            })
        }),
    );
    let mut mcp = connected_example(&core, disconnect).await;
    let session = MemorySession::new();
    let redirect = upstream.start_oauth_flow(&session, &mut mcp).await.unwrap();
    let state = query(&Url::parse(&redirect).unwrap(), "state");
    let oauth = read_oauth_session(&session, state.as_deref()).unwrap();

    let error = upstream
        .exchange_authorization_code(&mut mcp, &oauth, "the-code", None)
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Example rejected the token request (HTTP 401): Authorization Error (Application client_secret invalid). Check the Client ID and Client Secret."
    );
    assert!(!error.is_builtin_tool_error());
    assert!(!error.to_string().contains("example-client-secret"));
    let saved = find_mcp(&core, mcp.id).await;
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.status, McpStatus::Draft);
}
