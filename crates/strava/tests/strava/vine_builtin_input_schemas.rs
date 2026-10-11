//! The port of `tests/unit/vine_builtin_input_schemas.spec.ts`, for the
//! tools of Strava.
//!
//! The JSON Schema of a tool is written by hand, for its descriptions and
//! defaults. Its validator can describe itself in the same terms: the two
//! must agree, or agents are told one thing and held to another.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::support::STRAVA;

/// What a tool may say about an argument beyond its type. Whatever it says, its validator enforces.
const BOUNDS: [&str; 6] = [
    "minimum",
    "maximum",
    "maxLength",
    "minItems",
    "maxItems",
    "enum",
];

/// Arguments a validator checks although the tool neither advertises nor uses them.
const UNADVERTISED: [(&str, &[&str]); 1] = [
    // Strava returns the efforts on a segment on one page.
    ("list_segment_efforts", &["page"]),
];

/// Vine describes a choice among words by the words alone.
fn type_of(schema: &Value) -> Option<&str> {
    match schema.get("type") {
        Some(named) => named.as_str(),
        None => schema.get("enum").map(|_| "string"),
    }
}

fn sorted<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort_unstable();
    names
}

fn names(list: Option<&Value>) -> Vec<&str> {
    let list = list.and_then(Value::as_array);
    sorted(list.into_iter().flatten().filter_map(Value::as_str))
}

#[test]
fn every_tool_checks_the_arguments_it_advertises() {
    let no_properties = Map::new();
    let enforced_schemas = STRAVA.enforced_schemas();

    for tool in STRAVA.tools() {
        let id = tool.name;
        let advertised = tool.input_schema;
        let (_, enforced) = enforced_schemas
            .iter()
            .find(|(name, _)| *name == id)
            .unwrap();

        assert_eq!(advertised["type"], "object", "{id}");
        assert_eq!(enforced["type"], "object", "{id}");
        let advertised_properties = advertised
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&no_properties);
        let enforced_properties = enforced["properties"].as_object().unwrap();
        let unadvertised = UNADVERTISED
            .iter()
            .filter(|(tool, _)| *tool == id)
            .flat_map(|(_, arguments)| arguments.iter().copied());
        assert_eq!(
            sorted(enforced_properties.keys().map(String::as_str)),
            sorted(
                advertised_properties
                    .keys()
                    .map(String::as_str)
                    .chain(unadvertised)
            ),
            "{id}"
        );
        assert_eq!(
            names(enforced.get("required")),
            names(advertised.get("required")),
            "{id}"
        );

        for (name, described) in advertised_properties {
            let checked = &enforced_properties[name];
            assert_eq!(
                type_of(checked),
                described["type"].as_str(),
                "{id}: type of {name}"
            );
            for bound in BOUNDS {
                if let Some(advertised_bound) = described.get(bound) {
                    assert_eq!(
                        checked.get(bound),
                        Some(advertised_bound),
                        "{id}: {bound} of {name}"
                    );
                }
            }
            if let Some(items) = described.get("items") {
                assert_eq!(
                    type_of(&checked["items"]),
                    items["type"].as_str(),
                    "{id}: items of {name}"
                );
                assert_eq!(
                    checked["items"].get("enum"),
                    items.get("enum"),
                    "{id}: items of {name}"
                );
            }
        }
    }
}

#[test]
fn compares_every_tool_of_strava() {
    let tools = STRAVA.tools();
    assert_eq!(tools.len(), 21);
    assert_eq!(STRAVA.enforced_schemas().len(), 21);
    let ids: HashSet<&str> = tools.iter().map(|tool| tool.name).collect();
    assert_eq!(ids.len(), 21);
    for (id, _) in UNADVERTISED {
        assert!(ids.contains(id), "{id}");
    }
}
