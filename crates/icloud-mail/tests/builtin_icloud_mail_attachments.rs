//! The port of `tests/unit/builtin_icloud_mail_attachments.spec.ts`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mymcps_builtin::{BuiltinPasswordContext, BuiltinUploadTarget};
use mymcps_icloud_mail::testing::{FakeOptions, SmtpBehaviour};
use serde_json::{Value, json};
use support::{PERMISSIONS, Scenario, assert_same_json};

const PDF: &str = "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

fn mail(attachments: Value) -> Value {
    let mut mail = json!({ "to": ["dave@example.com"], "subject": "Quote", "text": "Here it is." });
    if !attachments.is_null() {
        mail["attachments"] = attachments;
    }
    mail
}

fn not_uploaded(id: &str) -> String {
    format!(
        "No file is uploaded as \"{id}\". Send the file to the link create_upload_link returned with this upload_id, then try again. An uploaded file can be attached for 60 minutes."
    )
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test]
async fn sends_uploaded_files_as_attachments_in_the_message_and_in_its_copy() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let quote = scenario
        .upload_file(&mcp, "Devis été.pdf", PDF, Some("application/pdf"))
        .await;
    let notes = scenario
        .upload_file(&mcp, "notes.txt", "Call back on Monday.\n", None)
        .await;

    // Naming a file twice attaches it once.
    let result = scenario
        .data(
            &mcp,
            "send_message",
            mail(json!([quote, notes, quote.to_uppercase()])),
        )
        .await;

    assert_same_json(
        &result,
        &json!({
            "sent": true,
            "message_id": result["message_id"],
            "from": "thomas@icloud.com",
            "subject": "Quote",
            "to": ["dave@example.com"],
            "attachments": [
                { "filename": "Devis été.pdf", "size": PDF.len() },
                { "filename": "notes.txt", "size": 21 },
            ],
            "saved_to": "Sent Messages",
        }),
    );

    // The bytes are handed over: the message carries the files, and names no path or URL to read.
    let sent = &scenario.icloud.sent()[0];
    let copy = &scenario.icloud.mailbox("Sent Messages").appended[0];
    for raw in [&sent.raw, &copy.raw] {
        assert!(raw.contains("Content-Type: multipart/mixed;"), "{raw}");
        assert!(raw.contains("Here it is.\r\n"));
        assert!(raw.contains("Content-Type: application/pdf;"));
        assert!(raw.contains("filename*0*=utf-8''Devis%20%C3%A9t%C3%A9.pdf"));
        assert!(raw.contains("Content-Disposition: attachment;"));
        assert!(raw.contains(&STANDARD.encode(PDF)[..60]));
        // Without a media type, the extension says what the file is.
        assert!(raw.contains("Content-Type: text/plain; name=notes.txt\r\n"));
        assert!(raw.contains("Content-Disposition: attachment; filename=notes.txt\r\n"));
        assert!(raw.contains(&STANDARD.encode("Call back on Monday.\n")));
        assert_eq!(raw.matches("Content-Disposition: attachment").count(), 2);
    }
}

#[tokio::test]
async fn keeps_an_upload_for_a_draft_and_for_the_message_sent_after_it() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["draft", "send"]);
    let quote = scenario.upload_file(&mcp, "quote.pdf", PDF, None).await;

    let draft = scenario
        .data(&mcp, "create_draft", mail(json!(quote)))
        .await;
    assert_eq!(
        draft["attachments"],
        json!([{ "filename": "quote.pdf", "size": PDF.len() }])
    );
    assert_eq!(draft["saved_to"], "Drafts");
    let saved = &scenario.icloud.mailbox("Drafts").appended[0];
    assert!(
        saved
            .raw
            .contains("Content-Type: application/pdf; name=quote.pdf\r\n")
    );
    assert_eq!(scenario.icloud.sent(), []);

    let sent = scenario
        .data(&mcp, "send_message", mail(json!([quote])))
        .await;
    assert_eq!(sent["sent"], true);
    assert!(
        scenario.icloud.sent()[0]
            .raw
            .contains(&STANDARD.encode(PDF))
    );

    // A message without attachments says nothing about them.
    let plain = scenario.data(&mcp, "send_message", mail(Value::Null)).await;
    assert_eq!(plain.get("attachments"), None);
    assert!(!scenario.icloud.sent()[1].raw.contains("multipart"));
}

#[tokio::test]
async fn refuses_what_was_not_uploaded_for_this_mcp_before_reaching_icloud() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let other = scenario.full_access();
    let elsewhere = scenario.upload_file(&other, "private.pdf", PDF, None).await;
    let unknown = new_id();
    let text = async |attachments: Value| {
        scenario
            .refusal(&mcp, "send_message", mail(attachments))
            .await
    };

    assert_eq!(text(json!(unknown)).await, not_uploaded(&unknown));
    // The upload of another MCP, even of the same account, is not one of this MCP.
    assert_eq!(text(json!([elsewhere])).await, not_uploaded(&elsewhere));
    assert_eq!(
        text(json!("/etc/passwd")).await,
        "attachments must be a list of at most 10 upload IDs, as returned by create_upload_link"
    );
    assert_eq!(
        scenario
            .refusal(&mcp, "create_draft", mail(json!(unknown)))
            .await,
        not_uploaded(&unknown)
    );

    assert_eq!(scenario.icloud.sign_ins(), []);
    assert_eq!(scenario.icloud.sent(), []);
}

