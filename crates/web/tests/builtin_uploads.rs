//! Port of `tests/functional/builtin_icloud_mail_uploads.spec.ts` and of the
//! upload case of `tests/functional/builtin_google_ads_mcp.spec.ts`, for what
//! happens once a file is sent to a link. What a tool puts in a link, and
//! what it does with the file afterwards, is tested with the provider that
//! has the tool.

#[path = "support/builtin_files.rs"]
mod support;

use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::{Method, StatusCode};
use mymcps_builtin::file_link::{BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE};
use mymcps_core::models::Mcp;
use serde_json::{Value, json};
use support::{
    Answer, Connection, Files, HOW_TO_UPLOAD, IMAGE_BYTES, INVALID_LINK, MAILBOX_UPLOAD_BYTES, PDF,
    UPLOAD_UNAVAILABLE, attachment, eventually, files, forged, impatient_files, new_upload_id,
    open_body, path_of, put, put_stream, request, send, serve, signature_of, unsigned,
};
use tokio::task::JoinHandle;

fn unavailable() -> (u16, String) {
    (404, UPLOAD_UNAVAILABLE.to_owned())
}

fn invalid() -> (u16, String) {
    (403, INVALID_LINK.to_owned())
}

fn too_large(megabytes: &str) -> (u16, String) {
    (
        413,
        format!("The file is larger than the {megabytes} MB this link takes."),
    )
}

async fn writing_mcp(files: &Files) -> Mcp {
    files.mailbox_mcp(&["send"]).await
}

/// Start an upload whose answer is read later.
fn upload(files: &Files, link: &str, file: &'static [u8]) -> JoinHandle<Answer> {
    tokio::spawn(send(&files.state(), put(link, file, &[])))
}

/// A body of `megabytes` pieces of one megabyte, made as it is read.
fn megabytes(megabytes: usize) -> Body {
    let megabyte = Bytes::from(vec![b'x'; 1_000_000]);
    Body::from_stream(futures::stream::iter(
        (0..megabytes).map(move |_| Ok::<_, std::io::Error>(megabyte.clone())),
    ))
}

#[tokio::test]
async fn takes_one_file_at_a_link_and_says_what_it_kept() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let upload = new_upload_id();
    let link = files.upload_link_to(
        mcp.id,
        &json!({ "upload": upload, "filename": "Devis été.pdf", "content_type": "application/pdf" }),
        Duration::from_secs(60),
    );

    let (path, query) = link.split_once('?').unwrap();
    let reference = path.strip_prefix(&format!("/uploads/{}/", mcp.id)).unwrap();
    assert!(
        reference
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
    );
    assert!(query.starts_with("signature=") && !query.contains('&'));
    // Making a link reaches neither the account nor the disk.
    assert!(files.stored_files(&mcp).is_empty());

    // No cookie, no CSRF token, no access token: the signature is the credential.
    let uploaded = files.put(&link, PDF).await;
    assert_eq!(uploaded.status, StatusCode::CREATED);
    assert_eq!(
        uploaded.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    let receipt = uploaded.json();
    let expires_at = receipt["expires_at"].as_str().unwrap().to_owned();
    assert_eq!(
        uploaded.text(),
        format!(
            "{{\"upload_id\":\"{upload}\",\"filename\":\"Devis été.pdf\",\"size\":{},\"expires_at\":\"{expires_at}\"}}",
            PDF.len()
        )
    );
    // As JavaScript writes a date: to the millisecond, in UTC.
    assert_eq!(expires_at.len(), "2026-10-07T12:00:00.000Z".len());
    assert!(expires_at.ends_with('Z'));
    let kept_for = DateTime::parse_from_rfc3339(&expires_at).unwrap().to_utc() - Utc::now();
    assert!(kept_for > chrono::Duration::minutes(59));
    assert!(kept_for <= chrono::Duration::minutes(60));

    // The link is used up, whoever else got hold of it.
    assert_eq!(
        files.put(&link, "something else").await.told(),
        (
            409,
            "A file was already sent to this link. Ask for a new link to send another one."
                .to_owned()
        )
    );

    // The file waits for the tool that will use it, as the tool described it.
    let kept = files
        .uploads()
        .find(mcp.id, &upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.filename, "Devis été.pdf");
    assert_eq!(kept.content_type.as_deref(), Some("application/pdf"));
    assert_eq!(kept.size, PDF.len() as u64);
    assert_eq!(files.uploaded(&mcp, &upload).await.as_deref(), Some(PDF));
    assert_eq!(files.mailbox.sign_ins(), 0);
    assert_eq!(uploaded.header("set-cookie"), None);
}

