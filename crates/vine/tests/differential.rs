//! The port against the real thing.
//!
//! `fixtures/vine.json` holds what Vine 4.4 answers for a few hundred schemas
//! over some eighteen thousand inputs: the output it returns or the errors it
//! reports, and the JSON Schema it gives for each schema. The schemas are
//! written in a small JSON notation that this file builds with the Rust API
//! and that the script that made the fixtures built with Vine. Callbacks
//! cannot be written in JSON: they go by name, and are written once here and
//! once in the script.
//!
//! The script is not part of the repository: it runs Node on the
//! TypeScript app's `node_modules`, which the port does away with. The
//! fixtures are what remains of it, and what the port is held to.

use std::collections::HashSet;

use mymcps_vine as vine;
use serde_json::{Map, Value, json};
use vine::{FieldContext, FieldRef, MessagesProvider, ParseContext, Rule, Schema};

const FIXTURES: &str = include_str!("fixtures/vine.json");

fn is_blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Named callbacks, as in the script.
// ---------------------------------------------------------------------------

fn parser(name: &str) -> fn(Option<Value>, &ParseContext<'_>) -> Option<Value> {
    match name {
        "blankAsMissing" => |value, _| match value {
            Some(Value::String(text)) if text.is_empty() => None,
            other => other,
        },
        "stringOrAbsent" => |value, _| value.filter(Value::is_string),
        "scopeList" => |value, _| match value {
            Some(Value::String(text)) => {
                Some(text.split(' ').filter(|scope| !scope.is_empty()).collect())
            }
            _ => None,
        },
        "single" => |value, _| match value {
            Some(text @ Value::String(_)) => Some(json!([text])),
            other => other,
        },
        "arrayOrEmpty" => {
            |value, _| Some(value.filter(Value::is_array).unwrap_or_else(|| json!([])))
        }
        "defaultTen" => |value, _| Some(value.unwrap_or_else(|| json!(10))),
        "defaultBasic" => |value, _| match value {
            None | Some(Value::Null) => Some(json!("client_secret_basic")),
            other => other,
        },
        "trimLower" => |value, _| match value {
            Some(Value::String(text)) => {
                let mode = vine::js::trim(&text).to_lowercase();
                (!mode.is_empty()).then_some(Value::String(mode))
            }
            other => other,
        },
        "boolWords" => |value, _| match value.as_ref().and_then(Value::as_str) {
            Some("true") => Some(json!(true)),
            Some("false") => Some(json!(false)),
            Some("") => None,
            _ => value,
        },
        "onlyWithA" => |value, context| {
            if is_blank(context.parent_get("a")) {
                None
            } else {
                value
            }
        },
        "fromMeta" => |value, context| {
            value.or_else(|| {
                context
                    .meta::<Value>()
                    .and_then(|meta| meta.get("fallback"))
                    .cloned()
            })
        },
        "rootKind" => |value, context| {
            value.or_else(|| {
                Some(json!(match context.data() {
                    None => "undefined",
                    Some(Value::Array(_)) => "array",
                    Some(Value::Null | Value::Object(_)) => "object",
                    Some(Value::String(_)) => "string",
                    Some(Value::Number(_)) => "number",
                    Some(Value::Bool(_)) => "boolean",
                }))
            })
        },
        "toNull" => |_, _| Some(Value::Null),
        "toUndefined" => |_, _| None,
        "distinctOrDefault" => |value, _| {
            let list = match value {
                None | Some(Value::Null) => json!(["authorization_code", "refresh_token"]),
                Some(list) => list,
            };
            let Value::Array(items) = list else {
                return Some(list);
            };
            let mut distinct: Vec<Value> = Vec::new();
            for item in items {
                if !vine::js::includes(&distinct, &item) {
                    distinct.push(item);
                }
            }
            Some(Value::Array(distinct))
        },
        other => panic!("unknown parser {other}"),
    }
}

