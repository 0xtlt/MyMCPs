//! The built-in MCPs behind `/mcp`: the gateway cases of
//! `tests/functional/builtin_strava_mcp.spec.ts`,
//! `builtin_icloud_mail_mcp.spec.ts`, `builtin_icloud_mail_uploads.spec.ts`
//! and `builtin_google_ads_mcp.spec.ts`, the built-in cases of
//! `tool_approvals.spec.ts`, and the first case of
//! `tests/browser/tool_approvals.spec.ts`, with the providers the server
//! registers.
//!
//! What a tool does with its account is tested in the crate of its
//! provider. Here an agent reaches it through the gateway, with an access
//! token, and a person through the pages and the links.

#[path = "support/gateway.rs"]
mod support;

use std::sync::{Arc, Mutex};

use http::StatusCode;
use mymcps_core::models::{
    ApprovalState, CallErrorCategory, CallOutcome, Mcp, McpTransport, ScopeMode,
};
use mymcps_google_ads::testing::{
    FakeGoogleAds, GOOGLE_ADS_CUSTOMER, GoogleAdsFault, fixtures, google_ads_failure, google_json,
};
use mymcps_icloud_mail::testing::{ATTACHMENT, FakeIcloud, FakeOptions, PASSWORD, USERNAME};
use mymcps_net::{CannedResponse, Fetcher};
use mymcps_web::testing::factories::{create_admin, create_admin_with, create_mcp};
use serde_json::{Value, json};

use support::*;

const NEEDS_APP_URL: &str = "File links need the public address of this MyMCPs instance. An administrator must set APP_URL.";