#[tokio::test]
async fn takes_a_file_from_a_signed_in_browser_without_a_csrf_token() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);

    // The route is none of the pages: the session of whoever sends the file
    // is neither read nor checked, and none is started.
    let uploaded = files
        .app
        .put(&link.link)
        .login_as(files.admin())
        .header("origin", "https://elsewhere.example")
        .raw_body(PDF, "application/pdf")
        .send()
        .await;
    assert_eq!(uploaded.status, StatusCode::CREATED);
    assert_eq!(uploaded.header("set-cookie"), None);
    assert_eq!(uploaded.header("access-control-allow-origin"), None);
    assert_eq!(
        files.uploaded(&mcp, &link.upload).await.as_deref(),
        Some(PDF)
    );

    let download = files
        .app
        .get(&files.attachment_link(&files.mailbox_mcp(&["read"]).await))
        .login_as(files.admin())
        .send()
        .await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.header("set-cookie"), None);
}

#[tokio::test]
async fn takes_a_file_for_an_mcp_that_signs_in_with_oauth_while_it_may_write() {
    let files = files().await;
    let mut mcp = files.images_mcp(true).await;
    let link = files.upload_link_named(&mcp, "banner.png");

    // Nothing was sent: the link is the one a file goes to.
    let nothing = send(
        &files.state(),
        request(Method::PUT, &link.link)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        nothing.told(),
        (400, format!("The request has no body. {HOW_TO_UPLOAD}"))
    );

    // What this provider takes is not what the other does.
    let one_byte_too_many = vec![0; IMAGE_BYTES as usize + 1];
    assert_eq!(
        files.put(&link.link, one_byte_too_many).await.told(),
        too_large("5.24288")
    );
    assert_eq!(
        send(&files.state(), put_stream(&link.link, megabytes(6)))
            .await
            .told(),
        too_large("5.24288")
    );
    assert!(files.stored_files(&mcp).is_empty());

    // The link outlives the call that made it: a file is only taken while
    // write access is on.
    mcp.builtin_write_enabled = false;
    files.save(&mut mcp).await;
    assert_eq!(files.put(&link.link, PDF).await.told(), unavailable());
    assert!(files.stored_files(&mcp).is_empty());

    mcp.builtin_write_enabled = true;
    files.save(&mut mcp).await;
    let image = vec![0x89; IMAGE_BYTES as usize];
    let uploaded = files.put(&link.link, image).await;
    assert_eq!(uploaded.status, StatusCode::CREATED);
    assert_eq!(uploaded.json()["size"], json!(IMAGE_BYTES));
    assert_eq!(uploaded.json()["filename"], json!("banner.png"));
    let kept = files
        .uploads()
        .find(mcp.id, &link.upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.content_type, None);

    // An MCP that is not connected takes nothing.
    mcp.oauth_access_token = None;
    files.save(&mut mcp).await;
    let link = files.upload_link_named(&mcp, "banner.png");
    assert_eq!(files.put(&link.link, PDF).await.told(), unavailable());
}

#[tokio::test]
async fn stores_the_body_as_it_is_whatever_it_is_labelled() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    // Larger than the 1 MB a parsed body may take, and not valid JSON.
    let mut large = b"{\"rows\":[".to_vec();
    large.extend(std::iter::repeat_n(b'7', 2_000_000));
    let bodies: Vec<(Vec<u8>, Option<&str>)> = vec![
        (PDF.to_vec(), None),
        (
            "id;name\r\n1;André\r\n".as_bytes().to_vec(),
            Some("text/plain;charset=UTF-8"),
        ),
        (large, Some("application/json")),
        (
            b"a=1&b=%zz".to_vec(),
            Some("application/x-www-form-urlencoded"),
        ),
        (vec![0, 255, 13, 10, 0], Some("application/octet-stream")),
        // Only a form is not a file: a type that merely mentions one is.
        (b"--x--".to_vec(), Some("application/x-multipart")),
    ];

    for (body, label) in bodies {
        let link = files.upload_link(&mcp);
        let response = match label {
            Some(label) => files.put_labelled(&link.link, body.clone(), label).await,
            None => files.put(&link.link, body.clone()).await,
        };
        assert_eq!(response.status, StatusCode::CREATED, "{label:?}");
        assert_eq!(response.json()["size"], json!(body.len()));
        assert_eq!(files.uploaded(&mcp, &link.upload).await, Some(body));
    }
}

