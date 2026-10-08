//! The definition of the MCP: its sign-in, its settings, and the tools it
//! lists. Ports what `tests/functional/builtin_google_ads_mcp.spec.ts` says
//! of the setup dialog and of the OAuth sign-in, as far as the definition
//! decides it, and `tests/unit/vine_builtin_input_schemas.spec.ts` for the
//! tools of this MCP.
//!
//! `fixtures/tools.json` holds the definition and its 28 tools as the
//! TypeScript has them, written by Node from
//! `app/services/builtin/google_ads/index.ts`. The script that wrote it is
//! not part of the repository, since it runs Node on the TypeScript app.

mod support;

use std::collections::BTreeSet;
use std::sync::LazyLock;

use mymcps_builtin::oauth::{
    builtin_authorization_url, exchange_builtin_authorization_code, requested_builtin_scopes,
};
use mymcps_builtin::{BuiltinMcpDefinition, BuiltinSettingField};
use mymcps_core::models::Mcp;
use mymcps_google_ads::testing::{FakeGoogleAds, GOOGLE_ADS_SCOPE};
use serde_json::{Value, json};
use support::{Google, provider};

const FIXTURE: &str = include_str!("fixtures/tools.json");

static EXPECTED: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(FIXTURE).unwrap());
static DEFINITION: LazyLock<BuiltinMcpDefinition> = LazyLock::new(mymcps_google_ads::definition);

/// The read tools, then the write tools.
const READ_TOOLS: usize = 13;
const TOOLS: usize = 28;

fn setting(key: &str) -> &'static BuiltinSettingField {
    DEFINITION
        .settings()
        .iter()
        .find(|setting| setting.key == key)
        .unwrap()
}

/// What the setup dialog stores for a setting, or the hint it shows instead.
fn stored(key: &str, value: &str) -> Result<String, String> {
    let setting = setting(key);
    let normalized = setting
        .normalize
        .map_or_else(|| value.to_owned(), |normalize| normalize(value));
    if setting.pattern.is_match(&normalized) {
        Ok(normalized)
    } else {
        Err(setting.hint.clone())
    }
}

#[test]
fn stores_the_accounts_of_the_dialog_without_their_dashes() {
    assert_eq!(
        stored("loginCustomerId", "987-654-3210").as_deref(),
        Ok("9876543210")
    );
    assert_eq!(
        stored("customerIds", "123-456-7890, 2345678901").as_deref(),
        Ok("1234567890 2345678901")
    );
    // It needs neither a manager nor a list of accounts.
    assert!(
        DEFINITION
            .settings()
            .iter()
            .all(|setting| !setting.required)
    );
}

#[test]
fn reports_every_wrong_field_of_the_dialog() {
    let oauth = DEFINITION.oauth().unwrap();
    assert!(
        !oauth
            .client_id_pattern
            .as_ref()
            .unwrap()
            .is_match("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-field")
    );
    assert!(
        oauth
            .client_id_pattern
            .as_ref()
            .unwrap()
            .is_match("1234567890-abc.apps.googleusercontent.com")
    );
    assert_eq!(
        oauth.client_id_hint,
        Some("The Google Client ID ends in .apps.googleusercontent.com")
    );
    assert_eq!(
        stored("loginCustomerId", "12345").unwrap_err(),
        "Enter the ID of the manager account, such as 123-456-7890"
    );
    assert_eq!(
        stored("customerIds", "123-456-7890, acme").unwrap_err(),
        "Enter up to 50 Google Ads account IDs, such as 123-456-7890, separated by commas"
    );
}

