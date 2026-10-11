//! What `tests/functional/builtin_icloud_mail_mcp.spec.ts` and
//! `builtin_icloud_mail_uploads.spec.ts` say of the links the tools hand
//! out, and of the files behind them. The routes that serve the links, the
//! gateway and the setup form are tested where they are ported.

mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use mymcps_builtin::BuiltinUploadTarget;
use mymcps_builtin::file_link::{
    BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE, decode_file_reference, has_valid_signature,
};
use mymcps_core::TestCore;
use mymcps_icloud_mail::testing::{ATTACHMENT, Download, FakeOptions, Selection, USERNAME};
use serde_json::{Value, json};
use support::{Scenario, assert_same_json};

const NEEDS_APP_URL: &str = "File links need the public address of this MyMCPs instance. An administrator must set APP_URL.";

/// A link as its route reads it: the path, the reference in it, and the signature.
struct Link {
    path: String,
    reference: Value,
    signature: String,
}

fn link(url: &str, kind: &str, mcp_id: i64) -> Link {
    let path_and_query = url
        .strip_prefix("http://localhost:3333")
        .unwrap_or_else(|| panic!("{url} is not on the instance"));
    let (path, query) = path_and_query.split_once('?').unwrap();
    // The signature is the only parameter.
    let signature = query.strip_prefix("signature=").unwrap();
    assert!(!signature.contains('&'), "{query}");
    let encoded = path
        .strip_prefix(&format!("/{kind}/{mcp_id}/"))
        .unwrap_or_else(|| panic!("{path} is not a link of this MCP"));
    assert!(
        encoded
            .chars()
            .all(|character| character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'),
        "{encoded}"
    );
    Link {
        path: path.to_owned(),
        reference: decode_file_reference(encoded).unwrap(),
        signature: signature.to_owned(),
    }
}

/// How long a link works, from now.
fn minutes_left(expires_at: &Value) -> f64 {
    let expires_at = DateTime::parse_from_rfc3339(expires_at.as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    (expires_at - Utc::now()).num_milliseconds() as f64 / 60_000.0
}

// Built-in iCloud Mail MCP: attachment links

#[tokio::test]
async fn hands_out_a_temporary_link_that_downloads_the_attachment_without_signing_in() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);
    let result = scenario
        .data(&mcp, "get_attachment_link", json!({ "uid": 11, "part": 2 }))
        .await;

    assert_same_json(
        &result,
        &json!({
            "url": result["url"],
            "expires_at": result["expires_at"],
            "filename": "Menu été.pdf",
            "content_type": "application/pdf",
            "size": 57000,
        }),
    );
    let left = minutes_left(&result["expires_at"]);
    assert!(left > 14.0 && left <= 15.0, "{left}");

    let url = result["url"].as_str().unwrap();
    let link = link(url, "files", mcp.mcp_id);
    // The link tells an onlooker nothing about the mailbox.
    assert!(!url.contains("INBOX") && !url.contains(USERNAME));
    assert_eq!(
        link.reference.to_string(),
        r#"{"mailbox":"INBOX","uid":11,"part":"2"}"#
    );
    // The signature is the credential, for this file and for nothing else.
    assert!(has_valid_signature(
        &scenario.core,
        &link.path,
        BUILTIN_FILE_PURPOSE,
        Some(&link.signature)
    ));
    assert!(!has_valid_signature(
        &scenario.core,
        &link.path,
        BUILTIN_UPLOAD_PURPOSE,
        Some(&link.signature)
    ));
    assert!(!has_valid_signature(
        &scenario.core,
        &link.path.replace("/files/", "/uploads/"),
        BUILTIN_FILE_PURPOSE,
        Some(&link.signature)
    ));

    // What the route does once the signature is checked: each download signs in to iCloud again.
    let sign_ins = scenario.icloud.sign_ins().len();
    let file = scenario
        .provider()
        .download_file(link.reference, mcp.clone())
        .await
        .unwrap();
    assert_eq!(file.content.concat(), ATTACHMENT.as_bytes());
    assert_eq!(file.content_type, "application/pdf");
    assert_eq!(file.filename, "Menu été.pdf");
    assert_eq!(scenario.icloud.sign_ins().len(), sign_ins + 1);
    assert_eq!(
        scenario.icloud.downloads().last(),
        Some(&Download {
            uid: 11,
            part: "2".into()
        })
    );
    assert_eq!(
        scenario.icloud.selections().last(),
        Some(&Selection {
            path: "INBOX".into(),
            read_only: true
        })
    );

    // Another duration, when the agent asks for one.
    let hour = scenario
        .data(
            &mcp,
            "get_attachment_link",
            json!({ "uid": 11, "part": "2", "expires_in_minutes": 60 }),
        )
        .await;
    let left = minutes_left(&hour["expires_at"]);
    assert!(left > 59.0 && left <= 60.0, "{left}");
}

