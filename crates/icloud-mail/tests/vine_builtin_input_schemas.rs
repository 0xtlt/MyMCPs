//! The port of `tests/unit/vine_builtin_input_schemas.spec.ts`, for the
//! tools of iCloud Mail.

use serde_json::Value;

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
fn type_of(schema: &Value) -> Option<&str> {
    schema["type"]
        .as_str()
        .or_else(|| schema.get("enum").map(|_| "string"))
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

fn keys(value: &Value) -> Vec<String> {
    sorted(
        value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default(),
    )
}

fn strings(value: &Value) -> Vec<String> {
    sorted(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// The JSON Schema of a tool is written by hand, for its descriptions and
/// defaults. Its validator can describe itself in the same terms: the two must
/// agree, or agents are told one thing and held to another.
#[test]
fn every_tool_checks_the_arguments_it_advertises() {
    let definition = mymcps_icloud_mail::definition();
    let enforced_schemas = definition.enforced_schemas();
    let tools = definition.tools();
    assert_eq!(tools.len(), 9);
    assert_eq!(enforced_schemas.len(), 9);

    for (tool, (name, enforced)) in tools.iter().zip(&enforced_schemas) {
        assert_eq!(tool.name, *name);
        let advertised = tool.input_schema;

        assert_eq!(advertised["type"], "object", "{name}");
        assert_eq!(enforced["type"], "object", "{name}");
        assert_eq!(
            keys(&enforced["properties"]),
            keys(&advertised["properties"]),
            "{name}"
        );
        assert_eq!(
            strings(&enforced["required"]),
            strings(&advertised["required"]),
            "{name}"
        );

        for (argument, described) in advertised["properties"].as_object().unwrap() {
            let checked = &enforced["properties"][argument];
            assert_eq!(
                type_of(checked),
                described["type"].as_str(),
                "type of {argument} of {name}"
            );
            for bound in BOUNDS {
                if let Some(advertised_bound) = described.get(bound) {
                    assert_eq!(
                        checked.get(bound),
                        Some(advertised_bound),
                        "{bound} of {argument} of {name}"
                    );
                }
            }
            if let Some(items) = described.get("items") {
                assert_eq!(
                    type_of(&checked["items"]),
                    items["type"].as_str(),
                    "items of {argument} of {name}"
                );
                assert_eq!(
                    checked["items"].get("enum"),
                    items.get("enum"),
                    "items of {argument} of {name}"
                );
            }
        }
    }
}

/// What agents and administrators read is kept word for word, and in the
/// order it was written in. The fixture is what the Node app advertises,
/// written by `fixtures/dump-tools.mjs`.
#[test]
fn advertises_its_tools_as_the_node_app_did() {
    let node: Value =
        serde_json::from_str(include_str!("fixtures/icloud_mail_tools.json")).unwrap();
    let definition = mymcps_icloud_mail::definition();

    assert_eq!(definition.key(), node["key"]);
    assert_eq!(definition.name(), node["name"]);
    let password = definition.password().unwrap();
    // The patterns are checked by what they accept, in `builtin_icloud_mail.rs`: the regex crate writes them its own way.
    assert_eq!(password.username_hint, node["password"]["usernameHint"]);
    assert_eq!(password.password_hint, node["password"]["passwordHint"]);
    assert_eq!(
        serde_json::json!(password.permissions),
        node["password"]["permissions"]
    );
    assert_eq!(password.alias_hint, node["password"]["aliasHint"]);

    let tools = definition.tools();
    let advertised = node["tools"].as_array().unwrap();
    assert_eq!(tools.len(), advertised.len());
    for (tool, node) in tools.iter().zip(advertised) {
        assert_eq!(tool.name, node["name"]);
        assert_eq!(tool.description, node["description"], "{}", tool.name);
        assert_eq!(tool.input_schema, &node["inputSchema"], "{}", tool.name);
        // The same keys in the same order, which is the order an agent reads them in.
        assert_eq!(
            tool.input_schema.to_string(),
            node["inputSchema"].to_string(),
            "{}",
            tool.name
        );
        assert_eq!(
            serde_json::json!(tool.requires_any_scope),
            node["requiresAnyScope"],
            "{}",
            tool.name
        );
        assert_eq!(tool.write, node["write"], "{}", tool.name);
        assert!(!tool.asks_approval, "{}", tool.name);
    }
}
