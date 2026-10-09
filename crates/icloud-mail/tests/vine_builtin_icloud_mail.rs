//! The port of `tests/unit/vine_builtin_icloud_mail.spec.ts`.

mod support;

use chrono::{DateTime, Utc};
use mymcps_builtin::BuiltinResult;
use mymcps_builtin::tool_input::tool_input_with;
use mymcps_icloud_mail::message::is_address;
use mymcps_icloud_mail::validators::{
    ATTACHMENT_REFERENCE_VALIDATOR, COMPOSITION_VALIDATOR, CREATE_UPLOAD_LINK_VALIDATOR,
    GET_ATTACHMENT_LINK_VALIDATOR, GET_MESSAGE_VALIDATOR, LIST_MESSAGES_VALIDATOR, ListMessages,
    MARK_MESSAGES_VALIDATOR, MOVE_MESSAGES_VALIDATOR, Senders, UPLOAD_REFERENCE_VALIDATOR,
};
use mymcps_vine::Validator;
use serde_json::{Value, json};
use support::Scenario;

const ADDRESSES: &str =
    "must be a list of at most 50 email addresses such as name@example.com, without display names";
const ATTACHMENTS: &str =
    "attachments must be a list of at most 10 upload IDs, as returned by create_upload_link";
const UPLOAD_ID: &str = "3f2b8c1e-7a4d-4e9b-9c55-0a1b2c3d4e5f";

/// The account the composition tools run for.
fn account() -> Senders {
    Senders {
        username: "thomas@icloud.com".into(),
        aliases: vec!["Hello@Thomas.example".into()],
    }
}

fn input(validator: &Validator, arguments: &Value) -> BuiltinResult<Value> {
    tool_input_with(validator, arguments, &account())
}

/// The sentence the agent reads when a tool refuses its arguments.
fn refusal(validator: &Validator, arguments: &Value) -> Option<String> {
    match input(validator, arguments) {
        Ok(_) => None,
        Err(error) => {
            assert!(error.is_tool_error(), "{error}");
            Some(error.to_string())
        }
    }
}

/// `{ ...base, ...change }`
fn with(base: &Value, change: Value) -> Value {
    let mut merged = base.clone();
    for (key, value) in change.as_object().unwrap() {
        merged[key] = value.clone();
    }
    merged
}

fn mail() -> Value {
    json!({ "subject": "Hi", "text": "Hello" })
}

// Built-in iCloud Mail MCP: validators

#[test]
fn returns_the_arguments_of_each_tool_the_way_the_tool_uses_them() {
    let cases: Vec<(&Validator, Value, Value)> = vec![
        (&LIST_MESSAGES_VALIDATOR, json!({}), json!({})),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "mailbox": " Archive ", "page": "2", "per_page": 50, "from": " alice ", "subject": "", "other": 1 }),
            json!({ "mailbox": "Archive", "page": 2, "per_page": 50, "from": "alice" }),
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "unread": "true", "flagged": false, "to": "   ", "text": null }),
            json!({ "unread": true, "flagged": false }),
        ),
        (
            &GET_MESSAGE_VALIDATOR,
            json!({ "uid": "11", "max_chars": 500 }),
            json!({ "uid": 11, "max_chars": 500 }),
        ),
        (
            &GET_ATTACHMENT_LINK_VALIDATOR,
            json!({ "mailbox": "INBOX", "uid": 11, "part": 2, "expires_in_minutes": "60" }),
            json!({ "mailbox": "INBOX", "uid": 11, "part": "2", "expires_in_minutes": 60 }),
        ),
        (
            &ATTACHMENT_REFERENCE_VALIDATOR,
            json!({ "mailbox": "INBOX", "uid": 11, "part": "1.2", "expires": 1 }),
            json!({ "mailbox": "INBOX", "uid": 11, "part": "1.2" }),
        ),
        (
            &MARK_MESSAGES_VALIDATOR,
            json!({ "uids": [11, "12", " 14 ", 11], "unread": false }),
            json!({ "uids": [11, 12, 14, 11], "unread": false }),
        ),
        (
            &MOVE_MESSAGES_VALIDATOR,
            json!({ "uids": [11], "destination": " Archive " }),
            json!({ "uids": [11], "destination": "Archive" }),
        ),
    ];

    for (validator, arguments, expected) in cases {
        let validated = input(validator, &arguments).unwrap();
        assert_eq!(validated, expected, "{arguments}");
        assert_eq!(validated.to_string(), expected.to_string(), "{arguments}");
    }

    let dates: ListMessages = tool_input_with(
        &LIST_MESSAGES_VALIDATOR,
        &json!({ "since": "2026-10-01", "before": "2026-10-02T00:00:00+02:00" }),
        &account(),
    )
    .unwrap();
    let instant = |text: &str| {
        Some(
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc),
        )
    };
    assert_eq!(dates.since, instant("2026-10-01T00:00:00.000Z"));
    assert_eq!(dates.before, instant("2026-10-01T22:00:00.000Z"));
}

