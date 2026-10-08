//! `tests/functional/tool_approvals.spec.ts`: the calls a person has to
//! approve, from the agent that makes them to the request the Approvals
//! pages read and decide. The pages themselves are the web server's.

use std::sync::{Arc, LazyLock, Mutex};

use chrono::Duration;
use http::StatusCode;
use mymcps_builtin::arguments::{TOOL_VINE, integer};
use mymcps_builtin::{
    ApprovalDetail, ApprovalSummary, BuiltinError, BuiltinMcpDefinition, BuiltinOauthConfig,
    BuiltinProvider, BuiltinRegistry, BuiltinTool, BuiltinToolContext,
};
use mymcps_core::Timestamp;
use mymcps_core::models::{
    AccessToken, ApprovalDecision, ApprovalRequest, ApprovalState, CallErrorCategory, CallOutcome,
    InstanceSetting, Mcp, McpLogLevel, McpTransport, ScopeMode, User,
};
use mymcps_gateway::approvals::{
    ApprovalGate, ApprovalService, Decision, GatedCall, arguments_hash,
};
use mymcps_net::{CannedResponse, Fetcher};
use mymcps_upstream::approvals::APPROVAL_NOTE;
use serde_json::{Map, Value, json};

use crate::support::mcp::*;
use crate::support::*;

const TOOLS: [(&str, Option<&str>); 2] = [
    ("list_contacts", Some("List the contacts of the account.")),
    ("delete_contact", Some("Delete a contact for good.")),
];

/// A gateway behind which the CRM of the TypeScript tests answers.
async fn crm_gateway() -> TestMcpGateway {
    TestMcpGateway::new(listing(&TOOLS)).await
}

/// An MCP reached over HTTP whose `delete_contact` asks for approval.
async fn crm_mcp(gateway: &TestMcpGateway, created_by: i64) -> Mcp {
    create_mcp(gateway.db(), created_by, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals = Some(json!({ "delete_contact": "ask" }).to_string());
    })
    .await
}

/// The id of the approval link in what an agent was told.
fn link_in(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("http://localhost:3333/approvals/")?;
    let id = rest.get(..32)?;
    let ends = rest[32..]
        .chars()
        .next()
        .is_none_or(|next| !(next.is_ascii_alphanumeric() || next == '_' || next == '-'));
    (ends
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        }))
    .then_some(id)
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

async fn find_request(gateway: &TestMcpGateway, id: i64) -> ApprovalRequest {
    ApprovalRequest::find(&**gateway.db(), id)
        .await
        .unwrap()
        .unwrap()
}

async fn last_log(gateway: &TestMcpGateway) -> mymcps_core::models::McpCallLog {
    call_logs(gateway.db()).await.pop().unwrap()
}

/// The text a held gate tells the agent.
fn held_text(gate: &ApprovalGate) -> &str {
    gate.held().map(|held| held.text()).unwrap_or_default()
}

// Tool approvals: gateway

#[tokio::test]
async fn holds_a_tool_that_asks_and_hands_the_agent_a_link_instead_of_running_it() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let created =
        create_named_access_token(db, admin.id, "Claude", ScopeMode::All, &[], None).await;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    settings.mcp_log_level = McpLogLevel::Responses;
    settings.save(&**db).await.unwrap();

    let result = gateway
        .call(
            &created.plaintext,
            "crm__delete_contact",
            json!({ "id": 42, "reason": "duplicate" }),
        )
        .await;

    assert_eq!(result["isError"], true);
    let text = result_text(&result);
    assert!(text.contains("Approval required: delete_contact on CRM was not run."));
    assert!(text.contains("call delete_contact again with exactly the same arguments"));
    assert!(gateway.upstreams.calls().is_empty());

    let requests = gateway.approval_requests().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(link_in(text), Some(request.public_id.as_str()));
    assert_eq!(request.mcp_id, mcp.id);
    assert_eq!(request.status, ApprovalDecision::Pending);
    assert_eq!(request.state(), ApprovalState::Pending);
    assert_eq!(request.access_token_id, created.token.id);
    assert_eq!(request.tool_name, "delete_contact");
    assert_eq!(
        request.arguments_hash,
        arguments_hash(Some(&object(json!({ "reason": "duplicate", "id": 42 }))))
    );
    // Encrypted at rest: neither the arguments nor the summary can be read in the table.
    assert!(!request.arguments.contains("duplicate"));
    assert!(!request.summary.contains("duplicate"));
    assert_eq!(
        gateway.approvals.arguments(request),
        Some(json!({ "id": 42, "reason": "duplicate" }))
    );
    // A person has a day to decide.
    let remaining = request.expires_at - Timestamp::now();
    assert!(remaining > Duration::hours(23) && remaining <= Duration::hours(24));

    // The whole of what the agent reads.
    assert_eq!(
        text,
        [
            "Approval required: delete_contact on CRM was not run.".to_owned(),
            format!(
                "A person has to approve this exact call in MyMCPs first. Give them this link: http://localhost:3333/approvals/{}",
                request.public_id
            ),
            format!(
                "They sign in, read what the call would do, and approve or deny it. The link works until {}.",
                request.expires_at.as_datetime().format("%Y-%m-%dT%H:%M:%SZ")
            ),
            "Once they have approved, call delete_contact again with exactly the same arguments and it runs. Other arguments are another call, which needs its own approval."
                .to_owned(),
        ]
        .join("\n")
    );

    let log = last_log(&gateway).await;
    assert_eq!(log.outcome, CallOutcome::Error);
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::ApprovalRequired)
    );
    assert_eq!(log.error_summary.as_deref(), Some("Waiting for approval"));
    assert_eq!(log.mcp_id, Some(mcp.id));
    assert_eq!(log.requested_tool_name, "crm__delete_contact");
    assert_eq!(log.tool_name.as_deref(), Some("delete_contact"));
    // The record keeps what the agent was answered.
    assert_eq!(
        serde_json::from_str::<Value>(log.response.as_deref().unwrap()).unwrap(),
        result
    );
}