fn tool_names(listed: &Value) -> Vec<&str> {
    listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

/// The token of an agent that may use every MCP.
async fn agent_token(gateway: &TestGateway, user_id: i64) -> String {
    create_access_token(gateway, user_id, ScopeMode::All, &[])
        .await
        .plaintext
}

async fn list_tools(gateway: &TestGateway, plaintext: &str) -> Value {
    gateway
        .rpc(plaintext, "tools/list", json!({}), "eager")
        .await
}

/// What the call log keeps of the calls to one MCP.
async fn logged(
    gateway: &TestGateway,
    mcp: &Mcp,
) -> Vec<(String, CallOutcome, Option<CallErrorCategory>)> {
    gateway
        .call_logs()
        .await
        .into_iter()
        .filter(|log| log.mcp_id == Some(mcp.id))
        .map(|log| {
            (
                log.tool_name.unwrap_or_default(),
                log.outcome,
                log.error_category,
            )
        })
        .collect()
}

// ------------------------------------------------------------------ Strava

/// The paths asked of the Strava API.
type StravaRequests = Arc<Mutex<Vec<String>>>;

/// The app in front of a Strava with one athlete and one activity.
async fn strava_gateway() -> (TestGateway, StravaRequests) {
    let requests = StravaRequests::default();
    let layer = {
        let requests = requests.clone();
        move |fetcher: Fetcher| {
            fetcher.answering(move |request| {
                if request.url.host_str() != Some("www.strava.com") {
                    return None;
                }
                let path = request.url.path().to_owned();
                requests.lock().unwrap().push(path.clone());
                Some(match path.as_str() {
                    "/api/v3/athlete" => CannedResponse::json(
                        StatusCode::OK,
                        &json!({
                            "id": 4242,
                            "resource_state": 3,
                            "firstname": "Test",
                            "lastname": "Athlete",
                            "weight": 70.5,
                            "profile": "https://images.example/large.jpg",
                        }),
                    ),
                    "/api/v3/athlete/activities" => CannedResponse::json(
                        StatusCode::OK,
                        &json!([{
                            "id": 15000000001_i64,
                            "name": "Morning Run",
                            "sport_type": "Run",
                            "start_date": "2026-09-30T05:30:00Z",
                            "distance": 10012.4,
                            "moving_time": 2890,
                        }]),
                    ),
                    _ => CannedResponse::json(
                        StatusCode::NOT_FOUND,
                        &json!({
                            "message": "Record Not Found",
                            "errors": [{ "resource": "Activity", "field": "", "code": "not found" }],
                        }),
                    ),
                })
            })
        }
    };
    let gateway = TestGateway::with(
        |_| {},
        layer,
        |_| Reply::Error("Strava is not reached as an MCP server".into()),
    )
    .await;
    (gateway, requests)
}

#[tokio::test]
async fn strava_exposes_namespaced_tools_and_runs_them_through_the_gateway() {
    let (gateway, requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    let mcp = create_strava_mcp(&gateway, admin.id, false, true).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let listed = list_tools(&gateway, &plaintext).await;
    let names = tool_names(&listed);
    assert!(names.contains(&"strava__list_activities"));
    assert!(names.contains(&"strava__get_athlete"));
    assert_eq!(names.len(), 17);
    assert!(requests.lock().unwrap().is_empty());

    let called = gateway
        .call(
            &plaintext,
            "strava__list_activities",
            json!({ "per_page": 1 }),
        )
        .await;
    assert!(called.get("isError").is_none(), "{called}");
    assert_eq!(result_json(&called)[0]["name"], "Morning Run");

    let failed = gateway
        .call(
            &plaintext,
            "strava__get_activity",
            json!({ "activity_id": 404 }),
        )
        .await;
    assert_eq!(failed["isError"], true);
    assert!(result_text(&failed).contains("Strava could not find this resource"));

    assert_eq!(
        logged(&gateway, &mcp).await,
        [
            ("list_activities".to_string(), CallOutcome::Success, None),
            (
                "get_activity".to_string(),
                CallOutcome::Error,
                Some(CallErrorCategory::ToolError)
            ),
        ]
    );
}

#[tokio::test]
async fn strava_finds_and_calls_tools_in_lazy_mode() {
    let (gateway, _requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    create_strava_mcp(&gateway, admin.id, false, true).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let search = gateway
        .rpc(
            &plaintext,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "mcp": "strava", "query": "heart rate zones" } }),
            "lazy",
        )
        .await;
    let found = result_json(&search);
    assert!(
        found["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "get_athlete_zones")
    );

    let called = gateway
        .call_lazily(&plaintext, "strava", "get_athlete", json!({}))
        .await;
    let athlete = result_json(&called);
    assert_eq!(athlete["firstname"], "Test");
    assert!(athlete.get("profile").is_none());
}

#[tokio::test]
async fn strava_skips_an_mcp_that_is_not_connected_yet() {
    let (gateway, requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    create_strava_mcp(&gateway, admin.id, false, false).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let listed = list_tools(&gateway, &plaintext).await;
    assert_eq!(listed["tools"], json!([]));

    let called = gateway
        .call(&plaintext, "strava__get_athlete", json!({}))
        .await;
    assert_eq!(called["isError"], true);
    assert!(result_text(&called).contains("Strava is not connected"));
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn strava_lists_the_write_tools_only_while_write_access_is_allowed() {
    let (gateway, requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    let mut mcp = create_strava_mcp(&gateway, admin.id, true, true).await;
    let plaintext = agent_token(&gateway, admin.id).await;
    let names = async || {
        let listed = list_tools(&gateway, &plaintext).await;
        tool_names(&listed)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let writable = names().await;
    assert_eq!(writable.len(), 21);
    assert!(writable.contains(&"strava__update_athlete_weight".to_string()));

    // Hidden as soon as write access is turned off, whatever Strava granted.
    mcp.builtin_write_enabled = false;
    mcp.save(&**gateway.db()).await.unwrap();
    let read_only = names().await;
    assert_eq!(read_only.len(), 17);
    assert!(!read_only.contains(&"strava__update_athlete_weight".to_string()));
    let refused = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 71.5 }),
        )
        .await;
    assert_eq!(refused["isError"], true);
    assert!(result_text(&refused).contains("write access is turned off for this MCP"));
    assert!(requests.lock().unwrap().is_empty());
}

// Tool approvals: built-in MCPs

#[tokio::test]
async fn strava_checks_the_call_before_asking_anyone_to_approve_it() {
    let (gateway, requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    let mut mcp = create_strava_mcp(&gateway, admin.id, true, true).await;
    mcp.tool_approvals = Some(json!({ "update_athlete_weight": "ask" }).to_string());
    mcp.save(&**gateway.db()).await.unwrap();
    let plaintext = agent_token(&gateway, admin.id).await;

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

    let held = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 71.5 }),
        )
        .await;
    assert!(result_text(&held).contains("Approval required: update_athlete_weight on Strava"));
    assert!(requests.lock().unwrap().is_empty());

    // The page lists the argument beside what MyMCPs says of its own tool.
    let request = gateway.last_request(&mcp).await;
    let html = gateway
        .get(&format!("/approvals/{}", request.public_id))
        .login_as(&admin)
        .send()
        .await
        .text();
    let page = text_of(&html);
    assert!(page.contains("Run the tool \"update_athlete_weight\" of Strava"));
    assert!(html.contains("<dt>weight</dt><dd class=\"preserve-lines\">71.5</dd>"));
    assert!(page.contains("Strava describes the tool as: Set the connected athlete's weight"));
}

#[tokio::test]
async fn strava_does_not_ask_for_a_tool_the_mcp_would_refuse_anyway() {
    let (gateway, _requests) = strava_gateway().await;
    let admin = create_admin(&gateway).await;
    let mut mcp = create_strava_mcp(&gateway, admin.id, false, true).await;
    mcp.tool_approvals = Some(json!({ "update_athlete_weight": "ask" }).to_string());
    mcp.save(&**gateway.db()).await.unwrap();
    let plaintext = agent_token(&gateway, admin.id).await;

    let response = gateway
        .call(
            &plaintext,
            "strava__update_athlete_weight",
            json!({ "weight": 71.5 }),
        )
        .await;

    assert!(result_text(&response).contains("write access is turned off for this MCP"));
    assert!(gateway.approval_requests().await.is_empty());
}

