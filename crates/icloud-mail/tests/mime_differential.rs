//! `mime` against nodemailer 10.0.14: every message in `tests/fixtures/mime` was
//! built by the library itself, on Node 24.21, and has to come out of the port
//! byte for byte. `tests/fixtures/mime/generate.mjs` writes the fixtures again.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::DateTime;
use mymcps_icloud_mail::mime::{
    Attachment, Envelope, Mail, boundary_from, random_boundary, smtp_data,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

// The fixtures.

#[derive(Deserialize)]
struct Fixture {
    name: String,
    /// A mail the tools never ask for, which the port still builds like nodemailer.
    #[serde(default)]
    beyond: bool,
    mail: MailFixture,
    boundary: String,
    envelope: EnvelopeFixture,
    /// What nodemailer sent to an SMTP server that offers SMTPUTF8, 8BITMIME,
    /// SIZE and PIPELINING.
    smtp: Option<Smtp>,
    /// The message built with `keepBcc`, and without.
    kept: Message,
    dropped: Message,
    /// What the server received after `DATA`.
    data: Option<Message>,
}

#[derive(Deserialize)]
struct MailFixture {
    from: String,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    text: Option<String>,
    /// A long text, as a piece and how many times it is repeated.
    text_repeat: Option<(String, usize)>,
    attachments: Vec<AttachmentFixture>,
    in_reply_to: Option<String>,
    references: Option<Vec<String>>,
    message_id: String,
    date_ms: i64,
}

#[derive(Deserialize)]
struct AttachmentFixture {
    filename: String,
    content_type: Option<String>,
    content: Content,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Base64 {
        base64: String,
    },
    /// Bytes that `generated_bytes` makes again, to keep large files out of the fixtures.
    Generated {
        seed: u32,
        length: usize,
    },
}

#[derive(Deserialize)]
struct EnvelopeFixture {
    from: String,
    to: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Smtp {
    Sent { commands: Vec<String> },
    Refused { error: String },
}

/// A message: its digest always, and its bytes when it is short.
#[derive(Deserialize)]
struct Message {
    length: usize,
    sha256: String,
    text: Option<String>,
    base64: Option<String>,
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mime")
        .join(name)
}

fn read_lines<T: for<'de> Deserialize<'de>>(name: &str) -> Vec<T> {
    let content = fs::read_to_string(fixture_path(name)).expect("the fixture is readable");
    content
        .lines()
        .map(|line| serde_json::from_str(line).expect("the fixture is valid JSON"))
        .collect()
}

/// The same bytes as `generatedBytes` of `generate.mjs`.
fn generated_bytes(seed: u32, length: usize) -> Vec<u8> {
    let mut value = if seed == 0 { 1 } else { seed };
    (0..length)
        .map(|_| {
            value ^= value << 13;
            value ^= value >> 17;
            value ^= value << 5;
            value as u8
        })
        .collect()
}

impl MailFixture {
    fn mail(&self) -> Mail {
        let text = match (&self.text, &self.text_repeat) {
            (Some(text), _) => text.clone(),
            (None, Some((piece, times))) => piece.repeat(*times),
            (None, None) => panic!("a mail has a text"),
        };
        let attachments = self
            .attachments
            .iter()
            .map(|attachment| Attachment {
                filename: attachment.filename.clone(),
                content_type: attachment.content_type.clone(),
                content: match &attachment.content {
                    Content::Base64 { base64 } => {
                        BASE64.decode(base64).expect("the content is base64")
                    }
                    Content::Generated { seed, length } => generated_bytes(*seed, *length),
                },
            })
            .collect();
        Mail {
            from: self.from.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            text,
            attachments,
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            message_id: self.message_id.clone(),
            date: DateTime::from_timestamp_millis(self.date_ms).expect("the date is in range"),
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

impl Message {
    /// Why `actual` is not this message, when it is not.
    fn difference(&self, actual: &[u8]) -> Option<String> {
        let expected = match (&self.text, &self.base64) {
            (Some(text), _) => Some(text.as_bytes().to_vec()),
            (None, Some(base64)) => Some(BASE64.decode(base64).expect("the message is base64")),
            (None, None) => None,
        };
        if actual.len() == self.length && sha256(actual) == self.sha256 {
            return None;
        }
        let Some(expected) = expected else {
            return Some(format!(
                "{} bytes instead of {}, or other bytes: the fixture only has the digest",
                actual.len(),
                self.length
            ));
        };
        let at = actual
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected)
            .unwrap_or(actual.len().min(expected.len()));
        let around = |bytes: &[u8]| {
            let start = at.saturating_sub(60);
            let end = (at + 60).min(bytes.len());
            String::from_utf8_lossy(&bytes[start.min(end)..end]).into_owned()
        };
        Some(format!(
            "differs at byte {at}\n  nodemailer: {:?}\n  port:       {:?}",
            around(&expected),
            around(actual)
        ))
    }
}

/// The commands nodemailer sends for an envelope before `DATA`, in the order
/// it sends them: `_setEnvelope` and `_actionMAIL` of smtp-connection.
///
/// `SMTPUTF8` is the only parameter it ever adds for these mails, when an
/// address is not ASCII and the server offers the extension. It never adds
/// `BODY=8BITMIME` or `SIZE=`, whatever the server offers. With nothing to
/// deliver to, it sends nothing and fails.
fn smtp_commands(envelope: &Envelope, offers_smtputf8: bool) -> Result<Vec<String>, &'static str> {
    if envelope.to.is_empty() {
        return Err("No recipients defined");
    }
    let mut mail_from = format!("MAIL FROM:<{}>", envelope.from);
    if offers_smtputf8 && envelope.needs_smtputf8() {
        mail_from.push_str(" SMTPUTF8");
    }
    let mut commands = vec![mail_from];
    commands.extend(
        envelope
            .to
            .iter()
            .map(|address| format!("RCPT TO:<{address}>")),
    );
    Ok(commands)
}

fn differences(fixture: &Fixture) -> Vec<String> {
    let mail = fixture.mail.mail();
    let mut found = Vec::new();

    let envelope = mail.envelope();
    if envelope.from != fixture.envelope.from || envelope.to != fixture.envelope.to {
        found.push(format!(
            "envelope\n  nodemailer: {:?} {:?}\n  port:       {:?} {:?}",
            fixture.envelope.from, fixture.envelope.to, envelope.from, envelope.to
        ));
    }

    let kept = mail.build(true, &fixture.boundary);
    if let Some(difference) = fixture.kept.difference(&kept) {
        found.push(format!("with Bcc: {difference}"));
    }
    // The message is sized before it is written. Only a message without parts
    // may be given two bytes it does not use, for its last line break.
    if kept.capacity() - kept.len() > 2 {
        found.push(format!(
            "allocated {} bytes for {}",
            kept.capacity(),
            kept.len()
        ));
    }
    let dropped = mail.build(false, &fixture.boundary);
    if let Some(difference) = fixture.dropped.difference(&dropped) {
        found.push(format!("without Bcc: {difference}"));
    }
    if let Some(data) = &fixture.data
        && let Some(difference) = data.difference(&smtp_data(&dropped))
    {
        found.push(format!("after DATA: {difference}"));
    }

    match (&fixture.smtp, smtp_commands(&envelope, true)) {
        (None, _) => {}
        (Some(Smtp::Sent { commands }), Ok(expected)) if *commands == expected => {}
        (Some(Smtp::Refused { error }), Err(expected)) if error == expected => {}
        (Some(Smtp::Sent { commands }), ours) => {
            found.push(format!(
                "SMTP\n  nodemailer: {commands:?}\n  port:       {ours:?}"
            ));
        }
        (Some(Smtp::Refused { error }), ours) => {
            found.push(format!(
                "SMTP\n  nodemailer: {error:?}\n  port:       {ours:?}"
            ));
        }
    }
    found
}

fn assert_same_as_nodemailer(file: &str, at_least: usize) {
    let fixtures: Vec<Fixture> = read_lines(file);
    assert!(
        fixtures.len() >= at_least,
        "{file} has {} mails",
        fixtures.len()
    );

    let mut failed = 0;
    for fixture in &fixtures {
        let found = differences(fixture);
        if !found.is_empty() {
            failed += 1;
            if failed <= 10 {
                eprintln!("{}:\n{}\n", fixture.name, found.join("\n"));
            }
        }
    }
    assert_eq!(
        failed,
        0,
        "{failed} of {} mails of {file} differ",
        fixtures.len()
    );
}

// The mails.

#[test]
fn builds_the_corpus_like_nodemailer() {
    assert_same_as_nodemailer("corpus.jsonl", 300);
}

#[test]
fn builds_random_mails_like_nodemailer() {
    assert_same_as_nodemailer("random.jsonl", 2000);
}

#[test]
fn the_corpus_stays_within_what_the_tools_pass() {
    let fixtures: Vec<Fixture> = read_lines("corpus.jsonl");
    let within = fixtures.iter().filter(|fixture| !fixture.beyond).count();
    assert!(
        within >= 300,
        "{within} mails of the corpus are ones the tools can ask for"
    );
}

fn fixture_named(name: &str) -> (Mail, String) {
    let fixtures: Vec<Fixture> = read_lines("corpus.jsonl");
    let fixture = fixtures
        .into_iter()
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("the corpus has a mail named {name:?}"));
    (fixture.mail.mail(), fixture.boundary)
}

fn text_of(message: &[u8]) -> String {
    String::from_utf8(message.to_vec()).expect("the message is UTF-8")
}

/// What the tests of the Node version looked at in the copy kept in the account.
#[test]
fn writes_what_the_app_expects() {
    let (mail, boundary) = fixture_named("app: a draft with a blind copy");
    let copy = text_of(&mail.build(true, &boundary));
    assert!(copy.contains("From: thomas@icloud.com\r\n"));
    assert!(copy.contains("To: dave@example.com\r\n"));
    assert!(copy.contains("Bcc: boss@example.com\r\n"));
    assert!(copy.contains("\r\nMessage-ID: <"));
    assert!(copy.contains("Subject: Quote for October\r\n"));
    assert!(!copy.contains("X-Mailer"));
    assert!(copy.ends_with("\r\n\r\nHello Dave,\r\n\r\nHere is the quote.\r\n"));
    let delivered = text_of(&mail.build(false, &boundary));
    assert!(!delivered.contains("Bcc"));
    assert_eq!(mail.envelope().to, ["dave@example.com", "boss@example.com"]);

    let (mail, boundary) = fixture_named("app: a reply");
    let copy = text_of(&mail.build(true, &boundary));
    assert!(copy.contains("In-Reply-To: <invoice@example.com>\r\n"));
    let (mail, boundary) = fixture_named("app: references without the message answered");
    let copy = text_of(&mail.build(true, &boundary));
    assert!(copy.contains("References: <root@example.com> <lunch@example.com>\r\n"));

    let (mail, boundary) = fixture_named("app: attachments");
    let copy = text_of(&mail.build(true, &boundary));
    assert!(copy.contains("Content-Type: multipart/mixed;"));
    assert!(copy.contains("Content-Type: application/pdf;"));
    assert!(copy.contains("filename*0*=utf-8''Devis%20%C3%A9t%C3%A9.pdf"));
    assert!(copy.contains("Content-Type: text/plain; name=notes.txt\r\n"));
    assert!(copy.contains("Content-Disposition: attachment; filename=notes.txt\r\n"));
    assert!(copy.contains("Content-Type: application/pdf; name=quote.pdf\r\n"));
}

#[test]
fn builds_20_mb_of_attachments_in_one_allocation() {
    let (mail, boundary) = fixture_named("app: two files of 10 MB");
    let message = mail.build(true, &boundary);
    assert!((27_000_000..28_000_000).contains(&message.len()));
    assert_eq!(message.capacity(), message.len());
}

/// A measure rather than a test: `cargo test --release -- --ignored --nocapture timing`.
#[test]
#[ignore = "a measure, to run in a release build"]
fn timing_of_20_mb_of_attachments() {
    let (mail, boundary) = fixture_named("app: two files of 10 MB");
    let mut times = Vec::new();
    let mut length = 0;
    for _ in 0..21 {
        let start = Instant::now();
        let message = mail.build(false, &boundary);
        times.push(start.elapsed().as_micros());
        length = message.len();
    }
    times.sort_unstable();
    let message = mail.build(false, &boundary);
    let start = Instant::now();
    let data = smtp_data(&message);
    let stuffing = start.elapsed().as_micros();
    println!(
        "build: {length} bytes in {} µs at best, {} µs at the median, {} µs at worst; smtp_data: {} bytes in {stuffing} µs",
        times[0],
        times[10],
        times[20],
        data.len()
    );
    assert!(times[10] < 200_000, "a build takes {} µs", times[10]);
}

// Addresses.

fn written(address: &str) -> String {
    let mail = Mail {
        from: address.to_owned(),
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: String::new(),
        text: String::new(),
        attachments: Vec::new(),
        in_reply_to: None,
        references: None,
        message_id: String::new(),
        date: DateTime::UNIX_EPOCH,
    };
    mail.envelope().from
}

#[test]
fn writes_addresses_like_nodemailer() {
    let addresses: Vec<(String, String)> = read_lines("addresses.jsonl");
    assert!(addresses.len() >= 10_000);
    let mut failed = 0;
    for (address, expected) in &addresses {
        let actual = written(address);
        if actual != *expected {
            failed += 1;
            if failed <= 10 {
                eprintln!("{address:?}\n  nodemailer: {expected:?}\n  port:       {actual:?}");
            }
        }
    }
    assert_eq!(
        failed,
        0,
        "{failed} of {} addresses differ",
        addresses.len()
    );
}

#[derive(Deserialize)]
struct Sweep {
    block: u32,
    digests: Vec<String>,
}

/// The addresses of `around` in `generate.mjs`.
fn around(c: char) -> Vec<String> {
    vec![
        format!("u@{c}.com"),
        format!("u@a{c}b.com"),
        format!("\u{e9}@a{c}b.com"),
        format!("u@x|.{c}a"),
        format!("\u{e9}@x|.{c}a"),
        format!("{c}@example.com"),
        format!("a{c}b@example.com"),
        format!("u@{c}1.com"),
        format!("u@\u{5d0}{c}.com"),
        format!("\u{e9}@{c}.com"),
        format!("u@\u{5d0}{c}\u{5d0}.\u{ff41}"),
        format!("u@\u{5d0}{c}.\u{ff41}"),
        format!("u@{c}\u{5d0}.\u{ff41}"),
        format!("u@\u{915}{c}.\u{ff41}"),
        format!("u@\u{5d0}1{c}\u{5d0}.\u{ff41}"),
        format!("u@\u{5d0}\u{661}{c}\u{5d0}.\u{ff41}"),
        format!("u@{c}\u{915}\u{94d}\u{200d}.\u{ff41}"),
        format!("u@\u{915}{c}\u{200d}.\u{ff41}"),
        format!("u@\u{915}{c}\u{200c}\u{1820}.\u{ff41}"),
        format!("u@\u{1820}\u{200c}{c}.\u{ff41}"),
        format!("\u{e9}@x{c}\u{301}.z"),
        format!("\u{e9}@x\u{301}{c}.z"),
        format!("\u{e9}@x{c}\u{334}.z"),
        format!("\u{e9}@x\u{e3a}{c}.z"),
        format!("\u{e9}@{c}\u{327}.z"),
        format!("\u{e9}@x{c}{c}.z"),
        format!("\u{e9}@\u{e9}{c}.z"),
        format!("\u{e9}@\u{ac00}{c}.z"),
    ]
}

/// Every Unicode code point, in 28 addresses each: 31 million addresses.
///
/// This pins the port to the Unicode data of Node 24.21, which is of four
/// different versions depending on the table (see `host_tables.rs`). It fails
/// when the `icu_normalizer` crate moves to a newer Unicode, for the
/// characters that version adds: that is news, not a bug.
#[test]
#[ignore = "takes a minute in a release build: cargo test --release -- --ignored every_code_point"]
fn writes_addresses_around_every_code_point_like_nodemailer() {
    let content = fs::read_to_string(fixture_path("address_sweep.json")).expect("readable");
    let sweep: Sweep = serde_json::from_str(&content).expect("valid JSON");
    let mut failed = Vec::new();
    for (index, expected) in sweep.digests.iter().enumerate() {
        let start = u32::try_from(index).expect("few blocks") * sweep.block;
        let mut hasher = Sha256::new();
        for c in (start..start + sweep.block).filter_map(char::from_u32) {
            for address in around(c) {
                hasher.update(written(&address).as_bytes());
                hasher.update(b"\n");
            }
        }
        let mut digest = String::new();
        for byte in hasher.finalize() {
            let _ = write!(digest, "{byte:02x}");
        }
        if digest != *expected {
            failed.push(format!("U+{start:04X}..U+{:04X}", start + sweep.block - 1));
        }
    }
    assert!(failed.is_empty(), "addresses differ around {failed:?}");
}

// What no fixture can say.

#[test]
fn draws_a_boundary_like_nodemailer() {
    assert_eq!(
        boundary_from([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]),
        "0123456789abcdef"
    );
    let boundary = random_boundary();
    assert_eq!(boundary.len(), 16);
    assert!(
        boundary
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    );
    assert_ne!(boundary, random_boundary());
}

#[test]
fn ends_the_data_of_any_message() {
    assert_eq!(smtp_data(b""), b"\r\n.\r\n");
    assert_eq!(smtp_data(b"a"), b"a\r\n.\r\n");
    assert_eq!(smtp_data(b"a\r"), b"a\r\n.\r\n");
    assert_eq!(smtp_data(b"a\n"), b"a\r\n.\r\n");
    assert_eq!(smtp_data(b"a\r\n"), b"a\r\n.\r\n");
    assert_eq!(
        smtp_data(b".a\r\n.\r\n..\r\nb."),
        b"..a\r\n..\r\n...\r\nb.\r\n.\r\n"
    );
    assert_eq!(
        smtp_data(b"a\r.b\rc\r\r.\n.\n"),
        b"a\r\n..b\r\nc\r\n\r\n..\r\n..\r\n.\r\n"
    );
}

/// A small generator of hostile text, for inputs the tools would refuse.
struct Hostile(u32);

impl Hostile {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 0
    }

    fn text(&mut self) -> String {
        const PIECES: [&str; 32] = [
            "",
            "a",
            "\r\n",
            "\n",
            "\r",
            "\0",
            "\t",
            " ",
            "\"",
            "\\",
            "<",
            ">",
            "@",
            ";",
            ":",
            "=",
            "?",
            "%",
            "*",
            "'",
            ".",
            "/",
            "-",
            "é",
            "好",
            "🎉",
            "\u{7f}",
            "\u{85}",
            "\u{2028}",
            "\u{200d}",
            "xn--",
            "Bcc: evil@example.com",
        ];
        let count = self.next() % 12;
        (0..count)
            .map(|_| PIECES[self.next() as usize % PIECES.len()])
            .collect::<String>()
            .repeat(if self.next().is_multiple_of(16) {
                40
            } else {
                1
            })
    }

    fn list(&mut self) -> Vec<String> {
        (0..self.next() % 4).map(|_| self.text()).collect()
    }
}

/// Whatever it is given, a message has the header fields of a message and no
/// other, and each of their lines is a line: nothing in an input can start a
/// header of its own.
#[test]
fn keeps_hostile_input_inside_its_header() {
    const FIELDS: [&str; 13] = [
        "From",
        "To",
        "Cc",
        "Bcc",
        "In-Reply-To",
        "References",
        "Subject",
        "Message-ID",
        "Date",
        "MIME-Version",
        "Content-Type",
        "Content-Transfer-Encoding",
        "Content-Disposition",
    ];
    let mut hostile = Hostile(0x9e37_79b9);
    for _ in 0..20_000 {
        let mail = Mail {
            from: hostile.text(),
            to: hostile.list(),
            cc: hostile.list(),
            bcc: hostile.list(),
            subject: hostile.text(),
            text: hostile.text(),
            attachments: (0..hostile.next() % 3)
                .map(|_| Attachment {
                    filename: hostile.text(),
                    content_type: hostile.coin().then(|| hostile.text()),
                    content: hostile.text().into_bytes(),
                })
                .collect(),
            in_reply_to: hostile.coin().then(|| hostile.text()),
            references: hostile.coin().then(|| hostile.list()),
            message_id: hostile.text(),
            date: DateTime::from_timestamp_millis(i64::from(hostile.next()) * 1000)
                .expect("in range"),
        };
        let boundary = hostile.text();
        mail.envelope();
        let message = mail.build(hostile.coin(), &boundary);
        smtp_data(&message);

        let end = message
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("a message has headers");
        let headers = String::from_utf8_lossy(&message[..end]);
        for line in headers.split("\r\n") {
            assert!(
                !line.contains(['\r', '\n']),
                "a bare line break in {line:?}"
            );
            let is_continuation = line.starts_with([' ', '\t']);
            // A value too long for the first line starts on the next one.
            let is_field = line.split_once(':').is_some_and(|(name, value)| {
                FIELDS.contains(&name) && (value.is_empty() || value.starts_with(' '))
            });
            // nodemailer folds at any white space of JavaScript, which a few
            // characters outside ASCII are: a line may start with one of them.
            let starts_with_space = line.chars().next().is_some_and(char::is_whitespace)
                || line.starts_with('\u{feff}');
            assert!(
                is_continuation || is_field || starts_with_space,
                "a line that is no header of the message: {line:?}"
            );
        }
    }
}
