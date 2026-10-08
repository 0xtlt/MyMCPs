//! What a request sends: its query string and its body, parsed once.
//!
//! As the body parser of the Node app did, a body is only parsed for POST,
//! PUT, PATCH and DELETE, as a form or as JSON, up to 1 MiB. Every string in
//! it is trimmed, and an empty one becomes `null`, before anything validates
//! it. Multipart bodies are never parsed: no form of the app takes a file.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{FromRequestParts, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::request::Parts;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};

use crate::csrf::{CSRF_FIELD, CSRF_HEADER, CSRF_MESSAGE, verify_csrf_token};
use crate::error::wants_html;
use crate::redirect::redirect_back;
use crate::session::Session;

/// The largest body the pages accept.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Limits of the `qs` parser the Node app used.
const MAX_PARAMETERS: usize = 1000;
/// A form body may hold more fields than a query string. The pages of the
/// Node app posted JSON, which had no such limit; their forms here are
/// url-encoded, and the largest one, the tools of an MCP, sends two fields
/// for each of up to 2,000 tools. The size limit of a body still applies.
const MAX_FORM_PARAMETERS: usize = 5000;
const MAX_DEPTH: usize = 5;
const MAX_ARRAY_INDEX: usize = 20;

/// The parsed body of the request. An empty object when there is none.
#[derive(Debug, Clone, Default)]
pub struct ParsedBody(pub Map<String, Value>);

/// Body and query string merged, the query string winning, as
/// `request.all()` did: what handlers validate.
#[derive(Debug, Clone, Default)]
pub struct Input(pub Map<String, Value>);

impl Input {
    pub fn into_value(self) -> Value {
        Value::Object(self.0)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }
}