#[test]
fn describes_the_mcp_as_the_typescript_does() {
    let expected = &EXPECTED["definition"];
    assert_eq!(DEFINITION.key(), expected["key"]);
    assert_eq!(DEFINITION.name(), expected["name"]);
    assert!(DEFINITION.password().is_none());
    assert_eq!(DEFINITION.has_download(), expected["hasDownload"]);
    assert_eq!(DEFINITION.has_upload(), expected["hasUpload"]);
    assert!(DEFINITION.has_upload());

    let oauth = DEFINITION.oauth().unwrap();
    let authorize_params: serde_json::Map<String, Value> = oauth
        .authorize_params
        .iter()
        .map(|(name, value)| ((*name).to_owned(), json!(value)))
        .collect();
    let described = json!({
        "issuer": oauth.issuer,
        "authorizeUrl": oauth.authorize_url,
        "tokenUrl": oauth.token_url,
        "scopes": oauth.scopes,
        "writeScopes": oauth.write_scopes,
        "scopeSeparator": oauth.scope_separator,
        "authorizeParams": authorize_params,
        "sendsRedirectUriWithCode": oauth.sends_redirect_uri_with_code,
        "clientIdHint": oauth.client_id_hint,
    });
    for (name, value) in described.as_object().unwrap() {
        assert_eq!(
            value.to_string(),
            expected["oauth"][name].to_string(),
            "oauth.{name}"
        );
    }
    let client_id = oauth.client_id_pattern.as_ref().unwrap();
    for case in expected["oauth"]["clientIds"].as_array().unwrap() {
        assert_eq!(
            client_id.is_match(case[0].as_str().unwrap()),
            case[1],
            "client ID {}",
            case[0]
        );
    }

    let settings = expected["settings"].as_array().unwrap();
    assert_eq!(DEFINITION.settings().len(), settings.len());
    for (setting, expected) in DEFINITION.settings().iter().zip(settings) {
        assert_eq!(setting.key, expected["key"]);
        assert_eq!(setting.required, expected["required"]);
        assert_eq!(setting.hint, expected["hint"]);
        for case in expected["values"].as_array().unwrap() {
            let value = case[0].as_str().unwrap();
            let normalized = setting.normalize.unwrap()(value);
            assert_eq!(normalized, case[1], "{} of {value:?}", setting.key);
            assert_eq!(
                setting.pattern.is_match(&normalized),
                case[2],
                "{} of {value:?}",
                setting.key
            );
        }
    }
}

#[test]
fn lists_its_tools_as_the_typescript_does() {
    let expected = EXPECTED["tools"].as_array().unwrap();
    assert_eq!(expected.len(), TOOLS);
    let tools = DEFINITION.tools();
    assert_eq!(tools.len(), TOOLS);

    for (tool, expected) in tools.iter().zip(expected) {
        let name = tool.name;
        assert_eq!(name, expected["name"]);
        assert_eq!(
            tool.description, expected["description"],
            "description of {name}"
        );
        // As text: the arguments are listed to agents in the order they are written.
        assert_eq!(
            tool.input_schema.to_string(),
            expected["inputSchema"].to_string(),
            "arguments of {name}"
        );
        assert_eq!(tool.write, expected["write"], "{name} writes");
        assert_eq!(
            tool.asks_approval,
            expected["approval"] == "ask",
            "{name} asks"
        );
        assert_eq!(
            json!(tool.requires_any_scope),
            expected["requiresAnyScope"],
            "scopes of {name}"
        );
    }
    for ((name, enforced), expected) in DEFINITION.enforced_schemas().iter().zip(expected) {
        assert_eq!(
            enforced.to_string(),
            expected["enforced"].to_string(),
            "arguments {name} checks"
        );
    }
    // No read tool changes anything, nor waits for anyone.
    assert!(
        tools
            .iter()
            .take(READ_TOOLS)
            .all(|tool| !tool.write && !tool.asks_approval)
    );
    // Every other tool does.
    assert!(tools.iter().skip(READ_TOOLS).all(|tool| tool.write));
}

/// What a tool may say about an argument beyond its type. Whatever it says, its validator enforces.
const BOUNDS: [&str; 6] = [
    "minimum",
    "maximum",
    "maxLength",
    "minItems",
    "maxItems",
    "enum",
];

/// Vine describes a choice among words by the words alone.
fn type_of(schema: &Value) -> Value {
    match schema.get("type") {
        Some(named) => named.clone(),
        None if schema.get("enum").is_some() => json!("string"),
        None => Value::Null,
    }
}