#[tokio::test]
async fn only_links_to_attachments_and_only_within_the_allowed_time() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);
    let call = async |arguments: Value| {
        scenario
            .refusal(&mcp, "get_attachment_link", arguments)
            .await
    };

    // The message text is not a file to hand out.
    assert_eq!(
        call(json!({ "uid": 11, "part": "1.1" })).await,
        "Message 11 has no attachment at part \"1.1\". Its attachments are at parts: 2."
    );
    assert_eq!(
        call(json!({ "uid": 12, "part": "1" })).await,
        "Message 12 has no attachments."
    );
    assert_eq!(
        call(json!({ "uid": 11, "part": "../2" })).await,
        "part must be the part of an attachment, such as 2, as returned by get_message"
    );
    assert_eq!(
        call(json!({ "uid": 11, "part": "2", "expires_in_minutes": 600 })).await,
        "expires_in_minutes must be an integer between 1 and 60"
    );

    // A signed link to a part that is not an attachment serves nothing either.
    let body = scenario
        .provider()
        .download_file(
            json!({ "mailbox": "INBOX", "uid": 11, "part": "1.1" }),
            mcp.clone(),
        )
        .await
        .unwrap_err();
    assert!(body.is_tool_error());
    assert_eq!(
        body.to_string(),
        "Message 11 has no attachment at part \"1.1\". Its attachments are at parts: 2."
    );
    let garbage = scenario
        .provider()
        .download_file(json!("INBOX"), mcp.clone())
        .await
        .unwrap_err();
    assert!(garbage.is_tool_error());
    // A message that was moved or deleted in the meantime.
    let gone = scenario
        .provider()
        .download_file(
            json!({ "mailbox": "INBOX", "uid": 99, "part": "2" }),
            mcp.clone(),
        )
        .await
        .unwrap_err();
    assert!(gone.is_tool_error());
}

#[tokio::test]
async fn stops_serving_a_link_once_the_mcp_no_longer_allows_reading() {
    let scenario = Scenario::new().await;
    let reference = json!({ "mailbox": "INBOX", "uid": 11, "part": "2" });

    let error = scenario
        .provider()
        .download_file(reference.clone(), scenario.mcp(&["send"]))
        .await
        .unwrap_err();
    assert!(error.is_tool_error());
    assert_eq!(
        error.to_string(),
        "The \"read\" permission is no longer allowed for this MCP"
    );
    assert_eq!(scenario.icloud.sign_ins(), []);

    scenario
        .provider()
        .download_file(reference, scenario.mcp(&["read"]))
        .await
        .unwrap();
}

#[tokio::test]
async fn links_need_the_public_address_of_the_instance() {
    let core = TestCore::with_config(|config| config.app_url = None).await;
    let scenario = Scenario::on(core, FakeOptions::default()).await;

    assert_eq!(
        scenario
            .refusal(
                &scenario.mcp(&["read"]),
                "get_attachment_link",
                json!({ "uid": 11, "part": "2" })
            )
            .await,
        NEEDS_APP_URL
    );
    assert_eq!(
        scenario
            .refusal(
                &scenario.mcp(&["draft"]),
                "create_upload_link",
                json!({ "filename": "a.pdf" })
            )
            .await,
        NEEDS_APP_URL
    );
    assert_eq!(scenario.icloud.sign_ins(), []);
}

// Built-in iCloud Mail MCP: upload links

const PDF: &str = "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

/// What the upload route does with the body of a request, once the signature is checked.
async fn send_file(
    scenario: &Scenario,
    mcp: &mymcps_builtin::BuiltinPasswordContext,
    target: &BuiltinUploadTarget,
    content: &str,
) {
    let body = futures::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(
        content.to_owned(),
    ))]);
    mcp.env
        .uploads
        .save(mcp.mcp_id, target, body)
        .await
        .unwrap();
    let _ = scenario;
}

