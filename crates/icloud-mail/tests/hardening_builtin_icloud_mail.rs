//! The port of `tests/unit/hardening_builtin_icloud_mail.spec.ts`: what a
//! message written by a stranger may cost the server, and tell the agent.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use mymcps_icloud_mail::html::{Converted, HtmlConversion, HtmlConverter};
use mymcps_icloud_mail::message::{
    Address, BodyStructure, Envelope, FetchedMessage, attachments_of, is_address, message_headers,
    message_summary, reply_to, tidy_text,
};
use mymcps_icloud_mail::testing::{Download, FakeMessage, attached, leaf, multipart};
use mymcps_vine::js;
use regex::Regex;
use serde_json::{Value, json};
use support::{PERMISSIONS, Scenario, assert_same_json};

/// Longer than any step of a tool call may keep the server from answering. The
/// steps measured here take a few milliseconds in a release build, and took
/// seconds in the Node app before they were hardened. A debug build runs
/// them some ten times slower.
const MAX_PAUSE: Duration = Duration::from_millis(if cfg!(debug_assertions) { 2500 } else { 500 });

/// The longest the runtime went without running a timer while `run` was
/// pending. The timer shares the thread of the test with `run`: whatever
/// keeps that thread busy delays it.
async fn longest_pause<T>(run: impl Future<Output = T>) -> (T, Duration) {
    let ticks = Arc::new(Mutex::new((Duration::ZERO, Instant::now())));
    let ticker = tokio::spawn({
        let ticks = ticks.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let mut ticks = ticks.lock().unwrap();
                let now = Instant::now();
                ticks.0 = ticks.0.max(now - ticks.1);
                ticks.1 = now;
            }
        }
    });

    let result = run.await;
    ticker.abort();
    let ticks = ticks.lock().unwrap();
    (result, ticks.0.max(ticks.1.elapsed()))
}

fn elapsed<T>(run: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let result = run();
    (result, start.elapsed())
}

/// `count` distinct people at example.com.
fn people(count: usize, prefix: &str) -> Vec<Address> {
    (0..count)
        .map(|index| {
            Address::new(
                format!("{prefix} {index}"),
                format!("{prefix}{index}@example.com"),
            )
        })
        .collect()
}

fn conversion(timeout: Duration) -> HtmlConversion {
    HtmlConversion { timeout }
}

// Built-in iCloud Mail hardening: message text

/// What tidyText did before its patterns were made linear.
fn original(text: &str) -> String {
    static LINE_ENDINGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n?").unwrap());
    static PADDING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new("(?:[\u{ad}\u{34f}\u{200b}-\u{200d}\u{2007}\u{feff}][ \u{a0}]*){3,}").unwrap()
    });
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+\n").unwrap());
    static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

    let text = LINE_ENDINGS.replace_all(text, "\n");
    let text = PADDING.replace_all(&text, "");
    let text = TRAILING.replace_all(&text, "\n");
    let text = BLANK_LINES.replace_all(&text, "\n\n");
    js::trim(&text).to_owned()
}

#[test]
fn tidies_a_long_run_of_spaces_or_tabs_in_linear_time() {
    // 80,000 spaces took the first pattern of the Node app close to three
    // seconds, and twice as many four times as long.
    let (spaces, time) = elapsed(|| tidy_text(&format!("Hello{}", " ".repeat(160_000))));
    assert_eq!(spaces, "Hello");
    assert!(time < MAX_PAUSE, "{time:?}");

    let (tabs, time) = elapsed(|| {
        tidy_text(&format!(
            "{}x{}y \n z",
            "\t".repeat(80_000),
            " \t".repeat(80_000)
        ))
    });
    assert_eq!(tabs, format!("x{}y\n z", " \t".repeat(80_000)));
    assert!(time < MAX_PAUSE, "{time:?}");

    // The most text a call can ask for.
    let (longest, time) = elapsed(|| tidy_text(&" ".repeat(400_000)));
    assert_eq!(longest, "");
    assert!(time < MAX_PAUSE, "{time:?}");
}

