//! `tests/functional/tool_approvals.spec.ts` for the pages: the page behind
//! the link an agent is given, the list, the decision, and the page where
//! the tools that ask are chosen. With them, the approval cases of
//! `vine_route_params.spec.ts`.
//!
//! The gate itself (which call is held, which approval lets which call
//! through, what an agent is told) is tested in the gateway crate. Here a
//! call goes through `/mcp` and its approval through the pages, as a person
//! and an agent meet them.

#[path = "support/gateway.rs"]
mod support;

use chrono::Duration;
use http::{Method, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::{
    ApprovalDecision, ApprovalRequest, ApprovalState, CallErrorCategory, Mcp, ScopeMode, User,
};
use mymcps_gateway::approvals::Decision;
use mymcps_web::testing::factories::{
    create_admin, create_admin_with, create_mcp, create_member, create_user,
};
use mymcps_web::testing::{TestRequest, TestResponse};
use serde_json::json;

use support::*;

const TOOLS: [(&str, Option<&str>); 2] = [
    ("list_contacts", Some("List the contacts of the account.")),
    ("delete_contact", Some("Delete a contact for good.")),
];

const NOT_FOUND: &str = "This approval request does not exist, was deleted after it expired, or belongs to the access token of another member";

/// A gateway in front of the CRM of the TypeScript tests.
async fn crm_gateway() -> TestGateway {
    TestGateway::new(listing(&TOOLS)).await
}

/// An MCP reached over HTTP whose `delete_contact` asks for approval.
async fn crm_mcp(gateway: &TestGateway, created_by: i64) -> Mcp {
    create_mcp(gateway, created_by, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals = Some(json!({ "delete_contact": "ask" }).to_string());
    })
    .await
}

/// The id in the link an agent was handed.
fn link_in(text: &str) -> Option<&str> {
    let start = "http://localhost:3333/approvals/";
    let id = &text[text.find(start)? + start.len()..];
    let end = id
        .find(|character: char| !(character.is_ascii_alphanumeric() || "_-".contains(character)))
        .unwrap_or(id.len());
    (end == 32).then(|| &id[..end])
}

/// A request waiting for a decision, as a call through the gateway leaves it.
async fn waiting_request(gateway: &TestGateway, user_id: i64) -> ApprovalRequest {
    let mcp = crm_mcp(gateway, user_id).await;
    let token = create_named_access_token(gateway, user_id, "Claude", ScopeMode::All, &[]).await;
    gateway
        .call(&token.plaintext, "crm__delete_contact", json!({ "id": 42 }))
        .await;
    gateway.last_request(&mcp).await
}

/// Another request on the MCP `waiting_request` created.
async fn waiting_request_for(gateway: &TestGateway, user_id: i64, id: i64) -> ApprovalRequest {
    let token = create_access_token(gateway, user_id, ScopeMode::All, &[]).await;
    gateway
        .call(&token.plaintext, "crm__delete_contact", json!({ "id": id }))
        .await;
    gateway
        .approval_requests()
        .await
        .pop()
        .expect("the request the call left")
}

fn page_of(request: &ApprovalRequest) -> String {
    format!("/approvals/{}", request.public_id)
}

async fn show(gateway: &TestGateway, request: &ApprovalRequest, user: &User) -> TestResponse {
    gateway.get(&page_of(request)).login_as(user).send().await
}

/// The decision form of a request, posted as the browser posts it.
fn decision<'app>(
    gateway: &'app TestGateway,
    request: &ApprovalRequest,
    user: &User,
    decision: &str,
) -> TestRequest<'app> {
    gateway
        .post(&page_of(request))
        .login_as(user)
        .csrf()
        .form(&[("decision", decision)])
}

async fn set_expiry(gateway: &TestGateway, request: &ApprovalRequest, expires_at: Timestamp) {
    sqlx::query("update `approval_requests` set `expires_at` = ? where `id` = ?")
        .bind(expires_at)
        .bind(request.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
}

fn flash(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

// Tool approvals: from the agent to the person and back

#[tokio::test]
async fn holds_a_tool_that_asks_and_hands_the_agent_a_link_instead_of_running_it() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let token = create_named_access_token(&gateway, admin.id, "Claude", ScopeMode::All, &[]).await;

    let result = gateway
        .call(
            &token.plaintext,
            "crm__delete_contact",
            json!({ "id": 42, "reason": "duplicate" }),
        )
        .await;

    assert_eq!(result["isError"], true);
    let text = result_text(&result);
    assert!(text.contains("Approval required: delete_contact on CRM was not run."));
    assert!(text.contains("call delete_contact again with exactly the same arguments"));
    assert!(gateway.upstreams.calls().is_empty());

    let request = gateway.last_request(&mcp).await;
    assert_eq!(link_in(text), Some(request.public_id.as_str()));
    assert_eq!(request.status, ApprovalDecision::Pending);
    assert_eq!(request.access_token_id, token.token.id);
    assert_eq!(request.tool_name, "delete_contact");
    // Encrypted at rest: neither the arguments nor the summary can be read in the table.
    assert!(!request.arguments.contains("duplicate"));
    assert!(!request.summary.contains("duplicate"));
    assert_eq!(
        gateway.state.mcp_gateway.approvals.arguments(&request),
        Some(json!({ "id": 42, "reason": "duplicate" }))
    );

    let logs = gateway.call_logs().await;
    let log = logs.last().unwrap();
    assert_eq!(
        log.error_category,
        Some(CallErrorCategory::ApprovalRequired)
    );

    // The link opens the page of that very call.
    let page = gateway
        .get(&format!("/approvals/{}", link_in(text).unwrap()))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(text_of(&page.text()).contains("reason duplicate"));
}

#[tokio::test]
async fn runs_the_call_a_person_approved_on_its_page_once_and_only_with_its_arguments() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = |id: i64| gateway.call(&plaintext, "crm__delete_contact", json!({ "id": id }));

    call(42).await;
    let request = gateway.last_request(&mcp).await;
    let approved = decision(&gateway, &request, &admin, "approve").send().await;
    assert_eq!(
        flash(&approved, "success").as_deref(),
        Some("Approved. The agent can now run this call, once.")
    );

    // Approving contact 42 does not let contact 43 go.
    let other = call(43).await;
    assert!(result_text(&other).contains("Approval required"));
    assert!(gateway.upstreams.calls().is_empty());

    let ran = call(42).await;
    assert!(ran.get("isError").is_none());
    assert_eq!(result_text(&ran), "ran delete_contact");
    assert_eq!(
        gateway.upstreams.calls(),
        [json!({ "name": "delete_contact", "arguments": { "id": 42 } })]
    );
    assert_eq!(
        find_request(&gateway, request.id).await.state(),
        ApprovalState::Used
    );
    let page = text_of(&show(&gateway, &request, &admin).await.text());
    assert!(page.contains("Approved and run"));
    assert!(page.contains("The agent ran the call on"));

    // The approval is spent: the same call asks again.
    let again = call(42).await;
    assert!(result_text(&again).contains("Approval required"));
    assert_eq!(gateway.upstreams.calls().len(), 1);
}

