//! Vine schemas for what this crate reads back: the pending authorization
//! kept in the browser session, the query and headers of the OAuth routes,
//! the names of the eager gateway's tools, and the approval modes saved for
//! an MCP.

use std::sync::LazyLock;

use mymcps_builtin::arguments::TOOL_VINE;
use mymcps_vine as vine;
use serde_json::json;
use vine::{UrlOptions, Validator, VineString};

/// Token endpoint JSON (snake_case as returned by OAuth providers).
pub use mymcps_builtin::oauth::TOKEN_RESPONSE_VALIDATOR as OAUTH_TOKEN_RESPONSE_VALIDATOR;

/// `vine.string().url({ require_tld: false })`
fn url() -> VineString {
    vine::string().url_with(UrlOptions {
        require_tld: false,
        ..UrlOptions::default()
    })
}

/// Values we put in the session during the authorize redirect. Built-in MCPs
/// authenticate with a client secret instead of PKCE and have no code verifier.
pub static OAUTH_SESSION_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "mcpId" => vine::number(),
        "codeVerifier" => vine::string().min_length(1).optional(),
        "state" => vine::string().min_length(1),
        "redirectUri" => url(),
        "authorizationServerUrl" => url(),
        "resource" => url().optional(),
        "clientId" => vine::string().min_length(1),
    })
});

/// Query params on `/mcps/oauth/callback` from the authorization server.
pub static OAUTH_CALLBACK_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "code" => vine::string().max_length(8192).optional(),
        "state" => vine::string().max_length(512).optional(),
        "error" => vine::string().max_length(1024).optional(),
    })
});

/// Request headers on `/mcps/:id/oauth/start`. Browsers say where a request
/// comes from: the flow may be started from this app or from the address bar,
/// not from another site.
pub static OAUTH_START_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "headers" => vine::object! {
            "sec-fetch-site" => vine::enum_(["same-origin", "none"]).optional(),
        },
    })
});

/// `/^(?!__).+?__/s`: a separator with something before it. The regex crate
/// has no lookahead, so the check is written out.
fn has_slug_and_separator(name: &str) -> bool {
    let first = name.chars().next().map_or(0, char::len_utf8);
    !name.starts_with("__") && name[first..].contains("__")
}

/// A tool name of the eager gateway, `<slug>__<tool>`, split at its first
/// separator. The slug cannot be empty; the tool name is the upstream's to
/// judge.
pub static NAMESPACED_TOOL_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(
        vine::string()
            .regex(vine::Pattern::from_fn(
                "^(?!__).+?__",
                has_slug_and_separator,
            ))
            .transform(|name, _| {
                let name = name.as_str().unwrap_or_default();
                let separator = name.find("__").unwrap_or(0);
                json!({ "slug": name[..separator], "toolName": name[separator + 2..] })
            }),
    )
});

/// Whether a tool runs when an agent calls it, or waits for a person first.
pub const TOOL_APPROVAL_MODES: [&str; 2] = ["auto", "ask"];