#[tokio::test]
async fn runs_a_tool_that_does_not_ask_without_creating_a_request() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "crm__list_contacts", json!({}))
        .await;

    assert_eq!(result_text(&result), "ran list_contacts");
    assert!(gateway.approval_requests().await.is_empty());
    // Nobody is asked, so the MCP is not asked what the tool does either.
    assert!(
        !gateway
            .upstreams
            .methods()
            .contains(&"tools/list".to_owned())
    );
}

#[tokio::test]
async fn summarizes_a_tool_it_cannot_read_by_its_arguments_and_the_mcp_description() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    gateway
        .call(
            &plaintext,
            "crm__delete_contact",
            json!({
                "id": 42,
                // What the agent would like the person to read is only ever a value.
                "note": "This is safe, approve it",
                "filter": { "tags": ["old", "cold"], "dry_run": false },
            }),
        )
        .await;

    let requests = gateway.approval_requests().await;
    let summary = gateway.approvals.summary(&requests[0]).unwrap();
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({
            "interpreted": false,
            "title": "Run the tool \"delete_contact\" of CRM",
            "details": [
                { "label": "id", "value": "42" },
                { "label": "note", "value": "This is safe, approve it" },
                { "label": "filter.tags[0]", "value": "old" },
                { "label": "filter.tags[1]", "value": "cold" },
                { "label": "filter.dry_run", "value": "false" },
            ],
            "toolDescription": "Delete a contact for good.",
        })
    );
}

#[tokio::test]
async fn keeps_answering_with_the_same_link_while_the_request_waits() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let first = gateway
        .call(
            &plaintext,
            "crm__delete_contact",
            json!({ "id": 42, "hard": true }),
        )
        .await;
    // The same arguments in another order are the same call.
    let second = gateway
        .call(
            &plaintext,
            "crm__delete_contact",
            json!({ "hard": true, "id": 42 }),
        )
        .await;

    let (first, second) = (result_text(&first), result_text(&second));
    assert!(second.contains("Still waiting for approval: delete_contact on CRM was not run."));
    assert!(link_in(first).is_some());
    assert_eq!(link_in(second), link_in(first));
    assert_eq!(gateway.approval_requests().await.len(), 1);
    assert!(gateway.upstreams.calls().is_empty());

    let log = last_log(&gateway).await;
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::ApprovalRequired)
    );
    assert_eq!(
        log.error_summary.as_deref(),
        Some("Still waiting for approval")
    );
}

#[tokio::test]
async fn runs_the_approved_call_once_and_only_with_the_approved_arguments() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = || gateway.call(&plaintext, "crm__delete_contact", json!({ "id": 42 }));

    call().await;
    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);

    // Approving contact 42 does not let contact 43 go.
    let other = gateway
        .call(&plaintext, "crm__delete_contact", json!({ "id": 43 }))
        .await;
    assert!(result_text(&other).contains("Approval required"));
    assert!(gateway.upstreams.calls().is_empty());

    let approved = call().await;
    assert!(approved.get("isError").is_none());
    assert_eq!(result_text(&approved), "ran delete_contact");
    assert_eq!(
        gateway.upstreams.calls(),
        [json!({ "name": "delete_contact", "arguments": { "id": 42 } })]
    );
    let log = last_log(&gateway).await;
    assert_eq!(log.outcome, CallOutcome::Success);
    assert_eq!(log.error_category, None);

    let hash = arguments_hash(Some(&object(json!({ "id": 42 }))));
    let used: ApprovalRequest =
        sqlx::query_as("select * from `approval_requests` where `arguments_hash` = ?")
            .bind(&hash)
            .fetch_one(&**db)
            .await
            .unwrap();
    assert_eq!(used.state(), ApprovalState::Used);
    assert!(used.consumed_at.is_some());

    // The approval is spent: the same call asks again.
    let again = call().await;
    assert!(result_text(&again).contains("Approval required"));
    assert_eq!(gateway.upstreams.calls().len(), 1);
}

#[tokio::test]
async fn does_not_let_another_access_token_use_an_approval() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let asking = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let other = create_access_token(db, admin.id, ScopeMode::All, &[]).await;

    gateway
        .call(
            &asking.plaintext,
            "crm__delete_contact",
            json!({ "id": 42 }),
        )
        .await;
    gateway.decide(&mcp, Decision::Approve, &admin).await;

    let result = gateway
        .call(&other.plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;
    assert!(result_text(&result).contains("Approval required"));
    assert!(gateway.upstreams.calls().is_empty());
}

#[tokio::test]
async fn does_not_let_an_approval_run_another_tool_or_another_mcp() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_mcp(db, admin.id, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals =
            Some(json!({ "delete_contact": "ask", "list_contacts": "ask" }).to_string());
    })
    .await;
    create_mcp(db, admin.id, |mcp| {
        mcp.name = "Other CRM".into();
        mcp.slug = "other".into();
        mcp.http_url = Some("https://other.example/mcp".into());
        mcp.tool_approvals = Some(json!({ "delete_contact": "ask" }).to_string());
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    gateway
        .call(&plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;
    gateway.decide(&mcp, Decision::Approve, &admin).await;

    for name in ["crm__list_contacts", "other__delete_contact"] {
        let result = gateway.call(&plaintext, name, json!({ "id": 42 })).await;
        assert!(result_text(&result).contains("Approval required"), "{name}");
    }
    assert!(gateway.upstreams.calls().is_empty());
}

#[tokio::test]
async fn tells_the_agent_once_that_the_call_was_denied() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = || gateway.call(&plaintext, "crm__delete_contact", json!({ "id": 42 }));

    call().await;
    gateway.decide(&mcp, Decision::Deny, &admin).await;

    let denied = call().await;
    assert_eq!(denied["isError"], true);
    assert_eq!(
        result_text(&denied),
        "Denied: a person refused this call to delete_contact on CRM in MyMCPs, and it was not run. Do not make it again unless they ask you to."
    );
    let log = last_log(&gateway).await;
    assert_eq!(log.error_category, Some(CallErrorCategory::ApprovalDenied));
    assert_eq!(
        log.error_summary.as_deref(),
        Some("A person denied this call")
    );

    // Asked again, it is a new request for the person to decide.
    let again = call().await;
    assert!(result_text(&again).contains("Approval required"));
    let requests = gateway.approval_requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].state(), ApprovalState::Denied);
    assert!(requests[0].consumed_at.is_some());
    assert_eq!(requests[1].state(), ApprovalState::Pending);
    assert!(gateway.upstreams.calls().is_empty());
}