#[tokio::test]
async fn tells_the_agent_once_that_the_call_was_denied_on_its_page() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let call = || gateway.call(&plaintext, "crm__delete_contact", json!({ "id": 42 }));

    call().await;
    let request = gateway.last_request(&mcp).await;
    let denied = decision(&gateway, &request, &admin, "deny").send().await;
    assert_eq!(
        flash(&denied, "success").as_deref(),
        Some("Denied. The agent is told the call was refused.")
    );

    let refused = call().await;
    assert_eq!(refused["isError"], true);
    assert!(result_text(&refused).contains("Denied: a person refused this call to delete_contact"));
    let logs = gateway.call_logs().await;
    assert_eq!(
        logs.last().unwrap().error_category,
        Some(CallErrorCategory::ApprovalDenied)
    );

    // Asked again, it is a new request for the person to decide.
    let again = call().await;
    assert!(result_text(&again).contains("Approval required"));
    assert_eq!(gateway.approval_requests().await.len(), 2);
    assert!(gateway.upstreams.calls().is_empty());
}

// Tool approvals: pages

#[tokio::test]
async fn sends_a_guest_to_sign_in_and_brings_them_back_to_the_request() {
    let gateway = crm_gateway().await;
    let admin = create_admin_with(&gateway, "admin@example.com").await;
    let request = waiting_request(&gateway, admin.id).await;
    let path = page_of(&request);

    // The query string of the link is not carried to the sign-in page.
    let guest = gateway.get(&format!("{path}?from=agent")).send().await;
    assert_eq!(guest.status, StatusCode::FOUND);
    assert_eq!(guest.location(), Some("/login"));
    assert_eq!(guest.session().get("approvalReturnTo"), Some(&json!(path)));
    // Nothing of the request is said to someone who is not signed in.
    assert!(!guest.text().contains("delete_contact"));

    let signed_in = gateway
        .post("/login")
        .csrf()
        .session(guest.session())
        .form(&[("email", "admin@example.com"), ("password", "password123")])
        .send()
        .await;
    assert_eq!(signed_in.status, StatusCode::FOUND);
    assert_eq!(signed_in.location(), Some(path.as_str()));
    assert_eq!(signed_in.session().get("approvalReturnTo"), None);
}

#[tokio::test]
async fn only_ever_returns_from_sign_in_to_an_approval_request() {
    let gateway = crm_gateway().await;
    create_admin_with(&gateway, "admin@example.com").await;

    for return_to in [
        "https://evil.example/approvals/x",
        "/approvals/../settings",
        "//x",
    ] {
        let mut session = serde_json::Map::new();
        session.insert("approvalReturnTo".into(), json!(return_to));
        let response = gateway
            .post("/login")
            .csrf()
            .session(session)
            .form(&[("email", "admin@example.com"), ("password", "password123")])
            .send()
            .await;
        assert_eq!(response.location(), Some("/"), "{return_to}");
    }

    // A link that is not one leaves no path to come back to.
    for id in ["short", "..%2Fsettings", &"A".repeat(33)] {
        let guest = gateway.get(&format!("/approvals/{id}")).send().await;
        assert_eq!(guest.location(), Some("/login"), "{id}");
        assert_eq!(guest.session().get("approvalReturnTo"), None, "{id}");
    }
}

#[tokio::test]
async fn answers_head_without_leaving_a_return_path_behind() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;

    let head = gateway
        .request(Method::HEAD, &page_of(&request))
        .send()
        .await;

    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(head.header("allow"), Some("GET"));
    assert_eq!(head.session().get("approvalReturnTo"), None);
}

#[tokio::test]
async fn shows_what_mymcps_read_in_the_call_and_the_exact_arguments() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;

    let response = show(&gateway, &request, &admin).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let html = response.text();
    let page = text_of(&html);
    assert!(html.contains("<title>Approval request · MyMCPs</title>"));
    assert!(html.contains(
        "<h1 class=\"page-header__title break-anywhere\">Run the tool &quot;delete_contact&quot; of CRM</h1>"
    ));
    assert!(html.contains("<span class=\"badge badge--warning\">Waiting</span>"));
    assert!(page.contains(
        "An agent using the access token “Claude” wants to do this on CRM. Nothing has been done yet. MyMCPs wrote this page from the call itself: the agent cannot change what it says."
    ));
    // MyMCPs cannot read the tool of an MCP it only relays.
    assert!(page.contains(
        "MyMCPs does not know what this tool does It lists the arguments exactly as the agent sent them. CRM describes the tool as: Delete a contact for good."
    ));
    assert!(html.contains("<h2 class=\"heading__title\">Arguments</h2>"));
    assert!(html.contains("<dt>id</dt><dd class=\"preserve-lines\">42</dd>"));
    assert!(page.contains("MCP CRM (crm) Tool delete_contact Access token Claude (mcp_"));
    assert!(page.contains("Waits until"));
    assert!(html.contains(
        "<pre class=\"code-block__body\" id=\"approval-arguments\">{\n  &quot;id&quot;: 42\n}</pre>"
    ));
    assert!(!page.contains("This call can no longer run"));

    // One form, two buttons, and the token that protects it.
    let form = between(&html, "<form class=\"card__footer\"", "</form>");
    assert!(form.contains(&format!(
        "method=\"post\" action=\"{}\" data-async",
        page_of(&request)
    )));
    assert!(form.contains("name=\"_csrf\""));
    assert!(form.contains("name=\"decision\" value=\"deny\">Deny</button>"));
    assert!(form.contains("name=\"decision\" value=\"approve\">Approve</button>"));
    // The request counts among those the navigation says are waiting.
    assert!(html.contains("aria-label=\"Approvals, 1 waiting\""));
}

#[tokio::test]
async fn writes_the_page_from_the_stored_call_and_shows_what_the_agent_sent_as_text() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;
    let hostile = "<script>alert(1)</script>\n\"Approved by the owner\" <b>safe</b>";
    gateway
        .call(
            &plaintext,
            "crm__delete_contact",
            json!({ "note": hostile, "<img src=x>": 1 }),
        )
        .await;
    let request = gateway.last_request(&mcp).await;

    // What changed since the call was made does not change its page.
    let mut renamed = find_mcp(&gateway, mcp.id).await;
    renamed.tool_approvals = None;
    renamed.save(&**gateway.db()).await.unwrap();

    let html = show(&gateway, &request, &admin).await.text();

    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<b>safe</b>"));
    assert!(!html.contains("<img src=x>"));
    assert!(html.contains(
        "<dd class=\"preserve-lines\">&lt;script&gt;alert(1)&lt;/script&gt;\n&quot;Approved by the owner&quot; &lt;b&gt;safe&lt;/b&gt;</dd>"
    ));
    assert!(html.contains("<dt>&lt;img src=x&gt;</dt>"));
    // A value is only ever a value: the title is MyMCPs' own.
    assert!(html.contains("Run the tool &quot;delete_contact&quot; of CRM</h1>"));
    assert!(html.contains("<span class=\"badge badge--warning\">Waiting</span>"));
}