#[tokio::test]
async fn refuses_a_form_an_empty_body_and_a_file_over_20_mb() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);
    let no_body = (400, format!("The request has no body. {HOW_TO_UPLOAD}"));

    let form = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"report.pdf\"\r\n\r\n%PDF-1.4\r\n--boundary--\r\n";
    for label in [
        "multipart/form-data; boundary=boundary",
        "Multipart/Form-Data; boundary=boundary",
        "multipart/mixed",
    ] {
        assert_eq!(
            files.put_labelled(&link.link, form, label).await.told(),
            (
                415,
                format!("A form cannot be stored as a file. {HOW_TO_UPLOAD}")
            ),
            "{label}"
        );
    }
    let nothing = send(
        &files.state(),
        request(Method::PUT, &link.link)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(nothing.told(), no_body);
    assert_eq!(files.put(&link.link, Bytes::new()).await.told(), no_body);

    // Refused on what the client says it will send.
    let over = vec![0; MAILBOX_UPLOAD_BYTES as usize + 1];
    // A form of that size is told it is a form first.
    assert_eq!(
        files
            .put_labelled(&link.link, over.clone(), "multipart/form-data; boundary=x")
            .await
            .status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        files.put(&link.link, over.clone()).await.told(),
        too_large("20")
    );
    // And at the first byte too many when it does not say.
    assert_eq!(
        send(&files.state(), put_stream(&link.link, megabytes(40)))
            .await
            .told(),
        too_large("20")
    );
    assert_eq!(
        send(&files.state(), put_stream(&link.link, Body::from(over)))
            .await
            .told(),
        too_large("20")
    );
    // Or says less than it sends: what arrives is what counts.
    let understated = request(Method::PUT, &link.link)
        .header("content-length", "5")
        .body(megabytes(21))
        .unwrap();
    assert_eq!(
        send(&files.state(), understated).await.told(),
        too_large("20")
    );
    // What it says is not believed when it cannot be read either.
    let unreadable = request(Method::PUT, &link.link)
        .header("content-length", "plenty")
        .body(megabytes(21))
        .unwrap();
    assert_eq!(
        send(&files.state(), unreadable).await.told(),
        too_large("20")
    );

    // Nothing was kept of any of them, and the link still takes a file that fits.
    assert!(files.stored_files(&mcp).is_empty());
    let exact = files
        .put(&link.link, vec![b'x'; MAILBOX_UPLOAD_BYTES as usize])
        .await;
    assert_eq!(exact.status, StatusCode::CREATED);
    let kept = files
        .uploads()
        .find(mcp.id, &link.upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.size, MAILBOX_UPLOAD_BYTES);
}

