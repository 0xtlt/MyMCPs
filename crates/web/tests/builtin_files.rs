//! Port of `tests/functional/vine_builtin_files.spec.ts` and of the
//! attachment link cases of `tests/functional/builtin_icloud_mail_mcp.spec.ts`,
//! for what happens once a link is opened. What a tool puts in a link is
//! tested with the provider that has the tool.

#[path = "support/builtin_files.rs"]
mod support;

use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use http::{Method, StatusCode};
use mymcps_builtin::file_link::{BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE};
use mymcps_core::models::McpTransport;
use serde_json::json;
use support::{
    ATTACHMENT, INVALID_LINK, UNAVAILABLE, attachment, encoded, files, forged, path_of, request,
    send, signature_of, unsigned,
};

fn refused() -> (u16, String) {
    (403, INVALID_LINK.to_owned())
}

fn unavailable() -> (u16, String) {
    (404, UNAVAILABLE.to_owned())
}

#[tokio::test]
async fn serves_the_file_behind_a_link_without_signing_in() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);

    let (path, query) = link.split_once('?').unwrap();
    let reference = path.strip_prefix(&format!("/files/{}/", mcp.id)).unwrap();
    assert!(
        reference
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
    );
    assert!(query.starts_with("signature=") && !query.contains('&'));
    // The link tells an onlooker nothing about the mailbox.
    assert!(!link.contains("INBOX"));
    assert!(!link.contains("thomas@example.com"));

    // No cookie, no access token: the signature is the credential.
    let download = files.get(&link).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.text(), ATTACHMENT);
    assert_eq!(download.header("content-type"), Some("application/pdf"));
    assert_eq!(
        download.header("content-disposition"),
        Some("attachment; filename=\"Menu _t_.pdf\"; filename*=UTF-8''Menu%20%C3%A9t%C3%A9.pdf")
    );
    assert_eq!(download.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        download.header("content-security-policy"),
        Some("sandbox; default-src 'none'")
    );
    assert_eq!(download.header("cache-control"), Some("private, no-store"));
    assert_eq!(
        download.header("content-length"),
        Some(ATTACHMENT.len().to_string().as_str())
    );
    // Nothing of a session comes with a file.
    assert_eq!(download.header("set-cookie"), None);
    assert_eq!(download.header("x-frame-options"), Some("DENY"));
    assert_eq!(files.mailbox.sign_ins(), 1);
    assert_eq!(files.mailbox.sign_outs(), 1);
}

#[tokio::test]
async fn refuses_a_link_that_was_changed_has_expired_or_was_signed_for_something_else() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);
    let signature = signature_of(&link);

    let other_message = files.file_link(
        mcp.id,
        &json!({ "mailbox": "INBOX", "uid": 14, "part": "2" }),
    );
    let swapped = format!("{}?signature={signature}", path_of(&other_message));
    assert_eq!(files.get(&swapped).await.told(), refused());

    let other_mcp = link.replace(
        &format!("/files/{}/", mcp.id),
        &format!("/files/{}/", mcp.id + 1),
    );
    assert_eq!(files.get(&other_mcp).await.told(), refused());

    assert_eq!(files.get(&unsigned(&link)).await.told(), refused());
    assert_eq!(files.get(&forged(&link)).await.told(), refused());

    let extra = format!("{link}&download=1");
    assert_eq!(files.get(&extra).await.told(), refused());
    let extra_first = format!("{}?download=1&signature={signature}", path_of(&link));
    assert_eq!(files.get(&extra_first).await.told(), refused());
    let twice = format!("{link}&signature={signature}");
    assert_eq!(files.get(&twice).await.told(), refused());
    let empty = format!("{}?signature=", path_of(&link));
    assert_eq!(files.get(&empty).await.told(), refused());

    let expired = files.file_link_for(mcp.id, &attachment(), Duration::from_millis(1));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(files.get(&expired).await.told(), refused());

    // Signed by us, but to take a file.
    let for_uploads = files.signed(path_of(&link), BUILTIN_UPLOAD_PURPOSE);
    assert_eq!(files.get(&for_uploads).await.told(), refused());
    let upload = files.upload_link_to(mcp.id, &attachment(), Duration::from_secs(60));
    let moved = upload.replace("/uploads/", "/files/");
    assert_eq!(files.get(&moved).await.told(), refused());

    // Nothing above reached the account, and the untouched link still works.
    assert_eq!(files.mailbox.sign_ins(), 0);
    let untouched = files.get(&link).await;
    assert_eq!(untouched.status, StatusCode::OK);
    assert_eq!(untouched.header("content-type"), Some("application/pdf"));
    let refusal = files.get(&forged(&link)).await;
    assert_eq!(
        refusal.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
}

