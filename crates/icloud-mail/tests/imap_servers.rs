//! The tools against servers that are not iCloud: fewer extensions, other
//! encodings. Apple may change what its server offers, and what the client
//! falls back on then is not what a test of the usual case goes through.

mod support;

use std::collections::HashMap;

use mymcps_icloud_mail::message::{Address, BodyStructure, Envelope};
use mymcps_icloud_mail::testing::{
    FakeMessage, FakeOptions, PASSWORD, SignInAttempt, USERNAME, attached, leaf, multipart,
};
use serde_json::json;
use support::{Scenario, assert_same_json};

fn with_capabilities(capabilities: &[&'static str]) -> FakeOptions {
    FakeOptions {
        capabilities: Some(capabilities.to_vec()),
        ..FakeOptions::default()
    }
}

#[tokio::test]
async fn signs_in_with_login_when_the_server_offers_no_other_way() {
    let scenario = Scenario::with(with_capabilities(&[
        "IMAP4rev1",
        "NAMESPACE",
        "SPECIAL-USE",
        "UIDPLUS",
        "MOVE",
    ]))
    .await;
    let listed = scenario
        .data(&scenario.mcp(&["read"]), "list_mailboxes", json!({}))
        .await;

    assert_eq!(listed.as_array().unwrap().len(), 5);
    assert_eq!(
        scenario.icloud.sign_ins(),
        [SignInAttempt {
            username: USERNAME.into(),
            password: PASSWORD.into()
        }]
    );
    // Without LIST-STATUS, each mailbox that can hold messages is asked for its counts.
    let commands = scenario.icloud.commands();
    assert_eq!(commands[..2], ["NAMESPACE", "LIST \"\" \"*\""]);
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.starts_with("STATUS "))
            .count(),
        5
    );
    assert!(
        commands.contains(&"STATUS \"Sent Messages\" (MESSAGES UNSEEN)".to_owned()),
        "{commands:?}"
    );
    assert_eq!(
        listed[0],
        json!({ "path": "INBOX", "role": "inbox", "messages": 3, "unread": 1 })
    );
}

#[tokio::test]
async fn does_without_the_extensions_icloud_has() {
    let scenario = Scenario::with(with_capabilities(&["IMAP4rev1"])).await;
    let mcp = scenario.full_access();

    // The special mailboxes are known by their names when the server does not flag them.
    assert_same_json(
        &scenario.data(&mcp, "list_mailboxes", json!({})).await,
        &json!([
            { "path": "INBOX", "role": "inbox", "messages": 3, "unread": 1 },
            { "path": "Sent Messages", "role": "sent", "messages": 1, "unread": 0 },
            { "path": "Drafts", "role": "drafts", "messages": 0, "unread": 0 },
            { "path": "Archive", "role": "archive", "messages": 1, "unread": 0 },
            { "path": "Deleted Messages", "role": "trash", "messages": 0, "unread": 0 },
        ]),
    );

    // Without UIDPLUS the server does not say which UID the draft got.
    let draft = scenario
        .data(
            &mcp,
            "create_draft",
            json!({ "subject": "Ideas", "text": "To finish later." }),
        )
        .await;
    assert_eq!(draft["saved_to"], "Drafts");
    assert_eq!(draft.get("uid"), None);
    assert_eq!(scenario.icloud.mailbox("Drafts").appended.len(), 1);

    // Without MOVE a message is copied, then deleted where it was.
    let moved = scenario
        .data(
            &mcp,
            "move_messages",
            json!({ "uids": [12, 14], "destination": "Deleted Messages" }),
        )
        .await;
    assert_eq!(moved["uids"], json!([12, 14]));
    assert_eq!(moved["destination"], "Deleted Messages");
    let uids = |path: &str| {
        scenario
            .icloud
            .mailbox(path)
            .messages
            .iter()
            .map(|message| message.uid)
            .collect::<Vec<_>>()
    };
    assert_eq!(uids("INBOX"), [11]);
    assert_eq!(
        scenario.icloud.mailbox("Deleted Messages").messages.len(),
        2
    );
    let commands = scenario.icloud.commands();
    let moving: Vec<&String> = commands
        .iter()
        .skip_while(|command| !command.starts_with("UID COPY"))
        .collect();
    assert_eq!(
        moving[..3],
        [
            "UID COPY 12,14 \"Deleted Messages\"",
            "UID STORE 12,14 +FLAGS.SILENT (\\Deleted)",
            "EXPUNGE"
        ]
    );

    // A sent message is still kept and its original still marked.
    let sent = scenario
        .data(
            &mcp,
            "send_message",
            json!({ "reply_to_uid": 11, "text": "Yes." }),
        )
        .await;
    assert_eq!(sent["saved_to"], "Sent Messages");
    assert_eq!(
        scenario.icloud.mailbox("INBOX").messages[0].flags,
        ["\\Answered"]
    );
}