#[tokio::test]
async fn takes_a_file_only_with_our_signature_for_uploads() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);
    let upload = &link.upload;
    let signature = signature_of(&link.link);

    // The reference of another link under this signature.
    let other = files.upload_link_named(&mcp, "other.pdf");
    let swapped = format!("{}?signature={signature}", path_of(&other.link));
    // Signed by us, but to download a file.
    let download = files.signed(path_of(&link.link), BUILTIN_FILE_PURPOSE);
    let attachment_link = files.attachment_link(&mcp).replace("/files/", "/uploads/");
    let expired = files.upload_link_to(
        mcp.id,
        &json!({ "upload": new_upload_id(), "filename": "late.pdf" }),
        Duration::from_millis(1),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;

    for refused in [
        forged(&link.link),
        unsigned(&link.link),
        swapped,
        download,
        attachment_link,
        expired,
        format!("{}&to=elsewhere", link.link),
    ] {
        assert_eq!(
            files.put(&refused, PDF).await.told(),
            invalid(),
            "{refused}"
        );
    }

    // Signed for uploads, but not to what a tool would write.
    let sign = |reference: Value| files.upload_link_to(mcp.id, &reference, Duration::from_secs(60));
    for link in [
        sign(attachment()),
        sign(json!({ "upload": "../../db.sqlite3", "filename": "report.pdf" })),
        sign(json!({ "upload": upload, "filename": "../report.pdf" })),
        sign(json!("report.pdf")),
        files.signed(
            &format!(
                "/uploads/{}.5/{}",
                mcp.id,
                support::encoded(&json!({ "upload": upload, "filename": "report.pdf" }))
            ),
            BUILTIN_UPLOAD_PURPOSE,
        ),
        files.signed(
            &format!("/uploads/{}/not-a-reference", mcp.id),
            BUILTIN_UPLOAD_PURPOSE,
        ),
        files.upload_link_to(
            mcp.id + 1000,
            &json!({ "upload": upload, "filename": "report.pdf" }),
            Duration::from_secs(60),
        ),
    ] {
        assert_eq!(files.put(&link, PDF).await.told(), unavailable(), "{link}");
    }
    assert!(files.stored_files(&mcp).is_empty());

    // The link is judged before what is sent to it.
    let form = "multipart/form-data; boundary=x";
    assert_eq!(
        files
            .put_labelled(&forged(&link.link), PDF, form)
            .await
            .told(),
        invalid()
    );
    assert_eq!(
        files
            .put_labelled(&sign(json!("report.pdf")), PDF, form)
            .await
            .told(),
        unavailable()
    );

    // A file cannot be read back from the upload route either.
    assert_eq!(files.get(&link.link).await.status, StatusCode::NOT_FOUND);

    assert_eq!(files.put(&link.link, PDF).await.status, StatusCode::CREATED);
    assert_eq!(files.get(&link.link).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn follows_the_mcp_its_permissions_whether_it_is_enabled_and_its_deletion() {
    let files = files().await;
    let mut mcp = files.mailbox_mcp(&["draft", "send"]).await;
    let link = files.upload_link(&mcp);

    mcp.builtin_permissions = Some("read organize".to_owned());
    files.save(&mut mcp).await;
    assert_eq!(files.put(&link.link, PDF).await.told(), unavailable());

    mcp.builtin_permissions = Some("draft".to_owned());
    mcp.enabled = false;
    files.save(&mut mcp).await;
    assert_eq!(files.put(&link.link, PDF).await.told(), unavailable());
    assert!(files.stored_files(&mcp).is_empty());

    mcp.enabled = true;
    files.save(&mut mcp).await;
    assert_eq!(files.put(&link.link, PDF).await.status, StatusCode::CREATED);
    assert_eq!(files.stored_files(&mcp).len(), 1);

    // An MCP that is deleted takes no more files.
    let later = files.upload_link(&mcp);
    mcp.delete(&*files.app.core.db).await.unwrap();
    assert_eq!(files.put(&later.link, PDF).await.told(), unavailable());
    // Nothing here ever signed in to the account.
    assert_eq!(files.mailbox.sign_ins(), 0);
}

#[tokio::test]
async fn counts_the_uploads_of_each_mcp_apart_without_the_ones_that_have_no_valid_signature() {
    let files = files().await;
    let busy = writing_mcp(&files).await;
    let other = writing_mcp(&files).await;
    let upload = async |mcp: &Mcp| files.put(&files.upload_link(mcp).link, "x").await;

    let forged = forged(&files.upload_link(&busy).link);
    for _ in 0..70 {
        assert_eq!(files.put(&forged, "x").await.status, StatusCode::FORBIDDEN);
    }

    // An MCP holds 50 files at a time.
    for _ in 0..50 {
        assert_eq!(upload(&busy).await.status, StatusCode::CREATED);
    }
    assert_eq!(
        upload(&busy).await.told(),
        (
            429,
            "Too many uploaded files are waiting for this MCP. They are deleted an hour after their upload: try again later."
                .to_owned()
        )
    );
    files.clear_uploads();

    // And takes 60 uploads in 15 minutes from one address, kept or not.
    for _ in 51..60 {
        assert_eq!(upload(&busy).await.status, StatusCode::CREATED);
    }
    let refused = upload(&busy).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.text(), "Too many uploads. Try again later.");
    let retry_after: u64 = refused.header("retry-after").unwrap().parse().unwrap();
    assert!(retry_after > 0);
    assert!(retry_after <= 15 * 60);

    assert_eq!(upload(&other).await.status, StatusCode::CREATED);

    // Another address has its own count: the test server trusts its
    // loopback peer, so the forwarded address is the one that is counted.
    let elsewhere = request(Method::PUT, &files.upload_link(&busy).link)
        .header("x-forwarded-for", "198.51.100.9")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        send(&files.state(), elsewhere).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn takes_at_most_three_uploads_of_an_mcp_at_once_and_keeps_nothing_of_one_that_breaks_off() {
    let files = files().await;
    let busy = writing_mcp(&files).await;
    let other = writing_mcp(&files).await;

    let mut open: Vec<_> = (0..3)
        .map(|_| {
            let (body, stream) = open_body("begin");
            let link = files.upload_link(&busy);
            let response = tokio::spawn(send(&files.state(), put_stream(&link.link, stream)));
            (body, link, response)
        })
        .collect();
    eventually(|| files.stored_files(&busy).len() == 3).await;

    let refused = files.put(&files.upload_link(&busy).link, PDF).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.header("retry-after"), Some("5"));
    assert_eq!(
        refused.text(),
        "Too many uploads at once. Try again in a few seconds."
    );
    // What could not be kept anyway is told why, not to try again.
    let link = files.upload_link(&busy).link;
    assert_eq!(
        files
            .put_labelled(&link, PDF, "multipart/form-data; boundary=x")
            .await
            .status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let over = request(Method::PUT, &link)
        .header("content-length", "20000001")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&files.state(), over).await.told(), too_large("20"));
    // A file that is still arriving cannot be attached.
    let arriving = files.uploads().find(busy.id, &open[0].1.upload).await;
    assert_eq!(arriving.unwrap(), None);
    // Another MCP is served in the meantime.
    assert_eq!(
        files.put(&files.upload_link(&other).link, PDF).await.status,
        StatusCode::CREATED
    );

    // One client hangs up: nothing is kept of its file, and its place is given back.
    let (body, abandoned, response) = open.remove(0);
    response.abort();
    assert!(response.await.unwrap_err().is_cancelled());
    body.hang_up();
    eventually(|| files.stored_files(&busy).len() == 2).await;
    assert!(!files.stored_files(&busy).contains(&abandoned.upload));
    assert_eq!(
        files.put(&files.upload_link(&busy).link, PDF).await.status,
        StatusCode::CREATED
    );
    // Its link can be tried again.
    assert_eq!(
        files.put(&abandoned.link, PDF).await.status,
        StatusCode::CREATED
    );

    for (body, link, response) in open {
        body.send(" and end");
        body.finish();
        assert_eq!(response.await.unwrap().status, StatusCode::CREATED);
        assert_eq!(
            files.uploaded(&busy, &link.upload).await.as_deref(),
            Some(&b"begin and end"[..])
        );
    }
    let next: Vec<_> = (0..3)
        .map(|_| upload(&files, &files.upload_link(&busy).link, PDF))
        .collect();
    for response in next {
        assert_eq!(response.await.unwrap().status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn keeps_nothing_of_a_file_that_stops_arriving() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);

    // The connection breaks while the request is still being answered.
    let (body, stream) = open_body("begin");
    let response = tokio::spawn(send(&files.state(), put_stream(&link.link, stream)));
    eventually(|| files.stored_files(&mcp).len() == 1).await;
    body.hang_up();
    let response = response.await.unwrap();
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.text(), "");
    assert!(files.stored_files(&mcp).is_empty());
    assert_eq!(
        files.uploads().find(mcp.id, &link.upload).await.unwrap(),
        None
    );

    // Its link can be tried again.
    assert_eq!(files.put(&link.link, PDF).await.status, StatusCode::CREATED);
}