#[test]
fn removes_preview_padding_of_any_length() {
    // Matched one repetition at a time, this many overflowed the regex stack of V8.
    let (padding, time) =
        elapsed(|| tidy_text(&format!("Sale{}today", "\u{200c}".repeat(5_000_000))));
    assert_eq!(padding, "Saletoday");
    assert!(time < MAX_PAUSE, "{time:?}");

    let (spaced, time) =
        elapsed(|| tidy_text(&format!("Sale{}today", "\u{200c}\u{a0}".repeat(200_000))));
    assert_eq!(spaced, "Saletoday");
    assert!(time < MAX_PAUSE, "{time:?}");

    assert_eq!(tidy_text("a\u{200b} \u{200b}b"), "a\u{200b} \u{200b}b");
    assert_eq!(tidy_text("a\u{200b} \u{200b}\u{a0}\u{200b}  b"), "ab");
}

#[test]
fn tidies_exactly_like_the_patterns_it_replaces() {
    let samples = [
        "Hi Thomas,\r\n\r\nAre you free on Thursday?\r\n\r\nAlice".to_owned(),
        "One  \r\n\r\n\r\n\r\nTwo\r\n".to_owned(),
        "Preview\u{a0}\u{200c}\u{a0}\u{200c}\u{a0}\u{200c}\u{a0}\u{200c} then the body  \n\n\n\nFooter\t \n".to_owned(),
        "> quoted line   \n> \t\n>\n\n\n\n-- \nSignature".to_owned(),
        "soft\u{ad}hyphen and zero\u{200b}width stay inside words".to_owned(),
        "code:\n    indented\n\tand tabbed\t\n  \n  \nend".to_owned(),
        format!("{}\n{}x{}", " ".repeat(500), "\t".repeat(500), " ".repeat(500)),
        format!("{}a{}b\u{200b}\u{200b}c", "\u{200b}".repeat(500), "\u{200b} \u{a0}".repeat(500)),
        "\u{feff}BOM at the start".to_owned(),
        " \n\t\r\n ".to_owned(),
        String::new(),
    ];
    for sample in &samples {
        assert_eq!(tidy_text(sample), original(sample), "{sample:?}");
    }

    // Short random texts over the characters the patterns treat specially.
    let alphabet = [
        ' ', ' ', '\t', '\n', '\n', '\r', 'a', '\u{a0}', '\u{200b}', '\u{200c}', '\u{feff}',
    ];
    let mut seed: u32 = 42;
    let mut pick = || {
        // `(seed * 1103515245 + 12345) & 0x7fffffff`, with the product rounded as JavaScript rounds it.
        let product = f64::from(seed) * 1_103_515_245.0 + 12_345.0;
        seed = (product.rem_euclid(4_294_967_296.0) as u32) & 0x7fff_ffff;
        alphabet[seed as usize % alphabet.len()]
    };
    for round in 0..20_000 {
        let text: String = (0..round % 25).map(|_| pick()).collect();
        assert_eq!(
            tidy_text(&text),
            original(&text),
            "tidy_text differs on {text:?}"
        );
    }
}

// Built-in iCloud Mail hardening: HTML

#[tokio::test]
async fn converts_a_large_newsletter_without_keeping_the_server_from_answering() {
    let row = "<tr><td style=\"padding:0 12px;font-family:Helvetica,Arial,sans-serif\">An offer, with a <a href=\"https://shop.example/deals\">link to follow</a>.</td></tr>";
    let html = format!(
        "<html><head><style>{}</style></head><body><table>{}</table></body></html>",
        "p{color:red}".repeat(2000),
        row.repeat(6000)
    );
    assert!(html.len() > 900_000);

    let converter = HtmlConverter::default();
    let (result, pause) =
        longest_pause(converter.html_to_text(html, 5000, HtmlConversion::default())).await;
    let result = result.unwrap();

    assert!(result.is_truncated);
    assert_eq!(result.text.encode_utf16().count(), 5000);
    assert!(
        result
            .text
            .starts_with("An offer, with a link to follow [https://shop.example/deals].")
    );
    assert!(pause < MAX_PAUSE, "{pause:?}");
}

