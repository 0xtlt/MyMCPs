//! Access tokens: what the Tokens page submits.
//! (the access token validators of `app/validators/mcp.ts`)

use std::sync::LazyLock;

use mymcps_vine as vine;
use vine::Operator;

fn access_token_payload() -> vine::VineObject {
    vine::object! {
        "name" => vine::string().trim().min_length(1).max_length(120),
        "scopeMode" => vine::enum_(["all", "selected"]),
        "mcpIds" => vine::array(vine::number().without_decimals().min(1))
            .min_length(1)
            .optional()
            .required_when("scopeMode", Operator::Eq, "selected"),
        "expiresAt" => vine::date_iso8601().nullable().optional(),
    }
}

/// Create an agent access token (identifier).
pub static CREATE_ACCESS_TOKEN_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(access_token_payload()));

/// Update an agent access token without rotating its secret.
pub static UPDATE_ACCESS_TOKEN_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(access_token_payload()));

/// Route parameters for access-token mutations.
pub static ACCESS_TOKEN_PARAMS_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "id" => vine::number().without_decimals().positive() })
});

/// Expired or revoked access tokens selected for permanent deletion.
pub static DELETE_ACCESS_TOKENS_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "ids" => vine::array(vine::number().without_decimals().positive()).min_length(1).max_length(500),
    })
});

/// The expiry a token form held, read again to show the form a second time.
pub static EXPIRY_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::date_iso8601()));

/// `?status=` of the token list: the tokens that work, or those that can be deleted.
pub static TOKEN_STATUS_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::enum_(["active", "inactive"])));

/// `?page=` of the token list, from 1.
pub static TOKEN_PAGE_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::number().without_decimals().positive()));

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn first_error(validator: &vine::Validator, input: serde_json::Value) -> (String, String) {
        let error = validator.validate(&input).unwrap_err();
        (
            error.messages[0].field.clone(),
            error.messages[0].message.clone(),
        )
    }

    #[test]
    fn reads_a_token_as_its_form_sends_it() {
        assert_eq!(
            CREATE_ACCESS_TOKEN_VALIDATOR
                .validate(
                    &json!({ "name": "  Cursor agent ", "scopeMode": "all", "expiresAt": null })
                )
                .unwrap(),
            json!({ "name": "Cursor agent", "scopeMode": "all", "expiresAt": null })
        );
        // Checked boxes arrive as text, and so does the instant the page script sends.
        assert_eq!(
            UPDATE_ACCESS_TOKEN_VALIDATOR
                .validate(&json!({
                    "name": "n8n",
                    "scopeMode": "selected",
                    "mcpIds": ["4", "9"],
                    "expiresAt": "2026-10-10T21:59:00.000Z",
                    "status": "inactive",
                }))
                .unwrap(),
            json!({
                "name": "n8n",
                "scopeMode": "selected",
                "mcpIds": [4, 9],
                "expiresAt": "2026-10-10T21:59:00.000Z",
            })
        );
        // Without the script a datetime-local field has no zone: it is read as UTC.
        assert_eq!(
            CREATE_ACCESS_TOKEN_VALIDATOR
                .validate(
                    &json!({ "name": "n8n", "scopeMode": "all", "expiresAt": "2026-10-10T21:59" })
                )
                .unwrap()["expiresAt"],
            json!("2026-10-10T21:59:00.000Z")
        );
    }

    #[test]
    fn refuses_a_token_with_the_messages_the_page_shows() {
        let validator = &*CREATE_ACCESS_TOKEN_VALIDATOR;
        assert_eq!(
            first_error(validator, json!({ "name": " ", "scopeMode": "all" })),
            ("name".into(), "The name field must be defined".into())
        );
        assert_eq!(
            first_error(
                validator,
                json!({ "name": "x".repeat(121), "scopeMode": "all" })
            ),
            (
                "name".into(),
                "The name field must not be greater than 120 characters".into()
            )
        );
        assert_eq!(
            first_error(validator, json!({ "name": "a", "scopeMode": "some" })),
            (
                "scopeMode".into(),
                "The selected scopeMode is invalid".into()
            )
        );
        assert_eq!(
            first_error(validator, json!({ "name": "a", "scopeMode": "selected" })),
            ("mcpIds".into(), "The mcpIds field must be defined".into())
        );
        assert_eq!(
            first_error(
                validator,
                json!({ "name": "a", "scopeMode": "selected", "mcpIds": [] })
            ),
            (
                "mcpIds".into(),
                "The mcpIds field must have at least 1 items".into()
            )
        );
        assert_eq!(
            first_error(
                validator,
                json!({ "name": "a", "scopeMode": "selected", "mcpIds": ["0"] })
            )
            .0,
            "mcpIds.0"
        );
        assert_eq!(
            first_error(
                validator,
                json!({ "name": "a", "scopeMode": "all", "expiresAt": "tomorrow" })
            )
            .0,
            "expiresAt"
        );
    }

    #[test]
    fn a_token_id_is_a_whole_positive_number() {
        assert_eq!(
            ACCESS_TOKEN_PARAMS_VALIDATOR
                .validate(&json!({ "id": "12" }))
                .unwrap(),
            json!({ "id": 12 })
        );
        for invalid in ["not-a-token-id", "1.5", "-3", "0", ""] {
            let error = ACCESS_TOKEN_PARAMS_VALIDATOR
                .validate(&json!({ "id": invalid }))
                .unwrap_err();
            assert_eq!(error.messages[0].field, "id", "{invalid}");
        }
    }

    #[test]
    fn a_deletion_names_between_one_and_five_hundred_tokens() {
        assert_eq!(
            DELETE_ACCESS_TOKENS_VALIDATOR
                .validate(&json!({ "ids": ["5", 6] }))
                .unwrap(),
            json!({ "ids": [5, 6] })
        );
        for invalid in [
            json!({}),
            json!({ "ids": [] }),
            json!({ "ids": "5" }),
            json!({ "ids": [0] }),
            json!({ "ids": ["abc"] }),
            json!({ "ids": (1..=501).collect::<Vec<i64>>() }),
        ] {
            assert!(
                DELETE_ACCESS_TOKENS_VALIDATOR.validate(&invalid).is_err(),
                "{invalid}"
            );
        }
        assert!(
            DELETE_ACCESS_TOKENS_VALIDATOR
                .validate(&json!({ "ids": (1..=500).collect::<Vec<i64>>() }))
                .is_ok()
        );
    }
}