// -------------------------------------------------------------- iCloud Mail

const SIGN_IN_REJECTED: &str = "iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.";

/// The app in front of a mail account served on 127.0.0.1.
async fn icloud_gateway(
    options: FakeOptions,
    adjust: impl FnOnce(&mut mymcps_core::Config),
) -> (TestGateway, FakeIcloud) {
    let icloud = FakeIcloud::start_with(options).await;
    let servers = icloud.servers();
    let gateway = TestGateway::with_builtin_env(adjust, |env| env.with_extension(servers)).await;
    (gateway, icloud)
}

async fn create_icloud_mail_mcp(gateway: &TestGateway, created_by: i64, permissions: &str) -> Mcp {
    let password = gateway.core.encrypt_secret(Some(PASSWORD));
    create_mcp(gateway, created_by, |mcp| {
        mcp.name = "iCloud Mail".into();
        mcp.slug = "icloud-mail".into();
        mcp.transport = McpTransport::Builtin;
        mcp.http_url = None;
        mcp.builtin_key = Some("icloud-mail".into());
        mcp.builtin_username = Some(USERNAME.into());
        mcp.builtin_password = password;
        mcp.builtin_permissions = Some(permissions.into());
    })
    .await
}

/// A mail MCP with these permissions, and the token of an agent that may use it.
async fn mail_agent(gateway: &TestGateway, permissions: &str) -> (Mcp, String) {
    let admin = create_admin(gateway).await;
    let mcp = create_icloud_mail_mcp(gateway, admin.id, permissions).await;
    let plaintext = agent_token(gateway, admin.id).await;
    (mcp, plaintext)
}

async fn call_mail(gateway: &TestGateway, plaintext: &str, tool: &str, arguments: Value) -> Value {
    gateway
        .call(plaintext, &format!("icloud-mail__{tool}"), arguments)
        .await
}

/// Whether a call was refused, and what the agent was told.
fn answer_of(result: &Value) -> (bool, &str) {
    (result["isError"] == true, result_text(result))
}

#[tokio::test]
async fn icloud_exposes_the_allowed_tools_through_the_gateway_and_refuses_the_others() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (mcp, plaintext) = mail_agent(&gateway, "read draft").await;

    let listed = list_tools(&gateway, &plaintext).await;
    assert_eq!(
        tool_names(&listed),
        [
            "icloud-mail__list_mailboxes",
            "icloud-mail__list_messages",
            "icloud-mail__get_message",
            "icloud-mail__get_attachment_link",
            "icloud-mail__create_upload_link",
            "icloud-mail__create_draft",
        ]
    );
    assert!(icloud.sign_ins().is_empty());

    let called = call_mail(
        &gateway,
        &plaintext,
        "list_messages",
        json!({ "unread": true }),
    )
    .await;
    assert!(called.get("isError").is_none(), "{called}");
    assert_eq!(
        result_json(&called)["messages"][0]["subject"],
        "Lunch on Thursday?"
    );

    let refused = call_mail(
        &gateway,
        &plaintext,
        "send_message",
        json!({ "to": ["dave@example.com"], "subject": "Hello", "text": "Hi" }),
    )
    .await;
    assert_eq!(refused["isError"], true);
    assert!(result_text(&refused).contains("send_message needs the \"send\" permission"));
    assert!(icloud.sent().is_empty());

    assert_eq!(
        logged(&gateway, &mcp).await,
        [
            ("list_messages".to_string(), CallOutcome::Success, None),
            (
                "send_message".to_string(),
                CallOutcome::Error,
                Some(CallErrorCategory::ToolError)
            ),
        ]
    );
}

#[tokio::test]
async fn icloud_reports_a_revoked_password_to_the_agent_without_leaking_it() {
    let options = FakeOptions {
        reject_sign_in: true,
        ..Default::default()
    };
    let (gateway, _icloud) = icloud_gateway(options, |_| {}).await;
    let (_, plaintext) = mail_agent(&gateway, "read").await;

    let called = call_mail(&gateway, &plaintext, "list_mailboxes", json!({})).await;

    assert_eq!(answer_of(&called), (true, SIGN_IN_REJECTED));
    assert!(!called.to_string().contains(PASSWORD));
}

