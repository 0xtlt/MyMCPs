//! The Strava API v3, called as the connected athlete.

use std::time::Duration;

use http::Method;
use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinToolContext};
use mymcps_net::{FetchError, FetchRequest, FetchResponse, UpstreamResponseLimits};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::validators::STRAVA_FAILURE_VALIDATOR;

const STRAVA_API_URL: &str = "https://www.strava.com/api/v3";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Most of Strava's own explanation that is passed on, in UTF-16 code units.
const MAX_SUMMARY_LENGTH: usize = 200;

/// The value of a parameter, written as JavaScript's `String(value)` writes
/// it. `None` is a parameter left out.
pub trait ParamValue {
    fn written(self) -> Option<String>;
}

impl ParamValue for &str {
    fn written(self) -> Option<String> {
        Some(self.to_owned())
    }
}

impl ParamValue for String {
    fn written(self) -> Option<String> {
        Some(self)
    }
}

impl ParamValue for bool {
    fn written(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl ParamValue for i64 {
    fn written(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl ParamValue for u64 {
    fn written(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl ParamValue for f64 {
    fn written(self) -> Option<String> {
        Some(vine::js::number_to_string(self))
    }
}

impl<T: ParamValue> ParamValue for Option<T> {
    fn written(self) -> Option<String> {
        self.and_then(ParamValue::written)
    }
}

/// The parameters of a query string or of a form, in the order they are sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(Vec<(&'static str, String)>);

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a parameter, unless its value was left out.
    #[must_use]
    pub fn with(mut self, name: &'static str, value: impl ParamValue) -> Self {
        if let Some(value) = value.written() {
            self.0.push((name, value));
        }
        self
    }

    /// Add the parameters of `more` after these.
    #[must_use]
    pub fn and(mut self, more: Params) -> Self {
        self.0.extend(more.0);
        self
    }

    fn encoded(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&self.0)
            .finish()
    }
}

/// What to ask of Strava at a path. A request that sets nothing is a `GET`.
#[derive(Debug, Default)]
pub struct StravaRequest {
    pub method: Method,
    pub query: Params,
    /// Sent as `application/x-www-form-urlencoded`.
    pub form: Option<Params>,
    /// Sent as JSON.
    pub json: Option<Map<String, Value>>,
}

/// One of the reasons Strava gives for refusing a request.
#[derive(Debug, Default, Deserialize)]
struct StravaFault {
    resource: Option<String>,
    field: Option<String>,
    code: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StravaFailure {
    message: Option<String>,
    #[serde(default)]
    errors: Vec<StravaFault>,
}

/// What Strava says went wrong. Nothing, when the body is not the one it documents.
async fn failure_of(response: &mut FetchResponse) -> StravaFailure {
    let body: Value = response.json().await.unwrap_or(Value::Null);
    STRAVA_FAILURE_VALIDATOR
        .validate_as(&body)
        .unwrap_or_default()
}

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// `text.slice(0, max)`, which counts UTF-16 code units. A character the cut
/// would split is left out.
fn truncated(text: &str, max: usize) -> &str {
    let mut length = 0;
    for (start, character) in text.char_indices() {
        length += character.len_utf16();
        if length > max {
            return &text[..start];
        }
    }
    text
}

/// Strava's own explanation, such as `Record Not Found (Activity not found)`.
fn fault_summary(failure: &StravaFailure) -> String {
    let details = failure
        .errors
        .iter()
        .map(|fault| {
            let parts = [&fault.resource, &fault.field, &fault.code];
            let parts: Vec<&str> = parts.into_iter().filter_map(present).collect();
            parts.join(" ")
        })
        .filter(|fault| !fault.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let details = (!details.is_empty()).then(|| format!("({details})"));
    let summary = [present(&failure.message), details.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if summary.is_empty() {
        String::new()
    } else {
        format!(": {}", truncated(&summary, MAX_SUMMARY_LENGTH))
    }
}

/// A missing scope is reported as HTTP 401 with a fault such as
/// `{"resource":"AccessToken","field":"activity:read_permission","code":"missing"}`.
fn missing_permission(failure: &StravaFailure) -> Option<&str> {
    let named = failure.errors.iter().find_map(|fault| {
        let permission = fault.field.as_deref()?.strip_suffix("_permission")?;
        (fault.code.as_deref() == Some("missing")).then_some(permission)
    });
    named.filter(|permission| !permission.is_empty())
}

/// Reads have their own, lower limit. Writes only count against the overall one.
fn rate_limit_message(response: &FetchResponse, is_read: bool) -> String {
    let header = |name: &str| {
        let read = if is_read {
            response.header(&format!("x-readratelimit-{name}"))
        } else {
            None
        };
        read.or_else(|| response.header(&format!("x-ratelimit-{name}")))
    };
    // The first two values a header lists: the 15 minutes, then the day.
    let pair = |header: Option<String>| -> Option<(String, String)> {
        let header = header?;
        let mut values = header.split(',');
        let short = values.next().filter(|value| !value.is_empty())?;
        let daily = values.next().filter(|value| !value.is_empty())?;
        Some((
            vine::js::trim(short).to_owned(),
            vine::js::trim(daily).to_owned(),
        ))
    };
    let counters = match (pair(header("usage")), pair(header("limit"))) {
        (Some((short_usage, daily_usage)), Some((short_limit, daily_limit))) => format!(
            " ({short_usage} of {short_limit} requests in 15 minutes, {daily_usage} of {daily_limit} today)"
        ),
        _ => String::new(),
    };
    format!(
        "Strava rate limit reached{counters}. The 15-minute window resets on the quarter hour and the daily window at midnight UTC."
    )
}

async fn strava_failure(response: &mut FetchResponse, is_read: bool) -> BuiltinError {
    let failure = failure_of(response).await;

    match response.status().as_u16() {
        401 => match missing_permission(&failure) {
            Some(permission) => BuiltinError::tool(format!(
                "Strava permission \"{permission}\" was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked."
            )),
            None => BuiltinError::authorization(
                "Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs.",
            ),
        },
        402 => BuiltinError::tool("Strava only returns this data to athletes with a subscription."),
        403 => BuiltinError::tool(format!(
            "Strava denied access to this resource{}",
            fault_summary(&failure)
        )),
        404 => BuiltinError::tool(format!(
            "Strava could not find this resource{}",
            fault_summary(&failure)
        )),
        429 => BuiltinError::tool(rate_limit_message(response, is_read)),
        status => BuiltinError::tool(format!(
            "Strava API returned HTTP {status}{}",
            fault_summary(&failure)
        )),
    }
}

/// The numbers of a response as JavaScript writes them back: `178.0` is
/// `178`. Agents pay for every token. Whole numbers are kept as Strava sent
/// them, also the ones too large for JavaScript to hold.
fn with_javascript_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(float) if number.is_f64() => vine::js::number(float),
            _ => Value::Number(number),
        },
        Value::Array(items) => {
            Value::Array(items.into_iter().map(with_javascript_numbers).collect())
        }
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, entry)| (key, with_javascript_numbers(entry)))
                .collect(),
        ),
        other => other,
    }
}

fn no_answer_in_time(is_read: bool) -> BuiltinError {
    // A write may have been applied even though its response never arrived.
    BuiltinError::tool(if is_read {
        "Strava did not respond in time. Try again."
    } else {
        "Strava did not respond in time. The change may still have been applied, so check before retrying."
    })
}

/// Call the Strava API v3 as the connected athlete.
pub async fn strava_request(
    context: &BuiltinToolContext,
    path: &str,
    request: StravaRequest,
) -> BuiltinResult<Value> {
    let StravaRequest {
        method,
        query,
        form,
        json,
    } = request;
    let is_read = method == Method::GET;

    let mut url = Url::parse(&format!("{STRAVA_API_URL}{path}")).map_err(BuiltinError::internal)?;
    let query = query.encoded();
    url.set_query((!query.is_empty()).then_some(query.as_str()));

    let mut fetch = FetchRequest::new(method, url)
        .header("Accept", "application/json")?
        .header("Authorization", &format!("Bearer {}", context.access_token))?
        .timeout(REQUEST_TIMEOUT);
    if let Some(form) = form {
        fetch = fetch
            .header("Content-Type", "application/x-www-form-urlencoded")?
            .body(form.encoded());
    } else if let Some(json) = json {
        fetch = fetch
            .header("Content-Type", "application/json")?
            .body(serde_json::to_string(&json)?);
    }

    let sent = context
        .env
        .fetcher
        .fetch_with_same_origin_redirects(fetch, "Strava API", UpstreamResponseLimits::default())
        .await;
    let mut response = match sent {
        Err(FetchError::Timeout) => return Err(no_answer_in_time(is_read)),
        sent => sent?,
    };

    if !response.ok() {
        return Err(strava_failure(&mut response, is_read).await);
    }
    Ok(with_javascript_numbers(response.json().await?))
}

pub async fn strava_get(
    context: &BuiltinToolContext,
    path: &str,
    query: Params,
) -> BuiltinResult<Value> {
    let request = StravaRequest {
        query,
        ..StravaRequest::default()
    };
    strava_request(context, path, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_parameters_as_javascript_writes_values() {
        let params = Params::new()
            .with("name", "Été à vélo & co")
            .with("left_out", None::<u64>)
            .with("page", 2_u64)
            .with("after", Some(-1_i64))
            .with("weight", 68.4)
            .with("whole", 70.0)
            .with("small", 1e-7)
            .with("large", 1e21)
            .with("starred", false)
            .and(Params::new().with("per_page", Some(30_u64)));

        assert_eq!(
            params.encoded(),
            "name=%C3%89t%C3%A9+%C3%A0+v%C3%A9lo+%26+co&page=2&after=-1&weight=68.4&whole=70\
             &small=1e-7&large=1e%2B21&starred=false&per_page=30"
        );
        assert_eq!(Params::new().encoded(), "");
    }

    #[test]
    fn cuts_an_explanation_where_javascript_does() {
        assert_eq!(truncated("abcdef", 3), "abc");
        assert_eq!(truncated("abc", 3), "abc");
        assert_eq!(truncated("", 3), "");
        assert_eq!(truncated("é€x", 2), "é€");
        // Two code units each: the one the cut would split is left out.
        assert_eq!(truncated("🚴🚴🚴", 4), "🚴🚴");
        assert_eq!(truncated("🚴🚴🚴", 3), "🚴");
        assert_eq!(truncated("x🚴", 2), "x");
    }

    #[test]
    fn warns_that_a_write_without_an_answer_may_have_been_applied() {
        let read = no_answer_in_time(true);
        let write = no_answer_in_time(false);

        assert!(matches!(read, BuiltinError::Tool(_)), "{read:?}");
        assert_eq!(
            read.to_string(),
            "Strava did not respond in time. Try again."
        );
        assert!(matches!(write, BuiltinError::Tool(_)), "{write:?}");
        assert_eq!(
            write.to_string(),
            "Strava did not respond in time. The change may still have been applied, so check before retrying."
        );
    }
}
