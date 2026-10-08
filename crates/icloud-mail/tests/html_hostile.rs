//! Documents written to be slow or huge: each comes back in well under a second, with the
//! text html-to-text would give if Node had the stack, the memory and the time for it, or
//! with the reason there is none.
//!
//! The texts expected here are spelled out. `fixtures/html/large.json` has Node's own answer
//! for the shapes Node can be made to finish, at the same sizes.
//!
//! `cargo test -p mymcps-icloud-mail --release --test html_hostile -- --nocapture` prints the timings.

use std::time::{Duration, Instant};

use mymcps_icloud_mail::html::convert::{Budget, GaveUp, convert};

const MAX_BYTES: usize = 128 << 20;

/// What "well under a second" is. A build without optimizations is some twenty times
/// slower, and is only held to finishing.
fn soon() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_secs(30)
    } else {
        Duration::from_millis(500)
    }
}

/// Converts with the budget of the Node server: 128 MB, five seconds.
fn converted(name: &str, html: &str) -> Result<String, GaveUp> {
    let start = Instant::now();
    let budget = Budget {
        deadline: Some(start + Duration::from_secs(5)),
        max_bytes: MAX_BYTES,
    };
    let converted = convert(html, &budget);
    let elapsed = start.elapsed();
    let outcome = match &converted {
        Ok(text) => format!("{} bytes of text", text.len()),
        Err(gave_up) => format!("{gave_up:?}"),
    };
    eprintln!(
        "{name}: {} bytes of HTML, {outcome}, {elapsed:?}",
        html.len()
    );
    assert!(elapsed < soon(), "{name} took {elapsed:?}");
    converted
}

#[test]
fn the_large_documents_of_the_node_server_tests_take_milliseconds() {
    let row = "<tr><td style=\"padding:0 12px;font-family:Helvetica,Arial,sans-serif\">An offer, with a <a href=\"https://shop.example/deals\">link to follow</a>.</td></tr>";
    let newsletter = format!(
        "<html><head><style>{}</style></head><body><table>{}</table></body></html>",
        "p{color:red}".repeat(2000),
        row.repeat(6000)
    );
    let text = converted("a newsletter of 6,000 rows", &newsletter).expect("it converts");
    assert!(text.starts_with("An offer, with a link to follow [https://shop.example/deals]."));
    assert_eq!(text.len(), 366_000);

    let text = converted("100,000 rules", &"<hr>".repeat(100_000)).expect("it converts");
    assert_eq!(text.len(), 100_000 * 40 + 99_999 * 2);

    let html = format!(
        "<p>Hello</p><pre>{}</pre><p>hidden</p>",
        " ".repeat(990_000)
    );
    let text = converted("990,000 spaces in pre", &html).expect("it converts");
    assert!(text.starts_with("Hello\n\n ") && text.ends_with(" \n\nhidden"));
}

#[test]
fn tags_never_closed_cost_no_stack_and_no_time() {
    assert_eq!(
        converted("300,000 <i>", &"<i>".repeat(300_000)).as_deref(),
        Ok("")
    );
    assert_eq!(
        converted(
            "300,000 <i> around a word",
            &format!("{}word", "<i>".repeat(300_000))
        )
        .as_deref(),
        Ok("word")
    );
    // Every `<p>` closes the one before it: 333,333 empty paragraphs.
    assert_eq!(
        converted("333,333 <p>", &"<p>".repeat(333_333)).as_deref(),
        Ok("")
    );
}

#[test]
fn end_tags_that_close_nothing_do_not_search_the_open_elements() {
    let html = format!("{}{}", "<i>".repeat(100_000), "</b>".repeat(100_000));
    assert_eq!(
        converted("100,000 <i> then 100,000 </b>", &html).as_deref(),
        Ok("")
    );

    // Names HTML does not define, each of them once: opened, then closed in another order.
    let opened: String = (0..60_000).map(|index| format!("<x{index}>")).collect();
    let closed: String = (0..60_000).map(|index| format!("</x{index}>")).collect();
    let html = format!("{opened}{closed}text");
    assert_eq!(
        converted("60,000 names opened, then closed", &html).as_deref(),
        Ok("text")
    );
}

