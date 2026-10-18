//! The proxy form: what the Proxies page submits.

use std::sync::LazyLock;

use mymcps_vine as vine;
use serde::Deserialize;
use vine::Pattern;

/// The name a new proxy's node takes in the tailnet until the person chooses another.
pub const DEFAULT_NODE_NAME: &str = "mymcps";

/// What a node name looks like: a DNS label.
const NODE_NAME_PATTERN: &str = r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$";

const NODE_NAME_HINT: &str = "Use lowercase letters, digits, and hyphens, starting and ending with a letter or a digit (63 characters at most)";

const AUTH_KEY_HINT: &str = "Paste an auth key: it starts with tskey-auth-";

const OAUTH_CLIENT_SECRET_HINT: &str =
    "OAuth client secrets are not supported yet: create an auth key";

static NODE_NAME: LazyLock<Pattern> = LazyLock::new(|| {
    vine::js::regex(NODE_NAME_PATTERN, "")
        .expect("a static pattern")
        .into()
});

static AUTH_KEY: LazyLock<Pattern> = LazyLock::new(|| {
    vine::js::regex(r"^tskey-auth-[A-Za-z0-9_-]+$", "")
        .expect("a static pattern")
        .into()
});

/// The name of the node in the tailnet. Required, so that a field left
/// empty is told what to enter rather than that it "must be defined".
fn node_name() -> vine::Rule {
    vine::rule(|value, field| {
        let name = value.as_str().unwrap_or_default();
        if name.is_empty() {
            field.report("Enter a node name", "tailscaleHostname");
        } else if !NODE_NAME.test(name) {
            field.report(NODE_NAME_HINT, "tailscaleHostname");
        }
    })
    .implicit()
}

/// The auth key the node logs in with. A secret of an OAuth client looks
/// alike and is refused with a sentence of its own.
fn auth_key() -> vine::Rule {
    vine::rule(|value, field| {
        let Some(key) = value.as_str() else {
            return;
        };
        if key.starts_with("tskey-client-") {
            field.report(OAUTH_CLIENT_SECRET_HINT, "tailscaleOauthClient");
        } else if !AUTH_KEY.test(key) {
            field.report(AUTH_KEY_HINT, "tailscaleAuthKey");
        }
    })
}

fn proxy_payload() -> vine::VineObject {
    vine::object! {
        "name" => vine::string().trim().min_length(1).max_length(100),
        "tailscaleHostname" => vine::string()
            .trim()
            .nullable()
            .optional()
            .use_rule(node_name()),
        // Left blank on a saved proxy, it keeps the key the proxy has.
        "tailscaleAuthKey" => vine::string()
            .trim()
            .max_length(500)
            .nullable()
            .optional()
            .use_rule(auth_key()),
    }
}

/// Create a proxy.
pub static CREATE_PROXY_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(proxy_payload()));

/// Change a proxy.
pub static UPDATE_PROXY_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(proxy_payload()));

/// What [`CREATE_PROXY_VALIDATOR`] and [`UPDATE_PROXY_VALIDATOR`] let through.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPayload {
    pub name: String,
    pub tailscale_hostname: String,
    #[serde(default)]
    pub tailscale_auth_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn first_error(input: Value) -> (String, String) {
        let error = CREATE_PROXY_VALIDATOR.validate(&input).unwrap_err();
        (
            error.messages[0].field.clone(),
            error.messages[0].message.clone(),
        )
    }

    #[test]
    fn reads_a_proxy_as_its_form_sends_it() {
        let payload: ProxyPayload = CREATE_PROXY_VALIDATOR
            .validate_as(&json!({
                "name": "  Home ",
                "tailscaleHostname": "mymcps",
                "tailscaleAuthKey": " tskey-auth-kAbC123-xYz_09 ",
            }))
            .unwrap();
        assert_eq!(payload.name, "Home");
        assert_eq!(payload.tailscale_hostname, "mymcps");
        assert_eq!(
            payload.tailscale_auth_key.as_deref(),
            Some("tskey-auth-kAbC123-xYz_09")
        );

        // A key left blank is none.
        let payload: ProxyPayload = UPDATE_PROXY_VALIDATOR
            .validate_as(
                &json!({ "name": "Home", "tailscaleHostname": "a", "tailscaleAuthKey": "" }),
            )
            .unwrap();
        assert_eq!(payload.tailscale_auth_key, None);
        let payload: ProxyPayload = UPDATE_PROXY_VALIDATOR
            .validate_as(&json!({ "name": "Home", "tailscaleHostname": "a" }))
            .unwrap();
        assert_eq!(payload.tailscale_auth_key, None);
    }

    #[test]
    fn refuses_a_name_that_is_empty_or_too_long() {
        let input = |name: &str| json!({ "name": name, "tailscaleHostname": "mymcps" });
        assert_eq!(
            first_error(input("  ")),
            ("name".into(), "The name field must be defined".into())
        );
        assert_eq!(
            first_error(input(&"x".repeat(101))),
            (
                "name".into(),
                "The name field must not be greater than 100 characters".into()
            )
        );
        assert!(
            CREATE_PROXY_VALIDATOR
                .validate(&input(&"x".repeat(100)))
                .is_ok()
        );
    }

    #[test]
    fn refuses_a_node_name_that_is_not_a_dns_label() {
        let input = |hostname: Value| json!({ "name": "Home", "tailscaleHostname": hostname });
        for valid in ["mymcps", "a", "box-1", "0x", "a".repeat(63).as_str()] {
            assert!(
                CREATE_PROXY_VALIDATOR
                    .validate(&input(json!(valid)))
                    .is_ok(),
                "{valid}"
            );
        }
        for invalid in [
            "MyMCPs",
            "-mymcps",
            "mymcps-",
            "my mcps",
            "my_mcps",
            "my.mcps",
            "café",
            "a".repeat(64).as_str(),
        ] {
            assert_eq!(
                first_error(input(json!(invalid))),
                ("tailscaleHostname".into(), NODE_NAME_HINT.into()),
                "{invalid}"
            );
        }
        for empty in [json!(""), json!(null), json!("  ")] {
            assert_eq!(
                first_error(input(empty)),
                ("tailscaleHostname".into(), "Enter a node name".into())
            );
        }
        assert_eq!(
            first_error(json!({ "name": "Home" })),
            ("tailscaleHostname".into(), "Enter a node name".into())
        );
    }

    #[test]
    fn refuses_a_key_that_is_not_an_auth_key() {
        let input = |key: &str| json!({ "name": "Home", "tailscaleHostname": "mymcps", "tailscaleAuthKey": key });
        assert_eq!(
            first_error(input("tskey-client-kXyZ-secret")),
            (
                "tailscaleAuthKey".into(),
                "OAuth client secrets are not supported yet: create an auth key".into()
            )
        );
        for invalid in [
            "hunter2",
            "tskey-auth-",
            "tskey-auth-a b",
            "tskey-api-abc",
            "TSKEY-auth-abc",
        ] {
            assert_eq!(
                first_error(input(invalid)),
                ("tailscaleAuthKey".into(), AUTH_KEY_HINT.into()),
                "{invalid}"
            );
        }
    }
}
