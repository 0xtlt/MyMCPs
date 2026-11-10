//! The definition of the MCP: its sign-in, its setting, and the tools it lists.

mod support;

use std::collections::BTreeSet;
use std::sync::LazyLock;

use mymcps_builtin::BuiltinMcpDefinition;
use mymcps_google_search_console::testing::FakeSearchConsole;
use mymcps_google_search_console::{READ_SCOPE, SITE_URLS_SETTING, WRITE_SCOPE};
use serde_json::{Value, json};
use support::{Google, provider};

static DEFINITION: LazyLock<BuiltinMcpDefinition> =
    LazyLock::new(mymcps_google_search_console::definition);

/// What the setup dialog stores for the setting, or the hint it shows instead.
fn stored(value: &str) -> Result<String, String> {
    let setting = &DEFINITION.settings()[0];
    assert_eq!(setting.key, SITE_URLS_SETTING);
    let normalized = (setting.normalize.unwrap())(value);
    if setting.pattern.is_match(&normalized) {
        Ok(normalized)
    } else {
        Err(setting.hint.clone())
    }
}

#[test]
fn signs_in_with_a_google_cloud_oauth_client() {
    assert_eq!(DEFINITION.key(), "google-search-console");
    assert_eq!(DEFINITION.name(), "Google Search Console");
    let oauth = DEFINITION.oauth().unwrap();
    assert_eq!(
        oauth.authorize_url,
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(oauth.token_url, "https://oauth2.googleapis.com/token");
    assert_eq!(oauth.scopes, [READ_SCOPE]);
    assert_eq!(oauth.write_scopes, [WRITE_SCOPE]);
    assert_eq!(
        oauth.authorize_params,
        [("access_type", "offline"), ("prompt", "consent")]
    );
    assert!(oauth.sends_redirect_uri_with_code);
    assert_eq!(oauth.credential_provider, Some("google"));
    let client_id = oauth.client_id_pattern.as_ref().unwrap();
    assert!(client_id.is_match("1234567890-abc.apps.googleusercontent.com"));
    assert!(!client_id.is_match("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-field"));
}

#[test]
fn stores_the_allowed_properties_as_google_writes_them() {
    assert_eq!(
        stored("SC-Domain:Example.com, https://WWW.Example.com;\nhttps://example.com/blog/")
            .as_deref(),
        Ok("sc-domain:example.com https://www.example.com/ https://example.com/blog/")
    );
    assert!(!DEFINITION.settings()[0].required);
    let hint = "Enter up to 50 Search Console properties, such as sc-domain:example.com or https://www.example.com/, separated by commas";
    for wrong in [
        "example.com",
        "https://example.com/blog",
        "ftp://example.com/",
        "sc-domain:",
    ] {
        assert_eq!(stored(wrong).unwrap_err(), hint, "{wrong}");
    }
    let too_many: Vec<String> = (0..51).map(|n| format!("sc-domain:site{n}.com")).collect();
    assert_eq!(stored(&too_many.join(",")).unwrap_err(), hint);
}

#[test]
fn reads_with_either_scope_and_writes_with_the_full_one() {
    let tools = DEFINITION.tools();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name).collect();
    assert_eq!(
        names,
        [
            "list_sites",
            "query_search_analytics",
            "list_sitemaps",
            "get_sitemap",
            "inspect_url",
            "submit_sitemap",
            "delete_sitemap",
        ]
    );
    for tool in &tools {
        let write = tool.name.ends_with("_sitemap") && tool.name != "get_sitemap";
        assert_eq!(tool.write, write, "{}", tool.name);
        assert!(!tool.asks_approval, "{}", tool.name);
        let scopes: &[&str] = if write {
            &[WRITE_SCOPE]
        } else {
            &[READ_SCOPE, WRITE_SCOPE]
        };
        assert_eq!(tool.requires_any_scope, scopes, "{}", tool.name);
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

/// Vine describes a choice among words by the words alone.
fn type_of(schema: &Value) -> Value {
    match schema.get("type") {
        Some(named) => named.clone(),
        None if schema.get("enum").is_some() => json!("string"),
        None => Value::Null,
    }
}

/// The schema a tool advertises is written by hand, for its descriptions and
/// defaults: it must agree with what its validator enforces.
#[test]
fn checks_the_arguments_it_advertises() {
    for (tool, (name, enforced)) in DEFINITION.tools().iter().zip(DEFINITION.enforced_schemas()) {
        assert_eq!(tool.name, name);
        let advertised = tool.input_schema;
        assert_eq!(
            names(enforced.get("properties")),
            names(advertised.get("properties")),
            "{name}"
        );
        assert_eq!(
            names(enforced.get("required")),
            names(advertised.get("required")),
            "{name}"
        );
        for (argument, described) in advertised["properties"].as_object().unwrap() {
            let checked = &enforced["properties"][argument];
            assert_eq!(type_of(checked), described["type"], "{name}.{argument}");
            for bound in ["minimum", "maximum", "minItems", "maxItems", "enum"] {
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
                assert_eq!(
                    names(checked["items"].get("properties")),
                    names(items.get("properties")),
                    "items of {name}.{argument}"
                );
            }
        }
    }
}

#[tokio::test]
async fn verifies_the_sign_in_by_listing_the_properties() {
    let google = Google::new(FakeSearchConsole::new()).await;
    let context =
        mymcps_google_search_console::testing::search_console_context(google.env.clone(), &[]);
    provider(&google.definition).verify(context).await.unwrap();

    let requests = google.fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, http::Method::GET);
    assert_eq!(
        requests[0].url.as_str(),
        "https://searchconsole.googleapis.com/webmasters/v3/sites"
    );
    assert_eq!(
        requests[0].header("authorization").as_deref(),
        Some("Bearer google-access-token")
    );
}