fn transformer(name: &str) -> fn(Value, &FieldContext<'_>) -> Value {
    match name {
        "words" => |value, _| match value.as_str() {
            Some(text) => text
                .split(vine::js::is_whitespace)
                .filter(|part| !part.is_empty())
                .collect(),
            None => json!([]),
        },
        "nameParts" => |value, _| {
            let name = value.as_str().unwrap_or_default();
            let separator = name.find("__").unwrap_or(0);
            json!({ "slug": name[..separator], "toolName": name[separator + 2..] })
        },
        "identity" => |value, _| value,
        "describe" => |value, field| {
            json!({
                "value": value,
                "name": field.name(),
                "path": field.path(),
                "wildCardPath": field.wildcard_path(),
                "isArrayMember": field.is_array_member(),
            })
        },
        "upper" => |value, _| match value {
            Value::String(text) => Value::String(text.to_uppercase()),
            other => other,
        },
        other => panic!("unknown transform {other}"),
    }
}

fn object_of(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        other => panic!("expected an object, got {other}"),
    }
}

/// `JSON.stringify(name)` of a field name or path: the index of an array
/// item is a number in Vine.
fn quoted(text: &str, is_index: bool) -> String {
    if is_index {
        text.to_owned()
    } else {
        Value::String(text.to_owned()).to_string()
    }
}

