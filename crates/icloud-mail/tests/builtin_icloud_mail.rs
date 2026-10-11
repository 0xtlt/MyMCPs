//! The port of `tests/unit/builtin_icloud_mail.spec.ts`.
//!
//! The Node tests replaced imapflow and nodemailer with objects in memory,
//! and looked at the calls they received. Here the tools talk to the servers
//! of `mymcps_icloud_mail::testing`, and the tests look at what those
//! received: commands instead of calls, and the message as it was delivered
//! instead of the options nodemailer was given.

mod support;

use std::collections::HashMap;

use mymcps_builtin::BuiltinMcpDefinition;
use mymcps_icloud_mail::html::{HtmlConversion, HtmlConverter};
use mymcps_icloud_mail::message::{
    Address, Attachment, BodyPart, BodyStructure, Envelope, FetchedMessage, Reply, attachments_of,
    body_part, reply_to, tidy_text,
};
use mymcps_icloud_mail::testing::{
    Download, FakeOptions, FetchCommand, PASSWORD, Selection, SignInAttempt, SmtpBehaviour,
    USERNAME, attached, leaf, multipart,
};
use mymcps_icloud_mail::{MailServer, MailServers};
use serde_json::{Value, json};
use support::{PERMISSIONS, Scenario, assert_same_json, pluck};

fn sign_in() -> SignInAttempt {
    SignInAttempt {
        username: USERNAME.to_owned(),
        password: PASSWORD.to_owned(),
    }
}

fn usernames(scenario: &Scenario) -> Vec<String> {
    scenario
        .icloud
        .sign_ins()
        .into_iter()
        .map(|attempt| attempt.username)
        .collect()
}

fn plain() -> BodyStructure {
    leaf(Some("1.1"), "text/plain", None, 10)
}

fn html() -> BodyStructure {
    leaf(Some("1.2"), "text/html", None, 40)
}

fn pdf() -> BodyStructure {
    attached("2", "application/pdf", 7800, "menu.pdf")
}