#[tokio::test]
async fn gives_up_on_markup_that_takes_too_long_to_convert() {
    let converter = HtmlConverter::default();

    // A conversion ends when its time is up, however far it got.
    let row = "<tr><td>An offer, with a <a href=\"https://shop.example/deals\">link to follow</a>.</td></tr>";
    let newsletter = format!("<table>{}</table>", row.repeat(9000));
    let start = Instant::now();
    let (result, pause) = longest_pause(converter.html_to_text(
        newsletter.clone(),
        80_000,
        conversion(Duration::ZERO),
    ))
    .await;
    assert_eq!(result, None);
    assert!(pause < MAX_PAUSE, "{pause:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        converter
            .html_to_text(newsletter, 80_000, HtmlConversion::default())
            .await
            .is_some()
    );

    // html-to-text spends a time that grows with the square of the nesting
    // on tags that are never closed: seconds for these, so the Node app gave
    // up on them. Here they take the time it takes to read them, and convert.
    let unclosed = [
        "<i>".repeat(300_000),
        format!("{}{}", "<i>".repeat(100_000), "</b>".repeat(100_000)),
    ];
    for html in unclosed {
        let start = Instant::now();
        let (result, pause) = longest_pause(converter.html_to_text(
            html,
            80_000,
            conversion(Duration::from_millis(400)),
        ))
        .await;

        assert_eq!(
            result,
            Some(Converted {
                text: String::new(),
                is_truncated: false
            })
        );
        assert!(pause < MAX_PAUSE, "{pause:?}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[tokio::test]
async fn gives_up_on_markup_that_converts_to_more_text_than_the_server_may_hold() {
    let converter = HtmlConverter::default();
    // Each of the 400,000 lines is quoted 500 times over: 400 MB of text.
    let quoted = format!(
        "{}<pre>{}",
        "<blockquote>".repeat(500),
        "x\n".repeat(400_000)
    );

    let start = Instant::now();
    let (result, pause) =
        longest_pause(converter.html_to_text(quoted, 80_000, HtmlConversion::default())).await;
    assert_eq!(result, None);
    assert!(pause < MAX_PAUSE, "{pause:?}");
    assert!(start.elapsed() < Duration::from_secs(2));

    // The next message is converted as usual.
    assert_eq!(
        converter
            .html_to_text(
                "<p>Still <b>here</b></p>".to_owned(),
                100,
                HtmlConversion::default()
            )
            .await,
        Some(Converted {
            text: "Still here".into(),
            is_truncated: false
        })
    );
}

#[tokio::test]
async fn converts_links_nested_in_links_without_a_copy_of_their_text_for_each() {
    // html-to-text keeps, for every link, its own copy of the words inside it:
    // hundreds of megabytes for this message, which the Node app lost a process to.
    let html = format!(
        "{}{}",
        "<a href=\"https://x.example/y\">".repeat(200),
        "x ".repeat(400_000)
    );

    let converter = HtmlConverter::default();
    let (result, pause) =
        longest_pause(converter.html_to_text(html, 80_000, HtmlConversion::default())).await;
    let result = result.unwrap();

    assert!(result.is_truncated);
    assert_eq!(result.text.len(), 80_000);
    assert!(result.text.starts_with("x x x x "));
    assert!(pause < MAX_PAUSE, "{pause:?}");
}

#[tokio::test]
async fn reads_at_most_the_text_it_was_asked_for_from_the_converter() {
    let converter = HtmlConverter::default();
    let convert = async |html: &str, max_chars: usize| {
        converter
            .html_to_text(html.to_owned(), max_chars, HtmlConversion::default())
            .await
    };
    let text = |text: &str, is_truncated: bool| {
        Some(Converted {
            text: text.to_owned(),
            is_truncated,
        })
    };

    // 40 dashes for each rule: far more text than markup.
    let (result, pause) = longest_pause(convert(&"<hr>".repeat(100_000), 2000)).await;
    let result = result.unwrap();
    assert_eq!(result.text.len(), 2000);
    assert!(result.is_truncated);
    assert!(pause < MAX_PAUSE, "{pause:?}");

    assert_eq!(convert("<pre>a\n  b</pre>", 6).await, text("a\n  b", false));
    assert_eq!(convert("<pre>a\n  b</pre>", 5).await, text("a\n  b", false));
    assert_eq!(convert("<pre>a\n  b</pre>", 4).await, text("a\n  ", true));
    assert_eq!(convert("", 10).await, text("", false));
    // A character JavaScript counts as two is kept whole or left out.
    assert_eq!(convert("<p>a😀b</p>", 2).await, text("a", true));
    assert_eq!(convert("<p>a😀b</p>", 3).await, text("a😀", true));
}

#[tokio::test]
async fn converts_messages_one_at_a_time_in_the_order_they_were_asked_for() {
    let converter = HtmlConverter::default();
    let order = std::sync::Mutex::new(Vec::new());
    let convert = async |index: usize| {
        let converted = converter
            .html_to_text(
                format!("<p>Message {index}</p>"),
                100,
                HtmlConversion::default(),
            )
            .await;
        order.lock().unwrap().push(index);
        converted.map(|converted| converted.text)
    };
    let texts = futures::future::join_all([1, 2, 3, 4].map(convert)).await;

    assert_eq!(
        texts,
        ["Message 1", "Message 2", "Message 3", "Message 4"].map(|text| Some(text.to_owned()))
    );
    assert_eq!(*order.lock().unwrap(), [1, 2, 3, 4]);
}

// Built-in iCloud Mail hardening: reading

fn hostile_envelope() -> Envelope {
    Envelope {
        date: Utc.with_ymd_and_hms(2026, 10, 4, 8, 0, 0).single(),
        subject: Some("Hello".into()),
        message_id: Some("<hostile@example.com>".into()),
        from: vec![Address::bare("mallory@example.com")],
        to: vec![Address::bare("thomas@icloud.com")],
        ..Envelope::default()
    }
}

#[tokio::test]
async fn reads_a_plain_text_message_padded_with_spaces_without_stalling() {
    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        FakeMessage {
            uid: 21,
            envelope: hostile_envelope(),
            body_structure: leaf(None, "text/plain", Some("7bit"), 160_005),
            ..FakeMessage::default()
        }
        .part("1", format!("Hello{}", " ".repeat(160_000))),
    );
    let mcp = scenario.mcp(&["read"]);

    let (result, pause) = longest_pause(scenario.data(
        &mcp,
        "get_message",
        json!({ "uid": 21, "max_chars": 40_000 }),
    ))
    .await;

    assert_eq!(result["text"], "Hello");
    assert_eq!(result["text_truncated"], true);
    assert!(pause < MAX_PAUSE, "{pause:?}");
}

#[tokio::test]
async fn reads_an_html_message_padded_with_spaces_without_stalling() {
    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        FakeMessage {
            uid: 22,
            envelope: hostile_envelope(),
            body_structure: leaf(None, "text/html", Some("7bit"), 999_999),
            ..FakeMessage::default()
        }
        .part(
            "1",
            format!(
                "<p>Hello</p><pre>{}</pre><p>hidden</p>",
                " ".repeat(990_000)
            ),
        ),
    );
    let mcp = scenario.mcp(&["read"]);

    let (result, pause) = longest_pause(scenario.data(
        &mcp,
        "get_message",
        json!({ "uid": 22, "max_chars": 100_000 }),
    ))
    .await;

    // The spaces fill what is kept of the converted text, and tidying drops them.
    assert_eq!(result["text"], "Hello");
    assert_eq!(result["text_truncated"], true);
    assert_eq!(result.get("warning"), None);
    assert!(pause < MAX_PAUSE, "{pause:?}");
}

#[tokio::test]
async fn returns_the_headers_of_an_html_message_that_cannot_be_converted() {
    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        FakeMessage {
            uid: 23,
            envelope: hostile_envelope(),
            body_structure: multipart(
                None,
                "mixed",
                vec![
                    leaf(Some("1"), "text/html", Some("7bit"), 999_999),
                    attached("2", "application/pdf", 780, "invoice.pdf"),
                ],
            ),
            ..FakeMessage::default()
        }
        // Lines quoted 500 times over, in the megabyte that is downloaded: more text than may be held.
        .part(
            "1",
            format!(
                "{}<pre>{}",
                "<blockquote>".repeat(500),
                "x\n".repeat(450_000)
            ),
        )
        .part("2", "PDF"),
    );
    let mcp = scenario.mcp(&["read"]);

    let (result, pause) =
        longest_pause(scenario.data(&mcp, "get_message", json!({ "uid": 23 }))).await;

    assert_same_json(
        &result,
        &json!({
            "mailbox": "INBOX",
            "uid": 23,
            "subject": "Hello",
            "from": "mallory@example.com",
            "to": ["thomas@icloud.com"],
            "date": "2026-10-04T08:00:00.000Z",
            "unread": true,
            "flagged": false,
            "answered": false,
            "message_id": "<hostile@example.com>",
            "text": "",
            "warning": "This message is written in HTML that could not be converted to text, so its text is missing.",
            "attachments": [{ "part": "2", "filename": "invoice.pdf", "content_type": "application/pdf", "size": 570 }],
        }),
    );
    assert_eq!(
        scenario.icloud.downloads(),
        [Download {
            uid: 23,
            part: "1".into()
        }]
    );
    assert_eq!(scenario.icloud.logouts(), 1);
    assert!(pause < MAX_PAUSE, "{pause:?}");

    // Other messages are still read.
    let newsletter = scenario
        .data(&mcp, "get_message", json!({ "uid": 12 }))
        .await;
    assert!(
        newsletter["text"]
            .as_str()
            .unwrap()
            .starts_with("Hello\n\nSee the deals")
    );
}