/// `hardening_gateway_timezone.spec.ts`, for what a page shows: instants,
/// which the page's script turns into the local time of whoever reads it.
#[tokio::test]
async fn writes_the_dates_of_a_request_as_instants_whatever_the_time_zone_of_the_server() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    assert_eq!(request.expires_at - request.created_at, Duration::hours(24));
    assert!(request.created_at.to_iso().ends_with('Z'));

    let html = show(&gateway, &request, &admin).await.text();

    assert!(html.contains(&format!(
        "Asked on <time datetime=\"{}\">",
        request.created_at.to_iso()
    )));
    assert!(html.contains(&format!(
        "<dt>Waits until</dt><dd><time datetime=\"{}\">",
        request.expires_at.to_iso()
    )));
    let list = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(list.contains(&format!(
        "<td class=\"cell-secondary\"><time datetime=\"{}\">",
        request.expires_at.to_iso()
    )));
}

#[tokio::test]
async fn shows_every_state_a_request_can_be_in() {
    let gateway = crm_gateway().await;
    let admin = create_user(&gateway, None, mymcps_core::models::UserRole::Admin).await;
    let request = waiting_request(&gateway, admin.id).await;
    let mcp = find_mcp(&gateway, request.mcp_id).await;
    let page = async || text_of(&show(&gateway, &request, &admin).await.text());
    let has_form = async || {
        show(&gateway, &request, &admin)
            .await
            .text()
            .contains("<form class=\"card__footer\"")
    };

    // Pending: nothing but the call and the two buttons.
    let pending = page().await;
    assert!(pending.contains("Waiting Asked on"));
    assert!(pending.contains("Approving lets the agent run this call once"));
    assert!(has_form().await);

    // Approved: the agent has a day to run it.
    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);
    let approved = page().await;
    assert!(approved.contains("Approved Asked on"));
    assert!(approved.contains("Approved Approved by Test User on "));
    assert!(
        approved.contains(". The agent can run this call once, with these exact arguments, until ")
    );
    assert!(approved.contains(
        "An agent using the access token “Claude” asked to do this on CRM. MyMCPs wrote"
    ));
    // A decided request: no "Waits until", no sentence about approving, no form.
    assert!(!approved.contains("Waits until"));
    assert!(!approved.contains("Approving lets the agent"));
    assert!(!has_form().await);

    // Approved, but the agent never ran it.
    set_expiry(&gateway, &request, Timestamp::now() - Duration::minutes(1)).await;
    let lapsed = page().await;
    assert!(lapsed.contains("Expired Asked on"));
    assert!(lapsed.contains("Expired Approved by Test User on "));
    assert!(lapsed.contains(", but the agent did not run the call in time. It was not run."));
    assert!(!has_form().await);

    // Approved and run.
    set_expiry(&gateway, &request, Timestamp::now() + Duration::hours(1)).await;
    sqlx::query("update `approval_requests` set `consumed_at` = ? where `id` = ?")
        .bind(Timestamp::now())
        .bind(request.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    let used = page().await;
    assert!(used.contains("Approved and run Asked on"));
    assert!(used.contains("Approved and run Approved by Test User on "));
    assert!(used.contains(". The agent ran the call on "));
    assert!(!has_form().await);

    // Denied.
    let denied = waiting_request_for(&gateway, admin.id, 43).await;
    assert!(gateway.decide(&mcp, Decision::Deny, &admin).await);
    let html = show(&gateway, &denied, &admin).await.text();
    let text = text_of(&html);
    assert!(html.contains("<span class=\"badge badge--critical\">Denied</span>"));
    assert!(html.contains("<div class=\"banner banner--critical\" role=\"status\">"));
    assert!(text.contains("Denied Denied by Test User on "));
    assert!(text.contains(". The call was not run."));
    assert!(!html.contains("<form class=\"card__footer\""));

    // Nobody decided in time.
    let late = waiting_request_for(&gateway, admin.id, 44).await;
    set_expiry(&gateway, &late, Timestamp::now() - Duration::minutes(1)).await;
    let html = show(&gateway, &late, &admin).await.text();
    assert!(html.contains("<span class=\"badge\">Expired</span>"));
    assert!(text_of(&html).contains(
        "Expired Nobody decided in time. The call was not run, and the agent has to ask again."
    ));
    assert!(!html.contains("<form class=\"card__footer\""));
}

#[tokio::test]
async fn names_a_decider_whose_account_is_gone_without_their_name() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let colleague = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    let mcp = find_mcp(&gateway, request.mcp_id).await;
    assert!(gateway.decide(&mcp, Decision::Deny, &colleague).await);
    sqlx::query("delete from `users` where `id` = ?")
        .bind(colleague.id)
        .execute(&**gateway.db())
        .await
        .unwrap();

    let page = text_of(&show(&gateway, &request, &admin).await.text());

    assert!(page.contains("Denied by a member who has since left on "));
}

#[tokio::test]
async fn says_when_a_waiting_call_can_no_longer_run() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    let warning = "This call can no longer run Its MCP is disabled, or its access token was revoked or has expired. Approving it changes nothing.";
    let page = async || text_of(&show(&gateway, &request, &admin).await.text());
    assert!(!page().await.contains(warning));

    let mut mcp = find_mcp(&gateway, request.mcp_id).await;
    mcp.enabled = false;
    mcp.save(&**gateway.db()).await.unwrap();
    assert!(page().await.contains(warning));

    mcp.enabled = true;
    mcp.save(&**gateway.db()).await.unwrap();
    sqlx::query("update `access_tokens` set `revoked_at` = ? where `id` = ?")
        .bind(Timestamp::now())
        .bind(request.access_token_id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    let revoked = page().await;
    assert!(revoked.contains(warning));
    // It can still be decided, which is how it leaves the list.
    assert!(revoked.contains("Deny Approve"));

    // Once decided, the page says what was decided and no more.
    assert!(gateway.decide(&mcp, Decision::Deny, &admin).await);
    assert!(!page().await.contains(warning));
}