#[test]
fn tells_the_agent_which_argument_is_wrong_and_what_it_must_be() {
    let cases: Vec<(&Validator, Value, &str)> = vec![
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "mailbox": 5 }),
            "mailbox must be text of at most 255 characters",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "mailbox": "a\r\nb" }),
            "mailbox must be a single line of text",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "page": 0 }),
            "page must be an integer of at least 1",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "per_page": 51 }),
            "per_page must be an integer between 1 and 50",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "from": "x".repeat(201) }),
            "from must be text of at most 200 characters",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "subject": "a\nb" }),
            "subject must be a single line of text",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "since": "yesterday" }),
            "since must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z",
        ),
        (
            &LIST_MESSAGES_VALIDATOR,
            json!({ "unread": "yes" }),
            "unread must be true or false",
        ),
        (&GET_MESSAGE_VALIDATOR, json!({}), "uid is required"),
        (
            &GET_MESSAGE_VALIDATOR,
            json!({ "uid": 0 }),
            "uid must be an integer between 1 and 4294967295",
        ),
        (
            &GET_MESSAGE_VALIDATOR,
            json!({ "uid": 4_294_967_296_u64 }),
            "uid must be an integer between 1 and 4294967295",
        ),
        (
            &GET_MESSAGE_VALIDATOR,
            json!({ "uid": 1, "max_chars": 499 }),
            "max_chars must be an integer between 500 and 100000",
        ),
        (
            &GET_ATTACHMENT_LINK_VALIDATOR,
            json!({ "uid": 1 }),
            "part is required",
        ),
        (
            &GET_ATTACHMENT_LINK_VALIDATOR,
            json!({ "uid": 1, "part": "2/../3" }),
            "part must be the part of an attachment, such as 2, as returned by get_message",
        ),
        (
            &GET_ATTACHMENT_LINK_VALIDATOR,
            json!({ "uid": 1, "part": "2", "expires_in_minutes": 61 }),
            "expires_in_minutes must be an integer between 1 and 60",
        ),
        (
            &MARK_MESSAGES_VALIDATOR,
            json!({ "uids": [1] }),
            "Set unread, flagged, or both",
        ),
        (
            &MARK_MESSAGES_VALIDATOR,
            json!({ "uids": [1], "unread": "", "flagged": null }),
            "Set unread, flagged, or both",
        ),
        (
            &MARK_MESSAGES_VALIDATOR,
            json!({ "uids": [1], "unread": "no" }),
            "unread must be true or false",
        ),
        (
            &MARK_MESSAGES_VALIDATOR,
            json!({ "uids": [1], "flagged": 0 }),
            "flagged must be true or false",
        ),
        (
            &MOVE_MESSAGES_VALIDATOR,
            json!({ "uids": [1] }),
            "destination is required",
        ),
        (
            &MOVE_MESSAGES_VALIDATOR,
            json!({ "uids": [1], "destination": "  " }),
            "destination is required",
        ),
        (
            &MOVE_MESSAGES_VALIDATOR,
            json!({ "uids": [1], "destination": "a\nb" }),
            "destination must be a single line of text",
        ),
        (
            &ATTACHMENT_REFERENCE_VALIDATOR,
            json!({ "mailbox": "INBOX", "part": "2" }),
            "uid is required",
        ),
        (
            &ATTACHMENT_REFERENCE_VALIDATOR,
            Value::Null,
            "arguments is required",
        ),
        (
            &ATTACHMENT_REFERENCE_VALIDATOR,
            json!("INBOX/11/2"),
            "arguments must be an object",
        ),
    ];

    for (validator, arguments, sentence) in cases {
        assert_eq!(
            refusal(validator, &arguments).as_deref(),
            Some(sentence),
            "{arguments}"
        );
    }
}

