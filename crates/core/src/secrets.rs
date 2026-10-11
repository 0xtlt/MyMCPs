//! MCP secrets at rest: single values, and maps of named values encrypted
//! one by one (the environment of an npm MCP, the settings of a built-in one).

use serde_json::{Map, Value};

use crate::crypto::Encryption;

/// Encrypt a secret column. Nothing to store for an empty value.
pub fn encrypt_secret(encryption: &Encryption, value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| encryption.encrypt(value))
}

/// Decrypt a secret column. Ciphertext that is corrupt, or was written with
/// another APP_KEY, is treated as a missing secret.
pub fn decrypt_secret(encryption: &Encryption, value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .and_then(|value| encryption.decrypt(value))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Environment variable configuration is corrupted")]
    Corrupted,
    #[error("Environment variable \"{0}\" could not be decrypted")]
    Undecryptable(String),
}

/// One variable as a form submits it. A blank value keeps the saved one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentInput {
    pub name: String,
    pub value: Option<String>,
}

fn parse_secret_map(value: Option<&str>) -> Result<Vec<(String, String)>, EnvironmentError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(value) else {
        return Err(EnvironmentError::Corrupted);
    };
    entries
        .into_iter()
        .map(|(name, ciphertext)| match ciphertext {
            Value::String(ciphertext) if !ciphertext.is_empty() => Ok((name, ciphertext)),
            _ => Err(EnvironmentError::Corrupted),
        })
        .collect()
}

/// The names of the saved variables. Empty when the column cannot be read.
pub fn environment_names(value: Option<&str>) -> Vec<String> {
    parse_secret_map(value)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

pub fn environment_has_name(value: Option<&str>, name: &str) -> bool {
    environment_names(value).iter().any(|saved| saved == name)
}

/// The column after a form was saved: a variable with a value is encrypted
/// anew, one left blank keeps its saved ciphertext, and one that is neither
/// submitted nor saved is dropped.
pub fn merge_environment(
    encryption: &Encryption,
    current_value: Option<&str>,
    entries: &[EnvironmentInput],
) -> Option<String> {
    let current = parse_secret_map(current_value).unwrap_or_default();
    let mut next = Map::new();

    for entry in entries {
        match entry.value.as_deref().filter(|value| !value.is_empty()) {
            Some(value) => {
                next.insert(entry.name.clone(), Value::String(encryption.encrypt(value)));
            }
            None => {
                if let Some((_, ciphertext)) = current.iter().find(|(name, _)| *name == entry.name)
                {
                    next.insert(entry.name.clone(), Value::String(ciphertext.clone()));
                }
            }
        }
    }

    (!next.is_empty()).then(|| Value::Object(next).to_string())
}

/// Every saved variable, decrypted, in the order saved.
pub fn decrypt_environment(
    encryption: &Encryption,
    value: Option<&str>,
) -> Result<Vec<(String, String)>, EnvironmentError> {
    parse_secret_map(value)?
        .into_iter()
        .map(|(name, ciphertext)| match encryption.decrypt(&ciphertext) {
            Some(plaintext) => Ok((name, plaintext)),
            None => Err(EnvironmentError::Undecryptable(name)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEST_APP_KEY;

    fn input(name: &str, value: Option<&str>) -> EnvironmentInput {
        EnvironmentInput {
            name: name.into(),
            value: value.map(str::to_string),
        }
    }

    #[test]
    fn empty_secrets_are_not_stored() {
        let encryption = Encryption::new(TEST_APP_KEY);
        assert_eq!(encrypt_secret(&encryption, None), None);
        assert_eq!(encrypt_secret(&encryption, Some("")), None);
        assert_eq!(decrypt_secret(&encryption, None), None);
        assert_eq!(decrypt_secret(&encryption, Some("")), None);
        assert_eq!(decrypt_secret(&encryption, Some("not ciphertext")), None);

        let stored = encrypt_secret(&encryption, Some("sk-live-123")).unwrap();
        assert_ne!(stored, "sk-live-123");
        assert_eq!(
            decrypt_secret(&encryption, Some(&stored)).as_deref(),
            Some("sk-live-123")
        );
        assert_eq!(
            decrypt_secret(&Encryption::new("another-key-of-16-chars"), Some(&stored)),
            None
        );
    }

    #[test]
    fn merges_keeps_and_drops_variables() {
        let encryption = Encryption::new(TEST_APP_KEY);
        let saved = merge_environment(
            &encryption,
            None,
            &[
                input("API_KEY", Some("one")),
                input("REGION", Some("eu")),
                input("EMPTY", Some("")),
            ],
        )
        .unwrap();
        assert_eq!(environment_names(Some(&saved)), ["API_KEY", "REGION"]);
        assert!(environment_has_name(Some(&saved), "REGION"));
        assert!(!environment_has_name(Some(&saved), "EMPTY"));
        assert!(!saved.contains("one"));

        // A blank value keeps the saved one, a new value replaces it, and a
        // variable left out of the form is removed.
        let next = merge_environment(
            &encryption,
            Some(&saved),
            &[
                input("REGION", None),
                input("API_KEY", Some("two")),
                input("UNKNOWN", None),
            ],
        )
        .unwrap();
        assert_eq!(
            decrypt_environment(&encryption, Some(&next)).unwrap(),
            [
                ("REGION".to_string(), "eu".to_string()),
                ("API_KEY".to_string(), "two".to_string())
            ]
        );
        assert_eq!(merge_environment(&encryption, Some(&next), &[]), None);
    }

    #[test]
    fn reports_corrupt_and_undecryptable_environments() {
        let encryption = Encryption::new(TEST_APP_KEY);
        for corrupt in ["[]", "\"text\"", "{\"A\":1}", "{\"A\":\"\"}", "{not json"] {
            assert_eq!(environment_names(Some(corrupt)), Vec::<String>::new());
            assert_eq!(
                decrypt_environment(&encryption, Some(corrupt)),
                Err(EnvironmentError::Corrupted),
                "{corrupt}"
            );
        }
        assert_eq!(decrypt_environment(&encryption, None), Ok(Vec::new()));

        let error =
            decrypt_environment(&encryption, Some("{\"API_KEY\":\"garbage\"}")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Environment variable \"API_KEY\" could not be decrypted"
        );
        assert_eq!(
            EnvironmentError::Corrupted.to_string(),
            "Environment variable configuration is corrupted"
        );
    }
}