#[tokio::test]
async fn gives_up_on_a_client_that_stops_sending() {
    // Long enough for a slow machine to send a request to a busy MCP meanwhile.
    let patience = Duration::from_millis(500);
    let files = impatient_files(patience, Duration::from_secs(60)).await;
    let mcp = writing_mcp(&files).await;

    // Three files that begin and never go on.
    let stalled: Vec<_> = (0..3)
        .map(|_| {
            let (body, stream) = open_body("begin");
            let link = files.upload_link(&mcp);
            let response = tokio::spawn(send(&files.state(), put_stream(&link.link, stream)));
            (body, link, response)
        })
        .collect();
    eventually(|| files.stored_files(&mcp).len() == 3).await;
    assert_eq!(
        files.put(&files.upload_link(&mcp).link, PDF).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    for (_body, link, response) in stalled {
        let response = response.await.unwrap();
        assert_eq!(response.status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(response.text(), "");
        assert_eq!(
            files.uploads().find(mcp.id, &link.upload).await.unwrap(),
            None
        );
    }
    // Nothing is kept of them, and their places are free.
    assert!(files.stored_files(&mcp).is_empty());
    let next: Vec<_> = (0..3)
        .map(|_| upload(&files, &files.upload_link(&mcp).link, PDF))
        .collect();
    for response in next {
        assert_eq!(response.await.unwrap().status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn gives_a_file_only_so_long_to_arrive_whole() {
    let whole_upload = Duration::from_millis(300);
    let files = impatient_files(Duration::from_secs(60), whole_upload).await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);

    // A few bytes now and then keep a connection busy, not a place.
    let (body, stream) = open_body("begin");
    let response = tokio::spawn(send(&files.state(), put_stream(&link.link, stream)));
    let trickle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            body.send(".");
        }
    });

    let response = response.await.unwrap();
    assert_eq!(response.status, StatusCode::REQUEST_TIMEOUT);
    assert!(files.stored_files(&mcp).is_empty());
    assert_eq!(
        files.put(&files.upload_link(&mcp).link, PDF).await.status,
        StatusCode::CREATED
    );
    trickle.abort();

    // Neither does a file with no end, whose next piece is always there.
    let endless = Body::from_stream(futures::stream::repeat_with(|| {
        Ok::<_, std::io::Error>(Bytes::from_static(b"."))
    }));
    let link = files.upload_link(&mcp);
    let response = send(&files.state(), put_stream(&link.link, endless)).await;
    assert_eq!(response.status, StatusCode::REQUEST_TIMEOUT);
    assert!(!files.stored_files(&mcp).contains(&link.upload));

    // What is read and dropped after a refusal stops at the same time, and
    // keeps nothing else waiting meanwhile.
    let endless = Body::from_stream(futures::stream::repeat_with(|| {
        Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 1_000_000]))
    }));
    let images = files.images_mcp(true).await;
    let link = files.upload_link_named(&images, "banner.png");
    let response = send(&files.state(), put_stream(&link.link, endless)).await;
    assert_eq!(response.told(), too_large("5.24288"));
    assert_eq!(files.put(&link.link, PDF).await.status, StatusCode::CREATED);
}

