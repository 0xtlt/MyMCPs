//! Vine schemas for the arguments of the tools that manage the MCP registry.
//! They check the shape of what an agent sends; what an MCP may be (a URL
//! that is reachable, a package that exists) is judged by the validators of
//! the MCPs page, which the tools run afterwards.
//!
//! A blank string is read as null here too, as the endpoint does before
//! the arguments get here: an agent clears a description by sending an empty
//! one, or null.

use std::collections::HashMap;
use std::sync::LazyLock;

use mymcps_vine as vine;
use serde_json::{Map, Value};
use vine::{FieldRef, MessagesProvider, Rule, Validator};

/// The arguments the create and update tools share, besides `name` and
/// `transport`, which only the first requires.
const SHARED_ARGUMENTS: [&str; 13] = [
    "description",
    "url",
    "npm_package",
    "npm_version",
    "npm_args",
    "npm_env",
    "auth_type",
    "bearer_token",
    "header_name",
    "header_value",
    "enabled",
    "timeout_seconds",
    "tailnet",
];

/// An agent that gets an argument wrong reads one sentence saying what that
/// argument must be, whichever rule refused it.
fn argument_messages<const N: usize>(
    messages: [(&'static str, &'static str); N],
) -> impl MessagesProvider {
    let messages = HashMap::from(messages);
    move |default_message: &str,
          _rule: &str,
          field: FieldRef<'_>,
          args: Option<&Map<String, Value>>| {
        match messages.get(field.wildcard_path) {
            Some(message) => (*message).to_owned(),
            // Named by its path, such as `npm_env.0.name`.
            None => vine::interpolate(default_message, field.path, args),
        }
    }
}

/// Refuse an argument the tool does not have: Vine drops it, and the agent
/// would believe a misspelled field was applied.
fn only_arguments(names: Vec<&'static str>) -> Rule {
    vine::rule(move |value, field| {
        let Some(object) = value.as_object() else {
            return;
        };
        if let Some(unknown) = object.keys().find(|key| !names.contains(&key.as_str())) {
            field.report(
                &format!("{unknown} is not an argument of this tool"),
                "unknownArgument",
            );
        }
    })
}

fn slug() -> vine::VineString {
    vine::string().trim().min_length(1).max_length(120)
}

/// The fields of an MCP as the tools take them. `create` makes the name and
/// the transport required, which an update leaves as they are saved.
fn fields(create: bool) -> Vec<(&'static str, vine::Schema)> {
    let name = vine::string().trim().min_length(1).max_length(120);
    let transport = vine::enum_(["http", "npm"]);
    vec![
        ("name", if create { name } else { name.optional() }.into()),
        (
            "transport",
            if create {
                transport
            } else {
                transport.optional()
            }
            .into(),
        ),
        (
            "description",
            vine::string()
                .trim()
                .max_length(500)
                .nullable()
                .optional()
                .into(),
        ),
        (
            "url",
            vine::string().trim().max_length(2048).optional().into(),
        ),
        (
            "npm_package",
            vine::string().trim().max_length(254).optional().into(),
        ),
        (
            "npm_version",
            vine::string().trim().max_length(64).optional().into(),
        ),
        (
            "npm_args",
            vine::array(
                vine::string()
                    .trim()
                    .min_length(1)
                    .max_length(1000)
                    .regex(vine::js::regex(r"^\S+$", "").expect("a static pattern")),
            )
            .max_length(50)
            .optional()
            .into(),
        ),
        (
            "npm_env",
            vine::array(vine::object! {
                "name" => vine::string().trim().min_length(1).max_length(128),
                // Secrets are not trimmed, and a missing value keeps the saved one.
                "value" => vine::string().max_length(8192).optional(),
            })
            .max_length(50)
            .optional()
            .into(),
        ),
        (
            "auth_type",
            vine::enum_(["auto", "bearer", "header"]).optional().into(),
        ),
        (
            "bearer_token",
            vine::string().trim().max_length(4000).optional().into(),
        ),
        (
            "header_name",
            vine::string().trim().max_length(120).optional().into(),
        ),
        (
            "header_value",
            vine::string().trim().max_length(4000).optional().into(),
        ),
        ("enabled", vine::boolean().strict().optional().into()),
        (
            "tailnet",
            vine::string()
                .trim()
                .max_length(40)
                .nullable()
                .optional()
                .into(),
        ),
        (
            "timeout_seconds",
            vine::number()
                .strict()
                .without_decimals()
                .min(1)
                .nullable()
                .optional()
                .into(),
        ),
    ]
}

fn fields_object(create: bool, with_slug: bool) -> vine::VineObject {
    let mut allowed = vec!["name", "transport"];
    allowed.extend(SHARED_ARGUMENTS);
    let mut properties = fields(create);
    if with_slug {
        allowed.push("slug");
        properties.insert(0, ("slug", slug().into()));
    }
    vine::object(properties).use_rule(only_arguments(allowed))
}

const FIELD_MESSAGES: [(&str, &str); 17] = [
    (
        "slug",
        "slug must be the slug of an MCP, from mcp_registry_list",
    ),
    ("name", "name must be non-empty and at most 120 characters"),
    (
        "transport",
        "transport must be \"http\" or \"npm\": configure built-in MCPs from the MyMCPs page",
    ),
    ("description", "description must be at most 500 characters"),
    ("url", "url must be a string of at most 2048 characters"),
    (
        "npm_package",
        "npm_package must be a string of at most 254 characters",
    ),
    (
        "npm_version",
        "npm_version must be a string of at most 64 characters",
    ),
    (
        "npm_args",
        "npm_args must be a list of at most 50 arguments, none of them empty or holding a space",
    ),
    (
        "npm_args.*",
        "npm_args must be a list of at most 50 arguments, none of them empty or holding a space",
    ),
    (
        "npm_env",
        "npm_env must be a list of at most 50 { name, value } objects",
    ),
    (
        "auth_type",
        "auth_type must be \"auto\", \"bearer\" or \"header\"",
    ),
    (
        "bearer_token",
        "bearer_token must be a string of at most 4000 characters",
    ),
    (
        "header_name",
        "header_name must be a string of at most 120 characters",
    ),
    (
        "header_value",
        "header_value must be a string of at most 4000 characters",
    ),
    ("enabled", "enabled must be true or false"),
    (
        "tailnet",
        "tailnet must be the name of a tailnet of the team, or null for the internet",
    ),
    (
        "timeout_seconds",
        "timeout_seconds must be a whole number of seconds, 1 or more, or null for the default",
    ),
];

fn schema(create: bool, with_slug: bool) -> Validator {
    vine::global()
        .create(fields_object(create, with_slug))
        .messages_provider(argument_messages(FIELD_MESSAGES))
}

/// Arguments of `mcp_registry_create`.
pub static CREATE_REGISTRY_MCP: LazyLock<Validator> = LazyLock::new(|| schema(true, false));

/// Arguments of `mcp_registry_update`: the slug, and what changes.
pub static UPDATE_REGISTRY_MCP: LazyLock<Validator> = LazyLock::new(|| schema(false, true));

/// Arguments of the tools that act on one MCP and nothing else.
pub static REGISTRY_MCP_SLUG: LazyLock<Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! { "slug" => slug() }.use_rule(only_arguments(vec!["slug"])))
        .messages_provider(argument_messages(FIELD_MESSAGES))
});

/// Arguments of `mcp_registry_list`, which has none.
pub static REGISTRY_NO_ARGUMENTS: LazyLock<Validator> =
    LazyLock::new(|| vine::global().create(vine::object! {}.use_rule(only_arguments(Vec::new()))));