#[tokio::test]
async fn icloud_hands_out_a_temporary_link_that_downloads_the_attachment_without_signing_in() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (mcp, plaintext) = mail_agent(&gateway, "read").await;

    let called = call_mail(
        &gateway,
        &plaintext,
        "get_attachment_link",
        json!({ "uid": 11, "part": 2 }),
    )
    .await;

    assert!(called.get("isError").is_none(), "{called}");
    let link = result_json(&called);
    assert_eq!(link["filename"], "Menu été.pdf");
    assert_eq!(link["content_type"], "application/pdf");
    assert_eq!(link["size"], 57000);
    let expires_at = chrono::DateTime::parse_from_rfc3339(link["expires_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    let expires_in = expires_at - chrono::Utc::now();
    assert!(expires_in > chrono::Duration::minutes(14));
    assert!(expires_in <= chrono::Duration::minutes(15));

    let url = url::Url::parse(link["url"].as_str().unwrap()).unwrap();
    assert_eq!(url.origin().ascii_serialization(), "http://localhost:3333");
    assert!(url.path().starts_with(&format!("/files/{}/", mcp.id)));
    assert_eq!(
        url.query_pairs()
            .map(|(name, _)| name.into_owned())
            .collect::<Vec<_>>(),
        ["signature"]
    );
    // The link tells an onlooker nothing about the mailbox.
    assert!(!url.as_str().contains("INBOX"));
    assert!(!url.as_str().contains(USERNAME));

    // No cookie, no access token: the signature is the credential.
    let download = follow(&gateway, url.as_str()).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.text(), ATTACHMENT);
    assert_eq!(download.header("content-type"), Some("application/pdf"));
    assert_eq!(
        download.header("content-disposition"),
        Some("attachment; filename=\"Menu _t_.pdf\"; filename*=UTF-8''Menu%20%C3%A9t%C3%A9.pdf")
    );
    assert_eq!(download.header("cache-control"), Some("private, no-store"));

    let downloads = icloud.downloads();
    let last = downloads.last().unwrap();
    assert_eq!((last.uid, last.part.as_str()), (11, "2"));
    let selections = icloud.selections();
    let selected = selections.last().unwrap();
    assert_eq!(
        (selected.path.as_str(), selected.read_only),
        ("INBOX", true)
    );
}

#[tokio::test]
async fn icloud_only_links_to_attachments_and_only_within_the_allowed_time() {
    let (gateway, _icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (_, plaintext) = mail_agent(&gateway, "read").await;
    let call = async |arguments: Value| {
        let called = call_mail(&gateway, &plaintext, "get_attachment_link", arguments).await;
        (called["isError"] == true, result_text(&called).to_string())
    };

    // The message text is not a file to hand out.
    assert_eq!(
        call(json!({ "uid": 11, "part": "1.1" })).await,
        (
            true,
            "Message 11 has no attachment at part \"1.1\". Its attachments are at parts: 2."
                .to_string()
        )
    );
    assert_eq!(
        call(json!({ "uid": 12, "part": "1" })).await,
        (true, "Message 12 has no attachments.".to_string())
    );
    assert_eq!(
        call(json!({ "uid": 11, "part": "../2" })).await,
        (
            true,
            "part must be the part of an attachment, such as 2, as returned by get_message"
                .to_string()
        )
    );
    assert_eq!(
        call(json!({ "uid": 11, "part": "2", "expires_in_minutes": 600 })).await,
        (
            true,
            "expires_in_minutes must be an integer between 1 and 60".to_string()
        )
    );
}

#[tokio::test]
async fn icloud_needs_the_read_permission_and_the_public_address_of_the_instance() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (_, plaintext) = mail_agent(&gateway, "send").await;
    let arguments = json!({ "uid": 11, "part": "2" });

    let send_only = call_mail(
        &gateway,
        &plaintext,
        "get_attachment_link",
        arguments.clone(),
    )
    .await;
    assert_eq!(send_only["isError"], true);
    assert!(result_text(&send_only).contains("get_attachment_link needs the \"read\" permission"));

    let (unreachable, elsewhere) =
        icloud_gateway(FakeOptions::default(), |config| config.app_url = None).await;
    let (_, plaintext) = mail_agent(&unreachable, "read").await;
    let called = call_mail(&unreachable, &plaintext, "get_attachment_link", arguments).await;
    assert_eq!(answer_of(&called), (true, NEEDS_APP_URL));
    assert!(icloud.sign_ins().is_empty());
    assert!(elsewhere.sign_ins().is_empty());
}

const PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

async fn send_file(
    gateway: &TestGateway,
    link: &str,
    body: &'static [u8],
) -> mymcps_web::testing::TestResponse {
    gateway
        .put(path_of(link))
        .api()
        .raw_body(body, "application/octet-stream")
        .send()
        .await
}