/// One piece of a body sent with `Transfer-Encoding: chunked`.
fn chunk(bytes: &[u8]) -> Vec<u8> {
    let mut chunk = format!("{:x}\r\n", bytes.len()).into_bytes();
    chunk.extend_from_slice(bytes);
    chunk.extend_from_slice(b"\r\n");
    chunk
}

#[tokio::test]
async fn answers_a_file_that_is_too_large_while_it_is_still_being_sent() {
    let files = files().await;
    let mcp = files.images_mcp(true).await;
    let link = files.upload_link_named(&mcp, "banner.png");
    let server = serve(&files.state()).await;
    let megabyte = vec![b'x'; 1_000_000];

    // The client says how much it sends, and has only begun.
    let mut connection = Connection::open(&server).await;
    connection
        .write_head("PUT", &link.link, &[("Content-Length", "8000000")])
        .await;
    connection.write(&megabyte).await.unwrap();
    let refused = connection.answer().await;
    assert_eq!(refused.told(), too_large("5.24288"));
    // What it still sends is read and dropped: the connection is not closed
    // under it, and serves its next request.
    for _ in 0..7 {
        connection.write(&megabyte).await.unwrap();
    }
    connection
        .write_head("PUT", &link.link, &[("Content-Length", "51")])
        .await;
    connection.write(PDF).await.unwrap();
    let kept = connection.answer().await;
    assert_eq!(kept.status, StatusCode::CREATED);
    assert_eq!(kept.json()["size"], json!(PDF.len()));

    // The client does not say, and is answered at the first byte too many.
    let link = files.upload_link_named(&mcp, "banner.png");
    connection
        .write_head("PUT", &link.link, &[("Transfer-Encoding", "chunked")])
        .await;
    for _ in 0..6 {
        connection.write(chunk(&megabyte)).await.unwrap();
    }
    let refused = connection.answer().await;
    assert_eq!(refused.told(), too_large("5.24288"));
    assert!(files.stored_files(&mcp).len() == 1);
    for _ in 0..3 {
        connection.write(chunk(&megabyte)).await.unwrap();
    }
    connection.write(chunk(b"")).await.unwrap();
    connection
        .write_head("PUT", &link.link, &[("Transfer-Encoding", "chunked")])
        .await;
    connection.write(chunk(PDF)).await.unwrap();
    connection.write(chunk(b"")).await.unwrap();
    assert_eq!(connection.answer().await.status, StatusCode::CREATED);
    assert_eq!(
        files.uploaded(&mcp, &link.upload).await.as_deref(),
        Some(PDF)
    );
}