#[tokio::test]
async fn asks_again_once_an_approval_has_expired() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = || gateway.call(&plaintext, "crm__delete_contact", json!({ "id": 42 }));

    call().await;
    gateway.decide(&mcp, Decision::Approve, &admin).await;
    let mut request = gateway.approval_requests().await.remove(0);
    request.expires_at = Timestamp::now() - Duration::minutes(1);
    request.save(&**db).await.unwrap();

    let result = call().await;
    assert!(result_text(&result).contains("Approval required"));
    assert!(gateway.upstreams.calls().is_empty());
    assert_eq!(
        find_request(&gateway, request.id).await.state(),
        ApprovalState::Expired
    );
    assert_eq!(gateway.approval_requests().await.len(), 2);
}

#[tokio::test]
async fn holds_the_calls_of_the_lazy_gateway_the_same_way() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = || gateway.call_lazily(&plaintext, "crm", "delete_contact", json!({ "id": 42 }));

    let held = call().await;
    assert!(result_text(&held).contains("Approval required"));
    let log = last_log(&gateway).await;
    assert_eq!(log.requested_tool_name, "crm__delete_contact");
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::ApprovalRequired)
    );
    gateway.decide(&mcp, Decision::Approve, &admin).await;

    let approved = call().await;
    assert_eq!(result_text(&approved), "ran delete_contact");
    assert_eq!(
        gateway.upstreams.calls(),
        [json!({ "name": "delete_contact", "arguments": { "id": 42 } })]
    );
}

#[tokio::test]
async fn lets_an_approval_given_to_one_gateway_mode_run_the_call_in_the_other() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    gateway
        .call(&plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;
    gateway.decide(&mcp, Decision::Approve, &admin).await;

    // The same token, MCP, tool and arguments: the same call.
    let approved = gateway
        .call_lazily(&plaintext, "crm", "delete_contact", json!({ "id": 42 }))
        .await;
    assert_eq!(result_text(&approved), "ran delete_contact");
}

#[tokio::test]
async fn tells_agents_which_tools_ask_where_they_list_and_search_them() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let listed = gateway
        .rpc(&plaintext, "tools/list", json!({}), "eager")
        .await;
    let description = |name: &str| {
        listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        description("crm__list_contacts"),
        "List the contacts of the account."
    );
    assert_eq!(
        description("crm__delete_contact"),
        format!("Delete a contact for good.\n\n{APPROVAL_NOTE}")
    );

    let found = gateway
        .rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "mcp": "crm", "query": "delete" } }),
            "lazy",
        )
        .await;
    assert!(result_text(&found).contains("Needs approval"));
    assert_eq!(
        found["structuredContent"]["tools"][0]["description"],
        format!("Delete a contact for good.\n\n{APPROVAL_NOTE}")
    );
}

#[tokio::test]
async fn refuses_a_tool_the_mcp_does_not_have_without_asking_anyone() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    create_mcp(db, admin.id, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals = Some(json!({ "drop_database": "ask" }).to_string());
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "crm__drop_database", json!({}))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(
        result_text(&result),
        "CRM has no tool named \"drop_database\""
    );
    assert!(gateway.approval_requests().await.is_empty());
    let log = last_log(&gateway).await;
    assert_eq!(log.error_category, Some(CallErrorCategory::ToolError));
    assert_eq!(
        log.error_summary.as_deref(),
        Some("The call was refused before asking for approval")
    );
}

#[tokio::test]
async fn stops_an_access_token_from_piling_up_requests() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let other = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let ask = |id: u32| gateway.call(&plaintext, "crm__delete_contact", json!({ "id": id }));

    for id in 1..=20 {
        ask(id).await;
    }
    let refused = ask(21).await;

    assert_eq!(refused["isError"], true);
    assert_eq!(
        result_text(&refused),
        "20 calls of this access token already wait for approval in MyMCPs. Ask the person to decide them before asking for more."
    );
    assert_eq!(gateway.approval_requests().await.len(), 20);
    // A call that already waits is still answered with its link.
    assert!(result_text(&ask(7).await).contains("Still waiting for approval"));
    // The limit is the token's own.
    let unaffected = gateway
        .call(&other, "crm__delete_contact", json!({ "id": 21 }))
        .await;
    assert!(result_text(&unaffected).contains("Approval required"));
    assert_eq!(gateway.approval_requests().await.len(), 21);
}

/// One call of `delete_contact` at the gate.
async fn gate(
    approvals: &ApprovalService,
    token: &AccessToken,
    mcp: &Mcp,
    id: i64,
) -> ApprovalGate {
    let mut mcp = mcp.clone();
    let args = object(json!({ "id": id }));
    approvals
        .gate(GatedCall {
            access_token: token,
            mcp: &mut mcp,
            tool_name: "delete_contact",
            args: Some(&args),
        })
        .await
        .unwrap()
}

/// Straight to the gate, one call for each of `ids`, all started together
/// and in that order, as `Promise.all` starts them.
async fn gate_together(
    approvals: &ApprovalService,
    token: &AccessToken,
    mcp: &Mcp,
    ids: impl IntoIterator<Item = i64>,
) -> Vec<ApprovalGate> {
    futures::future::join_all(ids.into_iter().map(|id| gate(approvals, token, mcp, id))).await
}