fn rule(name: &str, options: Option<&Value>) -> Rule {
    let options = options.cloned().unwrap_or(Value::Null);
    match name {
        "notNull" => vine::rule(|_, field| {
            if field.is_null() {
                field.report("The {{ field }} field must not be null", "notNull");
            }
        })
        .implicit(),
        "integer" => {
            let min = options["min"].as_f64().unwrap();
            let max = options["max"].as_f64();
            let digits = vine::js::regex(r"^-?\d+$", "").unwrap();
            vine::rule(move |value, field| {
                // Agents often quote large identifiers.
                let parsed = match value {
                    Value::String(text) if digits.test(vine::js::trim(text)) => {
                        Some(vine::js::string_to_number(text))
                    }
                    other => vine::js::as_f64(other),
                };
                let within = |number: f64| {
                    vine::js::is_safe_integer(number)
                        && number >= min
                        && number <= max.unwrap_or(9_007_199_254_740_991.0)
                };
                match parsed.filter(|number| within(*number)) {
                    Some(number) => field.mutate(vine::js::number(number)),
                    None => field.report_with(
                        if max.is_some() {
                            "{{ field }} must be an integer between {{ min }} and {{ max }}"
                        } else {
                            "{{ field }} must be an integer of at least {{ min }}"
                        },
                        "integer",
                        options.clone(),
                    ),
                }
            })
            .json_schema(move |schema| {
                schema.insert("type".to_owned(), json!("integer"));
                schema.insert("minimum".to_owned(), vine::js::number(min));
                if let Some(max) = max {
                    schema.insert("maximum".to_owned(), vine::js::number(max));
                }
            })
        }
        "text" => {
            let max = usize::try_from(options["max"].as_u64().unwrap()).unwrap();
            vine::rule(move |value, field| {
                if !value
                    .as_str()
                    .is_some_and(|text| vine::js::utf16_len(text) <= max)
                {
                    field.report_with(
                        "{{ field }} must be text of at most {{ max }} characters",
                        "text",
                        json!({ "max": max }),
                    );
                }
            })
            .json_schema(move |schema| {
                schema.insert("type".to_owned(), json!("string"));
                schema.insert("maxLength".to_owned(), json!(max));
            })
        }
        "pattern" => {
            let expression = vine::js::regex(options["source"].as_str().unwrap(), "").unwrap();
            let hint = options["hint"].clone();
            vine::rule(move |value, field| {
                let written = match value {
                    Value::Number(_) => Some(vine::js::to_string(value)),
                    Value::String(text) => Some(text.clone()),
                    _ => None,
                };
                match written.map(|written| vine::js::trim(&written).to_owned()) {
                    Some(written) if expression.test(&written) => field.mutate(written),
                    _ => field.report_with(
                        "{{ field }} must be {{ hint }}",
                        "pattern",
                        json!({ "hint": hint }),
                    ),
                }
            })
            .json_schema(|schema| {
                schema.insert("type".to_owned(), json!("string"));
            })
        }
        "requiredUnless" => {
            let other = options["other"].as_str().unwrap().to_owned();
            let sentence = options["sentence"].as_str().map(str::to_owned);
            vine::rule(move |value, field| {
                let blank = field.is_undefined() || is_blank(Some(value));
                if blank && is_blank(field.parent_get(&other)) {
                    field.report_with(
                        sentence
                            .as_deref()
                            .unwrap_or("{{ field }} is required unless {{ other }} is set"),
                        "requiredUnless",
                        json!({ "other": other }),
                    );
                }
            })
            .implicit()
        }
        "listLength" => {
            let min = options["min"].as_u64();
            let max = options["max"].as_u64().unwrap();
            let sentence = options["sentence"].as_str().unwrap().to_owned();
            vine::rule(move |value, field| {
                let length = value.as_array().map_or(0, |items| items.len() as u64);
                if length < min.unwrap_or(0) || length > max {
                    field.report(&sentence, "listLength");
                }
            })
            .json_schema(move |schema| {
                if let Some(min) = min {
                    schema.insert("minItems".to_owned(), json!(min));
                }
                schema.insert("maxItems".to_owned(), json!(max));
            })
        }
        "withAuthorizationCode" => vine::rule(|value, field| {
            let code = json!("authorization_code");
            if value
                .as_array()
                .is_some_and(|grants| !vine::js::includes(grants, &code))
            {
                field.report(
                    "The {{ field }} field must include authorization_code",
                    "withAuthorizationCode",
                );
            }
        }),
        "known" => vine::rule(|value, field| {
            let known = field
                .meta::<Value>()
                .and_then(|meta| meta.get("known"))
                .and_then(Value::as_array);
            if !known.is_some_and(|known| vine::js::includes(known, value)) {
                field.report("{{ field }} must be someone we know", "known");
            }
        }),
        "toObject" => vine::rule(|value, field| field.mutate(json!({ "raw": value }))),
        "toBlank" => vine::rule(|_, field| field.mutate(" ")),
        "fieldInfo" => vine::rule(|value, field| {
            let json = |value: Option<&Value>| {
                value.map_or_else(|| "undefined".to_owned(), Value::to_string)
            };
            let name = field.name().into_owned();
            let path = field.path();
            let message = format!(
                "name={} wild={} path={} member={} defined={} valid={} value={} parent={} data={}",
                quoted(&name, field.is_array_member()),
                field.wildcard_path(),
                quoted(&path, field.is_array_member() && path == name),
                field.is_array_member(),
                field.is_defined(),
                field.is_valid(),
                json((!field.is_undefined()).then_some(value)),
                json(field.parent()),
                json(field.data()),
            );
            field.report(&message, "fieldInfo");
        })
        .implicit(),
        "asUrl" => vine::rule(|_, field| field.report("custom reason for {{ field }}", "url")),
        "asOwn" => vine::rule(|_, field| field.report("custom reason for {{ field }}", "ownRule")),
        "twice" => vine::rule(|_, field| {
            field.report("first for {{ field }}", "twice");
            field.report_with("second for {{ field }}", "twice", json!({ "n": 2 }));
        }),
        "echo" => {
            let template = options["template"].as_str().unwrap().to_owned();
            let args = options["args"].clone();
            vine::rule(move |_, field| field.report_with(&template, "echo", args.clone()))
        }
        "describe" => {
            let properties = object_of(options);
            vine::rule(|_, _| {}).json_schema(move |schema| schema.extend(properties.clone()))
        }
        "nested" => {
            let key = options["key"].as_str().unwrap().to_owned();
            let expected = options["expected"].clone();
            vine::rule(move |_, field| {
                if !vine::js::strict_equals_opt(field.nested_value(&key), Some(&expected)) {
                    field.report_with(
                        &format!("{{{{ field }}}} expected {key} to be {{{{ expected }}}}"),
                        "nested",
                        json!({ "expected": expected }),
                    );
                }
            })
            .implicit()
        }
        other => panic!("unknown rule {other}"),
    }
}

// ---------------------------------------------------------------------------
// The schema notation.
// ---------------------------------------------------------------------------

enum Built {
    String(vine::VineString),
    Number(vine::VineNumber),
    Boolean(vine::VineBoolean),
    Enum(vine::VineEnum),
    Literal(vine::VineLiteral),
    Any(vine::VineAny),
    Date(vine::VineDate),
    Custom(vine::VineCustom),
    Object(vine::VineObject),
    Array(vine::VineArray),
    Record(vine::VineRecord),
}

