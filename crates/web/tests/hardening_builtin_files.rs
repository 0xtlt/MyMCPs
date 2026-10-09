//! Port of `tests/functional/hardening_builtin_files.spec.ts`, and what the
//! Rust server has to get right on its own: a download goes on when its
//! client hangs up, and a client that stops reading loses its place.

#[path = "support/builtin_files.rs"]
mod support;

use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use http::{Method, StatusCode};
use http_body_util::BodyExt;
use mymcps_core::models::Mcp;
use serde_json::json;
use support::{
    ATTACHMENT, Answer, Connection, Files, UNAVAILABLE, eventually, files, forged, impatient_files,
    open, request, send, serve, unsigned,
};
use tokio::task::JoinHandle;

fn get(link: &str) -> http::Request<Body> {
    request(Method::GET, link).body(Body::empty()).unwrap()
}

async fn status(files: &Files, link: &str) -> StatusCode {
    files.get(link).await.status
}

/// Start a download whose answer is read later.
fn download(files: &Files, link: &str) -> JoinHandle<Answer> {
    tokio::spawn(send(&files.state(), get(link)))
}

/// How long the clients of the tests about stalled clients may read
/// nothing: long enough for a slow machine to send a few requests meanwhile.
const PATIENCE: Duration = Duration::from_millis(500);

/// Wait, for a few seconds at most, until a download is served again.
/// Each try counts as a download: fewer than an address may make.
async fn until_served(files: &Files, link: &str) {
    for _ in 0..40 {
        if status(files, link).await == StatusCode::OK {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no place was given back in time");
}

async fn linked_mcp(files: &Files) -> (Mcp, String) {
    let mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);
    (mcp, link)
}

#[tokio::test]
async fn does_not_count_requests_that_have_no_valid_signature() {
    let files = files().await;
    let (_, link) = linked_mcp(&files).await;
    let forged = forged(&link);
    let unsigned = unsigned(&link);

    // More than the 60 downloads allowed in 15 minutes, from the same address.
    for attempt in 0..70 {
        let link = if attempt % 2 == 0 { &forged } else { &unsigned };
        assert_eq!(status(&files, link).await, StatusCode::FORBIDDEN);
    }
    assert_eq!(files.mailbox.sign_ins(), 0);

    let download = files.get(&link).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.text(), ATTACHMENT);
}

#[tokio::test]
async fn counts_the_downloads_of_each_mcp_apart_and_says_when_to_try_again() {
    let files = files().await;
    let (_, busy) = linked_mcp(&files).await;
    let (_, other) = linked_mcp(&files).await;

    for _ in 0..60 {
        assert_eq!(status(&files, &busy).await, StatusCode::OK);
    }

    let refused = files.get(&busy).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.text(), "Too many downloads. Try again later.");
    let retry_after: u64 = refused.header("retry-after").unwrap().parse().unwrap();
    assert!(retry_after > 0);
    assert!(retry_after <= 15 * 60);
    assert_eq!(files.mailbox.sign_ins(), 60);

    // Using up the downloads of one MCP leaves those of another alone.
    assert_eq!(status(&files, &other).await, StatusCode::OK);
}

#[tokio::test]
async fn counts_the_downloads_of_each_address_apart() {
    let files = files().await;
    let (_, link) = linked_mcp(&files).await;
    // The test server trusts its loopback peer, so the forwarded address is
    // the client address the limiter sees.
    let from = |address: &'static str| {
        let request = request(Method::GET, &link)
            .header("x-forwarded-for", address)
            .body(Body::empty())
            .unwrap();
        send(&files.state(), request)
    };

    for _ in 0..60 {
        assert_eq!(from("198.51.100.7").await.status, StatusCode::OK);
    }
    assert_eq!(
        from("198.51.100.7").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // One client cannot use up the downloads of another.
    assert_eq!(from("198.51.100.8").await.status, StatusCode::OK);
}

#[tokio::test]
async fn counts_a_link_that_is_ours_before_it_looks_for_what_it_names() {
    let files = files().await;
    let (mut mcp, link) = linked_mcp(&files).await;
    mcp.enabled = false;
    files.save(&mut mcp).await;
    let gone = files.attachment_link(&Mcp {
        id: mcp.id + 1000,
        ..Default::default()
    });

    // Whoever holds a link cannot ask about the MCPs of the instance, or
    // keep a provider busy, more often than a link allows.
    for link in [&link, &gone] {
        for _ in 0..60 {
            let unavailable = files.get(link).await;
            assert_eq!(unavailable.told(), (404, UNAVAILABLE.to_owned()));
        }
        let refused = files.get(link).await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.text(), "Too many downloads. Try again later.");
    }
    assert_eq!(files.mailbox.sign_ins(), 0);
}