#[tokio::test]
async fn answers_a_link_that_is_refused_while_a_file_is_still_being_sent() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let link = files.upload_link(&mcp);
    let server = serve(&files.state()).await;
    let megabyte = vec![b'x'; 1_000_000];

    let mut connection = Connection::open(&server).await;
    for (refused, told) in [
        (forged(&link.link), invalid()),
        (
            files.upload_link_to(mcp.id, &attachment(), Duration::from_secs(60)),
            unavailable(),
        ),
    ] {
        connection
            .write_head("PUT", &refused, &[("Content-Length", "4000000")])
            .await;
        connection.write(&megabyte).await.unwrap();
        assert_eq!(connection.answer().await.told(), told);
        for _ in 0..3 {
            connection.write(&megabyte).await.unwrap();
        }
    }
    // Nothing of it was kept, and the connection still serves.
    assert!(files.stored_files(&mcp).is_empty());
    connection
        .write_head("PUT", &link.link, &[("Content-Length", "51")])
        .await;
    connection.write(PDF).await.unwrap();
    assert_eq!(connection.answer().await.status, StatusCode::CREATED);
}

#[tokio::test]
async fn keeps_nothing_of_a_file_whose_client_hangs_up() {
    let files = files().await;
    let mcp = writing_mcp(&files).await;
    let server = serve(&files.state()).await;

    let mut links = Vec::new();
    let mut connections = Vec::new();
    for _ in 0..3 {
        let link = files.upload_link(&mcp);
        let mut connection = Connection::open(&server).await;
        connection
            .write_head("PUT", &link.link, &[("Content-Length", "1000")])
            .await;
        connection.write("begin").await.unwrap();
        links.push(link);
        connections.push(connection);
    }
    eventually(|| files.stored_files(&mcp).len() == 3).await;

    // The clients hang up before their files are whole.
    drop(connections);
    eventually(|| files.stored_files(&mcp).is_empty()).await;
    for link in &links {
        assert_eq!(
            files.uploads().find(mcp.id, &link.upload).await.unwrap(),
            None
        );
    }

    // Their places are free, and their links can be tried again.
    let next: Vec<_> = links
        .iter()
        .map(|link| upload(&files, &link.link, PDF))
        .collect();
    for response in next {
        assert_eq!(response.await.unwrap().status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn does_not_ask_a_client_that_waits_for_a_file_it_will_not_keep() {
    let files = files().await;
    let mcp = files.images_mcp(true).await;
    let link = files.upload_link_named(&mcp, "banner.png");
    let server = serve(&files.state()).await;

    // curl asks before it sends a large file, and is told at once.
    let mut connection = Connection::open(&server).await;
    connection
        .write_head(
            "PUT",
            &link.link,
            &[("Content-Length", "8000000"), ("Expect", "100-continue")],
        )
        .await;
    assert_eq!(connection.answer().await.told(), too_large("5.24288"));

    let mut connection = Connection::open(&server).await;
    connection
        .write_head(
            "PUT",
            &forged(&link.link),
            &[("Content-Length", "51"), ("Expect", "100-continue")],
        )
        .await;
    assert_eq!(connection.answer().await.told(), invalid());

    // A file that may be kept is asked for.
    let mut connection = Connection::open(&server).await;
    connection
        .write_head(
            "PUT",
            &link.link,
            &[("Content-Length", "51"), ("Expect", "100-continue")],
        )
        .await;
    assert_eq!(connection.head().await.status, StatusCode::CONTINUE);
    connection.write(PDF).await.unwrap();
    assert_eq!(connection.answer().await.status, StatusCode::CREATED);
    assert_eq!(
        files.uploaded(&mcp, &link.upload).await.as_deref(),
        Some(PDF)
    );
}