/// Call a method every schema type has, whichever type this one is.
macro_rules! each {
    ($built:expr, $schema:ident => $call:expr) => {
        match $built {
            Built::String($schema) => Built::String($call),
            Built::Number($schema) => Built::Number($call),
            Built::Boolean($schema) => Built::Boolean($call),
            Built::Enum($schema) => Built::Enum($call),
            Built::Literal($schema) => Built::Literal($call),
            Built::Any($schema) => Built::Any($call),
            Built::Date($schema) => Built::Date($call),
            Built::Custom($schema) => Built::Custom($call),
            Built::Object($schema) => Built::Object($call),
            Built::Array($schema) => Built::Array($call),
            Built::Record($schema) => Built::Record($call),
        }
    };
}

/// Call a method only the types holding one value have.
macro_rules! each_literal {
    ($built:expr, $schema:ident => $call:expr) => {
        match $built {
            Built::String($schema) => Built::String($call),
            Built::Number($schema) => Built::Number($call),
            Built::Boolean($schema) => Built::Boolean($call),
            Built::Enum($schema) => Built::Enum($call),
            Built::Literal($schema) => Built::Literal($call),
            Built::Any($schema) => Built::Any($call),
            Built::Date($schema) => Built::Date($call),
            Built::Custom($schema) => Built::Custom($call),
            _ => panic!("not a literal type"),
        }
    };
}