#[tokio::test]
async fn only_serves_what_the_provider_says_is_a_file() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;

    // A signed link to a part that is not an attachment serves nothing either.
    let body = files.file_link(
        mcp.id,
        &json!({ "mailbox": "INBOX", "uid": 11, "part": "1.1" }),
    );
    assert_eq!(files.get(&body).await.told(), unavailable());
    let garbage = files.file_link(mcp.id, &json!("INBOX"));
    assert_eq!(files.get(&garbage).await.told(), unavailable());
    let nothing = files.file_link(mcp.id, &json!(null));
    assert_eq!(files.get(&nothing).await.told(), unavailable());

    // An MCP whose provider hands out no file has none behind a link.
    let images = files.images_mcp(true).await;
    let link = files.file_link(images.id, &attachment());
    assert_eq!(files.get(&link).await.told(), unavailable());
}

#[tokio::test]
async fn stops_serving_a_link_once_the_mcp_no_longer_allows_it() {
    let files = files().await;
    let mut mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);
    assert_eq!(files.get(&link).await.status, StatusCode::OK);

    mcp.builtin_permissions = Some("send".to_owned());
    files.save(&mut mcp).await;
    assert_eq!(files.get(&link).await.told(), unavailable());

    mcp.builtin_permissions = Some("read".to_owned());
    mcp.enabled = false;
    files.save(&mut mcp).await;
    assert_eq!(files.get(&link).await.told(), unavailable());

    mcp.enabled = true;
    files.save(&mut mcp).await;
    assert_eq!(files.get(&link).await.status, StatusCode::OK);

    // The same row as another kind of MCP serves no file.
    mcp.transport = McpTransport::Http;
    mcp.http_url = Some("http://127.0.0.1:9999/mcp".to_owned());
    files.save(&mut mcp).await;
    assert_eq!(files.get(&link).await.told(), unavailable());
    mcp.transport = McpTransport::Builtin;
    files.save(&mut mcp).await;
    assert_eq!(files.get(&link).await.status, StatusCode::OK);

    mcp.delete(&*files.app.core.db).await.unwrap();
    assert_eq!(files.get(&link).await.told(), unavailable());
}

#[tokio::test]
async fn answers_a_signed_link_to_malformed_parameters_like_a_file_that_is_gone() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;
    // Links with our signature, to parameters no tool would ever write.
    let signed = |id: &str, reference: &str| {
        files.signed(&format!("/files/{id}/{reference}"), BUILTIN_FILE_PURPOSE)
    };
    let id = mcp.id.to_string();

    for link in [
        signed(&id, "not-a-reference"),
        signed(
            &id,
            &encoded(&json!({ "mailbox": "INBOX", "uid": "x", "part": "2" })),
        ),
        signed(&id, &encoded(&json!("INBOX/11/2"))),
        signed("abc", &encoded(&attachment())),
        signed(&format!("{id}.5"), &encoded(&attachment())),
        signed("0", &encoded(&attachment())),
        signed("99999999999999999999", &encoded(&attachment())),
    ] {
        let response = files.get(&link).await;
        assert_eq!(response.told(), unavailable(), "{link}");
        assert_eq!(
            response.header("content-type"),
            Some("text/plain; charset=utf-8")
        );
    }
    // None of them reached the account.
    assert_eq!(files.mailbox.sign_ins(), 0);
    // Nor do they count as downloads, however many there are.
    let malformed = signed(&id, "not-a-reference");
    for _ in 0..70 {
        assert_eq!(files.get(&malformed).await.status, StatusCode::NOT_FOUND);
    }

    let download = files.get(&files.attachment_link(&mcp)).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.text(), ATTACHMENT);
}

#[tokio::test]
async fn still_answers_a_link_without_our_signature_first_whatever_its_parameters() {
    let files = files().await;
    let link = files.signed("/files/abc/not-a-reference", BUILTIN_FILE_PURPOSE);

    let response = files.get(&forged(&link)).await;
    assert_eq!(response.told(), refused());
    // A path that is not even text is refused the same way.
    let response = files.get("/files/%ff/%fe?signature=forged").await;
    assert_eq!(response.told(), refused());
}