fn part(id: &str, is_html: bool) -> Option<BodyPart> {
    Some(BodyPart {
        id: id.to_owned(),
        is_html,
    })
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

// Built-in iCloud Mail MCP: messages

#[test]
fn reads_plain_text_before_html_and_never_an_attached_file() {
    let alternative = multipart(Some("1"), "alternative", vec![html(), plain()]);
    let notes = BodyStructure {
        encoding: None,
        size: None,
        ..attached("3", "text/plain", 0, "notes.txt")
    };

    assert_eq!(
        body_part(&multipart(
            None,
            "mixed",
            vec![notes.clone(), alternative, pdf()]
        )),
        part("1.1", false)
    );
    assert_eq!(
        body_part(&multipart(None, "mixed", vec![html(), pdf()])),
        part("1.2", true)
    );
    assert_eq!(
        body_part(&multipart(None, "mixed", vec![notes, pdf()])),
        None
    );
    // A message made of one part has no part number.
    assert_eq!(
        body_part(&BodyStructure {
            media_type: "text/html".into(),
            ..BodyStructure::default()
        }),
        part("1", true)
    );
}

#[test]
fn lists_named_parts_as_attachments_and_keeps_a_forwarded_message_whole() {
    let forwarded = BodyStructure {
        parameters: HashMap::from([("name".to_owned(), "Fwd.eml".to_owned())]),
        child_nodes: Some(vec![leaf(Some("3.1"), "text/plain", None, 500)]),
        ..leaf(Some("3"), "message/rfc822", None, 9000)
    };
    let photo = BodyStructure {
        disposition: Some("inline".to_owned()),
        disposition_parameters: HashMap::from([(
            "filename".to_owned(),
            "IMG_0042.jpeg".to_owned(),
        )]),
        ..leaf(Some("4"), "image/jpeg", None, 300_000)
    };
    let structure = multipart(None, "mixed", vec![plain(), pdf(), forwarded, photo]);
    let attachment = |part: &str, filename: &str, content_type: &str, size: u64| Attachment {
        part: part.to_owned(),
        filename: filename.to_owned(),
        content_type: content_type.to_owned(),
        size,
    };

    assert_eq!(
        attachments_of(Some(&structure)),
        [
            // Base64 makes a file about a third larger in transit.
            attachment("2", "menu.pdf", "application/pdf", 5700),
            attachment("3", "Fwd.eml", "message/rfc822", 9000),
            attachment("4", "IMG_0042.jpeg", "image/jpeg", 300_000),
        ]
    );
    assert_eq!(body_part(&structure), part("1.1", false));
    assert_eq!(attachments_of(None), []);
    // A message that is nothing but a file has no part number.
    let zip = BodyStructure {
        parameters: HashMap::from([("name".to_owned(), "a.zip".to_owned())]),
        ..leaf(None, "application/zip", None, 10)
    };
    assert_eq!(
        attachments_of(Some(&zip)),
        [attachment("1", "a.zip", "application/zip", 10)]
    );
}

#[tokio::test]
async fn turns_html_into_text_without_styles_images_or_preview_padding() {
    let converted = HtmlConverter::default()
        .html_to_text(
            [
                "<html><head><style>p { color: red }</style></head><body>",
                "<p>Hello\u{200c} \u{200c} \u{200c} \u{200c} </p>",
                "<img src=\"https://shop.example/pixel.gif\" alt=\"tracking\">",
                "<p><a href=\"https://shop.example/deals\">See the deals</a></p>",
                "<p><a href=\"https://shop.example\">https://shop.example</a></p>",
                "<p>This paragraph is long enough that a wrapping converter would break it across several lines of output.</p>",
                "</body></html>",
            ]
            .concat(),
            1000,
            HtmlConversion::default(),
        )
        .await
        .unwrap();

    assert!(!converted.is_truncated);
    assert_eq!(
        tidy_text(&converted.text),
        [
            "Hello",
            "See the deals [https://shop.example/deals]",
            "https://shop.example",
            "This paragraph is long enough that a wrapping converter would break it across several lines of output.",
        ]
        .join("\n\n")
    );
    assert_eq!(tidy_text("One  \r\n\r\n\r\n\r\nTwo\r\n"), "One\n\nTwo");
}

#[test]
fn addresses_a_reply_to_the_author_and_to_everyone_else_only_when_asked() {
    let original = FetchedMessage {
        uid: 11,
        headers: Some(b"References: <root@example.com>\r\n <second@example.com>\r\n\r\n".to_vec()),
        envelope: Some(Envelope {
            subject: Some("Lunch on Thursday?".into()),
            message_id: Some("<lunch@example.com>".into()),
            from: vec![Address::new("Alice", "alice@example.com")],
            to: vec![
                Address::bare("Thomas@iCloud.com"),
                Address::bare("bob@example.com"),
            ],
            cc: vec![
                Address::bare("carol@example.com"),
                Address::bare("BOB@example.com"),
            ],
            ..Envelope::default()
        }),
        ..FetchedMessage::default()
    };

    assert_eq!(
        reply_to(&original, &strings(&["thomas@icloud.com"]), false),
        Reply {
            from: Some("thomas@icloud.com".into()),
            to: strings(&["alice@example.com"]),
            cc: vec![],
            subject: "Re: Lunch on Thursday?".into(),
            in_reply_to: Some("<lunch@example.com>".into()),
            references: strings(&[
                "<root@example.com>",
                "<second@example.com>",
                "<lunch@example.com>"
            ]),
        }
    );
    assert_eq!(
        reply_to(&original, &strings(&["thomas@icloud.com"]), true).cc,
        ["bob@example.com", "carol@example.com"]
    );

    // Bob's address belongs to the account too: answer from it, and do not copy it.
    let as_bob = reply_to(
        &original,
        &strings(&["hello@thomas.example", "bob@example.com"]),
        true,
    );
    assert_eq!(as_bob.from.as_deref(), Some("bob@example.com"));
    assert_eq!(as_bob.cc, ["Thomas@iCloud.com", "carol@example.com"]);
}

#[test]
fn follows_reply_to_continues_a_sent_message_and_ignores_unusable_addresses() {
    let list = reply_to(
        &FetchedMessage {
            uid: 1,
            envelope: Some(Envelope {
                subject: Some("RE: Minutes".into()),
                from: vec![Address::bare("alice@example.com")],
                reply_to: vec![
                    Address::bare("team@example.com"),
                    Address::bare("not an address"),
                ],
                to: vec![Address::bare("thomas@icloud.com")],
                ..Envelope::default()
            }),
            ..FetchedMessage::default()
        },
        &strings(&["hello@thomas.example"]),
        false,
    );
    assert_eq!(list.to, ["team@example.com"]);
    // Sent to none of the known addresses: the caller picks the sender.
    assert_eq!(list.from, None);
    assert_eq!(list.subject, "RE: Minutes");
    assert_eq!(list.in_reply_to, None);
    assert_eq!(list.references, Vec::<String>::new());

    let follow_up = reply_to(
        &FetchedMessage {
            uid: 5,
            envelope: Some(Envelope {
                subject: Some("Quote".into()),
                message_id: Some("<quote@icloud.com>".into()),
                from: vec![Address::bare("Hello@Thomas.example")],
                to: vec![
                    Address::bare("dave@example.com"),
                    Address::bare("Dave <dave@example.com>, eve@x.io"),
                ],
                ..Envelope::default()
            }),
            ..FetchedMessage::default()
        },
        &strings(&["thomas@icloud.com", "hello@thomas.example"]),
        false,
    );
    assert_eq!(follow_up.to, ["dave@example.com"]);
    assert_eq!(follow_up.from.as_deref(), Some("hello@thomas.example"));
}

// Built-in iCloud Mail MCP: permissions

#[test]
fn signs_in_with_a_password_and_lets_the_admin_choose_among_four_permissions() {
    let definition = mymcps_icloud_mail::definition();
    assert_eq!(definition.key(), "icloud-mail");
    assert_eq!(definition.name(), "iCloud Mail");
    assert!(definition.oauth().is_none());
    let password = definition.password().unwrap();
    assert_eq!(password.permissions, PERMISSIONS);
    assert!(password.password_pattern.is_match("abcd-efgh-ijkl-mnop"));
    assert!(password.password_pattern.is_match("abcdefghijklmnop"));
    assert!(!password.password_pattern.is_match("Tr0ub4dor&3-horse"));
    assert!(password.username_pattern.is_match("thomas@icloud.com"));
    assert!(!password.username_pattern.is_match("thomas"));
    assert_eq!(
        password.username_hint,
        "Enter your iCloud Mail address, such as name@icloud.com"
    );
    assert_eq!(
        password.password_hint,
        "Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop. Your Apple Account password does not work here."
    );
    assert_eq!(
        password.alias_hint,
        "Enter up to 20 other addresses of this iCloud account, such as alias@icloud.com, separated by commas"
    );

    let mut needed: Vec<&str> = Vec::new();
    for tool in definition.tools() {
        assert!(
            !tool.requires_any_scope.is_empty(),
            "{} needs a permission",
            tool.name
        );
        assert!(
            !tool.write,
            "{} is allowed by a permission, not by write access",
            tool.name
        );
        for scope in tool.requires_any_scope {
            if !needed.contains(scope) {
                needed.push(scope);
            }
        }
    }
    assert_eq!(needed, PERMISSIONS);
    assert!(definition.has_download() && definition.has_upload());
}

/// The tools an MCP offers: those one of whose permissions the admin allowed.
fn tool_names(definition: &BuiltinMcpDefinition, permissions: &[&str]) -> Vec<&'static str> {
    definition
        .tools()
        .into_iter()
        .filter(|tool| {
            tool.requires_any_scope
                .iter()
                .any(|scope| permissions.contains(scope))
        })
        .map(|tool| tool.name)
        .collect()
}