impl From<Built> for Schema {
    fn from(built: Built) -> Schema {
        match built {
            Built::String(schema) => schema.into(),
            Built::Number(schema) => schema.into(),
            Built::Boolean(schema) => schema.into(),
            Built::Enum(schema) => schema.into(),
            Built::Literal(schema) => schema.into(),
            Built::Any(schema) => schema.into(),
            Built::Date(schema) => schema.into(),
            Built::Custom(schema) => schema.into(),
            Built::Object(schema) => schema.into(),
            Built::Array(schema) => schema.into(),
            Built::Record(schema) => schema.into(),
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_f64().unwrap())
        .collect()
}

fn length(value: &Value) -> usize {
    usize::try_from(value.as_u64().unwrap()).unwrap()
}

fn url_options(options: Option<&Value>) -> vine::UrlOptions {
    let mut url = vine::UrlOptions::default();
    let Some(Value::Object(options)) = options else {
        return url;
    };
    for (key, value) in options {
        let flag = || value.as_bool().unwrap();
        match key.as_str() {
            "protocols" => url.protocols = strings(value),
            "require_tld" => url.require_tld = flag(),
            "require_protocol" => url.require_protocol = flag(),
            "require_host" => url.require_host = flag(),
            "require_port" => url.require_port = flag(),
            "require_valid_protocol" => url.require_valid_protocol = flag(),
            "allow_underscores" => url.allow_underscores = flag(),
            "allow_trailing_dot" => url.allow_trailing_dot = flag(),
            "allow_protocol_relative_urls" => url.allow_protocol_relative_urls = flag(),
            "allow_fragments" => url.allow_fragments = flag(),
            "allow_query_components" => url.allow_query_components = flag(),
            "disallow_auth" => url.disallow_auth = flag(),
            "validate_length" => url.validate_length = flag(),
            "max_allowed_length" => url.max_allowed_length = length(value),
            other => panic!("unknown URL option {other}"),
        }
    }
    url
}

/// `/^(?!__).+?__/s`, which the regex crate cannot run: a separator that
/// is not at the very start.
fn namespaced(name: &str) -> bool {
    let after_first = name.chars().next().map_or(0, char::len_utf8);
    !name.starts_with("__") && name[after_first..].contains("__")
}

fn pattern(source: &str, flags: &str) -> vine::Pattern {
    if source == "^(?!__).+?__" {
        assert!(vine::js::regex(source, flags).is_err());
        return vine::Pattern::from_fn(source, namespaced);
    }
    vine::js::regex(source, flags).unwrap().into()
}

fn operator(name: &str) -> vine::Operator {
    match name {
        "=" => vine::Operator::Eq,
        "!=" => vine::Operator::NotEq,
        "in" => vine::Operator::In,
        "notIn" => vine::Operator::NotIn,
        ">" => vine::Operator::Gt,
        "<" => vine::Operator::Lt,
        ">=" => vine::Operator::Gte,
        "<=" => vine::Operator::Lte,
        other => panic!("unknown operator {other}"),
    }
}

fn build(spec: &Value) -> Built {
    let named_rules = || {
        spec["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| rule(entry[0].as_str().unwrap(), entry.get(1)))
            .collect::<Vec<_>>()
    };
    let strict = spec["strict"].as_bool().unwrap_or(false);
    let mut built = match spec["t"].as_str().unwrap() {
        "string" => Built::String(vine::string()),
        "number" if strict => Built::Number(vine::number().strict()),
        "number" => Built::Number(vine::number()),
        "boolean" if strict => Built::Boolean(vine::boolean().strict()),
        "boolean" => Built::Boolean(vine::boolean()),
        "enum" => Built::Enum(vine::enum_(spec["values"].as_array().unwrap().clone())),
        "literal" => Built::Literal(vine::literal(spec["value"].clone())),
        "any" => Built::Any(vine::any()),
        "date" => Built::Date(vine::date_iso8601()),
        "custom" => Built::Custom(vine::custom(named_rules())),
        "object" => {
            Built::Object(vine::object(spec["props"].as_array().unwrap().iter().map(
                |entry| (entry[0].as_str().unwrap(), Schema::from(build(&entry[1]))),
            )))
        }
        "array" => Built::Array(vine::array(build(&spec["each"]))),
        "record" => Built::Record(vine::record(build(&spec["each"]))),
        other => panic!("unknown type {other}"),
    };

    for call in spec["chain"].as_array().unwrap() {
        let method = call[0].as_str().unwrap();
        let text = |index: usize| call[index].as_str().unwrap();
        built = match (method, built) {
            ("optional", built) => each!(built, schema => schema.optional()),
            ("nullable", built) => each!(built, schema => schema.nullable()),
            ("bail", built) => {
                let state = call[1].as_bool().unwrap();
                each!(built, schema => schema.bail(state))
            }
            ("parse", built) => {
                let callback = parser(text(1));
                each!(built, schema => schema.parse(callback))
            }
            ("transform", built) => {
                let callback = transformer(text(1));
                each_literal!(built, schema => schema.transform(callback))
            }
            ("use", built) => {
                let added = rule(text(1), call.get(2));
                each!(built, schema => schema.use_rule(added))
            }
            ("requiredWhen", built) => {
                let (other, operator, expected) = (text(1), operator(text(2)), call[3].clone());
                each!(built, schema => schema.required_when(other, operator, expected))
            }
            ("requiredIfExists", built) => {
                let fields = strings(&call[1]);
                each!(built, schema => schema.required_if_exists(fields))
            }
            ("requiredIfAnyExists", built) => {
                let fields = strings(&call[1]);
                each!(built, schema => schema.required_if_any_exists(fields))
            }
            ("requiredIfMissing", built) => {
                let fields = strings(&call[1]);
                each!(built, schema => schema.required_if_missing(fields))
            }
            ("requiredIfAnyMissing", built) => {
                let fields = strings(&call[1]);
                each!(built, schema => schema.required_if_any_missing(fields))
            }
            ("allowUnknownProperties", Built::Object(schema)) => {
                Built::Object(schema.allow_unknown_properties())
            }

            ("trim", Built::String(schema)) => Built::String(schema.trim()),
            ("minLength", Built::String(schema)) => {
                Built::String(schema.min_length(length(&call[1])))
            }
            ("maxLength", Built::String(schema)) => {
                Built::String(schema.max_length(length(&call[1])))
            }
            ("fixedLength", Built::String(schema)) => {
                Built::String(schema.fixed_length(length(&call[1])))
            }
            ("regex", Built::String(schema)) => {
                Built::String(schema.regex(pattern(text(1), call[2].as_str().unwrap_or(""))))
            }
            ("url", Built::String(schema)) => match call.get(1) {
                None => Built::String(schema.url()),
                options => Built::String(schema.url_with(url_options(options))),
            },
            ("email", Built::String(schema)) => Built::String(schema.email()),
            ("confirmed", Built::String(schema)) => Built::String(
                match call
                    .get(1)
                    .map(|options| options["confirmationField"].as_str().unwrap())
                {
                    Some(field) => schema.confirmed(field),
                    None => schema.confirmed(None),
                },
            ),
            ("startsWith", Built::String(schema)) => Built::String(schema.starts_with(text(1))),
            ("endsWith", Built::String(schema)) => Built::String(schema.ends_with(text(1))),
            ("in", Built::String(schema)) => Built::String(schema.in_(strings(&call[1]))),

            ("min", Built::Number(schema)) => Built::Number(schema.min(call[1].as_f64().unwrap())),
            ("max", Built::Number(schema)) => Built::Number(schema.max(call[1].as_f64().unwrap())),
            ("range", Built::Number(schema)) => {
                let bounds = numbers(&call[1]);
                Built::Number(schema.range([bounds[0], bounds[1]]))
            }
            ("positive", Built::Number(schema)) => Built::Number(schema.positive()),
            ("withoutDecimals", Built::Number(schema)) => Built::Number(schema.without_decimals()),
            ("in", Built::Number(schema)) => Built::Number(schema.in_(numbers(&call[1]))),

            ("minLength", Built::Array(schema)) => {
                Built::Array(schema.min_length(length(&call[1])))
            }
            ("maxLength", Built::Array(schema)) => {
                Built::Array(schema.max_length(length(&call[1])))
            }
            ("fixedLength", Built::Array(schema)) => {
                Built::Array(schema.fixed_length(length(&call[1])))
            }
            (other, _) => panic!("unknown method {other}"),
        };
    }
    built
}

// ---------------------------------------------------------------------------
// Messages providers, as in the script.
// ---------------------------------------------------------------------------

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// `ArgumentMessages` of app/validators/builtin_tools.ts.
struct ArgumentMessages(vine::SimpleMessagesProvider);

impl MessagesProvider for ArgumentMessages {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        let argument = field.wildcard_path.split('.').next().unwrap_or_default();
        let field = field.with_name(if argument.is_empty() {
            "arguments"
        } else {
            argument
        });
        match args
            .and_then(|args| args.get("choices"))
            .and_then(Value::as_array)
        {
            Some(choices) => {
                let listed: Vec<String> = choices.iter().map(vine::js::to_string).collect();
                let mut args = args.cloned().unwrap_or_default();
                args.insert("choices".to_owned(), Value::String(listed.join(", ")));
                self.0.get_message(message, rule, field, Some(&args))
            }
            None => self.0.get_message(message, rule, field, args),
        }
    }
}

fn provider(config: &Value) -> Box<dyn MessagesProvider> {
    match config["kind"].as_str().unwrap() {
        "simple" => Box::new(
            vine::SimpleMessagesProvider::new(pairs(&config["messages"]))
                .with_fields(pairs(&config["fields"])),
        ),
        "argument" => Box::new(ArgumentMessages(vine::SimpleMessagesProvider::new(pairs(
            &config["messages"],
        )))),
        // `argumentMessages` of app/validators/gateway.ts: a sentence by
        // field, and otherwise the template as the rule reported it.
        "wildcardMap" => {
            let messages = pairs(&config["messages"]);
            Box::new(
                move |message: &str,
                      _rule: &str,
                      field: FieldRef<'_>,
                      _args: Option<&Map<String, Value>>| {
                    messages
                        .iter()
                        .find(|(path, _)| path == field.wildcard_path)
                        .map_or_else(|| message.to_owned(), |(_, sentence)| sentence.clone())
                },
            )
        }
        other => panic!("unknown provider {other}"),
    }
}

struct Boxed(Box<dyn MessagesProvider>);

impl MessagesProvider for Boxed {
    fn get_message(
        &self,
        message: &str,
        rule: &str,
        field: FieldRef<'_>,
        args: Option<&Map<String, Value>>,
    ) -> String {
        self.0.get_message(message, rule, field, args)
    }
}

// ---------------------------------------------------------------------------
// Comparison.
// ---------------------------------------------------------------------------

/// Deep equality where the order of keys counts, as it does in what the app
/// stores and sends, and where `5` and `5.0` are the same number.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((left_key, left), (right_key, right))| {
                        left_key == right_key && same(left, right)
                    })
        }
        _ => left == right,
    }
}