#[test]
fn takes_1_to_100_message_uids_and_says_which_of_the_two_is_wrong() {
    let list = "uids must be a list of 1 to 100 message UIDs";
    let hundred: Vec<u32> = (1..=100).collect();

    assert_eq!(
        input(
            &MARK_MESSAGES_VALIDATOR,
            &json!({ "uids": hundred, "flagged": true })
        )
        .unwrap(),
        json!({ "uids": hundred, "flagged": true })
    );
    let mut too_many = hundred.clone();
    too_many.push(101);
    // The first stands for an argument that was left out.
    assert_eq!(
        refusal(&MARK_MESSAGES_VALIDATOR, &json!({})).as_deref(),
        Some(list)
    );
    for uids in [
        Value::Null,
        json!(""),
        json!(11),
        json!("11"),
        json!({}),
        json!([]),
        json!(too_many),
    ] {
        assert_eq!(
            refusal(&MARK_MESSAGES_VALIDATOR, &json!({ "uids": uids })).as_deref(),
            Some(list),
            "{uids}"
        );
    }
    assert_eq!(
        refusal(&MARK_MESSAGES_VALIDATOR, &json!({ "uids": [11, 0] })).as_deref(),
        Some("uids must be an integer between 1 and 4294967295")
    );
    assert_eq!(
        refusal(&MARK_MESSAGES_VALIDATOR, &json!({ "uids": [11, null] })).as_deref(),
        Some("uids is required")
    );
    assert_eq!(
        refusal(&MARK_MESSAGES_VALIDATOR, &json!({ "uids": [""] })).as_deref(),
        Some("uids is required")
    );
}

// Built-in iCloud Mail MCP: validators of an upload

#[test]
fn names_the_file_the_way_the_recipient_sees_it() {
    assert_eq!(
        input(
            &CREATE_UPLOAD_LINK_VALIDATOR,
            &json!({
                "filename": " Devis été 2026.pdf ",
                "content_type": " application/pdf ",
                "expires_in_minutes": "30",
                "path": "/etc/passwd",
            })
        )
        .unwrap(),
        json!({ "filename": "Devis été 2026.pdf", "content_type": "application/pdf", "expires_in_minutes": 30 })
    );
    assert_eq!(
        input(
            &CREATE_UPLOAD_LINK_VALIDATOR,
            &json!({ "filename": "notes", "content_type": "" })
        )
        .unwrap(),
        json!({ "filename": "notes" })
    );

    let folder = "filename must be the name of the file, such as report.pdf, without its folder";
    let media_type = "content_type must be a media type, such as application/pdf";
    let cases = [
        (json!({}), "filename is required"),
        (json!({ "filename": "   " }), "filename is required"),
        (json!({ "filename": "/tmp/report.pdf" }), folder),
        (json!({ "filename": "..\\report.pdf" }), folder),
        (
            json!({ "filename": "a\r\nContent-Type: text/html" }),
            "filename must be a single line of text",
        ),
        (
            json!({ "filename": format!("{}.pdf", "n".repeat(252)) }),
            "filename must be text of at most 255 characters",
        ),
        (
            json!({ "filename": 42 }),
            "filename must be text of at most 255 characters",
        ),
        (
            json!({ "filename": "a.pdf", "content_type": "pdf" }),
            media_type,
        ),
        (
            json!({ "filename": "a.pdf", "content_type": "text/html; charset=utf-8" }),
            media_type,
        ),
        (
            json!({ "filename": "a.pdf", "content_type": "text/plain\r\nBcc: eve@example.com" }),
            media_type,
        ),
        (
            json!({ "filename": "a.pdf", "content_type": format!("application/{}", "x".repeat(101)) }),
            media_type,
        ),
        (
            json!({ "filename": "a.pdf", "expires_in_minutes": 61 }),
            "expires_in_minutes must be an integer between 1 and 60",
        ),
    ];
    for (arguments, sentence) in cases {
        assert_eq!(
            refusal(&CREATE_UPLOAD_LINK_VALIDATOR, &arguments).as_deref(),
            Some(sentence),
            "{arguments}"
        );
    }
}