/// The same calls, each on a task of its own: on a runtime with several
/// threads they run at the same time, and reach the gate in any order.
async fn gate_in_parallel(
    approvals: &ApprovalService,
    token: &AccessToken,
    mcp: &Mcp,
    ids: impl IntoIterator<Item = i64>,
) -> Vec<ApprovalGate> {
    let calls: Vec<_> = ids
        .into_iter()
        .map(|id| {
            let (approvals, token, mcp) = (approvals.clone(), token.clone(), mcp.clone());
            tokio::spawn(async move { gate(&approvals, &token, &mcp, id).await })
        })
        .collect();
    let mut gates = Vec::new();
    for call in calls {
        gates.push(call.await.unwrap());
    }
    gates
}

fn refused_before_asking(gate: &ApprovalGate) -> bool {
    gate.held()
        .is_some_and(|held| held.category == CallErrorCategory::ToolError)
}

#[tokio::test]
async fn holds_the_limit_when_the_calls_of_an_access_token_arrive_at_once() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    // Straight to the gate: each call reads what waits, asks the MCP for
    // its tools, then adds its request, and all of them start together.
    let gates = gate_together(&gateway.approvals, &token, &mcp, 0..25).await;

    assert_eq!(gateway.approval_requests().await.len(), 20);
    assert_eq!(
        gates
            .iter()
            .filter(|gate| refused_before_asking(gate))
            .count(),
        5
    );
    // The calls of a token pass in the order they came: the last are refused.
    assert!(
        gates[..20]
            .iter()
            .all(|gate| held_text(gate).starts_with("Approval required"))
    );
    assert!(gates[20..].iter().all(refused_before_asking));
    assert_eq!(
        held_text(&gates[24]),
        "20 calls of this access token already wait for approval in MyMCPs. Ask the person to decide them before asking for more."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn holds_the_limit_of_each_access_token_when_their_calls_run_in_parallel() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    let other = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    let (gates, others) = tokio::join!(
        gate_in_parallel(&gateway.approvals, &token, &mcp, 0..30),
        gate_in_parallel(&gateway.approvals, &other, &mcp, 0..30),
    );

    // Each token waits in a line of its own, and each line holds its limit.
    let requests = gateway.approval_requests().await;
    for (token, gates) in [(&token, &gates), (&other, &others)] {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.access_token_id == token.id)
                .count(),
            20
        );
        assert_eq!(
            gates
                .iter()
                .filter(|gate| refused_before_asking(gate))
                .count(),
            10
        );
    }
    assert_eq!(requests.len(), 40);
}

#[tokio::test]
async fn asks_once_for_the_same_call_made_twice_at_the_same_moment() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    let gates = gate_together(&gateway.approvals, &token, &mcp, [42, 42]).await;

    // Two requests would be two links to approve, and the call would run twice.
    assert_eq!(gateway.approval_requests().await.len(), 1);
    assert!(gates.iter().all(ApprovalGate::is_held));
    let link = link_in(held_text(&gates[0]));
    assert!(link.is_some());
    assert_eq!(link_in(held_text(&gates[1])), link);
    assert!(held_text(&gates[0]).starts_with("Approval required"));
    assert!(held_text(&gates[1]).starts_with("Still waiting for approval"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asks_once_for_the_same_call_made_many_times_in_parallel() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;

    let gates = gate_in_parallel(&gateway.approvals, &token, &mcp, [42; 16]).await;

    assert_eq!(gateway.approval_requests().await.len(), 1);
    let link = link_in(held_text(&gates[0]));
    assert!(link.is_some());
    assert!(gates.iter().all(|gate| link_in(held_text(gate)) == link));
    assert_eq!(
        gates
            .iter()
            .filter(|gate| held_text(gate).starts_with("Approval required"))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lets_one_of_the_calls_made_at_the_same_moment_spend_an_approval() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    gate(&gateway.approvals, &token, &mcp, 42).await;
    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);

    let gates = gate_in_parallel(&gateway.approvals, &token, &mcp, [42; 16]).await;

    // One call runs. The next asks again, and the others wait with it.
    assert_eq!(
        gates
            .iter()
            .filter(|gate| **gate == ApprovalGate::Open)
            .count(),
        1
    );
    assert_eq!(
        gates
            .iter()
            .filter(|gate| held_text(gate).starts_with("Approval required"))
            .count(),
        1
    );
    assert_eq!(
        gates
            .iter()
            .filter(|gate| held_text(gate).starts_with("Still waiting for approval"))
            .count(),
        14
    );
    let requests = gateway.approval_requests().await;
    assert_eq!(
        requests
            .iter()
            .map(ApprovalRequest::state)
            .collect::<Vec<_>>(),
        [ApprovalState::Used, ApprovalState::Pending]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spends_an_approval_once_even_between_two_servers_on_one_database() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .token;
    gate(&gateway.approvals, &token, &mcp, 42).await;
    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);

    // Two servers do not share a line: the database alone has to hand the
    // approval to one call.
    let elsewhere = ApprovalService::new(gateway.upstream.clone());
    let (here, there) = tokio::join!(
        gate_in_parallel(&gateway.approvals, &token, &mcp, [42; 8]),
        gate_in_parallel(&elsewhere, &token, &mcp, [42; 8]),
    );

    assert_eq!(
        here.iter()
            .chain(&there)
            .filter(|gate| **gate == ApprovalGate::Open)
            .count(),
        1
    );
    let requests = gateway.approval_requests().await;
    assert_eq!(requests[0].state(), ApprovalState::Used);
    assert!(
        requests[1..]
            .iter()
            .all(|request| request.state() == ApprovalState::Pending)
    );
}

#[tokio::test]
async fn holds_every_tool_when_the_saved_choices_cannot_be_read() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    create_mcp(db, admin.id, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals = Some("{\"delete_contact\":\"sometimes\"".into());
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "crm__list_contacts", json!({}))
        .await;

    assert!(result_text(&result).contains("Approval required"));
    assert!(gateway.upstreams.calls().is_empty());
}