/// The one known difference: `new Date(text)` in V8 falls back to a parser
/// of its own that makes a date of almost anything with a digit in it, such
/// as `"5"`, which it reads as the first of May 2001. Vine accepts these
/// through Day.js. The port reads ISO 8601 and refuses them.
const LEGACY_DATES: [&str; 12] = [
    "5", " 5 ", "5.5", "-3", "-0", ".5", "5.", "+7", "--7", "1", "0", "01",
];

fn is_legacy_date(input: &Value) -> bool {
    let value = input.get("v").map(|value| value.get("x").unwrap_or(value));
    value
        .and_then(Value::as_str)
        .is_some_and(|text| LEGACY_DATES.contains(&text))
}

fn validator(case: &Value) -> vine::Validator {
    let config = &case["vine"];
    let mut instance =
        vine::Vine::new().convert_empty_strings_to_null(config["convert"].as_bool().unwrap());
    if !config["provider"].is_null() {
        instance = instance.messages_provider(Boxed(provider(&config["provider"])));
    }
    let mut validator = instance.create(build(&case["schema"]));
    if !config["validatorProvider"].is_null() {
        validator = validator.messages_provider(Boxed(provider(&config["validatorProvider"])));
    }
    validator
}

fn answer(validator: &vine::Validator, input: &Value) -> Value {
    let from_text: Option<Value> = input
        .get("text")
        .map(|text| serde_json::from_str(text.as_str().unwrap()).unwrap());
    let data: Option<&Value> = from_text.as_ref().or_else(|| input.get("v"));
    let result = match input.get("meta") {
        Some(meta) => validator.validate_opt_with(data, meta),
        None => validator.validate_opt(data),
    };
    match result {
        Ok(Some(output)) => json!(["ok", output]),
        Ok(None) => json!(["undef"]),
        Err(error) => json!(["err", error.to_json()]),
    }
}

