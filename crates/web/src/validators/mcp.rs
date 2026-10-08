//! The MCP form, and what the MCPs page reads from its address.
//! (the MCP schemas of `app/validators/mcp.ts`)

use std::collections::HashMap;
use std::sync::LazyLock;

use mymcps_builtin::keys::BUILTIN_MCP_KEYS;
use mymcps_core::models::{McpAuthType, McpStatus, McpTransport};
use mymcps_deno::reserved_environment_name_reason;
use mymcps_net::parse_http_url;
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use vine::Operator;

/// Refuse names that would configure the Deno sandbox instead of the package.
fn npm_environment_name() -> vine::Rule {
    vine::rule(|value, field| {
        let Some(name) = value.as_str() else {
            return;
        };
        if let Some(reason) = reserved_environment_name_reason(name) {
            field.report(&reason, "npmEnvironmentName");
        }
    })
}

/// An MCP endpoint must be an HTTP(S) URL without a fragment. Checked for the
/// HTTP transport only: other transports ignore the field.
fn mcp_endpoint_url() -> vine::Rule {
    vine::rule(|value, field| {
        let Some(url) = value.as_str() else {
            return;
        };
        if field.parent_get("transport").and_then(Value::as_str) != Some("http") {
            return;
        }
        if let Err(error) = parse_http_url(url, "MCP URL") {
            // Its own rule name: under `url`, Vine would replace the reason with its generic message.
            field.report(&error.to_string(), "mcpEndpointUrl");
        }
    })
}

/// RFC 9110 token for custom header names.
fn header_name() -> vine::VineString {
    vine::string()
        .trim()
        .max_length(120)
        .regex(vine::js::regex(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$", "").expect("a static pattern"))
}

fn npm_environment() -> vine::VineArray {
    vine::array(vine::object! {
        "name" => vine::string()
            .trim()
            .max_length(128)
            .regex(vine::js::regex(r"^[A-Za-z_][A-Za-z0-9_]*$", "").expect("a static pattern"))
            .use_rule(npm_environment_name()),
        // Do not trim secrets. Empty HTML inputs become null through the global Vine transform.
        "value" => vine::string().max_length(8192).nullable(),
    })
    .max_length(50)
    .optional()
}

/// The words of a value that separates them with whitespace, and with commas
/// or semicolons too when `punctuation` says so.
fn words(value: &Value, punctuation: bool) -> Value {
    let Some(text) = value.as_str() else {
        return json!([]);
    };
    text.split(|character: char| {
        vine::js::is_whitespace(character) || (punctuation && matches!(character, ',' | ';'))
    })
    .filter(|part| !part.is_empty())
    .collect()
}

fn mcp_payload() -> vine::VineObject {
    vine::object! {
        "name" => vine::string().trim().min_length(1).max_length(120),
        "description" => vine::string().trim().max_length(500).optional(),
        "transport" => vine::enum_(["http", "npm", "builtin"]),
        "httpUrl" => vine::string()
            .trim()
            .url()
            .max_length(2048)
            .use_rule(mcp_endpoint_url())
            .optional()
            .required_when("transport", Operator::Eq, "http"),
        "npmPackage" => vine::string()
            .trim()
            .max_length(254)
            .optional()
            .required_when("transport", Operator::Eq, "npm"),
        "npmVersion" => vine::string().trim().max_length(64).optional(),
        "npmArgs" => vine::string()
            .trim()
            .max_length(1000)
            .nullable()
            .optional()
            .transform(|value, _| words(&value, false)),
        "npmEnv" => npm_environment(),
        "builtinKey" => vine::enum_(BUILTIN_MCP_KEYS.iter().copied())
            .optional()
            .required_when("transport", Operator::Eq, "builtin"),
        // Credentials of the API application the admin registered for a built-in MCP.
        "oauthClientId" => vine::string().trim().max_length(254).optional(),
        "oauthClientSecret" => vine::string().trim().max_length(4000).optional(),
        // Sign-in of a built-in MCP whose provider issues passwords for apps.
        "builtinUsername" => vine::string().trim().max_length(254).optional(),
        "builtinPassword" => vine::string().trim().max_length(4000).optional(),
        // What agents may do through that sign-in, which the provider cannot
        // restrict. A form posting a single checked permission sends it as a string.
        "builtinPermissions" => vine::array(vine::string().trim().max_length(32))
            .max_length(16)
            .parse(|value, _| match value {
                Some(Value::String(permission)) => Some(json!([permission])),
                other => other,
            })
            .optional(),
        // Other addresses of the same account, separated by commas or spaces.
        "builtinAliases" => vine::string()
            .trim()
            .max_length(4000)
            .nullable()
            .optional()
            .transform(|value, _| words(&value, true)),
        // What a built-in MCP needs beyond its sign-in, by the keys its
        // provider declares. Each provider checks its own values.
        "builtinSettings" => vine::record(vine::string().max_length(4000).nullable()).optional(),
        // Lets a built-in MCP request write scopes and expose its write tools.
        "builtinWriteEnabled" => vine::boolean().optional(),
        "authType" => vine::enum_(["auto", "bearer", "header"]),
        "authBearer" => vine::string().trim().max_length(4000).optional(),
        "authHeaderName" => header_name()
            .optional()
            .required_when("authType", Operator::Eq, "header"),
        "authHeaderValue" => vine::string().trim().max_length(4000).optional(),
        // A checkbox submits "on" when checked, and nothing when unchecked.
        "enabled" => vine::boolean().optional(),
    }
}

/// Create an upstream MCP registration.
pub static CREATE_MCP_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(mcp_payload()));

/// Update an upstream MCP registration.
pub static UPDATE_MCP_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(mcp_payload()));

