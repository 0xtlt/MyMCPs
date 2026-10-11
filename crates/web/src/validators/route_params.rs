//! Route parameters. (`app/validators/route_params.ts`)

use std::sync::LazyLock;

use mymcps_vine as vine;
use serde_json::json;

/// `:id` of a stored record.
pub static RECORD_ID_PARAMS_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "id" => vine::number().without_decimals().positive() })
});

/// `:token` of an invite link, in the form `Invite::generate_token()`
/// writes: 32 random bytes in hexadecimal.
pub static INVITE_TOKEN_PARAMS_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "token" => vine::string().regex(vine::js::regex(r"^[0-9a-f]{64}$", "").expect("a static pattern")),
    })
});

/// The id in a path segment, or `None` when the segment is not one: a
/// handler answers that like a record that does not exist.
pub fn record_id(segment: &str) -> Option<i64> {
    let validated = RECORD_ID_PARAMS_VALIDATOR
        .validate(&json!({ "id": segment }))
        .ok()?;
    validated.get("id")?.as_i64()
}

/// The invite token in a path segment, or `None` when it is malformed.
pub fn invite_token(segment: &str) -> Option<String> {
    let validated = INVITE_TOKEN_PARAMS_VALIDATOR
        .validate(&json!({ "token": segment }))
        .ok()?;
    validated.get("token")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use mymcps_core::models::Invite;

    use super::*;

    #[test]
    fn a_record_id_is_a_whole_positive_number() {
        assert_eq!(record_id("12"), Some(12));
        assert_eq!(
            RECORD_ID_PARAMS_VALIDATOR
                .validate(&json!({ "id": 7 }))
                .unwrap(),
            json!({ "id": 7 })
        );
        for invalid in ["abc", "1.5", "-3", "12abc", "", "0"] {
            assert_eq!(record_id(invalid), None, "{invalid}");
        }
        assert!(RECORD_ID_PARAMS_VALIDATOR.validate(&json!({})).is_err());
    }

    #[test]
    fn an_invite_token_is_64_hexadecimal_characters() {
        let token = Invite::generate_token();
        assert_eq!(invite_token(&token), Some(token));
        for invalid in [
            "a".repeat(63),
            "g".repeat(64),
            "A".repeat(64),
            format!("{} ", "a".repeat(64)),
            String::new(),
        ] {
            assert_eq!(invite_token(&invalid), None, "{invalid}");
        }
    }
}
