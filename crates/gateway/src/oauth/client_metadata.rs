//! The shape of the metadata a client registers itself with: RFC 7591, as
//! `OAuthClientMetadataSchema` of the MCP SDK reads it. The members it knows
//! come out in the order the schema lists them, the others are dropped.

use mymcps_vine as vine;
use serde_json::{Map, Value};
use url::Url;

/// How a member of the metadata is read.
#[derive(Clone, Copy)]
enum Member {
    Text,
    TextList,
    Url,
    /// Older clients send an empty logo or terms URI, which counts as none.
    UrlOrEmpty,
    UrlList,
    Any,
}

const MEMBERS: [(&str, Member, bool); 16] = [
    ("redirect_uris", Member::UrlList, true),
    ("token_endpoint_auth_method", Member::Text, false),
    ("grant_types", Member::TextList, false),
    ("response_types", Member::TextList, false),
    ("client_name", Member::Text, false),
    ("client_uri", Member::Url, false),
    ("logo_uri", Member::UrlOrEmpty, false),
    ("scope", Member::Text, false),
    ("contacts", Member::TextList, false),
    ("tos_uri", Member::UrlOrEmpty, false),
    ("policy_uri", Member::Text, false),
    ("jwks_uri", Member::Url, false),
    ("jwks", Member::Any, false),
    ("software_id", Member::Text, false),
    ("software_version", Member::Text, false),
    ("software_statement", Member::Text, false),
];

/// A URL the schema accepts, as it returns it: without the whitespace
/// around it and the tabs and line breaks a URL parser ignores. Schemes that
/// run code where the URL is opened are refused.
fn safe_url(value: &Value) -> Option<Value> {
    let trimmed = vine::js::trim(value.as_str()?);
    Url::parse(trimmed).ok()?;
    let stripped = trimmed.replace(['\t', '\n', '\r'], "");
    let url = Url::parse(&stripped).ok()?;
    if ["javascript", "data", "vbscript"].contains(&url.scheme()) {
        return None;
    }
    Some(Value::String(stripped))
}

fn list(value: &Value, member: impl Fn(&Value) -> Option<Value>) -> Option<Value> {
    value
        .as_array()?
        .iter()
        .map(member)
        .collect::<Option<Vec<Value>>>()
        .map(Value::Array)
}

fn text(value: &Value) -> Option<Value> {
    value.is_string().then(|| value.clone())
}