#[test]
fn answers_what_vine_answers() {
    let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
    let cases = fixtures["cases"].as_array().unwrap();
    let mut names = HashSet::new();
    let mut compared = 0;
    let mut legacy_dates = 0;
    let mut mismatches = Vec::new();

    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert!(names.insert(name), "duplicate case {name}");
        let validator = validator(case);
        let inputs = fixtures["batteries"][case["inputs"].as_str().unwrap()]
            .as_array()
            .unwrap();
        let results = case["results"].as_array().unwrap();
        assert_eq!(inputs.len(), results.len(), "{name}");

        for (input, outcome) in inputs.iter().zip(results) {
            let expected = &case["outcomes"][usize::try_from(outcome.as_u64().unwrap()).unwrap()];
            let actual = answer(&validator, input);
            compared += 1;
            if name.contains(".date.") && is_legacy_date(input) {
                assert_eq!(expected[0], "ok", "{name} {input}");
                assert_eq!(actual[1][0]["rule"], "date", "{name} {input}");
                legacy_dates += 1;
                continue;
            }
            if !same(&actual, expected) {
                mismatches.push(format!(
                    "{name}\n   input: {input}\n    vine: {expected}\n    rust: {actual}"
                ));
            }
        }
    }

    assert!(compared > 18_000, "only {compared} inputs compared");
    // Twelve strings, for the date type alone and as a property, with every
    // modifier and both ways of reading empty strings.
    assert_eq!(legacy_dates, 156);
    assert!(
        mismatches.is_empty(),
        "{} of {compared} answers differ from Vine's:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn describes_schemas_as_vine_does() {
    let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
    let mut mismatches = Vec::new();
    for case in fixtures["cases"].as_array().unwrap() {
        let validator = validator(case);
        let actual = validator.to_json_schema();
        if !same(actual, &case["jsonSchema"]) {
            mismatches.push(format!(
                "{}\n    vine: {}\n    rust: {actual}",
                case["name"], case["jsonSchema"]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} JSON Schemas differ from Vine's:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