#[test]
fn quotes_around_many_lines_are_given_up_on_before_they_are_written() {
    // 400,000 lines of 500 times "> " and an "x": 400 MB.
    let html = format!(
        "{}<pre>{}",
        "<blockquote>".repeat(500),
        "x\n".repeat(400_000)
    );
    assert_eq!(
        converted("500 quotes around 400,000 lines", &html),
        Err(GaveUp::TooLarge)
    );

    // The same with the lines that fit.
    let html = format!("{}<pre>{}", "<blockquote>".repeat(500), "x\n".repeat(2000));
    let line = format!("{}x", "> ".repeat(500));
    assert_eq!(
        converted("500 quotes around 2,000 lines", &html),
        Ok(vec![line; 2000].join("\n"))
    );

    // Line breaks a quote ends with are trimmed, at every level: nothing is written for
    // them, however many quotes they would have to be written in.
    let html = format!("{}a{}", "<blockquote>".repeat(2000), "<br>".repeat(200_000));
    assert_eq!(
        converted("2,000 quotes around 200,000 line breaks", &html),
        Ok(format!("{}a", "> ".repeat(2000)))
    );
}

#[test]
fn links_in_links_do_not_each_keep_the_words() {
    let html = format!(
        "{}{}",
        "<a href=\"https://x.example/y\">".repeat(200),
        "x ".repeat(400_000)
    );
    let words = vec!["x"; 400_000].join(" ");
    assert_eq!(
        converted("200 links around 400,000 words", &html),
        Ok(format!("{words}{}", " [https://x.example/y]".repeat(200)))
    );

    // Far more links: each compares the first word with its address, and no other.
    let html = format!("{}{}", "<a href=x>".repeat(10_000), "x ".repeat(440_000));
    let words = vec!["x"; 440_000].join(" ");
    assert_eq!(
        converted("10,000 links around 440,000 words", &html),
        Ok(format!("{words}{}", " [x]".repeat(10_000)))
    );

    // Addresses the words do spell, each as far as it goes: 1,000 words for every link.
    let address = "x".repeat(1000);
    let html = format!(
        "{}{}",
        format!("<a href={address}>").repeat(400),
        "x ".repeat(250_000)
    );
    let words = vec!["x"; 250_000].join(" ");
    assert_eq!(
        converted("400 links whose address is their first 1,000 words", &html),
        Ok(format!("{words}{}", format!(" [{address}]").repeat(400)))
    );
}

#[test]
fn blocks_in_blocks_do_not_copy_their_text_at_every_level() {
    let html = format!("{}{}", "<div>".repeat(100_000), "x ".repeat(250_000));
    assert_eq!(
        converted("100,000 blocks around 250,000 words", &html),
        Ok(vec!["x"; 250_000].join(" "))
    );

    let html = "<div>x".repeat(150_000);
    assert_eq!(
        converted("150,000 blocks, a word in each", &html),
        Ok(vec!["x"; 150_000].join("\n"))
    );

    // Each list item indents the lines of the items inside it: 18 GB.
    let html = format!("{}{}", "<ul><li>".repeat(60_000), "a<br>".repeat(100_000));
    assert_eq!(
        converted("60,000 lists around 100,000 lines", &html),
        Err(GaveUp::TooLarge)
    );

    // Lists without items indent nothing, and cost nothing for each line inside them.
    let html = format!("{}<pre>{}", "<ul>".repeat(200_000), "x\n".repeat(100_000));
    assert_eq!(
        converted("200,000 lists without items around 100,000 lines", &html),
        Ok("x\n".repeat(100_000))
    );
}