/// The metadata as the schema returns it, or `None` when it refuses it.
pub(crate) fn parse_client_metadata(input: &Value) -> Option<Map<String, Value>> {
    let input = input.as_object()?;
    let mut metadata = Map::new();
    for (name, member, required) in MEMBERS {
        let Some(value) = input.get(name) else {
            if required {
                return None;
            }
            continue;
        };
        let parsed = match member {
            Member::Text => text(value)?,
            Member::TextList => list(value, text)?,
            Member::Url => safe_url(value)?,
            Member::UrlOrEmpty if value.as_str() == Some("") => continue,
            Member::UrlOrEmpty => safe_url(value)?,
            Member::UrlList => list(value, safe_url)?,
            Member::Any => value.clone(),
        };
        metadata.insert(name.to_string(), parsed);
    }
    Some(metadata)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(input: Value) -> Option<Value> {
        parse_client_metadata(&input).map(Value::Object)
    }

    /// What `OAuthClientMetadataSchema.safeParse` of the MCP SDK 1.32 answers.
    #[test]
    fn returns_the_members_it_knows_in_the_order_of_the_schema() {
        let metadata = parsed(json!({
            "software_version": "1",
            "jwks": { "a": 1 },
            "zzz": 1,
            "redirect_uris": ["https://a.example/cb"],
            "client_name": "x",
            "logo_uri": "",
            "tos_uri": "https://a.example/tos",
            "scope": "mcp:tools",
            "contacts": ["a"],
            "client_uri": "https://a.example",
        }))
        .unwrap();
        assert_eq!(
            metadata.to_string(),
            r#"{"redirect_uris":["https://a.example/cb"],"client_name":"x","client_uri":"https://a.example","scope":"mcp:tools","contacts":["a"],"tos_uri":"https://a.example/tos","jwks":{"a":1},"software_version":"1"}"#
        );

        assert_eq!(
            parsed(json!({
                "redirect_uris": ["https://a/cb"],
                "policy_uri": "anything",
                "software_id": "s",
                "software_statement": "st",
                "token_endpoint_auth_method": "none",
                "grant_types": ["a"],
                "response_types": ["b"],
            }))
            .unwrap()
            .to_string(),
            r#"{"redirect_uris":["https://a/cb"],"token_endpoint_auth_method":"none","grant_types":["a"],"response_types":["b"],"policy_uri":"anything","software_id":"s","software_statement":"st"}"#
        );
    }

    #[test]
    fn reads_urls_as_the_schema_does() {
        assert_eq!(
            parsed(
                json!({ "redirect_uris": ["  https://a.example/cb  ", "https://a.exa\tmple/cb\n"] })
            ),
            Some(json!({ "redirect_uris": ["https://a.example/cb", "https://a.example/cb"] }))
        );
        assert_eq!(
            parsed(json!({ "redirect_uris": ["\u{a0}https://a/cb\u{a0}"] })),
            Some(json!({ "redirect_uris": ["https://a/cb"] }))
        );
        for accepted in [
            json!([]),
            json!(["cursor://anysphere.cursor-mcp/oauth/callback"]),
            json!(["http:example.com"]),
            json!(["mailto:a@b.c"]),
            json!(["\u{0}https://a/cb\u{1f}"]),
        ] {
            assert_eq!(
                parsed(json!({ "redirect_uris": accepted })),
                Some(json!({ "redirect_uris": accepted })),
            );
        }
        for refused in [
            json!(["javascript:alert(1)"]),
            json!(["JavaScript:alert(1)"]),
            json!(["data:text/plain,hi"]),
            json!(["vbscript:x"]),
            json!(["not a url"]),
            json!(["/callback"]),
            json!([" "]),
            json!([""]),
            json!([1]),
            json!([null]),
            json!("https://a/cb"),
            json!(null),
        ] {
            assert_eq!(
                parsed(json!({ "redirect_uris": refused })),
                None,
                "{refused}"
            );
        }
    }

    #[test]
    fn refuses_metadata_of_another_shape() {
        for refused in [
            json!({}),
            json!(["x"]),
            json!("x"),
            json!(null),
            json!({ "redirect_uris": ["https://a/cb"], "client_name": null }),
            json!({ "redirect_uris": ["https://a/cb"], "client_name": 5 }),
            json!({ "redirect_uris": ["https://a/cb"], "contacts": [1] }),
            json!({ "redirect_uris": ["https://a/cb"], "grant_types": "authorization_code" }),
            json!({ "redirect_uris": ["https://a/cb"], "scope": ["mcp:tools"] }),
            json!({ "redirect_uris": ["https://a/cb"], "logo_uri": "nope" }),
            json!({ "redirect_uris": ["https://a/cb"], "logo_uri": "javascript:1" }),
            json!({ "redirect_uris": ["https://a/cb"], "logo_uri": null }),
            json!({ "redirect_uris": ["https://a/cb"], "logo_uri": " " }),
            json!({ "redirect_uris": ["https://a/cb"], "jwks_uri": "" }),
            json!({ "redirect_uris": ["https://a/cb"], "client_uri": "" }),
        ] {
            assert_eq!(parsed(refused.clone()), None, "{refused}");
        }
    }

    #[test]
    fn reads_an_empty_logo_or_terms_uri_as_none() {
        assert_eq!(
            parsed(json!({
                "tos_uri": "",
                "redirect_uris": ["https://a/cb"],
                "logo_uri": "https://a/logo",
            })),
            Some(json!({ "redirect_uris": ["https://a/cb"], "logo_uri": "https://a/logo" }))
        );
        assert_eq!(
            parsed(json!({ "redirect_uris": ["https://a/cb"], "jwks": null })),
            Some(json!({ "redirect_uris": ["https://a/cb"], "jwks": null }))
        );
    }
}
