//! The OAuth clients a team saves on the Credentials page: what its dialog
//! submits, and the `{id}` of a credential in a path.

use std::sync::LazyLock;

use mymcps_vine as vine;
use serde::Deserialize;

/// The longest name of a credential.
pub const MAX_NAME_LENGTH: usize = 120;

/// The providers whose OAuth clients a team saves, as the `provider` column
/// names them.
pub const CREDENTIAL_PROVIDERS: &[&str] = &["google"];

/// What the person reads when a Google Client ID is not one.
pub const GOOGLE_CLIENT_ID_HINT: &str = "The Google Client ID ends in .apps.googleusercontent.com";

/// The name of a credential, also asked for when one is added from the
/// dialog of an MCP.
pub fn credential_name() -> vine::VineString {
    vine::string()
        .trim()
        .min_length(1)
        .max_length(MAX_NAME_LENGTH)
}

fn google_client_id() -> vine::VineString {
    vine::string().trim().min_length(1).max_length(254).regex(
        vine::js::regex(r"^[\w-]+\.apps\.googleusercontent\.com$", "").expect("a static pattern"),
    )
}

fn messages() -> vine::SimpleMessagesProvider {
    vine::SimpleMessagesProvider::new([
        ("name.required", "Enter a name"),
        ("clientId.required", "Enter the Client ID"),
        ("clientId.regex", GOOGLE_CLIENT_ID_HINT),
        ("clientSecret.required", "Enter the Client Secret"),
    ])
}

/// What the dialog that adds a credential sends: the secret is required.
pub static CREATE_CREDENTIAL_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "provider" => vine::enum_(CREDENTIAL_PROVIDERS.iter().copied()),
            "name" => credential_name(),
            "clientId" => google_client_id(),
            "clientSecret" => vine::string().trim().min_length(1).max_length(4000),
        })
        .messages_provider(messages())
});

/// What the dialog that edits a credential sends: a blank secret keeps the
/// saved one, which is never sent back to the browser.
pub static UPDATE_CREDENTIAL_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! {
            "name" => credential_name(),
            "clientId" => google_client_id(),
            "clientSecret" => vine::string().trim().max_length(4000).optional(),
        })
        .messages_provider(messages())
});

/// What the two validators let through. `provider` is only sent on create.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPayload {
    #[serde(default)]
    pub provider: Option<String>,
    pub name: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

// Never prints the secret, which a log line of a request could otherwise hold.
impl std::fmt::Debug for CredentialPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialPayload")
            .field("provider", &self.provider)
            .field("name", &self.name)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// The id of a credential in a path, or `None` when the segment is not one:
/// a handler answers that like a credential that does not exist.
pub fn credential_id(segment: &str) -> Option<i64> {
    crate::validators::route_params::record_id(segment)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn takes_a_google_client_and_refuses_a_secret_in_its_place() {
        let valid = json!({
            "provider": "google",
            "name": " Google Cloud ",
            "clientId": "123-abc.apps.googleusercontent.com",
            "clientSecret": "GOCSPX-secret",
        });
        let payload: CredentialPayload = CREATE_CREDENTIAL_VALIDATOR.validate_as(&valid).unwrap();
        assert_eq!(payload.name, "Google Cloud");

        let pasted_secret = json!({
            "provider": "google", "name": "x", "clientId": "GOCSPX-secret", "clientSecret": "s",
        });
        let error = CREATE_CREDENTIAL_VALIDATOR
            .validate(&pasted_secret)
            .unwrap_err();
        assert!(format!("{error:?}").contains(GOOGLE_CLIENT_ID_HINT));

        let without_secret = json!({ "name": "x", "clientId": "1-a.apps.googleusercontent.com" });
        assert!(
            CREATE_CREDENTIAL_VALIDATOR
                .validate(&without_secret)
                .is_err()
        );
        assert!(
            UPDATE_CREDENTIAL_VALIDATOR
                .validate(&without_secret)
                .is_ok()
        );
        assert!(
            CREATE_CREDENTIAL_VALIDATOR
                .validate(&json!({ "provider": "strava", "name": "x", "clientId": "1-a.apps.googleusercontent.com", "clientSecret": "s" }))
                .is_err()
        );
    }
}