#[tokio::test]
async fn signs_out_of_icloud_before_converting_a_message() {
    let mut scenario = Scenario::new().await;
    // No time at all: the conversion gives up, which takes no connection to find out.
    scenario.html_timeout(Duration::ZERO);
    let mcp = scenario.mcp(&["read"]);

    let result = scenario
        .data(&mcp, "get_message", json!({ "uid": 12 }))
        .await;
    assert_eq!(result["text"], "");
    assert_eq!(
        result["warning"],
        "This message is written in HTML that could not be converted to text, so its text is missing."
    );
    // iCloud is signed out of before the conversion, not kept waiting for it.
    assert_eq!(
        scenario.icloud.commands().last().map(String::as_str),
        Some("LOGOUT")
    );
    assert_eq!(scenario.icloud.logouts(), 1);
}

// Built-in iCloud Mail hardening: what a sender controls

fn crowded() -> FetchedMessage {
    FetchedMessage {
        uid: 31,
        envelope: Some(Envelope {
            date: Utc.with_ymd_and_hms(2026, 10, 4, 8, 0, 0).single(),
            subject: Some("S".repeat(5000)),
            message_id: Some(format!("<{}@example.com>", "m".repeat(5000))),
            from: people(60, "from"),
            reply_to: people(60, "reply"),
            to: people(300, "to"),
            cc: people(51, "cc"),
            bcc: vec![Address::new("N".repeat(1000), "bcc@example.com")],
            ..Envelope::default()
        }),
        ..FetchedMessage::default()
    }
}