#[tokio::test]
async fn shows_nothing_of_a_call_it_can_no_longer_decrypt_and_still_lets_it_be_denied() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    // What another APP_KEY would leave: text this key cannot read.
    sqlx::query("update `approval_requests` set `summary` = ?, `arguments` = ? where `id` = ?")
        .bind("v1:not-of-this-key")
        .bind("v1:not-of-this-key")
        .bind(request.id)
        .execute(&**gateway.db())
        .await
        .unwrap();

    let html = show(&gateway, &request, &admin).await.text();
    let page = text_of(&html);

    // The title falls back to the tool name.
    assert!(html.contains(
        "<h1 class=\"page-header__title break-anywhere\">Run the tool &quot;delete_contact&quot;</h1>"
    ));
    assert!(html.contains("<div class=\"banner banner--critical\" role=\"alert\">"));
    assert!(page.contains(
        "This request can no longer be read It was encrypted with another APP_KEY. Deny it and have the agent ask again."
    ));
    assert!(!page.contains("MyMCPs does not know what this tool does"));
    assert!(!html.contains("Arguments</h2>"));
    assert!(!html.contains("What it changes"));
    assert!(!html.contains("code-block"));
    assert!(page.contains("MCP CRM (crm) Tool delete_contact"));

    let list = text_of(
        &gateway
            .get("/approvals")
            .login_as(&admin)
            .send()
            .await
            .text(),
    );
    assert!(list.contains("Run the tool \"delete_contact\" delete_contact · CRM · token Claude"));

    let denied = decision(&gateway, &request, &admin, "deny").send().await;
    assert_eq!(
        flash(&denied, "success").as_deref(),
        Some("Denied. The agent is told the call was refused.")
    );
}

#[tokio::test]
async fn shows_a_call_without_arguments_and_the_warnings_of_a_long_one() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let plaintext = create_access_token(&gateway, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    gateway
        .call(&plaintext, "crm__delete_contact", json!({}))
        .await;
    let empty = gateway.last_request(&mcp).await;
    let html = show(&gateway, &empty, &admin).await.text();
    assert!(html.contains("<p class=\"text-secondary\">The call has no arguments.</p>"));
    assert!(html.contains("<pre class=\"code-block__body\" id=\"approval-arguments\">{}</pre>"));

    let ids: Vec<i64> = (0..72).collect();
    gateway
        .call(&plaintext, "crm__delete_contact", json!({ "ids": ids }))
        .await;
    let long = gateway.last_request(&mcp).await;
    let html = show(&gateway, &long, &admin).await.text();
    assert!(html.contains(
        "<p class=\"banner__title\">Only the first 60 values are listed, and 12 more are not. Read the exact arguments before you decide.</p>"
    ));
    assert!(html.contains("<dt>ids[59]</dt>"));
    assert!(!html.contains("<dt>ids[60]</dt>"));
}

#[tokio::test]
async fn lets_a_member_decide_the_calls_of_their_own_access_tokens_and_records_it() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mut member = create_member(&gateway).await;
    member.full_name = Some("Mona Member".into());
    member.save(&**gateway.db()).await.unwrap();
    let request = waiting_request(&gateway, member.id).await;

    let response = decision(&gateway, &request, &member, "approve")
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some(page_of(&request).as_str()));
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Approved. The agent can now run this call, once.")
    );
    let decided = find_request(&gateway, request.id).await;
    assert_eq!(decided.status, ApprovalDecision::Approved);
    assert_eq!(decided.decided_by, Some(member.id));
    assert!(decided.expires_at > Timestamp::now() + Duration::hours(23));

    // An administrator reads every request.
    let html = show(&gateway, &request, &admin).await.text();
    assert!(html.contains("<span class=\"badge badge--success\">Approved</span>"));
    assert!(text_of(&html).contains("Approved by Mona Member on "));
    assert!(!html.contains("waiting\""));
}

#[tokio::test]
async fn keeps_the_calls_of_an_access_token_from_the_other_members() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let other = create_member(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;

    // The same answer as for a request that does not exist.
    let page = show(&gateway, &request, &other).await;
    assert_eq!(page.status, StatusCode::FOUND);
    assert_eq!(page.location(), Some("/approvals"));
    assert_eq!(flash(&page, "error").as_deref(), Some(NOT_FOUND));
    assert!(!page.text().contains("delete_contact"));

    let refused = decision(&gateway, &request, &other, "approve").send().await;
    assert_eq!(refused.location(), Some("/approvals"));
    assert!(
        flash(&refused, "error")
            .unwrap()
            .contains("belongs to the access token of another member")
    );
    assert_eq!(
        find_request(&gateway, request.id).await.status,
        ApprovalDecision::Pending
    );

    let list = gateway
        .get("/approvals")
        .login_as(&other)
        .send()
        .await
        .text();
    assert!(list.contains("Nothing is waiting"));
    assert!(!list.contains(&request.public_id));
    assert!(!list.contains("Decided or expired"));
    assert!(!list.contains("waiting\""));

    let all = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(all.contains(&format!("href=\"{}\"", page_of(&request))));
    assert!(all.contains("aria-label=\"Approvals, 1 waiting\""));
}

#[tokio::test]
async fn refuses_a_decision_from_a_guest_a_second_decision_and_a_late_one() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    let status = async |request: &ApprovalRequest| find_request(&gateway, request.id).await.status;

    let guest = gateway
        .post(&page_of(&request))
        .csrf()
        .form(&[("decision", "approve")])
        .send()
        .await;
    assert_eq!(guest.location(), Some("/login"));
    assert_eq!(status(&request).await, ApprovalDecision::Pending);

    let unknown = decision(&gateway, &request, &admin, "maybe").send().await;
    assert_eq!(unknown.status, StatusCode::FOUND);
    assert_eq!(
        unknown.flashed("errors"),
        Some(json!({ "decision": "The selected decision is invalid" }))
    );
    assert_eq!(
        flash(&unknown, "error").as_deref(),
        Some("The selected decision is invalid")
    );
    assert_eq!(status(&request).await, ApprovalDecision::Pending);
    let missing = gateway
        .post(&page_of(&request))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert!(missing.flashed("errors").is_some());
    assert_eq!(status(&request).await, ApprovalDecision::Pending);

    let denied = decision(&gateway, &request, &admin, "deny").send().await;
    assert_eq!(
        flash(&denied, "success").as_deref(),
        Some("Denied. The agent is told the call was refused.")
    );
    let changed = decision(&gateway, &request, &admin, "approve").send().await;
    assert_eq!(
        flash(&changed, "error").as_deref(),
        Some("This request has expired or was already decided")
    );
    assert_eq!(changed.location(), Some(page_of(&request).as_str()));
    assert_eq!(status(&request).await, ApprovalDecision::Denied);

    let late = waiting_request_for(&gateway, admin.id, 43).await;
    set_expiry(&gateway, &late, Timestamp::now() - Duration::minutes(1)).await;
    let expired = decision(&gateway, &late, &admin, "approve").send().await;
    assert_eq!(
        flash(&expired, "error").as_deref(),
        Some("This request has expired or was already decided")
    );
    assert_eq!(status(&late).await, ApprovalDecision::Pending);
}