#[tokio::test]
async fn serves_at_most_three_downloads_of_an_mcp_at_once() {
    let files = files().await;
    let (busy_mcp, busy) = linked_mcp(&files).await;
    let (_, other) = linked_mcp(&files).await;
    let held = files.mailbox.hold_downloads(busy_mcp.id);

    let downloads: Vec<_> = (0..3).map(|_| download(&files, &busy)).collect();
    eventually(|| files.mailbox.sign_ins_of(busy_mcp.id) == 3).await;

    let refused = files.get(&busy).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.header("retry-after"), Some("5"));
    assert_eq!(
        refused.text(),
        "Too many downloads at once. Try again in a few seconds."
    );
    // Refused before signing in to the account a fourth time.
    assert_eq!(files.mailbox.sign_ins(), 3);

    // Another MCP is served in the meantime.
    assert_eq!(status(&files, &other).await, StatusCode::OK);

    held.open();
    for download in downloads {
        let download = download.await.unwrap();
        assert_eq!(download.status, StatusCode::OK);
        assert_eq!(download.text(), ATTACHMENT);
    }

    // Every place was given back.
    let next: Vec<_> = (0..3).map(|_| download(&files, &busy)).collect();
    for download in next {
        assert_eq!(download.await.unwrap().status, StatusCode::OK);
    }
}

#[tokio::test]
async fn gives_a_place_back_when_a_download_fails_and_keeps_it_while_one_is_running() {
    let files = files().await;
    let (mcp, link) = linked_mcp(&files).await;

    // The message text is not a file: each of these fails after signing in.
    let not_a_file = files.file_link(
        mcp.id,
        &json!({ "mailbox": "INBOX", "uid": 11, "part": "1.1" }),
    );
    for _ in 0..5 {
        assert_eq!(status(&files, &not_a_file).await, StatusCode::NOT_FOUND);
    }
    assert_eq!(files.mailbox.sign_ins(), 5);

    // A client that hangs up does not stop its download, so it keeps its place.
    let held = files.mailbox.hold_downloads(mcp.id);
    let abandoned = download(&files, &link);
    eventually(|| files.mailbox.sign_ins() == 6).await;
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());

    let downloads: Vec<_> = (0..2).map(|_| download(&files, &link)).collect();
    eventually(|| files.mailbox.sign_ins() == 8).await;
    assert_eq!(status(&files, &link).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(files.mailbox.sign_outs(), 5);

    held.open();
    for download in downloads {
        assert_eq!(download.await.unwrap().status, StatusCode::OK);
    }
    // The abandoned download went on to its end too, and gave its place back.
    eventually(|| files.mailbox.sign_outs() == 8).await;
    let next: Vec<_> = (0..3).map(|_| download(&files, &link)).collect();
    for download in next {
        assert_eq!(download.await.unwrap().status, StatusCode::OK);
    }
}

#[tokio::test]
async fn sends_a_file_received_in_pieces_whole_and_refuses_one_the_provider_will_not_serve() {
    let files = files().await;
    let (_, link) = linked_mcp(&files).await;

    files.mailbox.serve_in_pieces(vec![
        Bytes::from_static(b"%PDF-"),
        Bytes::from_static(b"1.4\n"),
        Bytes::from_static(b"%%EOF\n"),
    ]);
    let whole = files.get(&link).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(whole.header("content-length"), Some("15"));
    assert_eq!(whole.header("content-type"), Some("application/pdf"));
    assert_eq!(whole.text(), "%PDF-1.4\n%%EOF\n");

    files.mailbox.serve_in_pieces(Vec::new());
    let empty = files.get(&link).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.header("content-length"), Some("0"));
    assert_eq!(empty.text(), "");

    // Larger than the connection takes in one piece, and with pieces of nothing.
    let megabyte = Bytes::from(vec![b'x'; 1_000_000]);
    files.mailbox.serve_in_pieces(vec![
        Bytes::new(),
        megabyte.clone(),
        Bytes::new(),
        Bytes::from_static(b"end"),
    ]);
    let large = files.get(&link).await;
    assert_eq!(large.status, StatusCode::OK);
    assert_eq!(large.header("content-length"), Some("1000003"));
    assert_eq!(large.body.len(), 1_000_003);
    assert!(large.body.starts_with(b"xxx") && large.body.ends_with(b"xend"));

    // Over what the provider serves: the mailbox stops at 30 MB.
    let piece = Bytes::from(vec![0; 10_000_000]);
    files.mailbox.serve_in_pieces(vec![
        piece.clone(),
        piece.clone(),
        piece,
        Bytes::from_static(&[0]),
    ]);
    let too_large = files.get(&link).await;
    assert_eq!(too_large.status, StatusCode::NOT_FOUND);
    assert_eq!(too_large.text(), UNAVAILABLE);
}