fn length(value: &Value) -> usize {
    value.as_str().unwrap().encode_utf16().count()
}

#[test]
fn cuts_long_subjects_and_address_lists_and_says_which() {
    let summary = Value::Object(message_summary(&crowded()));

    assert_eq!(length(&summary["subject"]), 999);
    assert!(summary["subject"].as_str().unwrap().ends_with("S…"));
    assert_eq!(summary["to"].as_array().unwrap().len(), 50);
    assert_eq!(summary["to"][49], "to 49 <to49@example.com>");
    assert_eq!(summary["from"].as_str().unwrap().split(", ").count(), 50);
    for field in ["subject_truncated", "from_truncated", "to_truncated"] {
        assert_eq!(summary[field], true, "{field}");
    }

    let headers = Value::Object(message_headers(&crowded()));
    assert_eq!(headers["cc"].as_array().unwrap().len(), 50);
    assert_eq!(
        headers["reply_to"].as_str().unwrap().split(", ").count(),
        50
    );
    assert_eq!(length(&headers["bcc"][0]), 321);
    assert_eq!(length(&headers["message_id"]), 999);
    for field in [
        "subject_truncated",
        "from_truncated",
        "to_truncated",
        "reply_to_truncated",
        "cc_truncated",
        "bcc_truncated",
        "message_id_truncated",
    ] {
        assert_eq!(headers[field], true, "{field}");
    }
    // Everything a hostile message can put in a listing fits in a few pages.
    assert!(summary.to_string().len() < 12_000);
    assert!(headers.to_string().len() < 24_000);
    let keys: Vec<&String> = headers.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "uid",
            "subject",
            "from",
            "to",
            "date",
            "unread",
            "flagged",
            "answered",
            "subject_truncated",
            "from_truncated",
            "to_truncated",
            "reply_to",
            "cc",
            "bcc",
            "message_id",
            "reply_to_truncated",
            "cc_truncated",
            "bcc_truncated",
            "message_id_truncated",
        ]
    );
}