#[test]
fn exposes_only_the_tools_of_the_allowed_permissions() {
    let definition = mymcps_icloud_mail::definition();
    let names = |permissions: &[&str]| tool_names(&definition, permissions);

    assert_eq!(
        names(&["read"]),
        [
            "list_mailboxes",
            "list_messages",
            "get_message",
            "get_attachment_link"
        ]
    );
    // Either permission that writes a message can attach a file to it.
    assert_eq!(names(&["draft"]), ["create_upload_link", "create_draft"]);
    assert_eq!(names(&["send"]), ["create_upload_link", "send_message"]);
    assert_eq!(names(&["organize"]), ["mark_messages", "move_messages"]);
    assert_eq!(names(&PERMISSIONS).len(), 9);
    assert_eq!(names(&[]), Vec::<&str>::new());
}

#[tokio::test]
async fn does_not_let_a_reply_read_a_message_without_the_read_permission() {
    let scenario = Scenario::new().await;
    let send_only = scenario.mcp(&["send", "draft"]);
    for tool in ["send_message", "create_draft"] {
        let refused = scenario
            .refusal(
                &send_only,
                tool,
                json!({ "reply_to_uid": 11, "text": "Yes" }),
            )
            .await;
        assert!(
            refused.contains("the \"read\" permission is not allowed for this MCP"),
            "{refused}"
        );
    }
    assert_eq!(scenario.icloud.sign_ins(), []);

    scenario
        .data(
            &send_only,
            "send_message",
            json!({ "to": ["dave@example.com"], "subject": "Hello", "text": "Hi" }),
        )
        .await;
    assert_eq!(scenario.icloud.sent().len(), 1);
}

// Built-in iCloud Mail MCP: reading

#[tokio::test]
async fn lists_selectable_mailboxes_with_their_role_and_counts() {
    let scenario = Scenario::new().await;
    let result = scenario
        .data(&scenario.mcp(&["read"]), "list_mailboxes", json!({}))
        .await;

    // The special mailboxes first, in the order imapflow lists them, which
    // the Node test did not see: its fake listed mailboxes as it held them.
    assert_same_json(
        &result,
        &json!([
            { "path": "INBOX", "role": "inbox", "messages": 3, "unread": 1 },
            { "path": "Sent Messages", "role": "sent", "messages": 1, "unread": 0 },
            { "path": "Drafts", "role": "drafts", "messages": 0, "unread": 0 },
            { "path": "Archive", "role": "archive", "messages": 1, "unread": 0 },
            { "path": "Deleted Messages", "role": "trash", "messages": 0, "unread": 0 },
        ]),
    );
    assert_eq!(scenario.icloud.sign_ins(), [sign_in()]);
    assert_eq!(scenario.icloud.logouts(), 1);
}

#[tokio::test]
async fn lists_the_newest_messages_first_without_searching_the_mailbox() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);
    let result = scenario.data(&mcp, "list_messages", json!({})).await;

    assert_same_json(
        &result,
        &json!({
            "mailbox": "INBOX",
            "total": 3,
            "page": 1,
            "per_page": 20,
            "messages": [
                {
                    "uid": 14,
                    "subject": "Re: Invoice 42",
                    "from": "andre@example.com",
                    "to": ["thomas@icloud.com"],
                    "date": "2026-10-03T15:45:00.000Z",
                    "unread": false,
                    "flagged": true,
                    "answered": false,
                    "attachments": 0,
                },
                {
                    "uid": 12,
                    "subject": "Autumn sale",
                    "from": "Shop <news@shop.example>",
                    "to": ["thomas@icloud.com"],
                    "date": "2026-10-02T06:00:00.000Z",
                    "unread": false,
                    "flagged": false,
                    "answered": false,
                    "attachments": 0,
                },
                {
                    "uid": 11,
                    "subject": "Lunch on Thursday?",
                    "from": "Alice Martin <alice@example.com>",
                    "to": ["Thomas <thomas@icloud.com>", "bob@example.com"],
                    "date": "2026-10-01T09:30:00.000Z",
                    "unread": true,
                    "flagged": false,
                    "answered": false,
                    "attachments": 1,
                },
            ],
        }),
    );
    assert_eq!(scenario.icloud.searches(), Vec::<String>::new());
    assert_eq!(
        scenario.icloud.fetches(),
        [FetchCommand {
            range: "1:3".into(),
            by_uid: false
        }]
    );
    assert_eq!(
        scenario.icloud.selections(),
        [Selection {
            path: "INBOX".into(),
            read_only: true
        }]
    );

    let second = scenario
        .data(&mcp, "list_messages", json!({ "page": 2, "per_page": 2 }))
        .await;
    assert_eq!(pluck(&second["messages"], "uid"), [json!(11)]);
    assert_eq!(
        scenario.icloud.fetches()[1],
        FetchCommand {
            range: "1:1".into(),
            by_uid: false
        }
    );

    let beyond = scenario
        .data(&mcp, "list_messages", json!({ "page": 3 }))
        .await;
    assert_eq!(beyond["messages"], json!([]));
    assert_eq!(beyond["total"], 3);
    assert_eq!(scenario.icloud.fetches().len(), 2);
}