fn message(uid: u32, body_structure: BodyStructure) -> FakeMessage {
    FakeMessage {
        uid,
        envelope: Envelope {
            subject: Some("Encodings".into()),
            from: vec![Address::bare("alice@example.com")],
            ..Envelope::default()
        },
        body_structure,
        ..FakeMessage::default()
    }
}

#[tokio::test]
async fn reads_text_in_another_charset_and_joins_the_lines_of_flowed_text() {
    let scenario = Scenario::new().await;
    let latin1 = BodyStructure {
        parameters: HashMap::from([("charset".to_owned(), "ISO-8859-1".to_owned())]),
        ..leaf(None, "text/plain", Some("quoted-printable"), 40)
    };
    scenario.icloud.push_message(
        "INBOX",
        message(51, latin1).part("1", b"D\xe9jeuner jeudi \xe0 midi ?".to_vec()),
    );
    let flowed = BodyStructure {
        parameters: HashMap::from([("format".to_owned(), "flowed".to_owned())]),
        ..leaf(Some("1"), "text/plain", Some("7bit"), 80)
    };
    scenario.icloud.push_message(
        "INBOX",
        message(
            52,
            multipart(
                None,
                "mixed",
                vec![flowed, attached("2", "text/plain", 20, "notes.txt")],
            ),
        )
        .part(
            "1",
            "A paragraph that \r\ngoes on over \r\nthree lines.\r\n\r\n-- \r\nAlice\r\n",
        )
        .part("2", "Not the text."),
    );
    let japanese = BodyStructure {
        parameters: HashMap::from([("charset".to_owned(), "shift_jis".to_owned())]),
        ..leaf(None, "text/plain", Some("base64"), 12)
    };
    scenario.icloud.push_message(
        "INBOX",
        message(53, japanese).part("1", b"\x93\xfa\x96\x7b\x8c\xea".to_vec()),
    );
    let mcp = scenario.mcp(&["read"]);

    assert_eq!(
        scenario
            .data(&mcp, "get_message", json!({ "uid": 51 }))
            .await["text"],
        "Déjeuner jeudi à midi ?"
    );
    assert_eq!(
        scenario
            .data(&mcp, "get_message", json!({ "uid": 52 }))
            .await["text"],
        "A paragraph that goes on over three lines.\n\n--\nAlice"
    );
    assert_eq!(
        scenario
            .data(&mcp, "get_message", json!({ "uid": 53 }))
            .await["text"],
        "日本語"
    );
}

#[tokio::test]
async fn downloads_a_large_attachment_in_pieces() {
    let scenario = Scenario::new().await;
    // Every byte there is, over more than three pieces of 64 kB once encoded.
    let content: Vec<u8> = (0..200_000_u32).map(|index| (index % 251) as u8).collect();
    scenario.icloud.push_message(
        "INBOX",
        message(
            54,
            multipart(
                None,
                "mixed",
                vec![
                    leaf(Some("1"), "text/plain", Some("7bit"), 5),
                    attached("2", "application/zip", 270_000, "archive.zip"),
                ],
            ),
        )
        .part("1", "Files")
        .part("2", content.clone()),
    );
    let mcp = scenario.mcp(&["read"]);

    let file = scenario
        .provider()
        .download_file(json!({ "uid": 54, "part": "2" }), mcp.clone())
        .await
        .unwrap();
    assert_eq!(file.filename, "archive.zip");
    assert_eq!(file.content.concat(), content);
    let pieces = scenario
        .icloud
        .commands()
        .iter()
        .filter(|command| command.contains("BODY.PEEK[2]<"))
        .count();
    assert_eq!(pieces, 5);
    assert!(
        scenario
            .icloud
            .commands()
            .contains(&"UID FETCH 54 (UID BODY.PEEK[2]<65536.65536>)".to_owned())
    );
}