fn decode_component(raw: &str) -> String {
    percent_decode_str(&raw.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

/// `a.b.c` as `a[b][c]`: form bodies also nest with dots.
fn dots_to_brackets(key: &str) -> String {
    let mut converted = String::new();
    let mut rest = key;
    while let Some(dot) = rest.find('.') {
        let segment_end = rest[dot + 1..]
            .find(['.', '['])
            .map_or(rest.len(), |end| dot + 1 + end);
        if segment_end == dot + 1 {
            converted.push_str(&rest[..=dot]);
        } else {
            converted.push_str(&rest[..dot]);
            converted.push('[');
            converted.push_str(&rest[dot + 1..segment_end]);
            converted.push(']');
        }
        rest = &rest[segment_end.max(dot + 1)..];
    }
    converted.push_str(rest);
    converted
}

/// `a[b][]` as `["a", "b", ""]`. Past the depth limit, the rest of the key
/// is one segment, brackets included.
fn key_segments(key: &str) -> Vec<String> {
    let Some(open) = key.find('[') else {
        return vec![key.to_string()];
    };
    let mut segments = vec![key[..open].to_string()];
    let mut rest = &key[open..];
    while let Some(inner) = rest.strip_prefix('[') {
        let Some(close) = inner.find(']') else { break };
        if segments.len() > MAX_DEPTH {
            break;
        }
        segments.push(inner[..close].to_string());
        rest = &inner[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(rest.to_string());
    }
    segments
}

fn insert(target: &mut Value, segments: &[String], value: Value) {
    let Some((segment, deeper)) = segments.split_first() else {
        // The same key twice: both values, in a list.
        *target = match std::mem::take(target) {
            Value::Null => value,
            Value::Array(mut items) => {
                items.push(value);
                Value::Array(items)
            }
            existing => Value::Array(vec![existing, value]),
        };
        return;
    };

    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(entries) = target else {
        return;
    };
    // `[]` appends: it takes the next free index.
    let key = if segment.is_empty() {
        entries.len().to_string()
    } else {
        segment.clone()
    };
    insert(entries.entry(key).or_insert(Value::Null), deeper, value);
}

/// Objects whose keys are all small indexes are lists, in index order.
fn finish(value: Value) -> Value {
    match value {
        Value::Object(entries) => {
            let entries: Map<String, Value> = entries
                .into_iter()
                .map(|(key, value)| (key, finish(value)))
                .collect();
            let indexes: Option<Vec<usize>> = entries
                .keys()
                .map(|key| {
                    key.parse::<usize>()
                        .ok()
                        .filter(|index| *index <= MAX_ARRAY_INDEX && index.to_string() == *key)
                })
                .collect();
            match indexes {
                Some(indexes) if !indexes.is_empty() => {
                    let mut items: Vec<(usize, Value)> = indexes
                        .into_iter()
                        .zip(entries.into_iter().map(|(_, value)| value))
                        .collect();
                    items.sort_by_key(|(index, _)| *index);
                    Value::Array(items.into_iter().map(|(_, value)| value).collect())
                }
                _ => Value::Object(entries),
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(finish).collect()),
        other => other,
    }
}

/// Parse a query string, with `a[b]=c`, `a[]=1` and `a[0]=1` nesting.
pub fn parse_query(query: &str) -> Map<String, Value> {
    parse_pairs(query, false, MAX_PARAMETERS)
}

/// Parse a form body: like a query string, and `a.b=c` nests too.
pub fn parse_form(body: &str) -> Map<String, Value> {
    parse_pairs(body, true, MAX_FORM_PARAMETERS)
}

fn parse_pairs(query: &str, allow_dots: bool, limit: usize) -> Map<String, Value> {
    let mut root = Value::Object(Map::new());
    for pair in query.split('&').filter(|pair| !pair.is_empty()).take(limit) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let mut key = decode_component(key);
        if key.is_empty() {
            continue;
        }
        if allow_dots {
            key = dots_to_brackets(&key);
        }
        insert(
            &mut root,
            &key_segments(&key),
            Value::String(decode_component(value)),
        );
    }
    match root {
        Value::Object(entries) => entries
            .into_iter()
            .map(|(key, value)| (key, finish(value)))
            .collect(),
        _ => Map::new(),
    }
}

/// Trim every string of a body and turn the empty ones into `null`, at any depth.
pub fn normalize_body(value: &mut Value) {
    match value {
        Value::String(text) => {
            // What JavaScript trims, as the Node app did: the two differ on
            // U+FEFF and U+0085, and a value both servers read, such as the
            // password of a backup, must come out the same on each.
            let trimmed = mymcps_vine::js::trim(text);
            if trimmed.is_empty() {
                *value = Value::Null;
            } else if trimmed.len() != text.len() {
                *text = trimmed.to_string();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_body),
        Value::Object(entries) => entries.values_mut().for_each(normalize_body),
        _ => {}
    }
}

/// The media type a request declares for its body, without its parameters.
pub fn media_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

const JSON_TYPES: &[&str] = &[
    "application/json",
    "application/json-patch+json",
    "application/vnd.api+json",
    "application/csp-report",
];

/// The body as the handlers read it. A JSON body must be an object or an
/// array (anything else is refused with 422, invalid JSON with 400), and an
/// array is read as an object keyed by index.
pub fn parse_body(media_type: &str, bytes: &[u8]) -> Result<Map<String, Value>, StatusCode> {
    let mut parsed = if media_type == "application/x-www-form-urlencoded" {
        Value::Object(parse_form(&String::from_utf8_lossy(bytes)))
    } else if JSON_TYPES.contains(&media_type) {
        match bytes
            .iter()
            .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            None => Value::Object(Map::new()),
            Some(b'{' | b'[') => {
                serde_json::from_slice(bytes).map_err(|_| StatusCode::BAD_REQUEST)?
            }
            Some(_) => return Err(StatusCode::UNPROCESSABLE_ENTITY),
        }
    } else {
        Value::Object(Map::new())
    };
    normalize_body(&mut parsed);
    Ok(match parsed {
        Value::Object(entries) => entries,
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item))
            .collect(),
        _ => Map::new(),
    })
}

/// The time a client has to send a body of at most [`MAX_BODY_BYTES`].
/// Node's server gave a whole request five minutes (`requestTimeout`): one
/// that trickles in more slowly holds a connection for nothing.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Why the body of a request was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRefusal {
    /// Larger than [`MAX_BODY_BYTES`].
    TooLarge,
    /// Not there in time.
    TooSlow,
    /// The connection broke before its end.
    Broken,
}

impl IntoResponse for BodyRefusal {
    fn into_response(self) -> Response {
        match self {
            Self::TooLarge => {
                (StatusCode::PAYLOAD_TOO_LARGE, "request entity too large").into_response()
            }
            Self::TooSlow => StatusCode::REQUEST_TIMEOUT.into_response(),
            Self::Broken => StatusCode::BAD_REQUEST.into_response(),
        }
    }
}

/// The whole body of a request, unless it is too large, too slow or broken
/// off. A [`BodyRefusal`] is the answer to give then.
pub async fn read_body(body: Body) -> Result<Bytes, BodyRefusal> {
    read_body_within(body, BODY_READ_TIMEOUT).await
}

