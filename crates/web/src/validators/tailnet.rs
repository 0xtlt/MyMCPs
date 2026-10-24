//! The tailnets of a team: what the Connect dialog submits, and the
//! `{id}` of a tailnet in a path.

use std::sync::LazyLock;

use mymcps_net::parse_http_url;
use mymcps_vine as vine;
use serde::Deserialize;
use vine::Operator;

/// The longest name of a tailnet: the name of the device this server is in
/// it follows the name, and a hostname holds 63 characters.
pub const MAX_NAME_LENGTH: usize = 40;

/// The longest auth key a form takes. Tailscale's are about 60 characters;
/// the tailnet crate holds the same limit.
const MAX_AUTH_KEY_LENGTH: usize = 1024;

/// The control server of a Headscale: an http(s) URL that holds no secret.
/// The URL is stored as it is and its host is shown in the list, so a user
/// name, a password or a query are refused.
fn control_server_url() -> vine::Rule {
    vine::rule(|value, field| {
        let Some(text) = value.as_str() else {
            return;
        };
        let url = match parse_http_url(text, "Control server URL") {
            Ok(url) => url,
            Err(error) => {
                field.report(&error.to_string(), "controlServerUrl");
                return;
            }
        };
        if !url.username().is_empty() || url.password().is_some() {
            field.report(
                "Control server URL must not include a user name or a password",
                "controlServerUrl",
            );
        } else if url.query().is_some() {
            field.report(
                "Control server URL must not include a query",
                "controlServerUrl",
            );
        }
    })
}

fn tailnet_name() -> vine::VineString {
    vine::string()
        .trim()
        .min_length(1)
        .max_length(MAX_NAME_LENGTH)
        .regex(vine::js::regex(r"^[A-Za-z0-9][A-Za-z0-9 ._-]*$", "").expect("a static pattern"))
}

/// What the Connect a tailnet dialog sends. `authKey` is only asked for, and
/// only read, when the sign-in method is the key.
pub static CREATE_TAILNET_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "name" => tailnet_name(),
        "method" => vine::enum_(["link", "key"]),
        "authKey" => vine::string()
            .trim()
            .max_length(MAX_AUTH_KEY_LENGTH)
            .optional()
            .required_when("method", Operator::Eq, "key"),
        "controlUrl" => vine::string()
            .trim()
            .max_length(2048)
            .use_rule(control_server_url())
            .optional(),
    })
});

/// What [`CREATE_TAILNET_VALIDATOR`] lets through.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetPayload {
    pub name: String,
    pub method: SignInMethod,
    #[serde(default)]
    pub auth_key: Option<String>,
    #[serde(default)]
    pub control_url: Option<String>,
}

// Never prints the key, which a log line of a request could otherwise hold.
impl std::fmt::Debug for TailnetPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TailnetPayload")
            .field("name", &self.name)
            .field("method", &self.method)
            .field("auth_key", &self.auth_key.as_ref().map(|_| "[redacted]"))
            .field("control_url", &self.control_url)
            .finish()
    }
}

/// How this server proves it may join the tailnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInMethod {
    /// The person opens a sign-in link Tailscale answers.
    Link,
    /// A pre-authorised key of the admin console: nothing to sign in to.
    Key,
}

/// The id of a tailnet in a path, or `None` when the segment is not one: a
/// handler answers that like a tailnet that does not exist.
pub fn tailnet_id(segment: &str) -> Option<i64> {
    crate::validators::route_params::record_id(segment)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn messages(input: Value) -> Vec<(String, String)> {
        CREATE_TAILNET_VALIDATOR
            .validate(&input)
            .unwrap_err()
            .messages
            .into_iter()
            .map(|one| (one.field, one.message))
            .collect()
    }

    #[test]
    fn reads_a_tailnet_with_either_sign_in_method() {
        let link = CREATE_TAILNET_VALIDATOR
            .validate(
                &json!({ "name": "  pro ", "method": "link", "authKey": null, "controlUrl": null }),
            )
            .unwrap();
        assert_eq!(link["name"], "pro");
        let key: TailnetPayload = CREATE_TAILNET_VALIDATOR
            .validate_as(&json!({
                "name": "Home lab",
                "method": "key",
                "authKey": " tskey-auth-abc ",
                "controlUrl": "https://headscale.example.com"
            }))
            .unwrap();
        assert_eq!(key.method, SignInMethod::Key);
        assert_eq!(key.auth_key.as_deref(), Some("tskey-auth-abc"));
        assert!(!format!("{key:?}").contains("tskey-auth-abc"));
    }

    #[test]
    fn asks_for_a_key_only_when_it_is_the_method() {
        assert_eq!(messages(json!({ "name": "pro", "method": "key" })).len(), 1);
        assert!(
            CREATE_TAILNET_VALIDATOR
                .validate(&json!({ "name": "pro", "method": "link" }))
                .is_ok()
        );
    }

    #[test]
    fn refuses_a_name_a_hostname_cannot_follow() {
        for name in ["", "-pro", "pro/../x", "pro!", "日本"] {
            assert!(
                CREATE_TAILNET_VALIDATOR
                    .validate(&json!({ "name": name, "method": "link" }))
                    .is_err(),
                "{name:?}"
            );
        }
        assert!(
            CREATE_TAILNET_VALIDATOR
                .validate(&json!({ "name": "x".repeat(MAX_NAME_LENGTH + 1), "method": "link" }))
                .is_err()
        );
    }

    #[test]
    fn refuses_a_control_server_that_is_not_a_plain_http_address() {
        for (url, message) in [
            (
                "ftp://hs.example.com",
                "Control server URL must use HTTP or HTTPS",
            ),
            ("hs.example.com", "Control server URL must be a valid URL"),
            (
                "https://user:secret@hs.example.com",
                "Control server URL must not include a user name or a password",
            ),
            (
                "https://hs.example.com/?key=1",
                "Control server URL must not include a query",
            ),
        ] {
            assert_eq!(
                messages(json!({ "name": "pro", "method": "link", "controlUrl": url })),
                [("controlUrl".to_owned(), message.to_owned())],
                "{url}"
            );
        }
    }
}