#[test]
fn reads_back_what_a_link_was_made_for_and_nothing_else() {
    assert_eq!(
        input(
            &UPLOAD_REFERENCE_VALIDATOR,
            &json!({ "upload": UPLOAD_ID, "filename": "report.pdf", "content_type": "application/pdf", "mailbox": "INBOX" })
        )
        .unwrap(),
        json!({ "upload": UPLOAD_ID, "filename": "report.pdf", "content_type": "application/pdf" })
    );

    let references = [
        Value::Null,
        json!(UPLOAD_ID),
        json!({ "upload": UPLOAD_ID }),
        json!({ "filename": "report.pdf" }),
        json!({ "upload": "../../db.sqlite3", "filename": "report.pdf" }),
        json!({ "upload": format!("{UPLOAD_ID}/.."), "filename": "report.pdf" }),
        json!({ "upload": UPLOAD_ID.replace('-', ""), "filename": "report.pdf" }),
        json!({ "upload": UPLOAD_ID, "filename": "../report.pdf" }),
        json!({ "upload": UPLOAD_ID, "filename": "report.pdf", "content_type": "pdf" }),
        // What get_attachment_link puts in a download link.
        json!({ "mailbox": "INBOX", "uid": 11, "part": "2" }),
    ];
    for reference in references {
        assert!(
            refusal(&UPLOAD_REFERENCE_VALIDATOR, &reference).is_some(),
            "{reference}"
        );
    }
}

// Built-in iCloud Mail MCP: validators of a message to write

fn compose(arguments: Value) -> Value {
    input(&COMPOSITION_VALIDATOR, &arguments).unwrap()
}

#[test]
fn takes_one_upload_id_or_a_list_of_at_most_ten() {
    let ids: Vec<String> = (0..10)
        .map(|index| format!("{}{index}", &UPLOAD_ID[..UPLOAD_ID.len() - 1]))
        .collect();

    assert_eq!(
        compose(with(
            &mail(),
            json!({ "attachments": format!(" {} ", UPLOAD_ID.to_uppercase()) })
        )),
        with(&mail(), json!({ "attachments": [UPLOAD_ID] }))
    );
    assert_eq!(
        compose(with(&mail(), json!({ "attachments": ids }))),
        with(&mail(), json!({ "attachments": ids }))
    );
    assert_eq!(
        compose(with(&mail(), json!({ "attachments": [] }))),
        with(&mail(), json!({ "attachments": [] }))
    );
    assert_eq!(compose(mail()), mail());
    for none in [Value::Null, json!("")] {
        assert_eq!(
            compose(with(&mail(), json!({ "attachments": none }))),
            mail()
        );
    }

    let mut eleven = ids.clone();
    eleven.push(UPLOAD_ID.to_owned());
    let refused = [
        json!("report.pdf"),
        json!("/tmp/report.pdf"),
        json!("https://example.com/report.pdf"),
        json!([UPLOAD_ID, "../../db.sqlite3"]),
        json!([UPLOAD_ID, null]),
        json!([{ "path": "/etc/passwd" }]),
        json!([{ "filename": "a.txt", "content": "aGk=" }]),
        json!(42),
        json!(eleven),
    ];
    for attachments in refused {
        assert_eq!(
            refusal(
                &COMPOSITION_VALIDATOR,
                &with(&mail(), json!({ "attachments": attachments }))
            )
            .as_deref(),
            Some(ATTACHMENTS),
            "{attachments}"
        );
    }
}