#[tokio::test]
async fn icloud_hands_out_a_link_that_takes_one_file_and_attaches_it_to_a_message() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (mcp, plaintext) = mail_agent(&gateway, "send").await;

    let link = call_mail(
        &gateway,
        &plaintext,
        "create_upload_link",
        json!({ "filename": "Devis été.pdf", "content_type": "application/pdf" }),
    )
    .await;
    assert!(link.get("isError").is_none(), "{link}");
    let link = result_json(&link);
    assert_eq!(link["method"], "PUT");
    assert_eq!(link["filename"], "Devis été.pdf");
    assert_eq!(link["content_type"], "application/pdf");
    assert_eq!(link["max_bytes"], 20_000_000);
    let upload_id = link["upload_id"].as_str().unwrap();
    assert_eq!(upload_id.len(), 36);
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("http://localhost:3333/uploads/{}/", mcp.id)));
    assert!(url.contains("?signature="));
    // Asking for a link does not reach iCloud.
    assert!(icloud.sign_ins().is_empty());

    let uploaded = send_file(&gateway, url, PDF).await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{}", uploaded.text());
    let receipt = uploaded.json();
    assert_eq!(receipt["upload_id"], upload_id);
    assert_eq!(receipt["filename"], "Devis été.pdf");
    assert_eq!(receipt["size"], PDF.len());

    // The link is used up, whoever else got hold of it.
    let again = send_file(&gateway, url, b"something else").await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(
        again.text(),
        "A file was already sent to this link. Ask for a new link to send another one."
    );

    let sent = call_mail(
        &gateway,
        &plaintext,
        "send_message",
        json!({
            "to": "dave@example.com",
            "subject": "Quote",
            "text": "Here it is.",
            "attachments": [upload_id],
        }),
    )
    .await;
    assert!(sent.get("isError").is_none(), "{sent}");
    assert_eq!(
        result_json(&sent)["attachments"],
        json!([{ "filename": "Devis été.pdf", "size": PDF.len() }])
    );
    let deliveries = icloud.sent();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].to, ["dave@example.com"]);
    let encoded = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(PDF)
    };
    let unfolded = deliveries[0].raw.replace("\r\n", "");
    assert!(unfolded.contains(&encoded));
    assert!(deliveries[0].raw.contains("Content-Type: application/pdf"));
    let copy = &icloud.mailbox("Sent Messages").appended[0];
    assert!(copy.raw.replace("\r\n", "").contains(&encoded));
}

#[tokio::test]
async fn icloud_refuses_to_attach_a_file_before_it_was_sent_to_its_link() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (_, plaintext) = mail_agent(&gateway, "draft").await;

    let link = call_mail(
        &gateway,
        &plaintext,
        "create_upload_link",
        json!({ "filename": "notes.txt", "expires_in_minutes": 60 }),
    )
    .await;
    let link = result_json(&link);
    assert!(link.get("content_type").is_none());
    let upload_id = link["upload_id"].as_str().unwrap();

    let draft = json!({ "subject": "Notes", "text": "Attached.", "attachments": upload_id });
    let early = call_mail(&gateway, &plaintext, "create_draft", draft.clone()).await;
    assert_eq!(early["isError"], true);
    assert!(result_text(&early).contains(&format!("No file is uploaded as \"{upload_id}\".")));
    assert!(icloud.sign_ins().is_empty());

    let uploaded = send_file(
        &gateway,
        link["url"].as_str().unwrap(),
        b"Call back on Monday.\n",
    )
    .await;
    assert_eq!(uploaded.status, StatusCode::CREATED);
    let saved = call_mail(&gateway, &plaintext, "create_draft", draft).await;
    assert!(saved.get("isError").is_none(), "{saved}");
    assert_eq!(
        result_json(&saved)["attachments"],
        json!([{ "filename": "notes.txt", "size": 21 }])
    );
    let stored = &icloud.mailbox("Drafts").appended[0];
    assert!(
        stored
            .raw
            .contains("Content-Disposition: attachment; filename=notes.txt\r\n")
    );
}

#[tokio::test]
async fn icloud_needs_a_permission_that_writes_messages_and_the_public_address_of_the_instance() {
    let (gateway, icloud) = icloud_gateway(FakeOptions::default(), |_| {}).await;
    let (_, plaintext) = mail_agent(&gateway, "read organize").await;
    let arguments = json!({ "filename": "a.pdf" });

    let read_only = call_mail(
        &gateway,
        &plaintext,
        "create_upload_link",
        arguments.clone(),
    )
    .await;
    assert_eq!(
        answer_of(&read_only),
        (
            true,
            "create_upload_link needs the \"draft\" or \"send\" permission, which is not allowed for this iCloud Mail MCP. An administrator can allow it from the MCPs page in MyMCPs."
        )
    );

    let (unreachable, _elsewhere) =
        icloud_gateway(FakeOptions::default(), |config| config.app_url = None).await;
    let (_, plaintext) = mail_agent(&unreachable, "draft").await;
    let called = call_mail(&unreachable, &plaintext, "create_upload_link", arguments).await;
    assert_eq!(answer_of(&called), (true, NEEDS_APP_URL));
    assert!(icloud.sign_ins().is_empty());
}