#[tokio::test]
async fn takes_a_decision_only_from_the_form_of_the_page() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    let pending =
        async || find_request(&gateway, request.id).await.status == ApprovalDecision::Pending;

    // Without the token of the page, from a browser and from a script.
    let forged = gateway
        .post(&page_of(&request))
        .login_as(&admin)
        .form(&[("decision", "approve")])
        .send()
        .await;
    assert_eq!(forged.status, StatusCode::FOUND);
    assert_eq!(
        flash(&forged, "error").as_deref(),
        Some("Invalid or expired CSRF token")
    );
    let scripted = gateway
        .post(&page_of(&request))
        .login_as(&admin)
        .api()
        .json(json!({ "decision": "approve" }))
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::FORBIDDEN);
    assert!(pending().await);

    // A link cannot decide: following one only ever shows the page.
    let followed = gateway
        .get(&format!("{}?decision=approve", page_of(&request)))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(followed.status, StatusCode::OK);
    assert!(pending().await);

    // The page's script posts the same form, and is told where to go.
    let sent = decision(&gateway, &request, &admin, "approve")
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
        .send()
        .await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT);
    assert_eq!(sent.header("x-location"), Some(page_of(&request).as_str()));
    assert_eq!(
        flash(&sent, "success").as_deref(),
        Some("Approved. The agent can now run this call, once.")
    );
    assert!(!pending().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lets_one_of_two_decisions_made_at_once_win() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let colleague = create_admin(&gateway).await;
    let first = waiting_request(&gateway, admin.id).await;
    let mut requests = vec![first];
    for id in 43..51 {
        requests.push(waiting_request_for(&gateway, admin.id, id).await);
    }

    for request in &requests {
        let (approval, denial) = tokio::join!(
            decision(&gateway, request, &admin, "approve").send(),
            decision(&gateway, request, &colleague, "deny").send(),
        );

        let won: Vec<bool> = [&approval, &denial]
            .iter()
            .map(|response| response.flashed("success").is_some())
            .collect();
        assert_eq!(won.iter().filter(|won| **won).count(), 1, "{won:?}");
        let loser = if won[0] { &denial } else { &approval };
        assert_eq!(
            flash(loser, "error").as_deref(),
            Some("This request has expired or was already decided")
        );
        // What is recorded is the decision that won, whole.
        let decided = find_request(&gateway, request.id).await;
        let (status, decider) = if won[0] {
            (ApprovalDecision::Approved, admin.id)
        } else {
            (ApprovalDecision::Denied, colleague.id)
        };
        assert_eq!(decided.status, status);
        assert_eq!(decided.decided_by, Some(decider));
    }
}

#[tokio::test]
async fn lists_what_waits_apart_from_what_was_decided() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let first = waiting_request(&gateway, admin.id).await;
    let second = waiting_request_for(&gateway, admin.id, 43).await;
    let third = waiting_request_for(&gateway, admin.id, 44).await;
    let mcp = find_mcp(&gateway, first.mcp_id).await;
    let approvals = &gateway.state.mcp_gateway.approvals;
    assert!(
        approvals
            .decide(&first, Decision::Deny, &admin)
            .await
            .unwrap()
    );

    let response = gateway.get("/approvals").login_as(&admin).send().await;

    assert_eq!(response.status, StatusCode::OK);
    let html = response.text();
    assert!(html.contains("<title>Approvals · MyMCPs</title>"));
    let waiting = between(&html, "<section class=\"card hide-mobile\">", "</section>");
    assert!(waiting.contains("<h2 class=\"card__title\">Waiting for a decision</h2><p class=\"card__subtitle\">2 requests</p>"));
    assert!(waiting.contains("<th class=\"col-156\">Waits until</th>"));
    // Newest first, each with the button that opens it.
    let third_at = waiting.find(&page_of(&third)).unwrap();
    let second_at = waiting.find(&page_of(&second)).unwrap();
    assert!(third_at < second_at);
    assert!(!waiting.contains(&page_of(&first)));
    assert!(waiting.contains(&format!(
        "<a class=\"button button--secondary\" href=\"{}\" aria-label=\"Review: Run the tool &quot;delete_contact&quot; of CRM\">Review</a>",
        page_of(&second)
    )));
    assert!(waiting.contains("<span class=\"badge badge--warning\">Waiting</span>"));
    assert!(
        text_of(waiting)
            .contains("Run the tool \"delete_contact\" of CRM delete_contact · CRM · token ")
    );

    let past = &html[html.find("Decided or expired").unwrap()..];
    let past = between(past, "<table", "</table>");
    assert!(past.contains("<th class=\"col-156\">Expires</th>"));
    assert!(past.contains(&format!(
        "<a class=\"button\" href=\"{}\" aria-label=\"Open: Run the tool &quot;delete_contact&quot; of CRM\">Open</a>",
        page_of(&first)
    )));
    assert!(past.contains("<span class=\"badge badge--critical\">Denied</span>"));
    // A denied call has no date it counts until.
    assert!(past.contains("<td class=\"cell-tertiary\">—</td>"));
    assert!(!past.contains(&page_of(&second)));

    // Under 768px the same requests are a list whose rows open them.
    assert!(html.contains(&format!(
        "<a class=\"list-row list-row--wrap\" href=\"{}\">",
        page_of(&second)
    )));
    assert!(html.contains("<span class=\"list-row__meta\">Asked <time"));

    // An approval the agent has not used yet says until when it can.
    assert!(gateway.decide(&mcp, Decision::Approve, &admin).await);
    let html = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();
    let past = &html[html.find("Decided or expired").unwrap()..];
    assert!(past.contains("<span class=\"badge badge--success\">Approved</span>"));
    assert!(past.contains("<p class=\"card__subtitle\">2 requests</p>"));
    assert!(html.contains("<p class=\"card__subtitle\">1 request</p>"));
}

#[tokio::test]
async fn shows_an_empty_list_until_a_call_asks() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;

    let html = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();

    assert!(html.contains("<h2 class=\"card__title\">Waiting for a decision</h2>"));
    assert!(!html.contains("card__subtitle"));
    assert!(text_of(&html).contains(
        "Nothing is waiting When an agent calls a tool that asks for approval, it gets a link to give you, and the call is listed here."
    ));
    assert!(!html.contains("<table"));
    assert!(!html.contains("Decided or expired"));

    // The list is a page of signed-in people.
    let guest = gateway.get("/approvals").send().await;
    assert_eq!(guest.location(), Some("/login"));
}

#[tokio::test]
async fn lists_at_most_fifty_past_requests() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let request = waiting_request(&gateway, admin.id).await;
    for index in 0..54 {
        sqlx::query(
            "insert into `approval_requests` (`public_id`, `mcp_id`, `access_token_id`, `tool_name`, `arguments`, `arguments_hash`, `summary`, `status`, `expires_at`, `created_at`) \
             select ?, `mcp_id`, `access_token_id`, `tool_name`, `arguments`, ?, `summary`, 'denied', `expires_at`, `created_at` from `approval_requests` where `id` = ?",
        )
        .bind(format!("{index:0>32}"))
        .bind(format!("hash-{index}"))
        .bind(request.id)
        .execute(&**gateway.db())
        .await
        .unwrap();
    }

    let html = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();

    let past = &html[html.find("Decided or expired").unwrap()..];
    let table = between(past, "<tbody>", "</tbody>");
    assert_eq!(table.matches("<tr>").count(), 50);
    // The last ones made, whatever came before.
    assert!(table.contains(&format!("/approvals/{:0>32}", 53)));
    assert!(!table.contains(&format!("/approvals/{:0>32}\"", 3)));
}

