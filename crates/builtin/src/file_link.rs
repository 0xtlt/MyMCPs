//! Temporary links to the files of built-in MCPs. The signature in a link is
//! its only credential.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use mymcps_core::Core;
use mymcps_core::crypto::{sign_path, verify_signed_path};
use serde_json::Value;

use crate::error::{BuiltinError, BuiltinResult};

/// Keeps a signature issued for anything else from opening a file.
pub const BUILTIN_FILE_PURPOSE: &str = "builtin_file";

/// A link that downloads a file must not be one that stores a file, nor the reverse.
pub const BUILTIN_UPLOAD_PURPOSE: &str = "builtin_upload";

/// The path of a download link: `GET /files/{mcp_id}/{reference}`.
pub fn file_path(mcp_id: i64, encoded_reference: &str) -> String {
    format!("/files/{mcp_id}/{encoded_reference}")
}

/// The path of an upload link: `PUT /uploads/{mcp_id}/{reference}`.
pub fn upload_path(mcp_id: i64, encoded_reference: &str) -> String {
    format!("/uploads/{mcp_id}/{encoded_reference}")
}

fn signed_link(
    core: &Core,
    path: String,
    purpose: &str,
    expires_in: Duration,
) -> BuiltinResult<String> {
    let app_url = core.config.require_public_app_url().map_err(|_| {
        BuiltinError::tool(
            "File links need the public address of this MyMCPs instance. An administrator must set APP_URL.",
        )
    })?;
    let expires_at =
        Utc::now() + chrono::Duration::from_std(expires_in).unwrap_or(chrono::Duration::zero());
    let signature = sign_path(&core.encryption, &path, purpose, expires_at);
    Ok(format!("{app_url}{path}?signature={signature}"))
}

fn encode_reference(reference: &Value) -> String {
    URL_SAFE_NO_PAD.encode(reference.to_string())
}

/// A temporary link to a file of a built-in MCP, for whoever holds it: the
/// agent, or the person the agent gives it to. `reference` tells the
/// provider which file to serve and cannot be changed without breaking the
/// signature.
pub fn builtin_file_url(
    core: &Core,
    mcp_id: i64,
    reference: &Value,
    expires_in: Duration,
) -> BuiltinResult<String> {
    signed_link(
        core,
        file_path(mcp_id, &encode_reference(reference)),
        BUILTIN_FILE_PURPOSE,
        expires_in,
    )
}

/// A temporary link that takes one file for a built-in MCP, from whoever
/// holds it. `reference` tells the provider what the file is for, and is
/// signed like the reference of a download.
pub fn builtin_upload_url(
    core: &Core,
    mcp_id: i64,
    reference: &Value,
    expires_in: Duration,
) -> BuiltinResult<String> {
    signed_link(
        core,
        upload_path(mcp_id, &encode_reference(reference)),
        BUILTIN_UPLOAD_PURPOSE,
        expires_in,
    )
}

/// `None` when the path segment is not a reference encoded above.
pub fn decode_file_reference(value: &str) -> Option<Value> {
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Whether the request for `path` carries a signature issued for it and for
/// this purpose, which has not expired.
pub fn has_valid_signature(
    core: &Core,
    path: &str,
    purpose: &str,
    signature: Option<&str>,
) -> bool {
    signature
        .is_some_and(|signature| verify_signed_path(&core.encryption, path, purpose, signature))
}

#[cfg(test)]
mod tests {
    use mymcps_core::TestCore;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn links_carry_their_reference_and_a_signature_for_their_purpose() {
        let core = TestCore::new().await;
        let reference = json!({ "mailbox": "INBOX", "uid": 42, "part": "2" });

        let link = builtin_file_url(&core, 7, &reference, Duration::from_secs(900)).unwrap();
        let (path, signature) = link
            .strip_prefix("http://localhost:3333")
            .unwrap()
            .split_once("?signature=")
            .unwrap();
        let encoded = path.strip_prefix("/files/7/").unwrap();
        assert_eq!(decode_file_reference(encoded), Some(reference.clone()));

        assert!(has_valid_signature(
            &core,
            path,
            BUILTIN_FILE_PURPOSE,
            Some(signature)
        ));
        assert!(!has_valid_signature(
            &core,
            path,
            BUILTIN_UPLOAD_PURPOSE,
            Some(signature)
        ));
        assert!(!has_valid_signature(
            &core,
            &file_path(8, encoded),
            BUILTIN_FILE_PURPOSE,
            Some(signature)
        ));
        assert!(!has_valid_signature(
            &core,
            path,
            BUILTIN_FILE_PURPOSE,
            None
        ));

        let upload = builtin_upload_url(&core, 7, &reference, Duration::from_secs(900)).unwrap();
        assert!(upload.starts_with("http://localhost:3333/uploads/7/"));
        let expired = builtin_file_url(&core, 7, &reference, Duration::ZERO).unwrap();
        let (path, signature) = expired
            .strip_prefix("http://localhost:3333")
            .unwrap()
            .split_once("?signature=")
            .unwrap();
        assert!(!has_valid_signature(
            &core,
            path,
            BUILTIN_FILE_PURPOSE,
            Some(signature)
        ));

        for not_a_reference in ["", "%%%", "bm90IGpzb24", "a b"] {
            assert_eq!(
                decode_file_reference(not_a_reference),
                None,
                "{not_a_reference}"
            );
        }
    }

    #[tokio::test]
    async fn links_need_the_public_address_of_the_instance() {
        let core = TestCore::with_config(|config| config.app_url = None).await;
        let error = builtin_file_url(&core, 7, &json!({}), Duration::from_secs(60)).unwrap_err();
        assert!(error.is_tool_error());
        assert_eq!(
            error.to_string(),
            "File links need the public address of this MyMCPs instance. An administrator must set APP_URL."
        );
    }
}
