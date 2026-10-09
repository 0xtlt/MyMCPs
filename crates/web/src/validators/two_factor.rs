//! Two-step verification: the codes typed at sign-in, and the forms of the
//! Settings page that set it up.

use std::sync::LazyLock;

use mymcps_vine as vine;

const TOTP_CODE_MESSAGE: &str = "Enter the 6-digit code of your authenticator app";
const RECOVERY_CODE_MESSAGE: &str = "Enter one of your recovery codes";
const PASSKEY_NAME_MESSAGE: &str = "Name the passkey, in 60 characters at most";
const CREDENTIAL_MESSAGE: &str = "Your browser did not return a passkey. Try again.";

fn passkey_name() -> vine::VineString {
    vine::string().trim().min_length(1).max_length(60)
}

fn passkey_name_messages() -> [(&'static str, &'static str); 4] {
    [
        ("name.required", PASSKEY_NAME_MESSAGE),
        ("name.string", PASSKEY_NAME_MESSAGE),
        ("name.minLength", PASSKEY_NAME_MESSAGE),
        ("name.maxLength", PASSKEY_NAME_MESSAGE),
    ]
}

/// A code of an authenticator app: six digits, which apps often show as
/// two groups of three.
pub static TOTP_CODE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "code" => vine::string().trim().regex(
                vine::js::regex(r"^[0-9]{3}[ -]?[0-9]{3}$", "").expect("a static pattern"),
            ),
        })
        .messages_provider(vine::SimpleMessagesProvider::new([
            ("code.required", TOTP_CODE_MESSAGE),
            ("code.string", TOTP_CODE_MESSAGE),
            ("code.regex", TOTP_CODE_MESSAGE),
        ]))
});

/// A recovery code: sixteen letters and digits, in four groups or not.
pub static RECOVERY_CODE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "code" => vine::string().trim().regex(
                vine::js::regex(r"^[0-9a-z]{4}([ -]?[0-9a-z]{4}){3}$", "i").expect("a static pattern"),
            ),
        })
        .messages_provider(vine::SimpleMessagesProvider::new([
            ("code.required", RECOVERY_CODE_MESSAGE),
            ("code.string", RECOVERY_CODE_MESSAGE),
            ("code.regex", RECOVERY_CODE_MESSAGE),
        ]))
});

/// A change to two-step verification that only needs the account password:
/// turning the authenticator app on or off, new recovery codes, removing a
/// passkey.
pub static CONFIRM_PASSWORD_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! { "currentPassword" => vine::string().min_length(1) })
        .messages_provider(vine::SimpleMessagesProvider::new([(
            "currentPassword.required",
            "Enter your current password",
        )]))
});

/// The first half of adding a passkey: its name, and the account password.
pub static PASSKEY_OPTIONS_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    let mut messages = passkey_name_messages().to_vec();
    messages.push(("currentPassword.required", "Enter your current password"));
    vine::global()
        .create(vine::object! {
            "name" => passkey_name(),
            "currentPassword" => vine::string().min_length(1),
        })
        .messages_provider(vine::SimpleMessagesProvider::new(messages))
});

/// The answer of the browser to a passkey ceremony, as the page's script
/// serialises it.
pub static PASSKEY_CREDENTIAL_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "credential" => vine::string().min_length(2).max_length(16_384),
        })
        .messages_provider(vine::SimpleMessagesProvider::new([
            ("credential.required", CREDENTIAL_MESSAGE),
            ("credential.string", CREDENTIAL_MESSAGE),
            ("credential.minLength", CREDENTIAL_MESSAGE),
            ("credential.maxLength", CREDENTIAL_MESSAGE),
        ]))
});

/// Rename a passkey.
pub static RENAME_PASSKEY_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! { "name" => passkey_name() })
        .messages_provider(vine::SimpleMessagesProvider::new(passkey_name_messages()))
});

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn code(validator: &vine::Validator, input: Value) -> Result<String, String> {
        validator
            .validate(&input)
            .map(|valid| valid["code"].as_str().unwrap().to_string())
            .map_err(|error| error.messages[0].message.clone())
    }

    #[test]
    fn an_authenticator_code_is_six_digits() {
        for valid in ["123456", " 123 456 ", "123-456"] {
            assert!(
                code(&TOTP_CODE_VALIDATOR, json!({ "code": valid })).is_ok(),
                "{valid}"
            );
        }
        for invalid in [
            json!({}),
            json!({ "code": "12345" }),
            json!({ "code": "1234567" }),
            json!({ "code": "12a456" }),
        ] {
            assert_eq!(
                code(&TOTP_CODE_VALIDATOR, invalid),
                Err(TOTP_CODE_MESSAGE.to_string())
            );
        }
    }

    #[test]
    fn a_recovery_code_is_four_groups_of_four() {
        for valid in [
            "ab01-cd23-ef45-gh67",
            "AB01CD23EF45GH67",
            "ab01 cd23 ef45 gh67",
        ] {
            assert!(
                code(&RECOVERY_CODE_VALIDATOR, json!({ "code": valid })).is_ok(),
                "{valid}"
            );
        }
        for invalid in ["ab01-cd23-ef45", "ab01-cd23-ef45-gh6!", "123456"] {
            assert_eq!(
                code(&RECOVERY_CODE_VALIDATOR, json!({ "code": invalid })),
                Err(RECOVERY_CODE_MESSAGE.to_string()),
                "{invalid}"
            );
        }
    }

    #[test]
    fn a_passkey_has_a_short_name() {
        assert!(
            RENAME_PASSKEY_VALIDATOR
                .validate(&json!({ "name": "MacBook" }))
                .is_ok()
        );
        assert!(
            RENAME_PASSKEY_VALIDATOR
                .validate(&json!({ "name": "x".repeat(61) }))
                .is_err()
        );
        assert!(
            PASSKEY_OPTIONS_VALIDATOR
                .validate(&json!({ "name": "MacBook" }))
                .is_err()
        );
    }
}