#[tokio::test]
async fn searches_with_every_filter_combined_and_pages_through_the_matches() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);
    let result = scenario
        .data(
            &mcp,
            "list_messages",
            json!({
                "from": " alice ",
                "to": "bob",
                "subject": "lunch",
                "text": "thursday",
                "since": "2026-09-30",
                "before": "2026-10-04T00:00:00Z",
                "unread": true,
                "flagged": "false",
            }),
        )
        .await;

    assert_eq!(
        scenario.icloud.searches(),
        [
            "FROM alice TO bob SUBJECT lunch TEXT thursday SINCE 30-Sep-2026 BEFORE 04-Oct-2026 UNSEEN UNFLAGGED"
        ]
    );
    assert_eq!(result["total"], 1);
    assert_eq!(pluck(&result["messages"], "uid"), [json!(11)]);
    assert_eq!(
        scenario.icloud.fetches(),
        [FetchCommand {
            range: "11".into(),
            by_uid: true
        }]
    );

    let read = scenario
        .data(
            &mcp,
            "list_messages",
            json!({ "unread": false, "per_page": 1 }),
        )
        .await;
    assert_eq!(read["total"], 2);
    assert_eq!(
        scenario.icloud.fetches()[1],
        FetchCommand {
            range: "14".into(),
            by_uid: true
        }
    );

    let none = scenario
        .data(&mcp, "list_messages", json!({ "subject": "nothing" }))
        .await;
    assert_eq!((&none["total"], &none["messages"]), (&json!(0), &json!([])));
    assert_eq!(scenario.icloud.fetches().len(), 2);

    // A time of day is not a date: before the afternoon of a day is before the day after.
    // Text outside ASCII goes as a literal, in a charset the server is told.
    scenario
        .data(
            &mcp,
            "list_messages",
            json!({ "from": "André", "before": "2026-10-04T15:00:00+02:00", "flagged": true }),
        )
        .await;
    assert_eq!(
        scenario.icloud.searches()[3],
        "CHARSET UTF-8 FROM André BEFORE 05-Oct-2026 FLAGGED"
    );
    assert_eq!(
        scenario
            .icloud
            .commands()
            .iter()
            .filter(|command| command.contains("FROM {6}"))
            .count(),
        1
    );
}

#[tokio::test]
async fn reads_the_text_part_of_a_message_and_names_its_attachments() {
    let scenario = Scenario::new().await;
    let result = scenario
        .data(
            &scenario.mcp(&["read"]),
            "get_message",
            json!({ "uid": "11" }),
        )
        .await;

    assert_same_json(
        &result,
        &json!({
            "mailbox": "INBOX",
            "uid": 11,
            "subject": "Lunch on Thursday?",
            "from": "Alice Martin <alice@example.com>",
            "to": ["Thomas <thomas@icloud.com>", "bob@example.com"],
            "date": "2026-10-01T09:30:00.000Z",
            "unread": true,
            "flagged": false,
            "answered": false,
            "cc": ["Carol <carol@example.com>"],
            "message_id": "<lunch@example.com>",
            "text": "Hi Thomas,\n\nAre you free on Thursday?\n\nAlice",
            "attachments": [{ "part": "2", "filename": "Menu été.pdf", "content_type": "application/pdf", "size": 57000 }],
        }),
    );
    // Only the part that holds the text was transferred.
    assert_eq!(
        scenario.icloud.downloads(),
        [Download {
            uid: 11,
            part: "1.1".into()
        }]
    );
    // Selected read-only, so reading cannot mark the message as read.
    assert_eq!(
        scenario.icloud.selections(),
        [Selection {
            path: "INBOX".into(),
            read_only: true
        }]
    );
    assert_eq!(
        scenario.icloud.mailbox("INBOX").messages[0].flags,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn converts_an_html_only_message_and_reports_text_cut_at_max_chars() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);
    let newsletter = scenario
        .data(&mcp, "get_message", json!({ "uid": 12 }))
        .await;
    assert_eq!(
        newsletter["text"],
        "Hello\n\nSee the deals [https://shop.example/deals]\n\nhttps://shop.example"
    );
    assert_eq!(newsletter.get("text_truncated"), None);
    // The body of a message made of one part is its text, which IMAP does not number.
    assert_eq!(
        scenario.icloud.downloads()[0],
        Download {
            uid: 12,
            part: "TEXT".into()
        }
    );

    let long = scenario
        .data(&mcp, "get_message", json!({ "uid": 14, "max_chars": 500 }))
        .await;
    let text = long["text"].as_str().unwrap();
    assert_eq!(text.encode_utf16().count(), 500);
    assert!(text.starts_with("Paid today.\n\nThanks. Thanks."));
    assert_eq!(long["text_truncated"], true);
    assert_eq!(
        scenario.icloud.downloads()[1],
        Download {
            uid: 14,
            part: "TEXT".into()
        }
    );
}