#[tokio::test]
async fn carries_at_most_20_mb_of_files_in_a_message() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let half = vec![b'a'; 10_000_000];
    let first = scenario
        .upload_file(&mcp, "first.bin", half.clone(), None)
        .await;
    let second = scenario.upload_file(&mcp, "second.bin", half, None).await;
    let small = scenario.upload_file(&mcp, "small.txt", "x", None).await;

    assert_eq!(
        scenario
            .refusal(&mcp, "send_message", mail(json!([first, second, small])))
            .await,
        "These attachments take 20.0 MB together, and a message carries at most 20 MB. Attach fewer files, or send them in several messages."
    );
    assert_eq!(scenario.icloud.sign_ins(), []);

    let sent = scenario
        .data(&mcp, "send_message", mail(json!([first, second])))
        .await;
    assert_eq!(
        sent["attachments"],
        json!([{ "filename": "first.bin", "size": 10_000_000 }, { "filename": "second.bin", "size": 10_000_000 }])
    );
    // Encoded for mail, the files fit in the 28,319,744 bytes Apple's server accepts.
    for length in [
        scenario.icloud.mailbox("Sent Messages").appended[0]
            .raw
            .len(),
        scenario.icloud.sent()[0].raw.len(),
    ] {
        assert!(length > 27_000_000 && length < 28_000_000, "{length}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_at_most_two_messages_with_attachments_at_once_for_an_mcp() {
    let scenario = Arc::new(
        Scenario::with(FakeOptions {
            smtp: SmtpBehaviour::Hold,
            ..FakeOptions::default()
        })
        .await,
    );
    let busy = scenario.full_access();
    let other = scenario.full_access();
    let file = scenario.upload_file(&busy, "quote.pdf", PDF, None).await;
    let elsewhere = scenario.upload_file(&other, "quote.pdf", PDF, None).await;
    let send = |mcp: &Arc<BuiltinPasswordContext>, attachments: Value| {
        let (scenario, mcp) = (scenario.clone(), mcp.clone());
        tokio::spawn(async move { scenario.call(&mcp, "send_message", mail(attachments)).await })
    };
    let started = async |count: usize| {
        while scenario.icloud.deliveries_started() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };

    let sending = [send(&busy, json!([file])), send(&busy, json!([file]))];
    started(2).await;

    assert_eq!(
        scenario
            .refusal(&busy, "send_message", mail(json!([file])))
            .await,
        "Too many messages with attachments are being written at once. Try again in a few seconds."
    );
    scenario
        .refusal(&busy, "create_draft", mail(json!(file)))
        .await;
    // Messages without attachments, and those of another MCP, are not held back.
    let waiting = [send(&busy, Value::Null), send(&other, json!([elsewhere]))];
    started(4).await;

    scenario.icloud.release_deliveries();
    for result in sending.into_iter().chain(waiting) {
        result.await.unwrap().unwrap();
    }
    // Both places were given back, also by a message that failed.
    scenario
        .refusal(&busy, "send_message", mail(json!([file, new_id()])))
        .await;
    let (first, second) = tokio::join!(send(&busy, json!([file])), send(&busy, json!([file])));
    first.unwrap().unwrap();
    second.unwrap().unwrap();
}

#[tokio::test]
async fn takes_a_file_only_for_an_mcp_that_may_still_write_messages() {
    let scenario = Scenario::new().await;
    let upload = new_id();
    let reference =
        json!({ "upload": upload, "filename": "quote.pdf", "content_type": "application/pdf" });
    let target = async |mcp: Arc<BuiltinPasswordContext>, link: Value| {
        scenario.provider().upload_target(link, mcp).await
    };
    let refusal = async |mcp: Arc<BuiltinPasswordContext>, link: Value| {
        let error = target(mcp, link).await.unwrap_err();
        assert!(error.is_tool_error(), "{error}");
        error.to_string()
    };

    for permissions in [&["draft"][..], &["send"], &PERMISSIONS] {
        assert_eq!(
            target(scenario.mcp(permissions), reference.clone())
                .await
                .unwrap(),
            BuiltinUploadTarget {
                id: upload.clone(),
                filename: "quote.pdf".into(),
                content_type: Some("application/pdf".into()),
                max_bytes: 20_000_000,
            }
        );
    }
    assert_eq!(
        refusal(scenario.mcp(&["read", "organize"]), reference.clone()).await,
        "Neither the \"draft\" nor the \"send\" permission is allowed for this MCP any more"
    );

    let mcp = scenario.full_access();
    let with = |key: &str, value: &str| {
        let mut changed = reference.clone();
        changed[key] = json!(value);
        changed
    };
    // A download link refers to a message: it is not a place to store a file.
    refusal(
        mcp.clone(),
        json!({ "mailbox": "INBOX", "uid": 11, "part": "2" }),
    )
    .await;
    refusal(mcp.clone(), with("upload", "../../db.sqlite3")).await;
    refusal(mcp.clone(), with("filename", "../quote.pdf")).await;
    refusal(mcp.clone(), Value::Null).await;
}