// --------------------------------------------------------------- Google Ads

/// The app in front of a fake Google: its token endpoint and the Google Ads API.
async fn google_gateway(google: &FakeGoogleAds) -> TestGateway {
    let fetcher = google.fetcher();
    TestGateway::with(
        |_| {},
        |_| fetcher,
        |_| Reply::Error("Google Ads is not reached as an MCP server".into()),
    )
    .await
}

const MONEY_TOOLS: [&str; 4] = [
    "create_campaign",
    "update_campaign",
    "set_campaign_status",
    "update_campaign_budget",
];

/// A connected MCP that may write, with every tool running on its own
/// unless `asks` says otherwise.
async fn writable_mcp(gateway: &TestGateway, admin_id: i64, asks: &[&str]) -> Mcp {
    let mut mcp = create_google_ads_mcp(gateway, admin_id, true, true, &[]).await;
    let choices: serde_json::Map<String, Value> = MONEY_TOOLS
        .iter()
        .filter(|tool| !asks.contains(tool))
        .map(|tool| ((*tool).to_string(), json!("auto")))
        .collect();
    mcp.tool_approvals = Some(Value::Object(choices).to_string());
    mcp.save(&**gateway.db()).await.unwrap();
    mcp
}

async fn call_ads(gateway: &TestGateway, plaintext: &str, tool: &str, arguments: Value) -> Value {
    gateway
        .call(plaintext, &format!("google-ads__{tool}"), arguments)
        .await
}

fn budget_change() -> Value {
    json!([{
        "campaignBudgetOperation": {
            "update": {
                "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaignBudgets/222"),
                "amountMicros": "250000000",
            },
            "updateMask": "amount_micros",
        },
    }])
}