#[tokio::test]
async fn explains_an_unknown_message_mailbox_or_argument() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);

    assert_eq!(
        scenario
            .refusal(&mcp, "get_message", json!({ "uid": 99 }))
            .await,
        "Message 99 was not found in \"INBOX\". UIDs belong to one mailbox: call list_messages on it for the current ones."
    );
    for mailbox in ["Nope", "Projects"] {
        assert_eq!(
            scenario
                .refusal(&mcp, "list_messages", json!({ "mailbox": mailbox }))
                .await,
            format!(
                "Mailbox \"{mailbox}\" does not exist. Call list_mailboxes for the exact paths."
            )
        );
    }
    assert_eq!(
        scenario.refusal(&mcp, "get_message", json!({})).await,
        "uid is required"
    );
    assert_eq!(
        scenario
            .refusal(
                &mcp,
                "list_messages",
                json!({ "mailbox": "INBOX\r\nA1 DELETE INBOX" })
            )
            .await,
        "mailbox must be a single line of text"
    );
    assert_eq!(
        scenario
            .refusal(&mcp, "list_messages", json!({ "per_page": 500 }))
            .await,
        "per_page must be an integer between 1 and 50"
    );
    assert!(
        scenario
            .refusal(&mcp, "list_messages", json!({ "since": "yesterday" }))
            .await
            .contains("since must be an ISO")
    );
    assert!(scenario.provider().tool("archive_everything").is_none());
}

// Built-in iCloud Mail MCP: sending

#[tokio::test]
async fn sends_a_message_and_keeps_a_copy_with_its_blind_copies_in_sent() {
    let scenario = Scenario::new().await;
    let result = scenario
        .data(
            &scenario.full_access(),
            "send_message",
            json!({
                "to": ["dave@example.com", " Dave@example.com "],
                "bcc": "boss@example.com",
                "subject": "Quote for October",
                "text": "Hello Dave,\n\nHere is the quote.",
            }),
        )
        .await;

    let message_id = result["message_id"].as_str().unwrap().to_owned();
    let (uuid, domain) = message_id
        .strip_prefix('<')
        .unwrap()
        .strip_suffix('>')
        .unwrap()
        .split_once('@')
        .unwrap();
    assert_eq!((uuid.len(), domain), (36, "icloud.com"));
    assert!(
        uuid.chars()
            .all(|character| character == '-' || matches!(character, '0'..='9' | 'a'..='f')),
        "{uuid}"
    );
    assert_same_json(
        &result,
        &json!({
            "sent": true,
            "message_id": message_id,
            "from": "thomas@icloud.com",
            "subject": "Quote for October",
            "to": ["dave@example.com"],
            "bcc": ["boss@example.com"],
            "saved_to": "Sent Messages",
        }),
    );

    // The message as it was delivered: to everyone, without saying who was blind-copied.
    let sent = scenario.icloud.sent();
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.from, "thomas@icloud.com");
    assert_eq!(mail.to, ["dave@example.com", "boss@example.com"]);
    assert!(mail.raw.contains("From: thomas@icloud.com\r\n"));
    assert!(mail.raw.contains("To: dave@example.com\r\n"));
    assert!(!mail.raw.contains("Bcc:"), "{}", mail.raw);
    assert!(!mail.raw.contains("Cc:"), "{}", mail.raw);
    assert!(mail.raw.contains(&format!("Message-ID: {message_id}\r\n")));
    assert!(mail.raw.contains("Subject: Quote for October\r\n"));
    assert!(
        mail.raw
            .ends_with("\r\n\r\nHello Dave,\r\n\r\nHere is the quote.\r\n"),
        "{}",
        mail.raw
    );
    assert!(!mail.raw.contains("X-Mailer"));
    assert!(!mail.raw.contains("In-Reply-To") && !mail.raw.contains("References"));
    assert_eq!(scenario.icloud.smtp_sign_ins(), [sign_in()]);

    let copy = &scenario.icloud.mailbox("Sent Messages").appended[0];
    assert_eq!(copy.flags, ["\\Seen"]);
    assert!(copy.raw.contains("From: thomas@icloud.com\r\n"));
    assert!(copy.raw.contains("To: dave@example.com\r\n"));
    assert!(copy.raw.contains("Bcc: boss@example.com\r\n"));
    assert!(copy.raw.contains(&format!("Message-ID: {message_id}\r\n")));
    assert!(copy.raw.contains("Subject: Quote for October\r\n"));
    assert!(
        copy.raw
            .ends_with("\r\n\r\nHello Dave,\r\n\r\nHere is the quote.\r\n"),
        "{}",
        copy.raw
    );
    assert!(!copy.raw.contains("X-Mailer"));
}