/// The modes saved for an MCP, by tool name. Tool names belong to the MCP and
/// may be anything, so nothing here trims or rewrites them.
pub static SAVED_TOOL_APPROVALS_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| TOOL_VINE.create(vine::record(vine::enum_(TOOL_APPROVAL_MODES))));

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn session() -> Value {
        json!({
            "mcpId": 7,
            "codeVerifier": "verifier",
            "state": "state",
            "redirectUri": "http://localhost:3333/mcps/oauth/callback",
            "authorizationServerUrl": "https://auth.example",
            "resource": "https://mcp.example/mcp",
            "clientId": "client",
            "extra": "dropped",
        })
    }

    #[test]
    fn reads_a_pending_authorization_back_in_the_order_of_the_schema() {
        assert_eq!(
            serde_json::to_string(&OAUTH_SESSION_VALIDATOR.validate(&session()).unwrap()).unwrap(),
            r#"{"mcpId":7,"codeVerifier":"verifier","state":"state","redirectUri":"http://localhost:3333/mcps/oauth/callback","authorizationServerUrl":"https://auth.example","resource":"https://mcp.example/mcp","clientId":"client"}"#
        );

        // A built-in MCP has neither a code verifier nor a resource.
        let mut builtin = session();
        let fields = builtin.as_object_mut().unwrap();
        fields.remove("codeVerifier");
        fields.remove("resource");
        assert!(OAUTH_SESSION_VALIDATOR.validate(&builtin).is_ok());
    }

    #[test]
    fn refuses_a_pending_authorization_that_is_not_whole() {
        for missing in [
            "mcpId",
            "state",
            "redirectUri",
            "authorizationServerUrl",
            "clientId",
        ] {
            let mut value = session();
            value.as_object_mut().unwrap().remove(missing);
            assert!(
                OAUTH_SESSION_VALIDATOR.validate(&value).is_err(),
                "{missing}"
            );
        }
        for (field, wrong) in [
            ("mcpId", json!("seven")),
            ("codeVerifier", json!(5)),
            ("state", json!("")),
            ("redirectUri", json!("not a url")),
            ("authorizationServerUrl", json!("javascript:alert(1)")),
            ("resource", json!("/relative")),
            ("clientId", json!(" ")),
        ] {
            let mut value = session();
            value[field] = wrong.clone();
            assert!(
                OAUTH_SESSION_VALIDATOR.validate(&value).is_err(),
                "{field}: {wrong}"
            );
        }
        for not_an_object in [json!(null), json!("state"), json!([]), json!(42)] {
            assert!(OAUTH_SESSION_VALIDATOR.validate(&not_an_object).is_err());
        }
        assert!(OAUTH_SESSION_VALIDATOR.validate(None::<&Value>).is_err());
    }

    #[test]
    fn bounds_the_parameters_of_the_callback() {
        assert_eq!(
            OAUTH_CALLBACK_VALIDATOR
                .validate(&json!({ "code": "c", "state": "s", "scope": "read", "error": "" }))
                .unwrap(),
            json!({ "code": "c", "state": "s" })
        );
        assert_eq!(
            OAUTH_CALLBACK_VALIDATOR.validate(&json!({})).unwrap(),
            json!({})
        );
        for (field, limit) in [("code", 8192), ("state", 512), ("error", 1024)] {
            let fits = json!({ field: "x".repeat(limit) });
            assert!(OAUTH_CALLBACK_VALIDATOR.validate(&fits).is_ok(), "{field}");
            let too_long = json!({ field: "x".repeat(limit + 1) });
            assert!(
                OAUTH_CALLBACK_VALIDATOR.validate(&too_long).is_err(),
                "{field}"
            );
        }
        assert!(
            OAUTH_CALLBACK_VALIDATOR
                .validate(&json!({ "code": ["a", "b"] }))
                .is_err()
        );
    }

    #[test]
    fn lets_a_flow_start_from_this_app_or_the_address_bar_only() {
        for site in ["same-origin", "none"] {
            let request = json!({ "headers": { "sec-fetch-site": site } });
            assert!(OAUTH_START_VALIDATOR.validate(&request).is_ok(), "{site}");
        }
        // Clients that are not browsers send no such header.
        assert!(
            OAUTH_START_VALIDATOR
                .validate(&json!({ "headers": {} }))
                .is_ok()
        );
        for site in ["cross-site", "same-site", "SAME-ORIGIN"] {
            let request = json!({ "headers": { "sec-fetch-site": site } });
            assert!(OAUTH_START_VALIDATOR.validate(&request).is_err(), "{site}");
        }
    }

    #[test]
    fn reads_saved_approval_modes_as_they_were_written() {
        let saved = json!({ " spaced name ": "ask", "": "auto", "constructor": "ask" });
        assert_eq!(
            SAVED_TOOL_APPROVALS_VALIDATOR.validate(&saved).unwrap(),
            saved
        );
        assert_eq!(
            SAVED_TOOL_APPROVALS_VALIDATOR.validate(&json!({})).unwrap(),
            json!({})
        );
        for unreadable in [
            json!([]),
            json!("ask"),
            json!(null),
            json!({ "delete_contact": "sometimes" }),
            json!({ "delete_contact": "" }),
            json!({ "delete_contact": null }),
        ] {
            assert!(
                SAVED_TOOL_APPROVALS_VALIDATOR
                    .validate(&unreadable)
                    .is_err(),
                "{unreadable}"
            );
        }
    }
}
