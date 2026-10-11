//! The converter against html-to-text 10.0.1 itself: every input of the fixtures must give,
//! to the byte, the text Node gave for it. `fixtures/html/generate.mjs` writes the fixtures.

use std::time::{Duration, Instant};

use mymcps_icloud_mail::html::convert::{Budget, GaveUp, convert};
use serde_json::Value;

/// More than any fixture needs: these tests are about the text. `HTML_PORT_MAX_BYTES`
/// raises it for an extra fixture whose texts are larger still.
fn budget() -> Budget {
    let max_bytes = std::env::var("HTML_PORT_MAX_BYTES")
        .ok()
        .and_then(|bytes| bytes.parse().ok())
        .unwrap_or(128 << 20);
    Budget {
        deadline: Some(Instant::now() + Duration::from_secs(5)),
        max_bytes,
    }
}

/// What Node gave: the text, or nothing when html-to-text throws.
fn expected(fixture: &Value) -> Result<String, GaveUp> {
    match fixture["text"].as_str() {
        Some(text) => Ok(text.to_owned()),
        None => {
            assert_eq!(fixture["throws"], true, "a fixture has a text or throws");
            Err(GaveUp::Throws)
        }
    }
}

/// A value as the report of a failure shows it: the start of it when it is long.
fn shown(value: &impl std::fmt::Debug) -> String {
    let shown = format!("{value:?}");
    match shown.char_indices().nth(400) {
        Some((cut, _)) => format!("{}... ({} bytes)", &shown[..cut], shown.len()),
        None => shown,
    }
}

/// Converts every input of a fixture file and reports all that differ, not the first.
fn check(fixtures: &str) -> usize {
    let fixtures: Vec<Value> = serde_json::from_str(fixtures).expect("the fixtures are JSON");
    let mut different = Vec::new();
    for fixture in &fixtures {
        let html = fixture["html"].as_str().expect("a fixture has an input");
        let expected = expected(fixture);
        let converted = convert(html, &budget());
        if converted != expected {
            different.push(format!(
                "    html: {}\nexpected: {}\n     got: {}",
                shown(&html),
                shown(&expected),
                shown(&converted)
            ));
        }
    }
    assert!(
        different.is_empty(),
        "{} of {} inputs differ from html-to-text, the first of them:\n\n{}",
        different.len(),
        fixtures.len(),
        different
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    fixtures.len()
}

#[test]
fn converts_the_corpus_as_html_to_text_does() {
    assert!(check(include_str!("fixtures/html/cases.json")) >= 300);
}

#[test]
fn converts_random_tag_soup_as_html_to_text_does() {
    assert!(check(include_str!("fixtures/html/soup.json")) >= 2000);
}

/// More soup than is worth storing:
/// `node crates/icloud-mail/tests/fixtures/html/generate.mjs --soup 200000 --seed 7 --out /tmp/more.json`, then
/// `HTML_PORT_EXTRA_FIXTURE=/tmp/more.json cargo test extra`.
#[test]
fn converts_extra_fixtures_as_html_to_text_does() {
    if let Some(path) = std::env::var_os("HTML_PORT_EXTRA_FIXTURE") {
        let fixtures = std::fs::read_to_string(path).expect("the extra fixture is readable");
        assert!(check(&fixtures) > 0);
    }
}

/// FNV-1a, as `hash` of `generate.mjs`.
fn hash(text: &str) -> String {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        value = (value ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{value:016x}")
}

/// Inputs of up to a megabyte, stored as what they repeat and checked by the hash and
/// the length of their text.
#[test]
fn converts_large_documents_as_html_to_text_does() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures/html/large.json"))
        .expect("the fixtures are JSON");
    for fixture in &fixtures {
        let name = fixture["name"].as_str().expect("a fixture has a name");
        let html: String = fixture["recipe"]
            .as_array()
            .expect("a recipe is a list")
            .iter()
            .map(|part| {
                let text = part[0].as_str().expect("a part has a text");
                text.repeat(part[1].as_u64().expect("a part has a count") as usize)
            })
            .collect();

        let text = convert(&html, &budget()).unwrap_or_else(|gave_up| panic!("{name}: {gave_up}"));

        let start: String = text.chars().take(80).collect();
        assert_eq!(
            start,
            fixture["start"].as_str().expect("a fixture has a start"),
            "{name}"
        );
        assert_eq!(
            text.len() as u64,
            fixture["bytes"].as_u64().expect("a length"),
            "{name}"
        );
        assert_eq!(
            hash(&text),
            fixture["hash"].as_str().expect("a hash"),
            "{name}"
        );
    }
    assert!(fixtures.len() >= 3);
}

/// The tests of the Node server, with the texts they expect there.
#[test]
fn converts_what_the_node_server_tests_convert() {
    let text = |html: &str| convert(html, &budget());

    assert_eq!(
        text(concat!(
            "<html><head><style>p { color: red }</style></head><body>",
            "<p>Hello\u{200c} \u{200c} \u{200c} \u{200c} </p>",
            "<img src=\"https://shop.example/pixel.gif\" alt=\"tracking\">",
            "<p><a href=\"https://shop.example/deals\">See the deals</a></p>",
            "<p><a href=\"https://shop.example\">https://shop.example</a></p>",
            "<p>This paragraph is long enough that a wrapping converter would break it across several lines of output.</p>",
            "</body></html>",
        ))
        .as_deref(),
        Ok(concat!(
            "Hello\u{200c} \u{200c} \u{200c} \u{200c}\n\n",
            "See the deals [https://shop.example/deals]\n\n",
            "https://shop.example\n\n",
            "This paragraph is long enough that a wrapping converter would break it across several lines of output.",
        ))
    );
    assert_eq!(
        text("<p>Still <b>here</b></p>").as_deref(),
        Ok("Still here")
    );
    assert_eq!(text("<pre>a\n  b</pre>").as_deref(), Ok("a\n  b"));
    assert_eq!(text("").as_deref(), Ok(""));
    for index in 1..=4 {
        assert_eq!(
            text(&format!("<p>Message {index}</p>")),
            Ok(format!("Message {index}"))
        );
    }

    let row = "<tr><td style=\"padding:0 12px;font-family:Helvetica,Arial,sans-serif\">An offer, with a <a href=\"https://shop.example/deals\">link to follow</a>.</td></tr>";
    let newsletter = format!(
        "<html><head><style>{}</style></head><body><table>{}</table></body></html>",
        "p{color:red}".repeat(2000),
        row.repeat(6000)
    );
    assert!(newsletter.len() > 900_000);
    let converted = text(&newsletter).expect("a newsletter converts");
    assert!(converted.starts_with("An offer, with a link to follow [https://shop.example/deals]."));
    assert_eq!(
        converted,
        "An offer, with a link to follow [https://shop.example/deals].".repeat(6000)
    );

    let rule = "-".repeat(40);
    assert_eq!(
        text(&"<hr>".repeat(100_000)),
        Ok(vec![rule; 100_000].join("\n\n"))
    );

    let spaces = " ".repeat(990_000);
    assert_eq!(
        text(&format!("<p>Hello</p><pre>{spaces}</pre><p>hidden</p>")),
        Ok(format!("Hello\n\n{spaces}\n\nhidden"))
    );
}