#[tokio::test]
async fn answers_an_unknown_or_malformed_link_with_the_list() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;

    for id in ["A".repeat(32).as_str(), "short", "..%2Fsettings", "%FF"] {
        let response = gateway
            .get(&format!("/approvals/{id}?from=agent"))
            .login_as(&admin)
            .send()
            .await;
        assert_eq!(response.status, StatusCode::FOUND, "{id}");
        assert_eq!(response.location(), Some("/approvals"), "{id}");
        assert_eq!(flash(&response, "error").as_deref(), Some(NOT_FOUND));

        let decided = gateway
            .post(&format!("/approvals/{id}"))
            .login_as(&admin)
            .csrf()
            .form(&[("decision", "approve")])
            .send()
            .await;
        assert_eq!(decided.status, StatusCode::FOUND, "{id}");
        assert_eq!(decided.location(), Some("/approvals"), "{id}");
        assert_eq!(flash(&decided, "error").as_deref(), Some(NOT_FOUND));
    }
}

// Tool approvals: choosing the tools that ask

fn tools_path(mcp: &Mcp) -> String {
    format!("/mcps/{}/tools", mcp.id)
}

async fn tools_page(gateway: &TestGateway, mcp: &Mcp, user: &User) -> String {
    let response = gateway.get(&tools_path(mcp)).login_as(user).send().await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.text()
}

/// The rows of the page: each tool with the mode its radios show, and the
/// markers under its name.
fn tool_rows(html: &str) -> Vec<(String, &'static str, Vec<&'static str>)> {
    let body = between(html, "<tbody>", "</tbody>");
    body.split("<tr ")
        .skip(1)
        .map(|row| {
            let name = between(row, "<span class=\"cell-code\">", "</span>").to_string();
            let asks = row.contains("value=\"ask\" checked");
            let runs = row.contains("value=\"auto\" checked");
            assert!(asks != runs, "{row}");
            let mut markers = Vec::new();
            if row.contains("<span class=\"status\">No longer listed by the MCP</span>") {
                markers.push("unlisted");
            }
            if row.contains("<span class=\"status status--warning\">Asks by default</span>") {
                markers.push("asks by default");
            }
            (name, if asks { "ask" } else { "auto" }, markers)
        })
        .collect()
}

fn saved_choices(mcp: &Mcp) -> serde_json::Value {
    serde_json::from_str(mcp.tool_approvals.as_deref().unwrap_or("null")).unwrap()
}

#[tokio::test]
async fn lists_the_tools_of_a_connected_mcp_with_what_is_saved_for_them() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;

    let html = tools_page(&gateway, &mcp, &admin).await;

    assert!(html.contains("<title>Tool approvals · CRM · MyMCPs</title>"));
    assert_eq!(
        tool_rows(&html),
        [
            ("list_contacts".to_string(), "auto", vec![]),
            ("delete_contact".to_string(), "ask", vec![]),
        ]
    );
    let page = text_of(&html);
    assert!(page.contains("MCPs CRM Tool approvals Tool approvals Choose what happens when an agent calls a tool of CRM."));
    assert!(html.contains(&format!("<a href=\"/mcps/{}\">CRM</a>", mcp.id)));
    assert!(page.contains("1 of 2 tools ask for approval"));
    assert!(html.contains("<span class=\"cell-sub clamp-3\">Delete a contact for good.</span>"));
    assert!(html.contains("data-filter-text=\"list_contacts List the contacts of the account.\""));
    assert!(!html.contains("did not list its tools"));
    assert!(!html.contains("The saved choices could not be read"));

    // The form sends every tool, with the choice shown for it.
    assert!(html.contains(&format!(
        "<form class=\"page__body\" id=\"mcp-tools-form\" method=\"post\" action=\"/mcps/{}/tools?_method=PUT\" data-async data-dirty data-filter>",
        mcp.id
    )));
    assert!(html.contains("<input type=\"hidden\" name=\"toolCount\" value=\"2\">"));
    assert!(
        html.contains("<input type=\"hidden\" name=\"tools[1][name]\" value=\"delete_contact\">")
    );
    assert!(html.contains(
        "<input type=\"radio\" class=\"visually-hidden\" name=\"tools[1][mode]\" value=\"ask\" checked data-select-item>"
    ));
    assert!(html.contains(
        "<input type=\"radio\" class=\"visually-hidden\" name=\"tools[0][mode]\" value=\"auto\" checked>"
    ));
    assert!(html.contains("role=\"radiogroup\" aria-label=\"When an agent calls delete_contact\""));
}

#[tokio::test]
async fn saves_the_choices_that_differ_from_the_defaults() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let put = |tools: serde_json::Value| {
        gateway
            .put(&tools_path(&mcp))
            .login_as(&admin)
            .csrf()
            .json(json!({ "tools": tools }))
            .send()
    };

    let response = put(json!([
        { "name": "list_contacts", "mode": "ask" },
        { "name": "delete_contact", "mode": "auto" },
    ]))
    .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.location(), Some(tools_path(&mcp).as_str()));
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Tool approvals saved")
    );
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "list_contacts": "ask" })
    );

    let unknown = put(json!([{ "name": "list_contacts", "mode": "never" }])).await;
    assert_eq!(unknown.status, StatusCode::FOUND);
    assert_eq!(
        unknown.flashed("errors"),
        Some(json!({ "tools.0.mode": "tools must be one of: auto, ask" }))
    );
    assert_eq!(
        flash(&unknown, "error").as_deref(),
        Some("tools must be one of: auto, ask")
    );
    for refused in [
        json!("list_contacts"),
        json!([{ "mode": "ask" }]),
        json!(null),
    ] {
        let response = put(refused).await;
        assert!(response.flashed("errors").is_some());
    }
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "list_contacts": "ask" })
    );

    // Every tool back to what it does by default: nothing is left saved.
    put(json!([
        { "name": "list_contacts", "mode": "auto" },
        { "name": "delete_contact", "mode": "auto" },
    ]))
    .await;
    assert_eq!(find_mcp(&gateway, mcp.id).await.tool_approvals, None);
}