async fn read_body_within(body: Body, patience: Duration) -> Result<Bytes, BodyRefusal> {
    let read = Limited::new(body, MAX_BODY_BYTES).collect();
    match tokio::time::timeout(patience, read).await {
        Ok(Ok(collected)) => Ok(collected.to_bytes()),
        Ok(Err(error)) if error.downcast_ref::<LengthLimitError>().is_some() => {
            Err(BodyRefusal::TooLarge)
        }
        Ok(Err(_)) => Err(BodyRefusal::Broken),
        Err(_) => Err(BodyRefusal::TooSlow),
    }
}

/// Parses the body of state-changing requests, once, for the handlers and
/// for [`csrf_layer`].
pub async fn body_layer(request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    let changes_state =
        [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(&parts.method);
    if !changes_state {
        parts.extensions.insert(ParsedBody::default());
        return next.run(Request::from_parts(parts, body)).await;
    }

    let bytes = match read_body(body).await {
        Ok(bytes) => bytes,
        Err(refusal) => return refusal.into_response(),
    };
    match parse_body(&media_type(&parts.headers), &bytes) {
        Ok(parsed) => {
            parts.extensions.insert(ParsedBody(parsed));
            next.run(Request::from_parts(parts, Body::empty())).await
        }
        Err(status) => (status, "The request body is not valid JSON").into_response(),
    }
}

/// Refuses a state-changing request of the pages without a valid CSRF token.
/// Runs inside the session layer and [`body_layer`].
pub async fn csrf_layer(request: Request, next: Next) -> Response {
    let changes_state =
        [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(request.method());
    if !changes_state {
        return next.run(request).await;
    }

    let session = request
        .extensions()
        .get::<Session>()
        .cloned()
        .unwrap_or_default();
    let token = request
        .extensions()
        .get::<ParsedBody>()
        .and_then(|body| body.0.get(CSRF_FIELD))
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .headers()
                .get(CSRF_HEADER)
                .and_then(|value| value.to_str().ok())
        });
    if verify_csrf_token(&session, token) {
        return next.run(request).await;
    }

    if !wants_html(request.headers()) {
        return (StatusCode::FORBIDDEN, CSRF_MESSAGE).into_response();
    }
    session.flash("error", CSRF_MESSAGE);
    redirect_back(request.headers(), "/")
}