#[tokio::test]
async fn tells_the_agent_there_is_no_link_when_the_instance_does_not_know_its_address() {
    let gateway = TestMcpGateway::with(
        |config| config.app_url = None,
        BuiltinRegistry::default(),
        |fetcher| fetcher,
        listing(&TOOLS),
    )
    .await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;

    assert_eq!(result["isError"], true);
    assert_eq!(
        result_text(&result),
        "delete_contact on CRM needs the approval of a person, and it was not run. There is no link to give them, because this MyMCPs instance does not know its public address (APP_URL): ask them to open the Approvals page of MyMCPs and decide the call there, then call delete_contact again with exactly the same arguments."
    );
    // The request is there for the Approvals page all the same.
    let requests = gateway.approval_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(gateway.approvals.url(&requests[0]), None);
    let log = last_log(&gateway).await;
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::ApprovalRequired)
    );
    assert_eq!(
        log.error_summary.as_deref(),
        Some("Approval links need APP_URL")
    );
}

#[tokio::test]
async fn refuses_arguments_too_large_to_show_a_person() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    // `{"note":"…"}` is eleven bytes around its text.
    let call = |length: usize| {
        gateway.call(
            &plaintext,
            "crm__delete_contact",
            json!({ "note": "n".repeat(length) }),
        )
    };

    let refused = call(256 * 1024 - 10).await;
    assert_eq!(refused["isError"], true);
    assert_eq!(
        result_text(&refused),
        "delete_contact on CRM needs the approval of a person, who cannot be shown arguments of more than 256 KB. Make the call smaller."
    );
    assert!(gateway.approval_requests().await.is_empty());

    let held = call(256 * 1024 - 11).await;
    assert!(result_text(&held).contains("Approval required"));
    assert_eq!(gateway.approval_requests().await.len(), 1);
}

#[tokio::test]
async fn answers_a_call_it_cannot_put_to_a_person_as_one_that_failed() {
    // The MCP cannot say which tools it has.
    let gateway = TestMcpGateway::new(|message| match message.method.as_str() {
        "tools/list" => Reply::Error("Bearer very-secret-token was refused".into()),
        _ => Reply::Result(json!({ "content": [{ "type": "text", "text": "ran" }] })),
    })
    .await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(&plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;

    assert_eq!(
        result,
        json!({
            "content": [{ "type": "text", "text": "Upstream tool call failed" }],
            "isError": true,
        })
    );
    assert!(gateway.approval_requests().await.is_empty());
    assert!(gateway.upstreams.calls().is_empty());
    let log = last_log(&gateway).await;
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::UpstreamException)
    );
    let summary = log.error_summary.as_deref().unwrap();
    assert!(summary.contains("Bearer [REDACTED]"), "{summary}");
    assert!(!summary.contains("very-secret-token"));
}

// Tool approvals: built-in MCPs

/// A gateway with the Strava MCP MyMCPs implements, whose API answers
/// nothing: `requests` keeps what it was asked.
async fn strava_gateway() -> (TestMcpGateway, Arc<Mutex<Vec<String>>>) {
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let layer = {
        let requests = requests.clone();
        move |fetcher: Fetcher| {
            fetcher.answering(move |request| {
                if request.url.host_str() != Some("www.strava.com") {
                    return None;
                }
                requests.lock().unwrap().push(request.url.path().to_owned());
                Some(CannedResponse::json(
                    StatusCode::NOT_FOUND,
                    &json!({ "message": "Record Not Found" }),
                ))
            })
        }
    };
    let gateway = TestMcpGateway::with(
        |_| {},
        BuiltinRegistry::new(vec![mymcps_strava::definition()]),
        layer,
        |_| Reply::Error("Strava is not reached as an MCP server".into()),
    )
    .await;
    (gateway, requests)
}

async fn ask_for(gateway: &TestMcpGateway, mcp: &mut Mcp, tool: &str) {
    mcp.tool_approvals = Some(json!({ tool: "ask" }).to_string());
    mcp.save(&**gateway.db()).await.unwrap();
}

#[tokio::test]
async fn checks_the_call_before_asking_anyone_to_approve_it() {
    let (gateway, strava_requests) = strava_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut mcp = create_strava_mcp(&gateway.core, admin.id, true).await;
    ask_for(&gateway, &mut mcp, "update_athlete_weight").await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let invalid = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 4000 }),
        )
        .await;
    assert_eq!(invalid["isError"], true);
    assert_eq!(
        result_text(&invalid),
        "weight must be a number between 20 and 400"
    );
    assert!(gateway.approval_requests().await.is_empty());
    let log = last_log(&gateway).await;
    assert_eq!(log.error_category, Some(CallErrorCategory::ToolError));
    assert_eq!(
        log.error_summary.as_deref(),
        Some("The call was refused before asking for approval")
    );

    let held = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 71.5 }),
        )
        .await;
    assert!(result_text(&held).contains("Approval required: update_athlete_weight on Strava"));
    assert!(strava_requests.lock().unwrap().is_empty());

    let requests = gateway.approval_requests().await;
    assert_eq!(requests[0].mcp_id, mcp.id);
    let summary = gateway.approvals.summary(&requests[0]).unwrap();
    assert_eq!(
        summary.title,
        "Run the tool \"update_athlete_weight\" of Strava"
    );
    assert_eq!(
        serde_json::to_value(&summary.details).unwrap(),
        json!([{ "label": "weight", "value": "71.5" }])
    );
    assert!(!summary.interpreted);
    // Written by MyMCPs itself for its own tool.
    assert!(
        summary
            .tool_description
            .unwrap()
            .contains("Set the connected athlete's weight")
    );
}