#[tokio::test]
async fn answers_a_message_in_its_conversation_and_marks_it_answered() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let result = scenario
        .data(
            &mcp,
            "send_message",
            json!({
                "reply_to_uid": 11,
                "reply_all": true,
                "cc": ["erin@example.com", "alice@example.com"],
                "text": "Thursday works.",
            }),
        )
        .await;

    assert_eq!(
        (&result["subject"], &result["to"], &result["cc"]),
        (
            &json!("Re: Lunch on Thursday?"),
            &json!(["alice@example.com"]),
            &json!(["erin@example.com", "bob@example.com", "carol@example.com"])
        )
    );
    let sent = scenario.icloud.sent();
    assert!(sent[0].raw.contains("In-Reply-To: <lunch@example.com>\r\n"));
    assert!(
        sent[0]
            .raw
            .contains("References: <root@example.com> <lunch@example.com>\r\n")
    );
    assert_eq!(
        sent[0].to,
        [
            "alice@example.com",
            "erin@example.com",
            "bob@example.com",
            "carol@example.com"
        ]
    );
    assert!(
        scenario.icloud.mailbox("Sent Messages").appended[0]
            .raw
            .contains("References: <root@example.com> <lunch@example.com>\r\n")
    );
    assert_eq!(
        scenario.icloud.mailbox("INBOX").messages[0].flags,
        ["\\Answered"]
    );
    assert_eq!(
        scenario.icloud.selections(),
        [
            Selection {
                path: "INBOX".into(),
                read_only: true
            },
            Selection {
                path: "INBOX".into(),
                read_only: false
            }
        ]
    );

    // A follow-up on a sent message goes to the people it was sent to.
    let follow_up = scenario
        .data(
            &mcp,
            "send_message",
            json!({ "reply_to_uid": 5, "reply_to_mailbox": "Sent Messages", "text": "Any news?" }),
        )
        .await;
    assert_eq!(
        (&follow_up["subject"], &follow_up["to"]),
        (&json!("Re: Quote"), &json!(["dave@example.com"]))
    );
}

#[tokio::test]
async fn sends_from_another_address_of_the_account_only_when_the_administrator_allowed_it() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp_with_aliases(&PERMISSIONS, &["hello@thomas.example", "tt@icloud.com"]);
    let message = |from: &str| json!({ "to": ["dave@example.com"], "subject": "Hello", "text": "Hi", "from": from });

    let sent = scenario
        .data(&mcp, "send_message", message("HELLO@thomas.example"))
        .await;
    assert_eq!(sent["from"], "hello@thomas.example");
    assert!(
        sent["message_id"]
            .as_str()
            .unwrap()
            .ends_with("@thomas.example>")
    );
    assert_eq!(scenario.icloud.sent()[0].from, "hello@thomas.example");
    assert!(
        scenario.icloud.sent()[0]
            .raw
            .contains("From: hello@thomas.example\r\n")
    );
    assert!(
        scenario.icloud.mailbox("Sent Messages").appended[0]
            .raw
            .contains("From: hello@thomas.example\r\n")
    );
    // The account signs in as itself, whichever of its addresses the message is from.
    assert_eq!(scenario.icloud.smtp_sign_ins(), [sign_in()]);

    assert_eq!(
        scenario
            .refusal(&mcp, "send_message", message("ceo@bank.example"))
            .await,
        "from must be one of the sender addresses allowed for this MCP: thomas@icloud.com, hello@thomas.example, tt@icloud.com"
    );
    assert_eq!(scenario.icloud.sent().len(), 1);

    // A message written to an alias is answered from that alias.
    let answer = scenario
        .data(&mcp, "create_draft", json!({ "reply_to_uid": 3, "reply_to_mailbox": "Archive", "text": "Yes, tell me more." }))
        .await;
    assert_eq!(
        (&answer["from"], &answer["to"], &answer["subject"]),
        (
            &json!("hello@thomas.example"),
            &json!(["erin@example.com"]),
            &json!("Re: Website enquiry")
        )
    );

    let chosen = scenario
        .data(&mcp, "create_draft", json!({ "reply_to_uid": 3, "reply_to_mailbox": "Archive", "from": "tt@icloud.com", "text": "Yes." }))
        .await;
    assert_eq!(chosen["from"], "tt@icloud.com");
}

#[tokio::test]
async fn still_reports_a_delivered_message_when_its_copy_cannot_be_saved() {
    let scenario = Scenario::with(FakeOptions {
        fail_append: true,
        smtp: SmtpBehaviour::RejectRecipients {
            recipients: vec!["typo@example.invalid".into()],
            reply: "550 5.1.1 No such user".into(),
        },
        ..FakeOptions::default()
    })
    .await;
    let result = scenario
        .data(
            &scenario.full_access(),
            "send_message",
            json!({ "to": ["dave@example.com", "typo@example.invalid"], "subject": "Hello", "text": "Hi" }),
        )
        .await;

    assert_eq!(result["sent"], true);
    assert_eq!(
        result["warning"],
        "The message was sent, but its copy could not be saved to the Sent mailbox. Do not send it again."
    );
    assert_eq!(result["rejected"], json!(["typo@example.invalid"]));
    assert_eq!(result.get("saved_to"), None);
    assert_eq!(scenario.icloud.sent()[0].to, ["dave@example.com"]);
    let keys: Vec<&String> = result.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "sent",
            "message_id",
            "from",
            "subject",
            "to",
            "rejected",
            "warning"
        ]
    );
}