#[test]
fn flags_nothing_on_a_message_within_the_limits() {
    let message = FetchedMessage {
        uid: 32,
        envelope: Some(Envelope {
            subject: Some("s".repeat(998)),
            message_id: Some("<ok@example.com>".into()),
            from: vec![Address::new("Alice", "alice@example.com")],
            to: people(50, "to"),
            cc: people(50, "cc"),
            ..Envelope::default()
        }),
        ..FetchedMessage::default()
    };

    let headers = message_headers(&message);
    assert_eq!(length(&headers["subject"]), 998);
    assert_eq!(headers["to"].as_array().unwrap().len(), 50);
    assert_eq!(headers["cc"].as_array().unwrap().len(), 50);
    assert_eq!(
        headers
            .keys()
            .filter(|key| key.ends_with("_truncated"))
            .count(),
        0
    );
    // A message without a date has none to tell.
    assert!(!headers.contains_key("date"));
}

#[tokio::test]
async fn names_at_most_100_attachments_and_shortens_their_names() {
    let file = |index: usize| {
        attached(
            &(index + 2).to_string(),
            "application/pdf",
            780,
            &if index == 0 {
                "n".repeat(4000)
            } else {
                format!("file-{index}.pdf")
            },
        )
    };
    let mut children = vec![leaf(Some("1"), "text/plain", None, 5)];
    children.extend((0..150).map(file));
    children.push(BodyStructure {
        parameters: HashMap::from([("name".to_owned(), "odd".to_owned())]),
        size: None,
        ..leaf(
            Some("152"),
            &format!("application/{}", "x".repeat(4000)),
            None,
            0,
        )
    });
    let body_structure = multipart(None, "mixed", children);

    let attachments = attachments_of(Some(&body_structure));
    assert_eq!(attachments.len(), 151);
    assert_eq!(attachments[0].filename, format!("{}…", "n".repeat(255)));
    assert_eq!(attachments[150].content_type.chars().count(), 256);

    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        FakeMessage {
            uid: 33,
            envelope: Envelope {
                subject: Some("Files".into()),
                from: vec![Address::bare("mallory@example.com")],
                ..Envelope::default()
            },
            body_structure,
            ..FakeMessage::default()
        }
        .part("1", "Files")
        .part("151", "PDF"),
    );
    let mcp = scenario.mcp(&["read"]);

    let message = scenario
        .data(&mcp, "get_message", json!({ "uid": 33 }))
        .await;
    assert_eq!(message["attachments"].as_array().unwrap().len(), 100);
    assert_eq!(message["attachments_truncated"], true);
    assert_eq!(message["attachments"][99]["part"], "101");

    let listed = scenario.data(&mcp, "list_messages", json!({})).await;
    assert_eq!(listed["messages"][0]["attachments"], 151);

    // An attachment that is not named can still be downloaded, and a wrong
    // part is explained without naming all of them.
    let download = async |part: &str| {
        scenario
            .provider()
            .download_file(
                json!({ "mailbox": "INBOX", "uid": 33, "part": part }),
                mcp.clone(),
            )
            .await
    };
    let unnamed = download("151").await.unwrap();
    assert_eq!(unnamed.filename, "file-149.pdf");
    assert_eq!(unnamed.content.concat(), b"PDF");
    let missing = download("999").await.unwrap_err().to_string();
    assert!(missing.ends_with(", 100, 101, and more."), "{missing}");
    assert!(missing.len() < 600);

    let untruncated = scenario
        .data(&mcp, "get_message", json!({ "uid": 11 }))
        .await;
    assert_eq!(untruncated["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(untruncated.get("attachments_truncated"), None);
}

#[test]
fn refuses_addresses_written_to_be_slow_to_check_as_quickly_as_others() {
    // The longest an address can be, with a dot at every place the domain
    // could be split: the first pattern of the Node app tried each of them.
    let crafted = [
        format!("a@{}b ", "b.".repeat(125)),
        format!("a@{}b@c.d", "b.".repeat(120)),
    ];

    let (accepted, time) = elapsed(|| {
        (0..50_000)
            .map(|_| crafted.iter().filter(|address| is_address(address)).count())
            .sum::<usize>()
    });
    assert_eq!(accepted, 0);
    assert!(time < MAX_PAUSE * 4, "{time:?}");

    // The same addresses pass as before.
    let verdicts = [
        ("name@example.com".to_owned(), true),
        (format!("a@{}bb", "b.".repeat(125)), true),
        ("a@.b.c".to_owned(), true),
        ("a@b..c".to_owned(), true),
        ("a@b".to_owned(), false),
        ("a@.b".to_owned(), false),
        ("a@b.".to_owned(), false),
        ("@b.c".to_owned(), false),
        ("a@b@c.d".to_owned(), false),
        ("Dave <dave@example.com>".to_owned(), false),
        ("a@b.c,d@e.f".to_owned(), false),
        (format!("a@{}.cc", "b".repeat(250)), false),
    ];
    for (address, is_accepted) in verdicts {
        assert_eq!(is_address(&address), is_accepted, "{address}");
    }
}

#[test]
fn works_out_the_recipients_of_a_reply_to_a_crowded_message_without_stalling() {
    let original = FetchedMessage {
        uid: 34,
        envelope: Some(Envelope {
            subject: Some("Everyone".into()),
            from: vec![Address::bare("mallory@example.com")],
            reply_to: people(20_000, "reply"),
            to: people(20_000, "to"),
            cc: [people(20_000, "cc"), people(20_000, "reply")].concat(),
            ..Envelope::default()
        }),
        ..FetchedMessage::default()
    };

    let (result, time) = elapsed(|| reply_to(&original, &["thomas@icloud.com".to_owned()], true));

    assert_eq!(result.to.len(), 20_000);
    assert_eq!(result.cc.len(), 40_000);
    assert!(time < MAX_PAUSE, "{time:?}");
}

// Built-in iCloud Mail hardening: replies

fn petition(uid: u32, change: impl FnOnce(&mut Envelope)) -> FakeMessage {
    let mut envelope = Envelope {
        date: Utc.with_ymd_and_hms(2026, 10, 4, 8, 0, 0).single(),
        subject: Some("Sign the petition".into()),
        message_id: Some(format!("<petition-{uid}@example.com>")),
        from: vec![Address::bare("mallory@example.com")],
        to: vec![Address::bare("thomas@icloud.com")],
        ..Envelope::default()
    };
    change(&mut envelope);
    FakeMessage {
        uid,
        envelope,
        body_structure: leaf(None, "text/plain", Some("7bit"), 5),
        ..FakeMessage::default()
    }
    .part("1", "Hello")
}

#[tokio::test]
async fn refuses_to_answer_more_addresses_than_an_agent_may_name_itself() {
    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        petition(41, |envelope| envelope.reply_to = people(51, "target")),
    );
    scenario.icloud.push_message(
        "INBOX",
        petition(42, |envelope| {
            envelope.to = people(30, "to");
            envelope.cc = people(21, "cc");
        }),
    );
    scenario.icloud.push_message(
        "INBOX",
        petition(43, |envelope| {
            envelope.reply_to = people(50, "target");
            envelope.cc = people(50, "cc");
        }),
    );
    let mcp = scenario.mcp(&PERMISSIONS);

    for tool in ["send_message", "create_draft"] {
        assert_eq!(
            scenario
                .refusal(&mcp, tool, json!({ "reply_to_uid": 41, "text": "Signed." }))
                .await,
            "The message being answered asks for replies to 51 addresses, and at most 50 are allowed. Pass to with the addresses to answer."
        );
        assert_eq!(
            scenario
                .refusal(
                    &mcp,
                    tool,
                    json!({ "reply_to_uid": 42, "reply_all": true, "text": "Signed." })
                )
                .await,
            "Replying to all would copy 51 addresses, and at most 50 are allowed. Set reply_all to false, and pass to and cc with the addresses to answer."
        );
        // The agent's own copies count towards the same limit.
        let one_more = scenario
            .refusal(&mcp, tool, json!({ "reply_to_uid": 43, "reply_all": true, "cc": ["extra@example.com"], "text": "Signed." }))
            .await;
        assert!(
            one_more.contains("Replying to all would copy 51 addresses"),
            "{one_more}"
        );
    }
    assert_eq!(scenario.icloud.sent(), []);
    assert_eq!(scenario.icloud.mailbox("Drafts").appended, []);
    assert!(
        !scenario.icloud.mailbox("INBOX").messages[3]
            .flags
            .contains(&"\\Answered".to_owned())
    );
}

