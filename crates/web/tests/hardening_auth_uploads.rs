//! Port of `tests/functional/hardening_auth_uploads.spec.ts`.

use std::path::{Path, PathBuf};

use http::StatusCode;
use mymcps_core::crypto::random_hex;
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::create_admin_with;
use serde_json::Value;

/// Files under `directory` that hold the uploaded content. A body parser
/// would name them as it pleases, so they are found by what they contain.
fn uploaded_copies(directory: &Path, content: &[u8], depth: usize, copies: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Other processes create and remove their own files in the meantime.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() && depth > 0 {
            uploaded_copies(&path, content, depth - 1, copies);
        } else if metadata.is_file()
            && metadata.len() == content.len() as u64
            && std::fs::read(&path).is_ok_and(|stored| stored == content)
        {
            copies.push(path);
        }
    }
}

fn multipart(boundary: &str, fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> Vec<u8> {
    let mut body = Vec::new();
    let (name, filename, content) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(b"\r\n");
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn does_not_write_an_anonymous_multipart_upload_to_disk() {
    let app = TestApp::new().await;
    create_admin_with(&app, "admin@example.com").await;
    let content = format!("mymcps-upload-{}", random_hex(16))
        .repeat(64)
        .into_bytes();
    let body = multipart(
        "----mymcps-test-boundary",
        &[("email", "admin@example.com"), ("password", "password123")],
        ("attachment", "attachment.bin", &content),
    );

    let response = app
        .post("/login")
        .csrf()
        .raw_body(
            body,
            "multipart/form-data; boundary=----mymcps-test-boundary",
        )
        .send()
        .await;

    // Neither where a body parser would put it, nor anywhere the server writes.
    let mut copies = Vec::new();
    uploaded_copies(&std::env::temp_dir(), &content, 0, &mut copies);
    uploaded_copies(&app.core.config.data_dir, &content, 8, &mut copies);
    assert_eq!(copies, Vec::<PathBuf>::new());

    // The body is not parsed at all, so the login form sees no credentials.
    assert_eq!(response.status, StatusCode::FOUND);
    let errors = response.flashed("errors").unwrap_or(Value::Null);
    assert!(errors.get("email").is_some(), "{errors}");
    assert!(!response.session().contains_key("auth_web"));
}