#[tokio::test]
async fn does_not_ask_for_a_tool_the_mcp_would_refuse_anyway() {
    let (gateway, _) = strava_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut mcp = create_strava_mcp(&gateway.core, admin.id, false).await;
    ask_for(&gateway, &mut mcp, "update_athlete_weight").await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    let result = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 71.5 }),
        )
        .await;

    assert!(result_text(&result).contains("write access is turned off for this MCP"));
    assert!(gateway.approval_requests().await.is_empty());
}

/// A built-in MCP of the test's own, whose one tool sets a budget: it asks
/// by default, and says itself what a call would change.
fn ads() -> BuiltinMcpDefinition {
    static AMOUNT: LazyLock<mymcps_vine::Validator> = LazyLock::new(|| {
        TOOL_VINE.create(mymcps_vine::object! {
            "amount" => integer(1..=1000),
        })
    });

    #[derive(serde::Deserialize)]
    struct Amount {
        amount: i64,
    }

    let set_budget = BuiltinTool::new(
        "set_budget",
        "Sets the daily budget of the account.",
        json!({
            "type": "object",
            "properties": { "amount": { "type": "integer", "minimum": 1, "maximum": 1000 } },
            "required": ["amount"],
            "additionalProperties": false,
        }),
        &AMOUNT,
        |input: Amount, _: Arc<BuiltinToolContext>| async move {
            Ok(json!({ "budget": input.amount, "previous": 2.5 }))
        },
    )
    .write()
    .asks_approval()
    .describe(|input: Amount, _: Arc<BuiltinToolContext>| async move {
        match input.amount {
            13 => Err(BuiltinError::internal(
                "Ads API answered HTTP 500 to Bearer very-secret-token",
            )),
            amount if amount > 500 => Err(BuiltinError::tool("The amount is too large.")),
            amount => Ok(Some(ApprovalSummary {
                title: format!("Change the daily budget from €2.50 to €{amount}.00"),
                details: vec![
                    ApprovalDetail::new("Daily budget", format!("€{amount}.00 a day"))
                        .replacing("€2.50 a day"),
                ],
                warnings: (amount >= 250)
                    .then(|| vec!["The new budget is 100 times the current one.".to_owned()]),
            })),
        }
    });
    BuiltinMcpDefinition::Oauth {
        provider: BuiltinProvider::new(
            "ads",
            "Ads",
            vec![set_budget],
            |_: Arc<BuiltinToolContext>| async move { Ok(()) },
        ),
        oauth: BuiltinOauthConfig {
            issuer: "https://ads.example.test",
            authorize_url: "https://ads.example.test/oauth/authorize",
            token_url: "https://ads.example.test/oauth/token",
            scopes: vec!["ads"],
            write_scopes: vec![],
            scope_separator: " ",
            authorize_params: vec![],
            sends_redirect_uri_with_code: true,
            client_id_pattern: None,
            client_id_hint: None,
        },
    }
}

/// A gateway with the Ads MCP connected, and the token of an agent.
async fn ads_gateway(tool_approvals: Option<Value>) -> (TestMcpGateway, Mcp, User, String) {
    let gateway = TestMcpGateway::with(
        |_| {},
        BuiltinRegistry::new(vec![ads()]),
        |fetcher| fetcher,
        |_| Reply::Error("Ads is not reached as an MCP server".into()),
    )
    .await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let access_token = gateway.core.encrypt_secret(Some("ads-access-token"));
    let mcp = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Ads".into();
        mcp.slug = "ads".into();
        mcp.transport = McpTransport::Builtin;
        mcp.http_url = None;
        mcp.builtin_key = Some("ads".into());
        mcp.builtin_write_enabled = true;
        mcp.oauth_access_token = access_token;
        mcp.oauth_token_expires_at = Some(Timestamp::now() + Duration::hours(5));
        mcp.tool_approvals = tool_approvals.map(|saved| saved.to_string());
    })
    .await;
    let plaintext = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    (gateway, mcp, admin, plaintext)
}

#[tokio::test]
async fn describes_a_call_in_the_words_of_its_provider_and_runs_it_once_approved() {
    let (gateway, mcp, admin, plaintext) = ads_gateway(None).await;
    // The agent meant 2.50 and wrote 250.
    let call = || gateway.call(&plaintext, "ads__set_budget", json!({ "amount": 250 }));

    // The tool asks until an administrator says otherwise.
    let held = call().await;
    assert!(result_text(&held).contains("Approval required: set_budget on Ads was not run."));

    let requests = gateway.approval_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::to_value(gateway.approvals.summary(&requests[0]).unwrap()).unwrap(),
        json!({
            "interpreted": true,
            "title": "Change the daily budget from €2.50 to €250.00",
            "details": [
                { "label": "Daily budget", "value": "€250.00 a day", "before": "€2.50 a day" },
            ],
            "warnings": ["The new budget is 100 times the current one."],
            "toolDescription": null,
        })
    );

    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);
    let approved = call().await;
    assert!(approved.get("isError").is_none());
    // What the tool answered, as `JSON.stringify` wrote it.
    assert_eq!(result_text(&approved), r#"{"budget":250,"previous":2.5}"#);
    let log = last_log(&gateway).await;
    assert_eq!(log.outcome, CallOutcome::Success);
    assert_eq!(log.requested_tool_name, "ads__set_budget");
}

#[tokio::test]
async fn does_not_ask_anyone_to_approve_what_the_provider_would_refuse() {
    let (gateway, _, _, plaintext) = ads_gateway(None).await;

    for (arguments, refusal) in [
        (json!({ "amount": 600 }), "The amount is too large."),
        (
            json!({ "amount": 4000 }),
            "amount must be an integer between 1 and 1000",
        ),
        (json!({}), "amount is required"),
    ] {
        let refused = gateway
            .call(&plaintext, "ads__set_budget", arguments.clone())
            .await;

        assert_eq!(refused["isError"], true, "{arguments}");
        assert_eq!(result_text(&refused), refusal, "{arguments}");
        let log = last_log(&gateway).await;
        assert_eq!(log.error_category, Some(CallErrorCategory::ToolError));
        assert_eq!(
            log.error_summary.as_deref(),
            Some("The call was refused before asking for approval")
        );
    }
    assert!(gateway.approval_requests().await.is_empty());
}