#[tokio::test]
async fn answers_links_itself_whether_or_not_the_instance_is_set_up() {
    // No administrator yet: the pages send their visitors to the first-run
    // setup, and a link is still answered for what it is.
    let app = mymcps_web::testing::TestApp::new().await;
    assert_eq!(
        app.get("/").send().await.redirect_path().as_deref(),
        Some("/onboarding")
    );

    let download = app.get("/files/1/e30?signature=forged").send().await;
    assert_eq!(download.status, StatusCode::FORBIDDEN);
    assert_eq!(download.text(), INVALID_LINK);
    let upload = app
        .put("/uploads/1/e30?signature=forged")
        .raw_body("x", "application/pdf")
        .send()
        .await;
    assert_eq!(upload.status, StatusCode::FORBIDDEN);
    assert_eq!(upload.text(), INVALID_LINK);
}

#[tokio::test]
async fn sends_a_file_of_a_doubtful_type_as_bytes_to_save() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);

    // The file was written by a stranger, and so was what it says it is.
    for doubtful in [
        "text/html; charset=utf-8",
        "",
        "pdf",
        "text/plain\r\nX-Injected: 1",
    ] {
        files.mailbox.label_attachment(doubtful);
        let download = files.get(&link).await;
        assert_eq!(download.status, StatusCode::OK, "{doubtful:?}");
        assert_eq!(
            download.header("content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(download.header("x-injected"), None);
    }

    files.mailbox.label_attachment("image/svg+xml");
    files
        .mailbox
        .name_attachment("a\"; filename=\"b.exe\r\nSet-Cookie: x=1");
    let download = files.get(&link).await;
    assert_eq!(download.header("content-type"), Some("image/svg+xml"));
    assert_eq!(
        download.header("content-disposition"),
        Some(
            "attachment; filename=\"a_; filename=_b.exe__Set-Cookie: x=1\"; filename*=UTF-8''a_%3B%20filename%3D_b.exe__Set-Cookie%3A%20x%3D1"
        )
    );
    assert_eq!(download.header("set-cookie"), None);
}

#[tokio::test]
async fn answers_a_head_request_like_a_download_without_the_file() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read"]).await;
    let link = files.attachment_link(&mcp);

    for _ in 0..4 {
        let head = send(
            &files.state(),
            request(Method::HEAD, &link).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(head.body, Bytes::new());
        assert_eq!(head.header("content-type"), Some("application/pdf"));
        assert_eq!(
            head.header("content-length"),
            Some(ATTACHMENT.len().to_string().as_str())
        );
    }
    // Each one signed in to the account, and none kept its place.
    assert_eq!(files.mailbox.sign_ins(), 4);
    assert_eq!(files.get(&link).await.status, StatusCode::OK);
}

#[tokio::test]
async fn answers_any_other_method_like_a_path_that_does_not_exist() {
    let files = files().await;
    let mcp = files.mailbox_mcp(&["read", "send"]).await;
    let link = files.attachment_link(&mcp);
    let upload = files.upload_link(&mcp);

    for (method, link) in [
        (Method::POST, &link),
        (Method::PUT, &link),
        (Method::DELETE, &link),
        // A file cannot be read back from the upload route either.
        (Method::GET, &upload.link),
        (Method::HEAD, &upload.link),
        (Method::POST, &upload.link),
    ] {
        let answer = send(
            &files.state(),
            request(method.clone(), link).body(Body::from("x")).unwrap(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{method} {link}");
        assert_eq!(answer.header("allow"), None, "{method} {link}");
    }
    let answer = send(
        &files.state(),
        request(Method::POST, &link).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(
        answer.json(),
        json!({ "message": format!("Cannot POST:{}", path_of(&link)) })
    );
    assert_eq!(files.mailbox.sign_ins(), 0);
    assert!(files.stored_files(&mcp).is_empty());
}

#[tokio::test]
async fn fails_without_keeping_a_place_when_the_mcp_is_of_a_kind_nobody_knows() {
    let files = files().await;
    let mut mcp = files.mailbox_mcp(&["read", "send"]).await;
    let link = files.attachment_link(&mcp);
    let upload = files.upload_link(&mcp);
    mcp.builtin_key = Some("gone".to_owned());
    files.save(&mut mcp).await;

    // More often than an MCP serves files at once.
    for _ in 0..5 {
        let download = files.get(&link).await;
        assert_eq!(download.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            download.json(),
            json!({ "message": "Internal server error" })
        );
        let sent = files.put(&upload.link, "x").await;
        assert_eq!(sent.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(sent.json(), json!({ "message": "Internal server error" }));
    }
    assert!(files.stored_files(&mcp).is_empty());
}