#[tokio::test]
async fn says_when_it_is_unknown_whether_icloud_accepted_the_message() {
    let message = json!({ "to": ["dave@example.com"], "subject": "Hello", "text": "Hi" });
    let outcome = async |smtp: SmtpBehaviour| {
        let scenario = Scenario::with(FakeOptions {
            smtp,
            ..FakeOptions::default()
        })
        .await;
        let refused = scenario
            .refusal(&scenario.full_access(), "send_message", message.clone())
            .await;
        assert_eq!(scenario.icloud.mailbox("Sent Messages").appended, []);
        assert!(!refused.contains(PASSWORD));
        refused
    };

    // The server hangs up once it has the message, without saying whether it took it.
    assert_eq!(
        outcome(SmtpBehaviour::HangUp).await,
        "iCloud Mail did not confirm the message, so it may or may not have been sent. Check with the user before sending it again."
    );
    assert_eq!(
        outcome(SmtpBehaviour::RejectAllRecipients {
            reply: "550 5.1.1 unknown recipient".into()
        })
        .await,
        "iCloud Mail rejected the recipients: 550 5.1.1 unknown recipient"
    );
    assert_eq!(
        outcome(SmtpBehaviour::RejectMessage {
            reply: "554 5.7.1 Message refused as spam".into()
        })
        .await,
        "iCloud Mail refused the message: 554 5.7.1 Message refused as spam"
    );
    assert!(
        outcome(SmtpBehaviour::RejectSignIn)
            .await
            .contains("iCloud Mail rejected the sign-in.")
    );

    // A name that is not one of a server: no label of a name is that long, so nothing is asked of the network.
    let scenario = Scenario::new().await;
    let nowhere = MailServer {
        host: format!("{}.invalid", "a".repeat(64)),
        port: 587,
        tls: false,
    };
    let unreachable = scenario.mcp_on(
        &PERMISSIONS,
        MailServers {
            smtp: nowhere,
            ..scenario.icloud.servers()
        },
    );
    assert_eq!(
        scenario
            .refusal(&unreachable, "send_message", message.clone())
            .await,
        "Could not reach iCloud Mail. Nothing was sent. Try again."
    );
    assert_eq!(scenario.icloud.mailbox("Sent Messages").appended, []);

    // Nothing listens there: the connection is refused before anything is said.
    let closed = MailServer {
        host: "127.0.0.1".into(),
        port: 1,
        tls: false,
    };
    let refused = scenario.mcp_on(
        &PERMISSIONS,
        MailServers {
            smtp: closed,
            ..scenario.icloud.servers()
        },
    );
    assert_eq!(
        scenario
            .refusal(&refused, "send_message", message.clone())
            .await,
        "iCloud Mail did not confirm the message, so it may or may not have been sent. Check with the user before sending it again."
    );
}

#[tokio::test]
async fn validates_a_message_before_reaching_icloud() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let text = async |arguments: Value| scenario.refusal(&mcp, "send_message", arguments).await;
    let message = |change: Value| {
        let mut message = json!({ "to": ["dave@example.com"], "subject": "Hello", "text": "Hi" });
        for (key, value) in change.as_object().unwrap() {
            if value.is_null() {
                message.as_object_mut().unwrap().remove(key);
            } else {
                message[key] = value.clone();
            }
        }
        message
    };

    assert_eq!(
        text(message(json!({ "text": "" }))).await,
        "text is required"
    );
    assert_eq!(
        text(message(json!({ "subject": null }))).await,
        "subject is required unless reply_to_uid is set"
    );
    assert_eq!(
        text(message(
            json!({ "subject": "Hello\r\nBcc: eve@example.com" })
        ))
        .await,
        "subject must be a single line of text"
    );
    for to in [
        json!(["Dave <dave@example.com>"]),
        json!(["dave@example.com, eve@example.com"]),
        json!(["dave"]),
        json!([42]),
    ] {
        let refused = text(message(json!({ "to": to }))).await;
        assert!(
            refused.contains("to must be a list of at most 50 email"),
            "{refused}"
        );
    }
    assert_eq!(scenario.icloud.sign_ins(), []);

    assert_eq!(
        text(message(json!({ "to": [] }))).await,
        "Add at least one recipient in to, cc, or bcc"
    );
    assert_eq!(scenario.icloud.sent(), []);
}

#[tokio::test]
async fn saves_a_draft_for_the_user_to_review_without_sending_it() {
    let scenario = Scenario::new().await;
    let result = scenario
        .data(
            &scenario.full_access(),
            "create_draft",
            json!({ "reply_to_uid": 14, "text": "Received, thank you." }),
        )
        .await;

    assert_same_json(
        &result,
        &json!({
            "saved_to": "Drafts",
            "uid": 101,
            "message_id": result["message_id"],
            "from": "thomas@icloud.com",
            "subject": "Re: Invoice 42",
            "to": ["andre@example.com"],
        }),
    );
    let draft = &scenario.icloud.mailbox("Drafts").appended[0];
    assert_eq!(draft.flags, ["\\Draft", "\\Seen"]);
    assert!(draft.raw.contains("In-Reply-To: <invoice@example.com>\r\n"));
    assert_eq!(scenario.icloud.sent(), []);
    // Only a message that was sent counts as an answer.
    assert!(
        !scenario.icloud.mailbox("INBOX").messages[2]
            .flags
            .contains(&"\\Answered".to_owned())
    );

    let blank = scenario
        .data(
            &scenario.full_access(),
            "create_draft",
            json!({ "subject": "Ideas", "text": "To finish later." }),
        )
        .await;
    assert_eq!(blank["to"], json!([]));
}

// Built-in iCloud Mail MCP: organizing