#[tokio::test]
async fn answers_a_call_its_provider_could_not_describe_as_one_that_failed() {
    let (gateway, _, _, plaintext) = ads_gateway(None).await;

    let failed = gateway
        .call(&plaintext, "ads__set_budget", json!({ "amount": 13 }))
        .await;

    assert_eq!(
        failed,
        json!({
            "content": [{ "type": "text", "text": "Upstream tool call failed" }],
            "isError": true,
        })
    );
    assert!(gateway.approval_requests().await.is_empty());
    let log = last_log(&gateway).await;
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::UpstreamException)
    );
    let summary = log.error_summary.as_deref().unwrap();
    assert!(summary.contains("Ads API answered HTTP 500"), "{summary}");
    assert!(!summary.contains("very-secret-token"), "{summary}");
}

#[tokio::test]
async fn runs_a_built_in_tool_the_administrator_set_to_run_without_asking() {
    let (gateway, _, _, plaintext) = ads_gateway(Some(json!({ "set_budget": "auto" }))).await;

    let result = gateway
        .call(&plaintext, "ads__set_budget", json!({ "amount": 250 }))
        .await;

    assert_eq!(result_text(&result), r#"{"budget":250,"previous":2.5}"#);
    assert!(gateway.approval_requests().await.is_empty());
}

// Tool approvals: pages. What the pages read and decide, without the pages.

/// A request waiting for a decision, as a call through the gateway leaves it.
async fn waiting_request(gateway: &TestMcpGateway, user: &User, id: i64) -> ApprovalRequest {
    let db = gateway.db();
    if count(db, "mcps").await == 0 {
        crm_mcp(gateway, user.id).await;
    }
    let created = create_named_access_token(db, user.id, "Claude", ScopeMode::All, &[], None).await;
    gateway
        .call(
            &created.plaintext,
            "crm__delete_contact",
            json!({ "id": id }),
        )
        .await;
    gateway.approval_requests().await.pop().unwrap()
}

async fn visible_ids(gateway: &TestMcpGateway, user: &User, condition: &str) -> Vec<i64> {
    let mut query = ApprovalService::visible_to(user);
    query.push(condition);
    query
        .build_query_as::<ApprovalRequest>()
        .fetch_all(&**gateway.db())
        .await
        .unwrap()
        .into_iter()
        .map(|request| request.id)
        .collect()
}

#[tokio::test]
async fn shows_what_mymcps_read_in_the_call_and_the_exact_arguments() {
    let gateway = crm_gateway().await;
    let admin = create_admin(gateway.db()).await;
    let request = waiting_request(&gateway, &admin, 42).await;

    let summary = gateway.approvals.summary(&request).unwrap();
    assert!(!summary.interpreted);
    assert_eq!(summary.title, "Run the tool \"delete_contact\" of CRM");
    assert_eq!(
        serde_json::to_value(&summary.details).unwrap(),
        json!([{ "label": "id", "value": "42" }])
    );
    assert_eq!(
        summary.tool_description.as_deref(),
        Some("Delete a contact for good.")
    );
    assert_eq!(
        gateway.approvals.arguments_text(&request).as_deref(),
        Some("{\n  \"id\": 42\n}")
    );
    assert_eq!(
        gateway.approvals.url(&request),
        Some(format!(
            "http://localhost:3333/approvals/{}",
            request.public_id
        ))
    );
    assert_eq!(gateway.approvals.pending_count(&admin).await.unwrap(), 1);
}

#[tokio::test]
async fn reads_nothing_of_a_request_whose_key_has_changed() {
    let gateway = crm_gateway().await;
    let admin = create_admin(gateway.db()).await;
    let mut request = waiting_request(&gateway, &admin, 42).await;

    request.arguments = "not what this key encrypted".into();
    request.summary = gateway.core.encrypt_secret(Some("{\"title\":5}")).unwrap();
    assert_eq!(gateway.approvals.arguments(&request), None);
    assert_eq!(gateway.approvals.arguments_text(&request), None);
    assert_eq!(gateway.approvals.summary(&request), None);

    request.summary = gateway.core.encrypt_secret(Some("not json")).unwrap();
    assert_eq!(gateway.approvals.summary(&request), None);
}

#[tokio::test]
async fn lets_a_member_decide_the_calls_of_their_own_access_tokens_and_records_it() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let member = create_member(db).await;
    let mut request = waiting_request(&gateway, &member, 42).await;
    assert_eq!(gateway.approvals.pending_count(&member).await.unwrap(), 1);
    assert_eq!(visible_ids(&gateway, &member, "").await, [request.id]);
    // The person took their time: an hour is left to decide.
    request.expires_at = Timestamp::now() + Duration::hours(1);
    request.save(&**db).await.unwrap();

    assert!(
        gateway
            .approvals
            .decide(&request, Decision::Approve, &member)
            .await
            .unwrap()
    );

    let decided = find_request(&gateway, request.id).await;
    assert_eq!(decided.status, ApprovalDecision::Approved);
    assert_eq!(decided.state(), ApprovalState::Approved);
    assert_eq!(decided.decided_by, Some(member.id));
    assert!(decided.decided_at.is_some());
    // An approval gives the agent a day to run the call.
    assert!(decided.expires_at > Timestamp::now() + Duration::hours(23));
    assert!(decided.expires_at <= Timestamp::now() + Duration::hours(24));

    // An administrator reads every request.
    assert_eq!(visible_ids(&gateway, &admin, "").await, [request.id]);
    assert_eq!(gateway.approvals.pending_count(&admin).await.unwrap(), 0);
    assert_eq!(gateway.approvals.pending_count(&member).await.unwrap(), 0);
}