#[tokio::test]
async fn google_ads_lists_no_write_tool_until_write_access_is_allowed() {
    let google = FakeGoogleAds::new();
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    let mut mcp = create_google_ads_mcp(&gateway, admin.id, true, false, &[]).await;
    let plaintext = agent_token(&gateway, admin.id).await;
    let names = async || {
        let listed = list_tools(&gateway, &plaintext).await;
        tool_names(&listed)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let read_only = names().await;
    assert_eq!(read_only.len(), 13);
    assert!(!read_only.contains(&"google-ads__update_campaign_budget".to_string()));

    mcp.builtin_write_enabled = true;
    mcp.save(&**gateway.db()).await.unwrap();
    assert_eq!(names().await.len(), 28);
    // Listing asks nothing of Google.
    assert!(google.requests().is_empty());
}

#[tokio::test]
async fn google_ads_tells_agents_which_tools_ask_until_the_admin_decides_otherwise() {
    let google = FakeGoogleAds::new();
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    let mut mcp = create_google_ads_mcp(&gateway, admin.id, true, true, &[]).await;
    let plaintext = agent_token(&gateway, admin.id).await;
    let asking = async || {
        let listed = list_tools(&gateway, &plaintext).await;
        listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|tool| {
                tool["description"]
                    .as_str()
                    .unwrap()
                    .contains("Needs approval:")
            })
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    let mut by_default = asking().await;
    by_default.sort();
    assert_eq!(
        by_default,
        [
            "google-ads__create_campaign",
            "google-ads__set_campaign_status",
            "google-ads__update_campaign",
            "google-ads__update_campaign_budget",
        ]
    );

    mcp.tool_approvals =
        Some(json!({ "update_campaign": "auto", "add_keywords": "ask" }).to_string());
    mcp.save(&**gateway.db()).await.unwrap();
    let chosen = asking().await;
    assert!(!chosen.contains(&"google-ads__update_campaign".to_string()));
    assert!(chosen.contains(&"google-ads__add_keywords".to_string()));
    assert_eq!(chosen.len(), 4);
}

/// `tests/browser/tool_approvals.spec.ts`, "takes a person from the link an
/// agent was given to an approved call", with
/// "describes a budget change from what Google says, not from what the
/// agent says".
#[tokio::test]
async fn google_ads_takes_a_person_from_the_link_an_agent_was_given_to_an_approved_call() {
    let google = FakeGoogleAds::new();
    let gateway = google_gateway(&google).await;
    let admin = create_admin_with(&gateway, "owner@example.com").await;
    let mcp = create_google_ads_mcp(&gateway, admin.id, true, true, &[]).await;
    let token = create_named_access_token(&gateway, admin.id, "Claude", ScopeMode::All, &[]).await;
    let plaintext = token.plaintext;
    // The agent meant 2.50 and wrote 250.
    let call = json!({ "customer_id": "123-456-7890", "campaign_id": "111", "daily_budget": 250 });

    let held = call_ads(&gateway, &plaintext, "update_campaign_budget", call.clone()).await;
    assert_eq!(held["isError"], true);
    let text = result_text(&held);
    assert!(text.contains("Approval required: update_campaign_budget on Google Ads was not run."));
    let request = gateway.last_request(&mcp).await;
    let link = format!("http://localhost:3333/approvals/{}", request.public_id);
    assert!(text.contains(&link));
    // Google checked the change without making it.
    assert_eq!(google.validations(), [budget_change()]);
    assert!(google.mutations().is_empty());

    // The link grants nothing: whoever follows it signs in first, then lands on the request.
    let guest = follow_page(&gateway, &link, None).await;
    assert_eq!(guest.location(), Some("/login"));
    let signed_in = gateway
        .post("/login")
        .csrf()
        .session(guest.session())
        .form(&[("email", "owner@example.com"), ("password", "password123")])
        .send()
        .await;
    assert_eq!(signed_in.location(), Some(path_of(&link)));

    let response = follow_page(&gateway, &link, Some(&admin)).await;
    assert_eq!(response.status, StatusCode::OK);
    let html = response.text();
    let page = text_of(&html);
    assert!(html.contains(
        "<h1 class=\"page-header__title break-anywhere\">Change the daily budget of the campaign &quot;Spring sale&quot; from €2.50 to €250.00</h1>"
    ));
    assert!(
        page.contains("An agent using the access token “Claude” wants to do this on Google Ads.")
    );
    // What Google says of the campaign, not what the agent says.
    assert!(html.contains("<h2 class=\"heading__title\">What it changes</h2>"));
    assert!(page.contains("Account Acme Shoes (123-456-7890)"));
    assert!(page.contains("Campaign Spring sale (enabled)"));
    assert!(html.contains(
        "<dt>Daily budget</dt><dd class=\"preserve-lines\">€250.00 a day</dd><dd class=\"key-value__before preserve-lines\">Now: €2.50 a day</dd>"
    ));
    assert!(page.contains("Most it can cost in a month €7,600.00 Now: €76.00"));
    for warning in [
        "The new budget is 100 times the current one.",
        "The campaign is live: the new budget applies at once.",
    ] {
        assert!(html.contains(&format!("<p class=\"banner__title\">{warning}</p>")));
    }
    // MyMCPs knows this tool: nothing says it only lists arguments.
    assert!(!page.contains("MyMCPs does not know what this tool does"));
    assert!(html.contains("&quot;daily_budget&quot;: 250"));

    let approved = gateway
        .post(path_of(&link))
        .login_as(&admin)
        .csrf()
        .form(&[("decision", "approve")])
        .send()
        .await;
    assert_eq!(approved.location(), Some(path_of(&link)));
    let page = follow_page(&gateway, &link, Some(&admin)).await.text();
    assert!(text_of(&page).contains("Approved by Test User on "));
    assert!(!page.contains(">Approve</button>"));
    // The warnings stay on a decided request.
    assert!(page.contains("The new budget is 100 times the current one."));
    assert!(google.mutations().is_empty());

    let ran = call_ads(&gateway, &plaintext, "update_campaign_budget", call).await;
    assert!(ran.get("isError").is_none(), "{ran}");
    assert_eq!(
        result_json(&ran),
        json!({
            "campaign_id": "111",
            "name": "Spring sale",
            "daily_budget": 250,
            "previous_daily_budget": 2.5,
            "currency": "EUR",
        })
    );
    assert_eq!(google.mutations(), [budget_change()]);

    let list = gateway
        .get("/approvals")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(list.contains("<h2 class=\"card__title\">Decided or expired</h2>"));
    assert!(list.contains("<span class=\"badge badge--success\">Approved and run</span>"));
    // The row carries the icon of its MCP's template.
    assert!(text_of(&list).contains("update_campaign_budget · Google Ads · token Claude"));
    assert_eq!(
        find_request(&gateway, request.id).await.state(),
        ApprovalState::Used
    );
}

async fn follow_page(
    gateway: &TestGateway,
    link: &str,
    user: Option<&mymcps_core::models::User>,
) -> mymcps_web::testing::TestResponse {
    let request = gateway.get(path_of(link));
    match user {
        Some(user) => request.login_as(user).send().await,
        None => request.send().await,
    }
}

#[tokio::test]
async fn google_ads_does_not_ask_anyone_to_approve_what_google_would_refuse() {
    let google = FakeGoogleAds::responding(|request| {
        let validates = request
            .json
            .as_ref()
            .is_some_and(|json| json["validateOnly"] == true);
        (request.url.path().ends_with("/googleAds:mutate") && validates).then(|| {
            google_ads_failure(
                &[GoogleAdsFault {
                    error_code: ("campaignBudgetError", "MONEY_AMOUNT_TOO_LARGE"),
                    message: "The amount is too large.",
                    fields: Some(vec![
                        "mutate_operations",
                        "campaign_budget_operation",
                        "update",
                        "amount_micros",
                    ]),
                }],
                400,
            )
        })
    });
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    writable_mcp(&gateway, admin.id, &["update_campaign_budget"]).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let refused = call_ads(
        &gateway,
        &plaintext,
        "update_campaign_budget",
        json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": "111", "daily_budget": 900000 }),
    )
    .await;

    assert_eq!(
        answer_of(&refused),
        (
            true,
            "Google Ads refused the request: The amount is too large. (MONEY_AMOUNT_TOO_LARGE) at mutate_operations.campaign_budget_operation.update.amount_micros"
        )
    );
    assert!(gateway.approval_requests().await.is_empty());
}

#[tokio::test]
async fn google_ads_describes_enabling_a_campaign_by_the_budget_it_frees() {
    let google = FakeGoogleAds::responding(|request| {
        request.query().contains(" FROM campaign ").then(|| {
            let mut paused = fixtures::campaign();
            paused["campaign"]["status"] = json!("PAUSED");
            google_json(&json!({ "results": [paused] }), 200)
        })
    });
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    let mcp = writable_mcp(&gateway, admin.id, &["set_campaign_status"]).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    call_ads(
        &gateway,
        &plaintext,
        "set_campaign_status",
        json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": 111, "status": "ENABLED" }),
    )
    .await;

    let request = gateway.last_request(&mcp).await;
    let html = follow_page(
        &gateway,
        &format!("http://localhost:3333/approvals/{}", request.public_id),
        Some(&admin),
    )
    .await
    .text();
    assert!(
        html.contains(
            "Enable the campaign &quot;Spring sale&quot;, which can spend €2.50 a day</h1>"
        )
    );
    assert!(html.contains(
        "<dt>Status</dt><dd class=\"preserve-lines\">Enabled</dd><dd class=\"key-value__before preserve-lines\">Now: Paused</dd>"
    ));
}

#[tokio::test]
async fn google_ads_runs_a_tool_the_admin_set_to_run_without_asking() {
    let google = FakeGoogleAds::new();
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    writable_mcp(&gateway, admin.id, &[]).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let ran = call_ads(
        &gateway,
        &plaintext,
        "update_campaign_budget",
        json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": "111", "daily_budget": 250 }),
    )
    .await;

    assert!(ran.get("isError").is_none(), "{ran}");
    assert_eq!(result_json(&ran)["daily_budget"], 250);
    assert_eq!(google.mutations(), [budget_change()]);
    assert!(gateway.approval_requests().await.is_empty());
}

/// The start of a PNG file: its signature and the header that carries its size.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0; 33];
    bytes[..8].copy_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    bytes[8..12].copy_from_slice(&13_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

#[tokio::test]
async fn google_ads_turns_an_uploaded_image_into_an_asset_and_says_what_it_can_be_used_as() {
    let google = FakeGoogleAds::new();
    let gateway = google_gateway(&google).await;
    let admin = create_admin(&gateway).await;
    let mcp = writable_mcp(&gateway, admin.id, &[]).await;
    let plaintext = agent_token(&gateway, admin.id).await;

    let link = call_ads(
        &gateway,
        &plaintext,
        "create_image_upload_link",
        json!({ "filename": "banner.png" }),
    )
    .await;
    let link = result_json(&link);
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("http://localhost:3333/uploads/{}/", mcp.id)));
    assert_eq!(link["max_bytes"], 5_242_880);
    let upload_id = link["upload_id"].as_str().unwrap();

    // Nothing was sent: the link is the one a file goes to.
    let empty = gateway.put(path_of(url)).api().send().await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let image = png(1200, 628);
    let uploaded = gateway
        .put(path_of(url))
        .api()
        .raw_body(image.clone(), "image/png")
        .send()
        .await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{}", uploaded.text());
    let asset = call_ads(
        &gateway,
        &plaintext,
        "create_image_asset",
        json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "upload_id": upload_id, "name": "Spring banner" }),
    )
    .await;

    assert_eq!(
        result_json(&asset),
        json!({
            "asset_id": "9000",
            "name": "Spring banner",
            "width": 1200,
            "height": 628,
            "shape": "landscape",
            "large_enough": true,
        })
    );
    let encoded = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&image)
    };
    assert_eq!(
        google.mutations(),
        [json!([{
            "assetOperation": {
                "create": {
                    "name": "Spring banner",
                    "type": "IMAGE",
                    "imageAsset": { "data": encoded },
                },
            },
        }])]
    );

    let missing = call_ads(
        &gateway,
        &plaintext,
        "create_image_asset",
        json!({
            "customer_id": GOOGLE_ADS_CUSTOMER,
            "upload_id": "11111111-2222-4333-8444-555555555555",
            "name": "Nothing",
        }),
    )
    .await;
    assert!(
        result_text(&missing)
            .contains("No file is uploaded as \"11111111-2222-4333-8444-555555555555\"")
    );
}