/// One environment variable of the form. A blank value keeps the saved one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NpmEnvEntry {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// What [`CREATE_MCP_VALIDATOR`] and [`UPDATE_MCP_VALIDATOR`] let through.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPayload {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub transport: McpTransport,
    #[serde(default)]
    pub http_url: Option<String>,
    #[serde(default)]
    pub npm_package: Option<String>,
    #[serde(default)]
    pub npm_version: Option<String>,
    #[serde(default)]
    pub npm_args: Option<Vec<String>>,
    #[serde(default)]
    pub npm_env: Option<Vec<NpmEnvEntry>>,
    #[serde(default)]
    pub builtin_key: Option<String>,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    #[serde(default)]
    pub oauth_client_secret: Option<String>,
    #[serde(default)]
    pub builtin_username: Option<String>,
    #[serde(default)]
    pub builtin_password: Option<String>,
    #[serde(default)]
    pub builtin_permissions: Option<Vec<String>>,
    #[serde(default)]
    pub builtin_aliases: Option<Vec<String>>,
    #[serde(default)]
    pub builtin_settings: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub builtin_write_enabled: Option<bool>,
    pub auth_type: McpAuthType,
    #[serde(default)]
    pub auth_bearer: Option<String>,
    #[serde(default)]
    pub auth_header_name: Option<String>,
    #[serde(default)]
    pub auth_header_value: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// The sign-in an MCP is listed under: its authentication, or for a built-in
/// one the way its provider signs in.
pub const AUTH_LABELS: [&str; 5] = ["auto", "bearer", "header", "oauth", "password"];

/// The search, the filters and the page of the MCP list, and the template
/// the create dialog is opened on. The page is new: the Node app listed
/// every MCP and filtered nothing.
pub static MCP_LIST_QUERY_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "q" => vine::string().trim().max_length(200).optional(),
        "status" => vine::enum_(McpStatus::ALL.iter().map(McpStatus::as_str)).optional(),
        "transport" => vine::enum_(McpTransport::ALL.iter().map(McpTransport::as_str)).optional(),
        "auth" => vine::enum_(AUTH_LABELS).optional(),
        "page" => vine::number().without_decimals().positive().max(1_000_000).optional(),
        "template" => vine::string().trim().max_length(64).optional(),
    })
});

/// The switch of a row: the state it asks for. Without one, the MCP is
/// switched over.
pub static MCP_TOGGLE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "enabled" => vine::boolean().optional() })
});

/// Where a request to test or update an MCP comes from. From the list, its
/// answer is read there: the edit dialog does not open on it.
pub static MCP_ACTION_ORIGIN_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "from" => vine::enum_(["list"]).optional() })
});

/// What [`MCP_LIST_QUERY_VALIDATOR`] lets through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct McpListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub status: Option<McpStatus>,
    #[serde(default)]
    pub transport: Option<McpTransport>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub template: Option<String>,
}