#[tokio::test]
async fn answers_a_crowded_message_once_the_agent_names_the_recipients() {
    let scenario = Scenario::new().await;
    scenario.icloud.push_message(
        "INBOX",
        petition(41, |envelope| envelope.reply_to = people(51, "target")),
    );
    scenario.icloud.push_message(
        "INBOX",
        petition(43, |envelope| {
            envelope.reply_to = people(50, "target");
            envelope.cc = people(50, "cc");
        }),
    );
    let mcp = scenario.mcp(&PERMISSIONS);

    let chosen = scenario
        .data(
            &mcp,
            "send_message",
            json!({ "reply_to_uid": 41, "to": ["target0@example.com"], "text": "Signed." }),
        )
        .await;
    assert_eq!(chosen["to"], json!(["target0@example.com"]));
    assert_eq!(chosen["subject"], "Re: Sign the petition");
    assert!(
        scenario.icloud.sent()[0]
            .raw
            .contains("In-Reply-To: <petition-41@example.com>\r\n")
    );

    // Exactly at the limit: 50 addresses to answer, and 50 to copy.
    let full = scenario
        .data(
            &mcp,
            "create_draft",
            json!({ "reply_to_uid": 43, "reply_all": true, "text": "Signed." }),
        )
        .await;
    assert_eq!(full["to"].as_array().unwrap().len(), 50);
    assert_eq!(full["cc"].as_array().unwrap().len(), 50);
}