fn names(list: Option<&Value>) -> BTreeSet<String> {
    match list {
        Some(Value::Array(names)) => names
            .iter()
            .map(|name| name.as_str().unwrap().to_owned())
            .collect(),
        Some(Value::Object(properties)) => properties.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

/// The JSON Schema of a tool is written by hand, for its descriptions and
/// defaults. Its validator can describe itself in the same terms: the two must
/// agree, or agents are told one thing and held to another.
#[test]
fn checks_the_arguments_it_advertises() {
    let tools = DEFINITION.tools();
    let enforced = DEFINITION.enforced_schemas();
    assert_eq!(tools.len(), enforced.len());

    for (tool, (name, enforced)) in tools.iter().zip(&enforced) {
        assert_eq!(tool.name, *name);
        let advertised = tool.input_schema;
        assert_eq!(advertised["type"], "object", "{name}");
        assert_eq!(enforced["type"], "object", "{name}");
        assert_eq!(
            names(enforced.get("properties")),
            names(advertised.get("properties")),
            "arguments of {name}"
        );
        assert_eq!(
            names(enforced.get("required")),
            names(advertised.get("required")),
            "required of {name}"
        );

        for (argument, described) in advertised["properties"].as_object().unwrap() {
            let checked = &enforced["properties"][argument];
            assert_eq!(
                type_of(checked),
                described["type"],
                "type of {name}.{argument}"
            );
            for bound in BOUNDS {
                if let Some(said) = described.get(bound) {
                    assert_eq!(
                        checked.get(bound),
                        Some(said),
                        "{bound} of {name}.{argument}"
                    );
                }
            }
            if let Some(items) = described.get("items") {
                assert_eq!(
                    type_of(&checked["items"]),
                    items["type"],
                    "items of {name}.{argument}"
                );
                assert_eq!(
                    checked["items"].get("enum"),
                    items.get("enum"),
                    "items of {name}.{argument}"
                );
            }
        }
    }
}

#[test]
fn compares_every_tool() {
    let names: BTreeSet<&str> = DEFINITION.tools().iter().map(|tool| tool.name).collect();
    assert_eq!(names.len(), TOOLS);
    assert_eq!(provider(&DEFINITION).tools.len(), TOOLS);
    assert!(provider(&DEFINITION).tool("list_campaigns").is_some());
    assert!(provider(&DEFINITION).tool("list_everything").is_none());
}

#[tokio::test]
async fn asks_google_for_offline_access_and_sends_the_redirect_uri_again_with_the_code() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let oauth = google.definition.oauth().unwrap();
    let mcp = Mcp {
        id: 7,
        oauth_client_id: Some("1234567890-abc.apps.googleusercontent.com".into()),
        oauth_client_secret: google.core.encrypt_secret(Some("google-client-secret")),
        ..Default::default()
    };
    let redirect_uri = "http://localhost:3333/mcps/oauth/callback";

    // One scope reads and writes, so write access asks for nothing more.
    let writable = Mcp {
        builtin_write_enabled: true,
        ..Default::default()
    };
    assert_eq!(requested_builtin_scopes(oauth, &mcp), [GOOGLE_ADS_SCOPE]);
    assert_eq!(
        requested_builtin_scopes(oauth, &writable),
        [GOOGLE_ADS_SCOPE]
    );

    let authorization_url = builtin_authorization_url(
        oauth,
        mcp.oauth_client_id.as_deref().unwrap(),
        redirect_uri,
        "state-value",
        &requested_builtin_scopes(oauth, &mcp),
    )
    .unwrap();
    let authorization_url = url::Url::parse(&authorization_url).unwrap();
    assert_eq!(
        format!(
            "{}{}",
            authorization_url.origin().ascii_serialization(),
            authorization_url.path()
        ),
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    let parameter = |name: &str| {
        authorization_url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    assert_eq!(parameter("scope").as_deref(), Some(GOOGLE_ADS_SCOPE));
    assert_eq!(parameter("access_type").as_deref(), Some("offline"));
    assert_eq!(parameter("prompt").as_deref(), Some("consent"));

    let tokens = exchange_builtin_authorization_code(
        &google.env,
        google.definition.name(),
        oauth,
        &mcp,
        "google-code",
        redirect_uri,
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "google-access-token");
    assert_eq!(
        tokens.refresh_token.as_deref(),
        Some("google-refresh-token")
    );
    assert_eq!(tokens.scope.as_deref(), Some(GOOGLE_ADS_SCOPE));
    let exchange = &google.fake.token_requests()[0];
    assert_eq!(exchange.url.as_str(), "https://oauth2.googleapis.com/token");
    assert_eq!(
        exchange.form.as_deref().unwrap(),
        [
            ("client_id", "1234567890-abc.apps.googleusercontent.com"),
            ("client_secret", "google-client-secret"),
            ("grant_type", "authorization_code"),
            ("code", "google-code"),
            ("redirect_uri", "http://localhost:3333/mcps/oauth/callback"),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
    );

    // The connection is checked with one request, which carries no developer token.
    provider(&google.definition)
        .verify(google.context(&[]))
        .await
        .unwrap();
    let checks: Vec<_> = google
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.url.host_str() == Some("googleads.googleapis.com"))
        .collect();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].method, http::Method::GET);
    assert_eq!(
        checks[0].url.path(),
        "/v25/customers:listAccessibleCustomers"
    );
    assert_eq!(
        checks[0].header("authorization").as_deref(),
        Some("Bearer google-access-token")
    );
    assert_eq!(checks[0].header("developer-token"), None);
}

#[tokio::test]
async fn a_sign_in_google_no_longer_knows_does_not_verify() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        (request.url.host_str() == Some("googleads.googleapis.com")).then(|| {
            mymcps_google_ads::testing::google_json(
                &json!({ "error": { "code": 401, "message": "Invalid credentials", "status": "UNAUTHENTICATED" } }),
                401,
            )
        })
    }))
    .await;

    let error = provider(&google.definition)
        .verify(google.context(&[]))
        .await
        .unwrap_err();
    assert!(error.is_authorization_error());
    assert_eq!(
        error.to_string(),
        "Google rejected the saved authorization. Re-authorize this MCP in MyMCPs."
    );
}