#[tokio::test]
async fn keeps_the_calls_of_an_access_token_from_the_other_members() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let other = create_member(db).await;
    let request = waiting_request(&gateway, &admin, 42).await;
    let by_link = format!(" and `public_id` = '{}'", request.public_id);

    // The same answer as for a request that does not exist.
    assert!(visible_ids(&gateway, &other, &by_link).await.is_empty());
    assert!(visible_ids(&gateway, &other, "").await.is_empty());
    assert_eq!(gateway.approvals.pending_count(&other).await.unwrap(), 0);

    assert_eq!(visible_ids(&gateway, &admin, &by_link).await, [request.id]);
    assert_eq!(gateway.approvals.pending_count(&admin).await.unwrap(), 1);
    assert_eq!(
        find_request(&gateway, request.id).await.status,
        ApprovalDecision::Pending
    );
}

#[tokio::test]
async fn refuses_a_second_decision_and_a_late_one() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut request = waiting_request(&gateway, &admin, 42).await;
    request.expires_at = Timestamp::now() + Duration::hours(1);
    request.save(&**db).await.unwrap();
    let decide = async |request: &ApprovalRequest, decision: Decision| {
        gateway
            .approvals
            .decide(request, decision, &admin)
            .await
            .unwrap()
    };

    assert!(decide(&request, Decision::Deny).await);
    assert!(!decide(&request, Decision::Approve).await);
    let denied = find_request(&gateway, request.id).await;
    assert_eq!(denied.status, ApprovalDecision::Denied);
    assert_eq!(denied.decided_by, Some(admin.id));
    // A refusal leaves the request the time it had.
    assert_eq!(denied.expires_at, request.expires_at);

    let mut late = waiting_request(&gateway, &admin, 43).await;
    late.expires_at = Timestamp::now() - Duration::minutes(1);
    late.save(&**db).await.unwrap();
    assert!(!decide(&late, Decision::Approve).await);
    let expired = find_request(&gateway, late.id).await;
    assert_eq!(expired.status, ApprovalDecision::Pending);
    assert_eq!(expired.state(), ApprovalState::Expired);
    assert_eq!(expired.decided_by, None);
}

#[tokio::test]
async fn lists_what_waits_apart_from_what_was_decided() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let first = waiting_request(&gateway, &admin, 42).await;
    let second = waiting_request(&gateway, &admin, 43).await;
    gateway
        .approvals
        .decide(&first, Decision::Deny, &admin)
        .await
        .unwrap();
    let now = Timestamp::now().to_sql();

    let waiting = visible_ids(
        &gateway,
        &admin,
        &format!(" and `status` = 'pending' and `expires_at` > '{now}' order by `created_at` desc"),
    )
    .await;
    let past = visible_ids(
        &gateway,
        &admin,
        &format!(
            " and (`status` <> 'pending' or `expires_at` <= '{now}') order by `created_at` desc limit 50"
        ),
    )
    .await;

    assert_eq!(waiting, [second.id]);
    assert_eq!(past, [first.id]);
}

// What requests leave behind

#[tokio::test]
async fn deletes_the_requests_nobody_can_act_on_a_month_after_they_ended() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut old = waiting_request(&gateway, &admin, 42).await;
    let mut recent = waiting_request(&gateway, &admin, 43).await;
    let waiting = waiting_request(&gateway, &admin, 44).await;
    old.expires_at = Timestamp::now() - Duration::days(31);
    old.save(&**db).await.unwrap();
    recent.expires_at = Timestamp::now() - Duration::days(29);
    recent.save(&**db).await.unwrap();
    let remaining = async || {
        gateway
            .approval_requests()
            .await
            .into_iter()
            .map(|request| request.id)
            .collect::<Vec<_>>()
    };

    // The calls above let go of what had expired by then: within the hour,
    // nothing is looked for again.
    gateway.approvals.prune_expired(false).await;
    assert_eq!(remaining().await, [old.id, recent.id, waiting.id]);

    gateway.approvals.prune_expired(true).await;
    assert_eq!(remaining().await, [recent.id, waiting.id]);
}

#[tokio::test]
async fn lets_go_of_what_expired_when_a_request_comes_in() {
    let gateway = crm_gateway().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let created = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    // A request and a record of a call from long ago, written before any
    // request reached this server.
    let mut old = ApprovalRequest {
        public_id: "A".repeat(32),
        mcp_id: mcp.id,
        access_token_id: created.token.id,
        tool_name: "delete_contact".into(),
        arguments: "unread".into(),
        arguments_hash: arguments_hash(None),
        summary: "unread".into(),
        expires_at: Timestamp::now() - Duration::days(31),
        ..Default::default()
    };
    old.insert(&**db).await.unwrap();
    create_mcp_call_log(
        db,
        &created.token,
        Some(Timestamp::now() - Duration::days(15)),
    )
    .await;
    let recent_log = create_mcp_call_log(db, &created.token, None).await;

    // A request with a tool mode the gateway does not have is answered before that.
    let refused = gateway
        .post(
            &created.plaintext,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
            &[("x-mymcps-tool-mode", "sometimes")],
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(count(db, "approval_requests").await, 1);

    let pinged = gateway
        .post(
            &created.plaintext,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
            &[],
        )
        .await;
    assert_eq!(pinged.status, StatusCode::OK);

    // The answer does not wait for it.
    for _ in 0..500 {
        if count(db, "approval_requests").await == 0 && count(db, "mcp_call_logs").await == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(count(db, "approval_requests").await, 0);
    assert_eq!(
        call_logs(db)
            .await
            .iter()
            .map(|log| log.id)
            .collect::<Vec<_>>(),
        [recent_log.id]
    );
}