#[test]
fn takes_one_address_or_a_list_trimmed_and_nothing_but_bare_addresses() {
    assert_eq!(
        compose(with(
            &mail(),
            json!({ "to": " bob@example.com ", "cc": [], "bcc": "" })
        )),
        with(&mail(), json!({ "to": ["bob@example.com"], "cc": [] }))
    );
    assert_eq!(
        compose(with(
            &mail(),
            json!({ "to": ["bob@example.com", " Bob@Example.com "], "bcc": null })
        )),
        with(
            &mail(),
            json!({ "to": ["bob@example.com", "Bob@Example.com"] })
        )
    );

    let fifty: Vec<String> = (0..50)
        .map(|index| format!("user{index}@example.com"))
        .collect();
    assert_eq!(
        compose(with(&mail(), json!({ "bcc": fifty })))["bcc"]
            .as_array()
            .unwrap()
            .len(),
        50
    );

    let mut one_more = fifty.clone();
    one_more.push("one@more.example".to_owned());
    for name in ["to", "cc", "bcc"] {
        for value in [
            json!("Bob <bob@example.com>"),
            json!("bob@example.com, carol@example.com"),
            json!(["bob@example.com", "carol"]),
            json!(["bob@example.com", null]),
            json!(["bob@example.com", ["carol@example.com"]]),
            json!([""]),
            json!(5),
            json!({}),
            json!(one_more),
        ] {
            let mut arguments = mail();
            arguments[name] = value.clone();
            assert_eq!(
                refusal(&COMPOSITION_VALIDATOR, &arguments),
                Some(format!("{name} {ADDRESSES}")),
                "{value}"
            );
        }
    }
}

#[test]
fn checks_an_address_with_the_pattern_that_checks_the_addresses_of_a_message() {
    let addresses = [
        "bob@example.com".to_owned(),
        "bob+tag@sub.example.co".to_owned(),
        "bob@example".to_owned(),
        "bob@.example.com".to_owned(),
        "bob@example..com".to_owned(),
        "bob@exa mple.com".to_owned(),
        "\"bob\"@example.com".to_owned(),
        "bob@[127.0.0.1]".to_owned(),
        "bob@example.com.".to_owned(),
        format!("{}@example.com", "b".repeat(242)),
        format!("{}@example.com", "b".repeat(243)),
    ];

    for address in &addresses {
        let is_accepted = refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "to": [address] })),
        )
        .is_none();
        assert_eq!(is_accepted, is_address(address), "{address}");
    }
    let accepted = addresses
        .iter()
        .filter(|address| is_address(address))
        .count();
    assert!(accepted > 2);
    assert!(accepted < addresses.len() - 2);
}

#[test]
fn sends_from_an_address_of_the_account_only_in_the_spelling_that_was_saved() {
    let sender = |from: &str| compose(with(&mail(), json!({ "from": from })))["from"].clone();
    assert_eq!(sender(" THOMAS@icloud.com "), "thomas@icloud.com");
    assert_eq!(sender("hello@thomas.example"), "Hello@Thomas.example");
    assert_eq!(
        compose(with(&mail(), json!({ "from": "  " }))).get("from"),
        None
    );

    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "from": "boss@example.com" }))
        )
        .as_deref(),
        Some(
            "from must be one of the sender addresses allowed for this MCP: thomas@icloud.com, Hello@Thomas.example"
        )
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "from": "thomas@icloud.com\nBcc: x@y.z" }))
        )
        .as_deref(),
        Some("from must be a single line of text")
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "from": "x".repeat(255) }))
        )
        .as_deref(),
        Some("from must be text of at most 254 characters")
    );
}

#[test]
fn requires_a_subject_unless_the_message_answers_another() {
    let sentence = Some("subject is required unless reply_to_uid is set");

    assert_eq!(
        refusal(&COMPOSITION_VALIDATOR, &json!({ "text": "Hello" })).as_deref(),
        sentence
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "text": "Hello", "subject": "  " })
        )
        .as_deref(),
        sentence
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "text": "Hello", "subject": null, "reply_to_uid": "" })
        )
        .as_deref(),
        sentence
    );
    let answer = compose(json!({ "text": "Yes", "reply_to_uid": "11" }));
    assert_eq!(answer, json!({ "reply_to_uid": 11, "text": "Yes" }));
    assert_eq!(
        answer.to_string(),
        json!({ "reply_to_uid": 11, "text": "Yes" }).to_string()
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "subject": "Re: a\nBcc: x@y.z" }))
        )
        .as_deref(),
        Some("subject must be a single line of text")
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "subject": "x".repeat(256) }))
        )
        .as_deref(),
        Some("subject must be text of at most 255 characters")
    );
}

