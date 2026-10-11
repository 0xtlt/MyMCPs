//! Backups: the export form of the Settings page, and the import form of
//! the setup screen.

use std::sync::LazyLock;

use mymcps_vine as vine;

/// Export a backup. The password is the one of the file, and has nothing to
/// do with an account: it may be longer than one. `currentPassword` is the
/// account password of the administrator who asks.
pub static EXPORT_BACKUP_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "password" => vine::string().min_length(8).max_length(128).confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
        "currentPassword" => vine::string().min_length(1),
    })
});

/// What an import needs, once its form was read: `backup` is the size in
/// bytes of the file that was sent, missing when there was none, and
/// `password` is not judged here: only the file can tell whether it is right.
pub static IMPORT_BACKUP_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "backup" => vine::number().min(1),
            "password" => vine::string(),
        })
        .messages_provider(vine::SimpleMessagesProvider::new([
            ("backup.required", "Choose a backup file"),
            ("backup.number", "Choose a backup file"),
            ("backup.min", "Choose a backup file"),
            ("password.required", "Enter the password of the backup"),
            ("password.string", "Enter the password of the backup"),
        ]))
});

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn messages(validator: &vine::Validator, input: serde_json::Value) -> Vec<(String, String)> {
        match validator.validate(&input) {
            Ok(_) => Vec::new(),
            Err(error) => error
                .messages
                .into_iter()
                .map(|message| (message.field, message.message))
                .collect(),
        }
    }

    #[test]
    fn asks_for_a_file_and_for_its_password() {
        let field = |name: &str, message: &str| (name.to_string(), message.to_string());
        assert_eq!(
            messages(&IMPORT_BACKUP_VALIDATOR, json!({})),
            [
                field("backup", "Choose a backup file"),
                field("password", "Enter the password of the backup"),
            ]
        );
        assert_eq!(
            messages(
                &IMPORT_BACKUP_VALIDATOR,
                json!({ "backup": 0, "password": "   " })
            ),
            [
                field("backup", "Choose a backup file"),
                field("password", "Enter the password of the backup"),
            ]
        );
        assert_eq!(
            messages(
                &IMPORT_BACKUP_VALIDATOR,
                json!({ "backup": 159, "password": "x" })
            ),
            []
        );
    }

    #[test]
    fn takes_a_backup_password_of_8_to_128_characters_typed_twice() {
        let export = |password: &str, confirmation: &str| {
            messages(
                &EXPORT_BACKUP_VALIDATOR,
                json!({
                    "password": password,
                    "passwordConfirmation": confirmation,
                    "currentPassword": "x",
                }),
            )
        };
        let long = "p".repeat(128);
        assert_eq!(export("12345678", "12345678"), []);
        assert_eq!(export(&long, &long), []);
        assert_eq!(
            export("1234567", "1234567")[0].1,
            "The password field must have at least 8 characters"
        );
        let longer = "p".repeat(129);
        assert_eq!(
            export(&longer, &longer)[0].1,
            "The password field must not be greater than 128 characters"
        );
        assert_eq!(
            export("12345678", "12345679")[0].1,
            "The password field and passwordConfirmation field must be the same"
        );
        assert_eq!(
            messages(
                &EXPORT_BACKUP_VALIDATOR,
                json!({ "password": "12345678", "passwordConfirmation": "12345678" })
            )[0],
            (
                "currentPassword".to_string(),
                "The currentPassword field must be defined".to_string()
            )
        );
    }
}