impl<S: Send + Sync> FromRequestParts<S> for ParsedBody {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<ParsedBody>()
            .cloned()
            .unwrap_or_default())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Input {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let mut input = parts
            .extensions
            .get::<ParsedBody>()
            .map(|body| body.0.clone())
            .unwrap_or_default();
        // The query string is left as sent: validators decide what an empty value means.
        for (key, value) in parse_query(parts.uri.query().unwrap_or("")) {
            input.insert(key, value);
        }
        input.shift_remove(CSRF_FIELD);
        input.shift_remove("_method");
        Ok(Self(input))
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::json;

    use super::*;

    fn parsed(query: &str) -> Value {
        Value::Object(parse_query(query))
    }

    #[test]
    fn reads_more_fields_from_a_form_than_from_a_query_string() {
        let many = |count: usize| {
            (0..count)
                .map(|index| format!("f{index}=1"))
                .collect::<Vec<_>>()
                .join("&")
        };
        // The tools page of an MCP with 2,000 tools: two fields a tool.
        assert_eq!(parse_form(&many(4_002)).len(), 4_002);
        assert_eq!(parse_form(&many(6_000)).len(), MAX_FORM_PARAMETERS);
        assert_eq!(parse_query(&many(1_500)).len(), MAX_PARAMETERS);
    }

    #[tokio::test]
    async fn reads_a_body_unless_it_is_too_large_or_too_slow() {
        let whole = read_body(Body::from("a=1")).await.unwrap();
        assert_eq!(&whole[..], b"a=1");

        let large = read_body(Body::from(vec![b'a'; MAX_BODY_BYTES + 1])).await;
        assert_eq!(large, Err(BodyRefusal::TooLarge));
        assert_eq!(
            BodyRefusal::TooLarge.into_response().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        // A body that never ends: a first piece, then nothing.
        let stalled = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from("a=")) })
            .chain(futures::stream::pending());
        let slow = read_body_within(Body::from_stream(stalled), Duration::from_millis(50)).await;
        assert_eq!(slow, Err(BodyRefusal::TooSlow));
        assert_eq!(
            BodyRefusal::TooSlow.into_response().status(),
            StatusCode::REQUEST_TIMEOUT
        );

        let broken = futures::stream::once(async {
            Err::<Bytes, _>(std::io::Error::other("connection reset"))
        });
        let cut = read_body(Body::from_stream(broken)).await;
        assert_eq!(cut, Err(BodyRefusal::Broken));
        assert_eq!(
            BodyRefusal::Broken.into_response().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn parses_flat_and_repeated_keys() {
        assert_eq!(
            parsed("name=My+MCP&url=https%3A%2F%2Fx.test%2Fmcp%3Fa%3D1&empty="),
            json!({"name": "My MCP", "url": "https://x.test/mcp?a=1", "empty": ""})
        );
        assert_eq!(parsed("id=1&id=2&id=3"), json!({"id": ["1", "2", "3"]}));
        assert_eq!(parsed("flag&=ignored&&x=1"), json!({"flag": "", "x": "1"}));
    }

    #[test]
    fn parses_nested_keys_like_qs() {
        assert_eq!(
            parsed("mcpIds[]=4&mcpIds[]=9"),
            json!({"mcpIds": ["4", "9"]})
        );
        assert_eq!(
            parsed("a[1]=second&a[0]=first"),
            json!({"a": ["first", "second"]})
        );
        assert_eq!(
            parsed(
                "npmEnv[0][name]=API_KEY&npmEnv[0][value]=secret&npmEnv[1][name]=REGION&npmEnv[1][value]="
            ),
            json!({"npmEnv": [{"name": "API_KEY", "value": "secret"}, {"name": "REGION", "value": ""}]})
        );
        assert_eq!(
            parsed("tools[search][mode]=ask"),
            json!({"tools": {"search": {"mode": "ask"}}})
        );
        assert_eq!(parsed("a[99]=x"), json!({"a": {"99": "x"}}));
        assert_eq!(
            parsed("a[b][c][d][e][f][g]=deep"),
            json!({"a": {"b": {"c": {"d": {"e": {"f": {"[g]": "deep"}}}}}}})
        );
    }

    #[test]
    fn trims_strings_and_turns_empty_ones_into_null_at_any_depth() {
        let mut value =
            json!({"a": "", "b": ["", " x ", {"c": "  "}], "d": "\tkept inside \n", "n": 1});
        normalize_body(&mut value);
        assert_eq!(
            value,
            json!({"a": null, "b": [null, "x", {"c": null}], "d": "kept inside", "n": 1})
        );

        // The spaces of JavaScript: a byte order mark is one, a next-line
        // character is not.
        let mut value =
            json!({"mark": "\u{FEFF}x\u{FEFF}", "next": "\u{0085}x\u{0085}", "only": "\u{FEFF}"});
        normalize_body(&mut value);
        assert_eq!(
            value,
            json!({"mark": "x", "next": "\u{0085}x\u{0085}", "only": null})
        );
    }

    #[test]
    fn form_bodies_also_nest_with_dots() {
        let form = |body: &str| Value::Object(parse_form(body));
        assert_eq!(
            form("user.name=Ada&user.roles[]=admin"),
            json!({"user": {"name": "Ada", "roles": ["admin"]}})
        );
        assert_eq!(
            form("a.b.c=1&a.b.d=2"),
            json!({"a": {"b": {"c": "1", "d": "2"}}})
        );
        assert_eq!(form("a[b].c=1"), json!({"a": {"b": {"c": "1"}}}));
        // A query string does not.
        assert_eq!(parsed("user.name=Ada"), json!({"user.name": "Ada"}));
    }

    #[test]
    fn parses_bodies_by_content_type() {
        let object = |value: Value| value.as_object().cloned().unwrap();
        assert_eq!(
            parse_body(
                "application/x-www-form-urlencoded",
                b"email=+a%40b.c+&fullName="
            )
            .unwrap(),
            object(json!({"email": "a@b.c", "fullName": null}))
        );
        assert_eq!(
            parse_body(
                "application/json",
                br#"{"name":"","ids":[1,2],"note":" x "}"#
            )
            .unwrap(),
            object(json!({"name": null, "ids": [1, 2], "note": "x"}))
        );
        assert!(parse_body("application/json", b"  ").unwrap().is_empty());
        assert_eq!(
            parse_body("application/json", b" [1,\"a\"]").unwrap(),
            object(json!({"0": 1, "1": "a"}))
        );
        assert_eq!(
            parse_body("application/json", b"{not json"),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            parse_body("application/json", b"\"text\""),
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
        assert_eq!(
            parse_body("application/json", b"42"),
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
        // A file or a multipart form is never read as input.
        assert!(
            parse_body("multipart/form-data", b"--x\r\n")
                .unwrap()
                .is_empty()
        );
        assert!(parse_body("text/plain", b"a=1").unwrap().is_empty());
    }
}