#[tokio::test]
async fn names_a_download_whose_filename_is_not_well_formed_unicode() {
    let files = files().await;
    let (_, link) = linked_mcp(&files).await;
    // Half of a surrogate pair, as a mis-encoded header decodes to: a
    // provider can only hold the character that stands for it.
    files.mailbox.name_attachment("Menu \u{fffd}.pdf");

    let download = files.get(&link).await;

    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(
        download.header("content-disposition"),
        Some("attachment; filename=\"Menu _.pdf\"; filename*=UTF-8''Menu%20%EF%BF%BD.pdf")
    );
    assert_eq!(download.text(), ATTACHMENT);
}

#[tokio::test]
async fn keeps_a_place_until_the_client_has_the_file_or_has_gone() {
    let files = files().await;
    let (_, link) = linked_mcp(&files).await;

    // Three clients that have their answer and have not read the file yet.
    let mut unread = Vec::new();
    for _ in 0..3 {
        let response = open(&files.state(), get(&link)).await;
        assert_eq!(response.status(), StatusCode::OK);
        unread.push(response);
    }
    assert_eq!(status(&files, &link).await, StatusCode::TOO_MANY_REQUESTS);

    // One reads its file, another hangs up.
    let read = unread.pop().unwrap().into_body().collect().await.unwrap();
    assert_eq!(read.to_bytes(), ATTACHMENT.as_bytes());
    drop(unread.pop());
    let next: Vec<_> = (0..2).map(|_| download(&files, &link)).collect();
    for download in next {
        assert_eq!(download.await.unwrap().status, StatusCode::OK);
    }
    assert_eq!(status(&files, &link).await, StatusCode::OK);
}

#[tokio::test]
async fn takes_the_place_of_a_client_that_stops_reading() {
    let files = impatient_files(PATIENCE, Duration::from_secs(60)).await;
    let (_, link) = linked_mcp(&files).await;
    let megabyte = Bytes::from(vec![b'x'; 1_000_000]);
    files
        .mailbox
        .serve_in_pieces(vec![megabyte.clone(), megabyte]);

    let mut stalled = Vec::new();
    for _ in 0..3 {
        let mut body = open(&files.state(), get(&link)).await.into_body();
        // Each has read the beginning of its file.
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert!(!first.is_empty() && first.len() < 1_000_000);
        stalled.push(body);
    }
    assert_eq!(status(&files, &link).await, StatusCode::TOO_MANY_REQUESTS);

    // Their places are free once they have read nothing for a while, and a
    // client that reads keeps its own to the end.
    tokio::time::sleep(PATIENCE).await;
    until_served(&files, &link).await;
    let downloads: Vec<_> = (0..3).map(|_| download(&files, &link)).collect();
    for download in downloads {
        let download = download.await.unwrap();
        assert_eq!(download.status, StatusCode::OK);
        assert_eq!(download.body.len(), 2_000_000);
    }

    // What a stalled client asks for later is not passed off as the whole file.
    for mut body in stalled {
        let mut received = 0;
        let broken = loop {
            match body.frame().await {
                Some(Ok(frame)) => received += frame.into_data().unwrap().len(),
                Some(Err(_)) => break true,
                None => break false,
            }
        };
        assert!(broken);
        assert!(received < 2_000_000);
    }
}

#[tokio::test]
async fn takes_the_place_of_a_connection_that_stops_reading() {
    let files = impatient_files(PATIENCE, Duration::from_secs(60)).await;
    let (_, link) = linked_mcp(&files).await;
    // More than a connection holds on its way to a client that does not read.
    let size = 30_000_000;
    files
        .mailbox
        .serve_in_pieces(vec![Bytes::from(vec![b'x'; 1_000_000]); 30]);
    let server = serve(&files.state()).await;

    let mut stalled = Vec::new();
    for _ in 0..3 {
        let mut connection = Connection::open(&server).await;
        connection.write_head("GET", &link, &[]).await;
        let head = connection.head().await;
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(
            head.header("content-length"),
            Some(size.to_string().as_str())
        );
        stalled.push(connection);
    }
    let mut reader = Connection::open(&server).await;
    reader.write_head("GET", &link, &[]).await;
    assert_eq!(reader.answer().await.status, StatusCode::TOO_MANY_REQUESTS);

    // The places come back once the three have read nothing for a while.
    tokio::time::sleep(PATIENCE).await;
    until_served(&files, &link).await;
    reader.write_head("GET", &link, &[]).await;
    let served = reader.answer().await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.body.len(), size);

    // A stalled client that reads again finds its file cut short.
    let mut late = stalled.pop().unwrap();
    assert!(late.read_to_end().await < size);
}