#[tokio::test]
async fn marks_the_messages_that_exist_as_read_unread_or_flagged() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let result = scenario
        .data(
            &mcp,
            "mark_messages",
            json!({ "uids": [11, "14", 11, 99], "unread": false, "flagged": true }),
        )
        .await;
    assert_same_json(
        &result,
        &json!({ "mailbox": "INBOX", "uids": [11, 14], "unread": false, "flagged": true }),
    );
    assert_eq!(scenario.icloud.searches(), ["UID 11,14,99"]);
    assert_eq!(
        scenario.icloud.mailbox("INBOX").messages[0].flags,
        ["\\Seen", "\\Flagged"]
    );
    assert_eq!(
        scenario.icloud.selections(),
        [Selection {
            path: "INBOX".into(),
            read_only: false
        }]
    );

    scenario
        .data(
            &mcp,
            "mark_messages",
            json!({ "uids": [14], "unread": true, "flagged": false }),
        )
        .await;
    assert_eq!(
        scenario.icloud.mailbox("INBOX").messages[2].flags,
        Vec::<String>::new()
    );

    let text = async |arguments: Value| scenario.refusal(&mcp, "mark_messages", arguments).await;
    assert_eq!(
        text(json!({ "uids": [11] })).await,
        "Set unread, flagged, or both"
    );
    assert_eq!(
        text(json!({ "unread": true })).await,
        "uids must be a list of 1 to 100 message UIDs"
    );
    assert_eq!(
        text(json!({ "uids": [98, 99], "flagged": true })).await,
        "None of these UIDs exist in \"INBOX\". UIDs belong to one mailbox: call list_messages on it for the current ones."
    );
}

#[tokio::test]
async fn moves_messages_to_another_mailbox_and_returns_their_new_uids() {
    let scenario = Scenario::new().await;
    let mcp = scenario.full_access();
    let result = scenario
        .data(
            &mcp,
            "move_messages",
            json!({ "uids": [12, 14, 99], "destination": "Deleted Messages" }),
        )
        .await;

    assert_same_json(
        &result,
        &json!({
            "mailbox": "INBOX",
            "destination": "Deleted Messages",
            "uids": [12, 14],
            "new_uids": { "12": 1, "14": 2 },
        }),
    );
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

    assert_eq!(
        scenario
            .refusal(
                &mcp,
                "move_messages",
                json!({ "uids": [11], "destination": "Nope" })
            )
            .await,
        "iCloud Mail could not move these messages to \"Nope\". Call list_mailboxes for the exact destination path."
    );
    assert_eq!(
        scenario
            .refusal(&mcp, "move_messages", json!({ "uids": [11] }))
            .await,
        "destination is required"
    );
}

// Built-in iCloud Mail MCP: sign-in

const SIGN_IN_REJECTED: &str = "iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.";

#[tokio::test]
async fn proves_the_saved_password_with_one_sign_in() {
    let scenario = Scenario::new().await;
    scenario
        .provider()
        .verify(scenario.mcp(&["read"]))
        .await
        .unwrap();

    assert_eq!(scenario.icloud.sign_ins(), [sign_in()]);
    assert_eq!(scenario.icloud.logouts(), 1);
    // Proving the sign-in looks at no mail.
    assert_eq!(scenario.icloud.commands(), ["NAMESPACE", "LOGOUT"]);
}

#[tokio::test]
async fn reports_a_rejected_password_as_an_error_to_fix() {
    let scenario = Scenario::with(FakeOptions {
        reject_sign_in: true,
        ..FakeOptions::default()
    })
    .await;
    let mcp = scenario.mcp(&["read"]);

    let error = scenario.provider().verify(mcp.clone()).await.unwrap_err();
    assert!(error.is_authorization_error());
    assert_eq!(error.to_string(), SIGN_IN_REJECTED);

    let error = scenario
        .call(&mcp, "list_mailboxes", json!({}))
        .await
        .unwrap_err();
    assert!(error.is_authorization_error());
    assert_eq!(error.to_string(), SIGN_IN_REJECTED);
    // Each attempt tries both usernames Apple documents, and nothing else.
    assert_eq!(
        usernames(&scenario),
        ["thomas@icloud.com", "thomas", "thomas@icloud.com", "thomas"]
    );
    assert_eq!(scenario.icloud.logouts(), 0);
}

#[tokio::test]
async fn signs_in_with_the_name_before_the_at_when_the_full_address_is_refused() {
    let scenario = Scenario::with(FakeOptions {
        imap_username: Some("thomas".into()),
        ..FakeOptions::default()
    })
    .await;
    let mcp = scenario.mcp(&["read"]);
    scenario.provider().verify(mcp.clone()).await.unwrap();
    assert_eq!(usernames(&scenario), ["thomas@icloud.com", "thomas"]);

    // The form that worked is remembered, so iCloud sees no more failed sign-ins.
    scenario.data(&mcp, "list_mailboxes", json!({})).await;
    assert_eq!(
        usernames(&scenario),
        ["thomas@icloud.com", "thomas", "thomas"]
    );
    assert_eq!(scenario.icloud.logouts(), 2);
}

#[tokio::test]
async fn says_when_icloud_cannot_be_reached_or_refuses_a_request() {
    let scenario = Scenario::new().await;
    let closed = MailServer {
        host: "127.0.0.1".into(),
        port: 1,
        tls: false,
    };
    let unreachable = scenario.mcp_on(
        &["read"],
        MailServers {
            imap: closed,
            ..scenario.icloud.servers()
        },
    );
    let error = scenario
        .call(&unreachable, "list_mailboxes", json!({}))
        .await
        .unwrap_err();
    assert!(error.is_tool_error() && !error.is_authorization_error());
    assert_eq!(error.to_string(), "Could not reach iCloud Mail. Try again.");

    // What the server answers to a command it refuses is told to the agent.
    let failing = Scenario::with(FakeOptions {
        fail_append: true,
        ..FakeOptions::default()
    })
    .await;
    assert_eq!(
        failing
            .refusal(
                &failing.full_access(),
                "create_draft",
                json!({ "subject": "Ideas", "text": "Later." })
            )
            .await,
        "iCloud Mail refused the request: APPEND failed"
    );
}
