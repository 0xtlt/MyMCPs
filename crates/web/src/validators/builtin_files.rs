//! Vine schema for the links to files of built-in MCPs.
//! (`app/validators/builtin_files.ts`)

use std::sync::LazyLock;

use mymcps_builtin::file_link::decode_file_reference;
use mymcps_vine as vine;
use serde_json::{Value, json};

/// Replace the path segment with what the tool put in the link.
fn file_reference_rule() -> vine::Rule {
    vine::rule(
        |value, field| match value.as_str().and_then(decode_file_reference) {
            Some(reference) => field.mutate(reference),
            None => field.report(
                "The {{ field }} field must be a file reference",
                "fileReference",
            ),
        },
    )
}

/// Route parameters of a file link. The signature of the link covers them,
/// so the request is only validated once it has been checked. What a
/// reference must contain is for the provider to say.
pub static BUILTIN_FILE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "params" => vine::object! {
            "id" => vine::number().without_decimals().positive(),
            "reference" => vine::string()
                .use_rule(file_reference_rule())
                .transform(|reference, _| reference),
        },
    })
});

/// What a file link names: an MCP, and what one of its tools put in the link.
#[derive(Debug, Clone, PartialEq)]
pub struct FileLink {
    pub id: i64,
    pub reference: Value,
}

/// The MCP and the reference in the two path segments of a file link, or
/// `None` when they are not what a link is made of: a handler answers that
/// like a file that is gone.
pub fn file_link(id: &str, reference: &str) -> Option<FileLink> {
    let validated = BUILTIN_FILE_VALIDATOR
        .validate(&json!({ "params": { "id": id, "reference": reference } }))
        .ok()?;
    let params = validated.get("params")?;
    Some(FileLink {
        id: params.get("id")?.as_i64()?,
        reference: params.get("reference").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;

    fn encoded(reference: &Value) -> String {
        URL_SAFE_NO_PAD.encode(reference.to_string())
    }

    fn attachment() -> Value {
        json!({ "mailbox": "INBOX", "uid": 11, "part": "2" })
    }

    #[test]
    fn reads_the_mcp_and_what_the_tool_put_in_the_link() {
        assert_eq!(
            BUILTIN_FILE_VALIDATOR
                .validate(&json!({
                    "params": { "id": "12", "reference": encoded(&attachment()) },
                    "signature": "ignored",
                }))
                .unwrap(),
            json!({ "params": { "id": 12, "reference": attachment() } })
        );
        // A reference is whatever the provider put there: its content is not checked here.
        assert_eq!(
            BUILTIN_FILE_VALIDATOR
                .validate(
                    &json!({ "params": { "id": "1", "reference": encoded(&json!(["x", 1])) } })
                )
                .unwrap(),
            json!({ "params": { "id": 1, "reference": ["x", 1] } })
        );
        assert_eq!(
            file_link("12", &encoded(&attachment())),
            Some(FileLink {
                id: 12,
                reference: attachment()
            })
        );
        assert_eq!(
            file_link("3", &encoded(&Value::Null)),
            Some(FileLink {
                id: 3,
                reference: Value::Null
            })
        );
    }

    #[test]
    fn refuses_an_mcp_that_is_not_a_positive_whole_number() {
        for id in ["abc", "0", "-1", "1.5", "12abc"] {
            let malformed = BUILTIN_FILE_VALIDATOR
                .validate(&json!({ "params": { "id": id, "reference": encoded(&attachment()) } }));
            assert!(malformed.is_err(), "{id}");
            assert_eq!(file_link(id, &encoded(&attachment())), None, "{id}");
        }
        let missing = BUILTIN_FILE_VALIDATOR
            .validate(&json!({ "params": { "reference": encoded(&attachment()) } }));
        assert!(missing.is_err());
    }

    #[test]
    fn refuses_a_reference_that_does_not_decode() {
        let cut = encoded(&attachment());
        let cut = &cut[..cut.len() - 3];
        for reference in ["not-a-reference", cut, "%%%"] {
            let malformed = BUILTIN_FILE_VALIDATOR
                .validate(&json!({ "params": { "id": "1", "reference": reference } }));
            let error = malformed.expect_err(reference);
            assert_eq!(error.messages[0].rule, "fileReference", "{reference}");
            assert_eq!(
                error.messages[0].message,
                "The reference field must be a file reference"
            );
            assert_eq!(file_link("1", reference), None, "{reference}");
        }
        let undefined = BUILTIN_FILE_VALIDATOR.validate(&json!({ "params": { "id": "1" } }));
        assert!(undefined.is_err());
        let missing = BUILTIN_FILE_VALIDATOR.validate(&json!({}));
        assert!(missing.is_err());
    }
}