#[test]
fn requires_a_text_of_which_spaces_alone_are_one() {
    assert_eq!(
        refusal(&COMPOSITION_VALIDATOR, &json!({ "subject": "Hi" })).as_deref(),
        Some("text is required")
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "subject": "Hi", "text": "" })
        )
        .as_deref(),
        Some("text is required")
    );
    assert_eq!(
        compose(json!({ "subject": "Hi", "text": " \n" })),
        json!({ "subject": "Hi", "text": " \n" })
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "subject": "Hi", "text": "x".repeat(100_001) })
        )
        .as_deref(),
        Some("text must be text of at most 100000 characters")
    );
}

#[test]
fn only_looks_at_what_describes_the_reply_when_there_is_a_message_to_answer() {
    assert_eq!(
        compose(with(
            &mail(),
            json!({ "reply_to_mailbox": 5, "reply_all": "maybe" })
        )),
        mail()
    );
    assert_eq!(
        compose(
            json!({ "text": "Yes", "reply_to_uid": 5, "reply_to_mailbox": " Sent ", "reply_all": "true" })
        ),
        json!({ "reply_to_uid": 5, "text": "Yes", "reply_to_mailbox": "Sent", "reply_all": true })
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "text": "Yes", "reply_to_uid": 5, "reply_all": "maybe" })
        )
        .as_deref(),
        Some("reply_all must be true or false")
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &json!({ "text": "Yes", "reply_to_uid": 5, "reply_to_mailbox": 5 })
        )
        .as_deref(),
        Some("reply_to_mailbox must be text of at most 255 characters")
    );
    assert_eq!(
        refusal(
            &COMPOSITION_VALIDATOR,
            &with(&mail(), json!({ "reply_to_uid": 0 }))
        )
        .as_deref(),
        Some("reply_to_uid must be an integer between 1 and 4294967295")
    );
}

// Built-in iCloud Mail MCP: validated tools

#[tokio::test]
async fn names_a_uid_or_an_address_once_however_often_the_agent_names_it() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["draft", "organize"]);

    scenario
        .data(
            &mcp,
            "mark_messages",
            json!({ "uids": [11, "11", 12, " 12 "], "flagged": true }),
        )
        .await;
    assert_eq!(scenario.icloud.searches(), ["UID 11,12"]);

    let saved = scenario
        .data(
            &mcp,
            "create_draft",
            json!({
                "subject": "Hi",
                "text": "Hello",
                "to": ["bob@example.com", "BOB@example.com"],
                "cc": ["Bob@example.com", "carol@example.com"],
            }),
        )
        .await;
    assert_eq!(saved["to"], json!(["bob@example.com"]));
    assert_eq!(saved["cc"], json!(["carol@example.com"]));
}

#[tokio::test]
async fn refuses_to_answer_a_message_without_the_read_permission_once_the_arguments_are_right() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["draft"]);

    let refused = scenario
        .refusal(
            &mcp,
            "create_draft",
            json!({ "reply_to_uid": 11, "text": "Yes" }),
        )
        .await;
    assert!(refused.contains("reply_to_uid reads the message being answered, and the \"read\" permission is not allowed"), "{refused}");
    assert_eq!(
        scenario
            .refusal(&mcp, "create_draft", json!({ "reply_to_uid": 11 }))
            .await,
        "text is required"
    );
    assert_eq!(scenario.icloud.sign_ins(), []);
}

#[tokio::test]
async fn serves_a_file_only_for_a_reference_a_link_could_hold() {
    let scenario = Scenario::new().await;
    let mcp = scenario.mcp(&["read"]);

    let file = scenario
        .provider()
        .download_file(
            json!({ "mailbox": "INBOX", "uid": "11", "part": 2 }),
            mcp.clone(),
        )
        .await
        .unwrap();
    assert_eq!(file.filename, "Menu été.pdf");

    for reference in [
        Value::Null,
        json!("INBOX"),
        json!([]),
        json!({ "uid": 11 }),
        json!({ "uid": "x", "part": "2" }),
    ] {
        let error = scenario
            .provider()
            .download_file(reference.clone(), mcp.clone())
            .await
            .unwrap_err();
        assert!(error.is_tool_error(), "{reference}: {error}");
    }
    assert_eq!(scenario.icloud.sign_ins().len(), 1);
}