#[tokio::test]
async fn saves_what_the_form_of_the_page_sends_with_and_without_its_script() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;
    let form = |list_contacts: &'static str, delete_contact: &'static str| {
        gateway
            .post(&format!("{}?_method=PUT", tools_path(&mcp)))
            .login_as(&admin)
            .csrf()
            .form(&[
                ("toolCount", "2"),
                ("tools[0][name]", "list_contacts"),
                ("tools[0][mode]", list_contacts),
                ("tools[1][name]", "delete_contact"),
                ("tools[1][mode]", delete_contact),
            ])
    };

    // Without the script: a redirect back to the page, which shows the message.
    let plain = form("ask", "ask").send().await;
    assert_eq!(plain.status, StatusCode::FOUND);
    assert_eq!(plain.location(), Some(tools_path(&mcp).as_str()));
    assert_eq!(
        flash(&plain, "success").as_deref(),
        Some("Tool approvals saved")
    );
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "list_contacts": "ask", "delete_contact": "ask" })
    );

    // With it: the saved form again, in place, and the message with it.
    let scripted = form("auto", "ask")
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::OK);
    assert_eq!(scripted.flashed("success"), None);
    let fragment = scripted.text();
    assert!(!fragment.contains("<html"));
    assert!(fragment.starts_with("<form class=\"page__body\" id=\"mcp-tools-form\""));
    assert!(text_of(&fragment).contains("1 of 2 tools ask for approval"));
    assert!(fragment.contains("<div class=\"toast\" data-toast role=\"status\">"));
    assert!(fragment.contains("<p class=\"toast__message\">Tool approvals saved</p>"));
    assert_eq!(
        tool_rows(&fragment),
        [
            ("list_contacts".to_string(), "auto", vec![]),
            ("delete_contact".to_string(), "ask", vec![]),
        ]
    );
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "delete_contact": "ask" })
    );

    // A choice that is none: the form again as it is saved, and why.
    let refused = form("sometimes", "auto")
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let fragment = refused.text();
    assert!(fragment.contains("<div class=\"toast toast--error\" data-toast role=\"alert\">"));
    assert!(fragment.contains("<p class=\"toast__message\">tools must be one of: auto, ask</p>"));
    assert_eq!(tool_rows(&fragment)[1].1, "ask");

    // Nothing is saved without the token of the page.
    let forged = gateway
        .post(&format!("{}?_method=PUT", tools_path(&mcp)))
        .login_as(&admin)
        .form(&[
            ("tools[0][name]", "delete_contact"),
            ("tools[0][mode]", "auto"),
        ])
        .send()
        .await;
    assert_eq!(
        flash(&forged, "error").as_deref(),
        Some("Invalid or expired CSRF token")
    );
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "delete_contact": "ask" })
    );
}

#[tokio::test]
async fn keeps_the_saved_choices_in_view_when_the_mcp_cannot_be_reached() {
    let gateway = TestGateway::new(|_| {
        Reply::Error("connect ECONNREFUSED with Bearer crm-secret-token".into())
    })
    .await;
    let admin = create_admin(&gateway).await;
    let secret = gateway.core.encrypt_secret(Some("crm-secret-token"));
    let mcp = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.auth_type = mymcps_core::models::McpAuthType::Bearer;
        mcp.auth_bearer = secret;
        mcp.tool_approvals =
            Some(json!({ "zeta": "auto", "delete_contact": "ask", "Archive": "ask" }).to_string());
    })
    .await;

    let html = tools_page(&gateway, &mcp, &admin).await;

    // In the order `sort()` gave the names.
    assert_eq!(
        tool_rows(&html),
        [
            ("Archive".to_string(), "ask", vec!["unlisted"]),
            ("delete_contact".to_string(), "ask", vec!["unlisted"]),
            ("zeta".to_string(), "auto", vec!["unlisted"]),
        ]
    );
    assert!(!html.contains("cell-sub clamp-3"));
    let page = text_of(&html);
    assert!(page.contains("CRM did not list its tools"));
    assert!(page.contains("connect ECONNREFUSED"));
    assert!(page.contains(" Only the tools with a saved choice are shown."));
    assert!(html.contains("<div class=\"banner banner--critical\" role=\"alert\">"));
    assert!(!html.contains("crm-secret-token"));

    // They can still be changed while the MCP is away.
    let saved = gateway
        .put(&tools_path(&mcp))
        .login_as(&admin)
        .csrf()
        .json(json!({ "tools": [{ "name": "delete_contact", "mode": "ask" }] }))
        .send()
        .await;
    assert_eq!(saved.status, StatusCode::FOUND);
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "delete_contact": "ask" })
    );
}

#[tokio::test]
async fn says_there_is_nothing_to_set_up_when_no_tool_is_known() {
    let unreachable = TestGateway::new(|_| Reply::Error("connect ECONNREFUSED".into())).await;
    let admin = create_admin(&unreachable).await;
    let mcp = create_http_mcp(&unreachable, admin.id, "Sentry", "sentry").await;

    let html = tools_page(&unreachable, &mcp, &admin).await;
    let page = text_of(&html);
    assert!(page.contains("Sentry did not list its tools"));
    assert!(page.contains(
        "No tools to set up Fix the connection from the MCPs page, then come back. Back to MCPs"
    ));
    assert!(html.contains("<a class=\"button button--secondary\" href=\"/mcps\">Back to MCPs</a>"));
    // Without tools there is no search, no bulk button and no save bar.
    assert!(!html.contains("<form class=\"page__body\""));
    assert!(!html.contains("data-check-all"));

    let empty = TestGateway::new(listing(&[])).await;
    let admin = create_admin(&empty).await;
    let mcp = create_http_mcp(&empty, admin.id, "Sentry", "sentry").await;
    let html = tools_page(&empty, &mcp, &admin).await;
    assert!(text_of(&html).contains("No tools to set up This MCP has no tools. Back to MCPs"));
    assert!(!html.contains("did not list its tools"));
}

#[tokio::test]
async fn makes_every_tool_ask_while_the_saved_choices_cannot_be_read() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = create_mcp(&gateway, admin.id, |mcp| {
        mcp.name = "CRM".into();
        mcp.slug = "crm".into();
        mcp.http_url = Some("https://crm.example/mcp".into());
        mcp.tool_approvals = Some("{\"delete_contact\":\"sometimes\"".into());
    })
    .await;

    let html = tools_page(&gateway, &mcp, &admin).await;

    assert_eq!(
        tool_rows(&html),
        [
            ("list_contacts".to_string(), "ask", vec![]),
            ("delete_contact".to_string(), "ask", vec![]),
        ]
    );
    let warning = "The saved choices could not be read Every tool of this MCP asks for approval until you save this page again.";
    assert!(text_of(&html).contains(warning));
    assert!(text_of(&html).contains("2 of 2 tools ask for approval"));

    // Saving the page writes choices that can be read, and says so no more.
    let saved = gateway
        .post(&format!("{}?_method=PUT", tools_path(&mcp)))
        .login_as(&admin)
        .csrf()
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
        .form(&[
            ("toolCount", "2"),
            ("tools[0][name]", "list_contacts"),
            ("tools[0][mode]", "auto"),
            ("tools[1][name]", "delete_contact"),
            ("tools[1][mode]", "ask"),
        ])
        .send()
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert!(!text_of(&saved.text()).contains(warning));
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "delete_contact": "ask" })
    );
}