impl McpListQuery {
    /// Read the address of the page. A parameter that is not one the page
    /// writes itself is ignored rather than refused: the list is still shown.
    pub fn read(input: &Map<String, Value>) -> Self {
        let mut input = input.clone();
        for _ in 0..2 {
            match MCP_LIST_QUERY_VALIDATOR.validate_as::<Self>(&Value::Object(input.clone())) {
                Ok(query) => return query,
                Err(vine::Error::Validation(error)) => {
                    for refused in &error.messages {
                        let field = refused.field.split('.').next().unwrap_or_default();
                        input.shift_remove(field);
                    }
                }
                Err(vine::Error::Output(_)) => break,
            }
        }
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http() -> Value {
        json!({ "name": "Docs", "transport": "http", "authType": "auto" })
    }

    fn with(mut base: Value, overrides: Value) -> Value {
        for (key, value) in overrides.as_object().unwrap() {
            base[key] = value.clone();
        }
        base
    }

    fn messages(error: &vine::ValidationError) -> Vec<(&str, &str, &str)> {
        error
            .messages
            .iter()
            .map(|one| (one.field.as_str(), one.message.as_str(), one.rule.as_str()))
            .collect()
    }

    // --- tests/unit/vine_mcp_url.spec.ts ---

    #[test]
    fn accepts_an_http_endpoint() {
        for http_url in [
            "https://mcp.example.com/mcp",
            "http://127.0.0.1:9999/mcp",
            "https://mcp.example.com/mcp?key=value",
            "https://user:secret@mcp.example.com/mcp",
        ] {
            let payload = CREATE_MCP_VALIDATOR
                .validate(&with(http(), json!({ "httpUrl": http_url })))
                .unwrap();
            assert_eq!(payload["httpUrl"], http_url);
        }
    }

    #[test]
    fn refuses_an_endpoint_that_is_not_plain_http() {
        for (http_url, message) in [
            (
                "ftp://mcp.example.com/mcp",
                "MCP URL must use HTTP or HTTPS",
            ),
            ("mcp.example.com/mcp", "MCP URL must be a valid URL"),
            (
                "https://mcp.example.com/mcp#tools",
                "MCP URL must not include a fragment",
            ),
        ] {
            let error = CREATE_MCP_VALIDATOR
                .validate(&with(http(), json!({ "httpUrl": http_url })))
                .unwrap_err();
            assert_eq!(messages(&error), [("httpUrl", message, "mcpEndpointUrl")]);
        }
    }

    #[test]
    fn leaves_the_endpoint_alone_for_another_transport() {
        let payload = json!({
            "name": "Package",
            "transport": "npm",
            "npmPackage": "@example/mcp",
            "authType": "auto",
            "httpUrl": "ftp://mcp.example.com/mcp",
        });
        assert!(CREATE_MCP_VALIDATOR.validate(&payload).is_ok());
    }

    // --- the rest of the schema ---

    #[test]
    fn asks_for_what_each_transport_and_authentication_needs() {
        let missing = |payload: Value| {
            let error = UPDATE_MCP_VALIDATOR.validate(&payload).unwrap_err();
            error
                .messages
                .iter()
                .map(|one| format!("{}:{}", one.field, one.rule))
                .collect::<Vec<_>>()
        };
        assert_eq!(missing(http()), ["httpUrl:required"]);
        assert_eq!(
            missing(json!({ "name": "Package", "transport": "npm", "authType": "auto" })),
            ["npmPackage:required"]
        );
        assert_eq!(
            missing(json!({ "name": "Strava", "transport": "builtin", "authType": "auto" })),
            ["builtinKey:required"]
        );
        assert_eq!(
            missing(
                json!({ "name": "Garmin", "transport": "builtin", "builtinKey": "garmin", "authType": "auto" })
            ),
            ["builtinKey:enum"]
        );
        assert_eq!(
            missing(with(
                http(),
                json!({ "httpUrl": "https://x.test/mcp", "authType": "header" })
            )),
            ["authHeaderName:required"]
        );
        assert_eq!(
            missing(with(
                http(),
                json!({ "httpUrl": "https://x.test/mcp", "authType": "header", "authHeaderName": "X Api Key" })
            )),
            ["authHeaderName:regex"]
        );
        assert_eq!(
            missing(json!({ "name": "  ", "transport": "ftp", "authType": "oauth" })),
            ["name:required", "transport:enum", "authType:enum"]
        );
    }

    #[test]
    fn reads_a_form_as_the_controller_expects_it() {
        let payload: McpPayload = CREATE_MCP_VALIDATOR
            .validate_as(&json!({
                "name": " Everything ",
                "description": null,
                "transport": "npm",
                "httpUrl": null,
                "npmPackage": "@modelcontextprotocol/server-everything",
                "npmVersion": null,
                "npmArgs": "--flag   --other\tvalue",
                "npmEnv": [{ "name": "API_KEY", "value": " kept as typed " }, { "name": "REGION", "value": null }],
                "builtinPermissions": "read",
                "builtinAliases": "hello@thomas.example; THOMAS@icloud.com\ntt@icloud.com, Hello@Thomas.example",
                "builtinSettings": { "customerIds": "123-456-7890", "loginCustomerId": null },
                "builtinWriteEnabled": "on",
                "authType": "header",
                "authHeaderName": "X-Api-Key",
                "authHeaderValue": "value",
                "enabled": "on",
                "unknown": "dropped",
            }))
            .unwrap();
        assert_eq!(payload.name, "Everything");
        assert_eq!(payload.description, None);
        assert_eq!(payload.transport, McpTransport::Npm);
        assert_eq!(payload.npm_args.unwrap(), ["--flag", "--other", "value"]);
        assert_eq!(
            payload.npm_env.unwrap(),
            [
                NpmEnvEntry {
                    name: "API_KEY".into(),
                    value: Some(" kept as typed ".into())
                },
                NpmEnvEntry {
                    name: "REGION".into(),
                    value: None
                },
            ]
        );
        assert_eq!(payload.builtin_permissions.unwrap(), ["read"]);
        assert_eq!(
            payload.builtin_aliases.unwrap(),
            [
                "hello@thomas.example",
                "THOMAS@icloud.com",
                "tt@icloud.com",
                "Hello@Thomas.example"
            ]
        );
        let settings = payload.builtin_settings.unwrap();
        assert_eq!(settings["customerIds"].as_deref(), Some("123-456-7890"));
        assert_eq!(settings["loginCustomerId"], None);
        assert_eq!(payload.builtin_write_enabled, Some(true));
        assert_eq!(payload.auth_type, McpAuthType::Header);
        assert_eq!(payload.enabled, Some(true));

        // A field the form sent empty reads as an empty list, one it did not
        // send is left out.
        let blank: McpPayload = CREATE_MCP_VALIDATOR
            .validate_as(&with(
                http(),
                json!({ "httpUrl": "https://x.test/mcp", "npmArgs": null, "builtinAliases": null }),
            ))
            .unwrap();
        assert_eq!(blank.npm_args, Some(Vec::new()));
        assert_eq!(blank.builtin_aliases, Some(Vec::new()));
        assert_eq!(blank.npm_env, None);
        assert_eq!(blank.enabled, None);
    }

    #[test]
    fn refuses_environment_names_that_address_the_sandbox() {
        let npm = |environment: Value| {
            json!({
                "name": "Package",
                "transport": "npm",
                "npmPackage": "@example/mcp",
                "authType": "auto",
                "npmEnv": environment,
            })
        };
        let error = CREATE_MCP_VALIDATOR
            .validate(&npm(json!([
                { "name": "API_KEY", "value": "a" },
                { "name": "HOME", "value": "elsewhere" },
                { "name": "1BAD", "value": "b" },
                { "name": "LD_PRELOAD", "value": "/tmp/x.so" },
            ])))
            .unwrap_err();
        assert_eq!(
            messages(&error),
            [
                (
                    "npmEnv.1.name",
                    "\"HOME\" is set by MyMCPs for the sandbox and cannot be changed",
                    "npmEnvironmentName"
                ),
                ("npmEnv.2.name", "The name field format is invalid", "regex"),
                (
                    "npmEnv.3.name",
                    "\"LD_PRELOAD\" changes how the sandbox process itself is loaded and cannot be set",
                    "npmEnvironmentName"
                ),
            ]
        );

        let too_many: Vec<Value> = (0..51)
            .map(|index| json!({ "name": format!("VALUE_{index}"), "value": "x" }))
            .collect();
        let error = CREATE_MCP_VALIDATOR
            .validate(&npm(Value::Array(too_many)))
            .unwrap_err();
        assert_eq!(error.messages[0].field, "npmEnv");
        assert_eq!(error.messages[0].rule, "array.maxLength");
    }

    #[test]
    fn reads_the_address_of_the_list_and_ignores_what_it_does_not_know() {
        let read = |query: Value| McpListQuery::read(query.as_object().unwrap());
        assert_eq!(read(json!({})), McpListQuery::default());
        assert_eq!(
            read(json!({
                "q": " notion ",
                "status": "error",
                "transport": "builtin",
                "auth": "password",
                "page": "3",
                "template": "strava",
                "other": "x",
            })),
            McpListQuery {
                q: Some("notion".into()),
                status: Some(McpStatus::Error),
                transport: Some(McpTransport::Builtin),
                auth: Some("password".into()),
                page: Some(3),
                template: Some("strava".into()),
            }
        );
        // What is wrong is dropped, what is right is kept.
        assert_eq!(
            read(
                json!({ "q": "", "status": "broken", "page": "0", "transport": "npm", "auth": ["auto", "bearer"] })
            ),
            McpListQuery {
                transport: Some(McpTransport::Npm),
                ..Default::default()
            }
        );
        assert_eq!(read(json!({ "page": "-2" })).page, None);
    }
}