#[tokio::test]
async fn hands_out_a_link_that_takes_one_file_and_attaches_it_to_a_message() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["send"]);

    let created = scenario
        .data(
            &mcp,
            "create_upload_link",
            json!({ "filename": "Devis été.pdf", "content_type": "application/pdf" }),
        )
        .await;
    assert_same_json(
        &created,
        &json!({
            "upload_id": created["upload_id"],
            "url": created["url"],
            "method": "PUT",
            "expires_at": created["expires_at"],
            "filename": "Devis été.pdf",
            "content_type": "application/pdf",
            "max_bytes": 20_000_000,
        }),
    );
    let upload_id = created["upload_id"].as_str().unwrap();
    assert_eq!(upload_id.len(), 36);
    assert!(
        upload_id
            .chars()
            .all(|character| character == '-' || matches!(character, '0'..='9' | 'a'..='f')),
        "{upload_id}"
    );
    let left = minutes_left(&created["expires_at"]);
    assert!(left > 14.0 && left <= 15.0, "{left}");

    let link = link(created["url"].as_str().unwrap(), "uploads", mcp.mcp_id);
    assert_eq!(
        link.reference,
        json!({ "upload": upload_id, "filename": "Devis été.pdf", "content_type": "application/pdf" })
    );
    // It takes a file, and cannot be turned into a link that hands one out.
    assert!(has_valid_signature(
        &scenario.core,
        &link.path,
        BUILTIN_UPLOAD_PURPOSE,
        Some(&link.signature)
    ));
    assert!(!has_valid_signature(
        &scenario.core,
        &link.path,
        BUILTIN_FILE_PURPOSE,
        Some(&link.signature)
    ));
    // Asking for a link reaches neither iCloud nor the disk.
    assert_eq!(scenario.icloud.sign_ins(), []);
    assert_eq!(
        mcp.env.uploads.find(mcp.mcp_id, upload_id).await.unwrap(),
        None
    );
    assert!(!mcp.env.uploads.root().join(mcp.mcp_id.to_string()).exists());

    let target = scenario
        .provider()
        .upload_target(link.reference, mcp.clone())
        .await
        .unwrap();
    assert_eq!(
        target,
        BuiltinUploadTarget {
            id: upload_id.to_owned(),
            filename: "Devis été.pdf".into(),
            content_type: Some("application/pdf".into()),
            max_bytes: 20_000_000,
        }
    );
    send_file(&scenario, &mcp, &target, PDF).await;

    let sent = scenario
        .data(&mcp, "send_message", json!({ "to": "dave@example.com", "subject": "Quote", "text": "Here it is.", "attachments": [upload_id] }))
        .await;
    assert_eq!(
        sent["attachments"],
        json!([{ "filename": "Devis été.pdf", "size": PDF.len() }])
    );
    assert!(
        scenario.icloud.sent()[0]
            .raw
            .contains(&STANDARD.encode(PDF))
    );
    assert!(
        scenario.icloud.mailbox("Sent Messages").appended[0]
            .raw
            .contains(&STANDARD.encode(PDF))
    );
}

#[tokio::test]
async fn refuses_to_attach_a_file_before_it_was_sent_to_its_link() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["draft"]);
    let created = scenario
        .data(
            &mcp,
            "create_upload_link",
            json!({ "filename": "notes.txt", "expires_in_minutes": 60 }),
        )
        .await;
    assert_eq!(created.get("content_type"), None);
    let left = minutes_left(&created["expires_at"]);
    assert!(left > 59.0 && left <= 60.0, "{left}");
    let upload_id = created["upload_id"].as_str().unwrap();

    let draft = json!({ "subject": "Notes", "text": "Attached.", "attachments": upload_id });
    let early = scenario.refusal(&mcp, "create_draft", draft.clone()).await;
    assert!(
        early.contains(&format!("No file is uploaded as \"{upload_id}\".")),
        "{early}"
    );
    assert_eq!(scenario.icloud.sign_ins(), []);

    let link = link(created["url"].as_str().unwrap(), "uploads", mcp.mcp_id);
    assert_eq!(
        link.reference.to_string(),
        format!(r#"{{"upload":"{upload_id}","filename":"notes.txt"}}"#)
    );
    let target = scenario
        .provider()
        .upload_target(link.reference, mcp.clone())
        .await
        .unwrap();
    assert_eq!(target.content_type, None);
    send_file(&scenario, &mcp, &target, "Call back on Monday.\n").await;

    let saved = scenario.data(&mcp, "create_draft", draft).await;
    assert_eq!(
        saved["attachments"],
        json!([{ "filename": "notes.txt", "size": 21 }])
    );
    let stored = &scenario.icloud.mailbox("Drafts").appended[0];
    assert!(
        stored
            .raw
            .contains("Content-Disposition: attachment; filename=notes.txt\r\n")
    );
}