#[test]
fn text_that_looks_like_markup_is_read_once() {
    assert_eq!(
        converted("1,000,000 ampersands", &"&".repeat(1_000_000)),
        Ok("&".repeat(1_000_000))
    );
    assert_eq!(
        converted("1,000,000 less-than signs", &"<".repeat(1_000_000)),
        Ok("<".repeat(1_000_000))
    );
    // The longest name of an entity, less its last letter: read to the end every time.
    let almost = "&CounterClockwiseContourIntegra";
    assert_eq!(
        converted("32,000 entities cut short", &almost.repeat(32_000)),
        Ok(almost.repeat(32_000))
    );
    // One tag with 499,000 attributes, none of which is the one that matters.
    let html = format!("<a {}href=u>t</a>", "b ".repeat(499_000));
    assert_eq!(
        converted("499,000 attributes", &html).as_deref(),
        Ok("t [u]")
    );
    // Comments and declarations that never end.
    assert_eq!(
        converted("300,000 comments opened", &"<!--".repeat(300_000)).as_deref(),
        Ok("")
    );
    // An end tag that never ends takes the rest of the document with it, except its last
    // character, which upstream takes for text: see `handle_trailing_data`.
    assert_eq!(
        converted("200,000 end tags opened", &"</a x".repeat(200_000)).as_deref(),
        Ok("x")
    );
}

#[test]
fn ten_thousand_roman_numerals_are_where_html_to_text_throws() {
    let html = format!("<ol type=I>{}</ol>", "<li>x".repeat(9_999));
    let text = converted("9,999 Roman numerals", &html).expect("9,999 has four digits");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 9_999);
    assert!(
        lines[3_998].starts_with(" MMMCMXCIX. "),
        "{:?}",
        lines[3_998]
    );
    // Past 3,999 upstream reads past the end of its numerals.
    assert!(
        lines[9_998].starts_with(" MundefinedCMXCIX. "),
        "{:?}",
        lines[9_998]
    );

    let html = format!("<ol type=I>{}</ol>", "<li>x".repeat(10_000));
    assert_eq!(
        converted("10,000 Roman numerals", &html),
        Err(GaveUp::Throws)
    );
}

#[test]
fn gives_up_within_milliseconds_of_the_deadline() {
    // More than a quarter of a second of writing, were it left to finish.
    let html = format!(
        "{}<pre>{}",
        "<blockquote>".repeat(500),
        "x\n".repeat(400_000)
    );
    let start = Instant::now();
    let budget = Budget {
        deadline: Some(start + Duration::from_millis(5)),
        max_bytes: usize::MAX,
    };

    assert_eq!(convert(&html, &budget), Err(GaveUp::TooSlow));

    let late = start.elapsed().saturating_sub(Duration::from_millis(5));
    eprintln!("gave up {late:?} after the deadline");
    let allowed = if cfg!(debug_assertions) { 200 } else { 20 };
    assert!(
        late < Duration::from_millis(allowed),
        "gave up {late:?} after the deadline"
    );
}

#[test]
fn gives_up_at_once_when_the_deadline_has_passed() {
    let budget = Budget {
        deadline: Some(Instant::now()),
        max_bytes: MAX_BYTES,
    };
    assert_eq!(convert("<p>text</p>", &budget), Err(GaveUp::TooSlow));

    let budget = Budget {
        deadline: None,
        max_bytes: MAX_BYTES,
    };
    assert_eq!(convert("<p>text</p>", &budget).as_deref(), Ok("text"));
}

#[test]
fn gives_up_before_holding_more_than_it_may() {
    let convert_within = |html: &str, max_bytes: usize| {
        convert(
            html,
            &Budget {
                deadline: None,
                max_bytes,
            },
        )
    };

    // A megabyte of markup is a megabyte of text and three megabytes of tree.
    let html = "<p>One paragraph of <b>text</b> &amp; an entity.</p>\n".repeat(20_000);
    assert_eq!(convert_within(&html, 1 << 20), Err(GaveUp::TooLarge));
    assert!(convert_within(&html, 16 << 20).is_ok());

    // Far more text than markup.
    let rules = "<hr>".repeat(100_000);
    assert_eq!(convert_within(&rules, 6 << 20), Err(GaveUp::TooLarge));
    assert_eq!(
        convert_within(&rules, 16 << 20).map(|text| text.len()),
        Ok(4_199_998)
    );

    // A short message needs little, and not nothing.
    assert_eq!(
        convert_within("<p>Still <b>here</b></p>", 8192).as_deref(),
        Ok("Still here")
    );
    assert_eq!(
        convert_within("<p>Still <b>here</b></p>", 0),
        Err(GaveUp::TooLarge)
    );
}