#[tokio::test]
async fn lists_the_tools_of_a_built_in_mcp_before_its_account_is_connected() {
    let gateway = TestGateway::offline().await;
    let admin = create_admin(&gateway).await;
    let mcp = create_strava_mcp(&gateway, admin.id, false, false).await;

    let html = tools_page(&gateway, &mcp, &admin).await;

    let rows = tool_rows(&html);
    assert_eq!(rows.len(), 21);
    let names: Vec<&str> = rows.iter().map(|(name, _, _)| name.as_str()).collect();
    assert!(names.contains(&"get_athlete"));
    assert!(names.contains(&"update_athlete_weight"));
    assert!(
        rows.iter()
            .all(|(_, mode, markers)| *mode == "auto" && markers.is_empty())
    );
    assert!(text_of(&html).contains("0 of 21 tools ask for approval"));
    // Nothing was asked of Strava.
    assert!(gateway.upstreams.requests().is_empty());
}

#[tokio::test]
async fn asks_by_default_before_the_tools_that_commit_money_and_saves_a_long_list() {
    let gateway = TestGateway::offline().await;
    let admin = create_admin(&gateway).await;
    let mcp = create_google_ads_mcp(&gateway, admin.id, true, true, &[]).await;

    // The dialog of the MCP and its row in the list lead to the page.
    let dialog = gateway
        .get(&format!("/mcps/{}/edit", mcp.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(dialog.contains(&format!("href=\"{}\"", tools_path(&mcp))));

    let html = tools_page(&gateway, &mcp, &admin).await;

    let rows = tool_rows(&html);
    assert_eq!(rows.len(), 28);
    assert!(text_of(&html).contains("4 of 28 tools ask for approval"));
    let asking: Vec<&str> = rows
        .iter()
        .filter(|(_, mode, _)| *mode == "ask")
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(
        asking,
        [
            "create_campaign",
            "update_campaign",
            "set_campaign_status",
            "update_campaign_budget",
        ]
    );
    for (name, mode, markers) in &rows {
        let by_default = *mode == "ask";
        assert_eq!(markers.contains(&"asks by default"), by_default, "{name}");
    }

    // The form of the page, as a browser sends it: every tool, more than the
    // body parser keeps as a list, with `add_keywords` now asking.
    let mut fields: Vec<(String, String)> = vec![("toolCount".into(), "28".into())];
    for (index, (name, mode, _)) in rows.iter().enumerate() {
        let mode = if name == "add_keywords" { "ask" } else { mode };
        fields.push((format!("tools[{index}][name]"), name.clone()));
        fields.push((format!("tools[{index}][mode]"), mode.to_string()));
    }
    let borrowed: Vec<(&str, &str)> = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let send = |fields: &[(&str, &str)]| {
        gateway
            .post(&format!("{}?_method=PUT", tools_path(&mcp)))
            .login_as(&admin)
            .csrf()
            .header("x-requested-with", "fetch")
            .header("accept", "text/html")
            .form(fields)
            .send()
    };
    let saved = send(&borrowed).await;

    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert!(text_of(&saved.text()).contains("5 of 28 tools ask for approval"));
    assert!(saved.text().contains("Tool approvals saved"));
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "add_keywords": "ask" })
    );

    // A form that did not arrive whole saves nothing: the tools it lost
    // would run without asking.
    let cut = send(&borrowed[..41]).await;
    assert_eq!(cut.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        cut.text().contains(
            "Only 20 of the 28 tools of this page reached the server, so nothing was saved"
        )
    );
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({ "add_keywords": "ask" })
    );

    // A default can be turned off, which is a choice to save.
    let relaxed: Vec<(&str, &str)> = borrowed
        .iter()
        .map(|(name, value)| {
            if *name == "tools[0][mode]" || value == &"ask" {
                (*name, "auto")
            } else {
                (*name, *value)
            }
        })
        .collect();
    send(&relaxed).await;
    assert_eq!(
        saved_choices(&find_mcp(&gateway, mcp.id).await),
        json!({
            "create_campaign": "auto",
            "update_campaign": "auto",
            "update_campaign_budget": "auto",
            "set_campaign_status": "auto",
        })
    );
}

#[tokio::test]
async fn cuts_a_long_description_and_shows_what_a_tool_says_of_itself_as_text() {
    let long = "é".repeat(700);
    let gateway = TestGateway::new(listing(&[
        ("long", Some(&long)),
        ("silent", None),
        ("blank", Some("")),
        ("<b>bold</b>", Some("<script>alert(1)</script>")),
    ]))
    .await;
    let admin = create_admin(&gateway).await;
    let mcp = create_http_mcp(&gateway, admin.id, "Notes", "notes").await;

    let html = tools_page(&gateway, &mcp, &admin).await;

    assert!(html.contains(&format!(
        "<span class=\"cell-sub clamp-3\">{}</span>",
        "é".repeat(600)
    )));
    assert!(!html.contains(&"é".repeat(601)));
    assert_eq!(html.matches("cell-sub clamp-3").count(), 2);
    assert!(html.contains("data-filter-text=\"silent\""));
    assert!(html.contains("<span class=\"cell-code\">&lt;b&gt;bold&lt;/b&gt;</span>"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;</span>"));
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(html.contains("name=\"tools[3][name]\" value=\"&lt;b&gt;bold&lt;/b&gt;\""));
}

#[tokio::test]
async fn answers_an_mcp_that_does_not_exist_with_the_list_of_mcps() {
    let gateway = crm_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = crm_mcp(&gateway, admin.id).await;

    // An id that is not a number is answered like a missing record.
    for id in ["abc", "1.5", "-3", "0", "999999"] {
        let page = gateway
            .get(&format!("/mcps/{id}/tools"))
            .login_as(&admin)
            .send()
            .await;
        assert_eq!(page.status, StatusCode::FOUND, "{id}");
        assert_eq!(page.location(), Some("/mcps"), "{id}");
        assert_eq!(flash(&page, "error").as_deref(), Some("MCP not found"));

        let saved = gateway
            .put(&format!("/mcps/{id}/tools"))
            .login_as(&admin)
            .csrf()
            .json(json!({ "tools": [] }))
            .send()
            .await;
        assert_eq!(saved.status, StatusCode::FOUND, "{id}");
        assert_eq!(saved.location(), Some("/mcps"), "{id}");
        assert_eq!(flash(&saved, "error").as_deref(), Some("MCP not found"));
    }

    // The page is for people who are signed in, whatever their role.
    let guest = gateway.get(&tools_path(&mcp)).send().await;
    assert_eq!(guest.location(), Some("/login"));
    let member = create_member(&gateway).await;
    assert!(
        tools_page(&gateway, &mcp, &member)
            .await
            .contains("delete_contact")
    );
}
