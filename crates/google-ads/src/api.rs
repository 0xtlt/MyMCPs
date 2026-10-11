//! The Google Ads API, called as the connected Google sign-in: requests,
//! what a refusal says, reports, and changes.

use std::time::Duration;

use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinToolContext};
use mymcps_net::{FetchError, FetchRequest, FetchResponse, UpstreamResponseLimits};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::format::{customer_label, customer_number};
use crate::js;
use crate::validators::{
    GOOGLE_ADS_FAILURE_VALIDATOR, GOOGLE_ADS_MUTATION_VALIDATOR, GOOGLE_ADS_ROWS_VALIDATOR,
};

/// Google retires a version about a year after its release, and announces it
/// months ahead: <https://developers.google.com/google-ads/api/docs/sunset-dates>
/// A retired version answers every request with an HTML 404. Every field and
/// enum of a version is in its discovery document, which is what to check the
/// tools against when moving to another one:
/// <https://googleads.googleapis.com/$discovery/rest?version=v25>
pub const GOOGLE_ADS_API_VERSION: &str = "v25";
const GOOGLE_ADS_API_ORIGIN: &str = "https://googleads.googleapis.com";

/// A report over a large account takes Google a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REPORTED_FAULTS: usize = 5;
const MAX_FAILURE_CHARS: usize = 900;

/// What to do about the errors that the setup, not the call, has to fix.
fn setup_hint(code: &str) -> Option<&'static str> {
    Some(match code {
        "CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION" => {
            "The Google Cloud project of the OAuth client only has test access, which reaches test accounts only. Apply for Explorer access on the Google Ads API Overview page of that project."
        }
        "PROJECT_DISABLED" => {
            "The Google Ads API is not enabled in the Google Cloud project of the OAuth client. Enable it in the Google Cloud console."
        }
        "USER_PERMISSION_DENIED" => {
            "The connected Google sign-in cannot open this account directly. If it is managed through a manager account, an administrator must set that manager as Manager account ID from the MCPs page in MyMCPs."
        }
        "CUSTOMER_NOT_ENABLED" => {
            "This Google Ads account is not active: it was closed, suspended, or never finished its setup."
        }
        "NOT_ADS_USER" => "The connected Google sign-in has no Google Ads account.",
        "TWO_STEP_VERIFICATION_NOT_ENROLLED" => {
            "Google requires 2-Step Verification on the connected Google account before it can use the Google Ads API."
        }
        "INVALID_LOGIN_CUSTOMER_ID" => {
            "The Manager account ID saved for this MCP is not one the connected Google sign-in can use. An administrator must correct it from the MCPs page in MyMCPs."
        }
        "CANNOT_BE_EXECUTED_BY_MANAGER_ACCOUNT" => {
            "This is a manager account, which holds no campaigns. Use the ID of one of its client accounts, as list_accounts returns them."
        }
        "EU_POLITICAL_ADVERTISING_DECLARATION_REQUIRED" => {
            "A campaign of this account has not declared whether it carries EU political advertising, and Google blocks every change until it has. Declare it in Google Ads."
        }
        "RESOURCE_EXHAUSTED" => {
            "The Google Cloud project has used up its Google Ads API operations for today. Try again later, or apply for a higher access level."
        }
        "RESOURCE_TEMPORARILY_EXHAUSTED" => {
            "Google Ads is rate limiting these requests. Try again in a minute."
        }
        _ => return None,
    })
}

/// The errors that say the sign-in itself is no longer good, whatever the HTTP status.
const REJECTED_SIGN_IN: [&str; 5] = [
    "OAUTH_TOKEN_EXPIRED",
    "OAUTH_TOKEN_INVALID",
    "OAUTH_TOKEN_REVOKED",
    "OAUTH_TOKEN_DISABLED",
    "GOOGLE_ACCOUNT_COOKIE_INVALID",
];

/// The `error` of a refusal, as `GOOGLE_ADS_FAILURE_VALIDATOR` leaves it.
#[derive(Debug, Default, Deserialize)]
struct GoogleAdsFailure {
    message: Option<String>,
    #[serde(default)]
    details: Vec<FailureDetail>,
}

#[derive(Debug, Deserialize)]
struct FailureBody {
    error: GoogleAdsFailure,
}

#[derive(Debug, Deserialize)]
struct FailureDetail {
    #[serde(default)]
    errors: Vec<Fault>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fault {
    #[serde(default)]
    error_code: Map<String, Value>,
    message: Option<String>,
    location: Option<FaultLocation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaultLocation {
    #[serde(default)]
    field_path_elements: Vec<FieldPathElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FieldPathElement {
    field_name: Option<String>,
    index: Option<f64>,
}

impl Fault {
    /// `campaign.name`, `operations[0].create.keyword.text`: where in the request the mistake is.
    fn location(&self) -> String {
        let elements = self
            .location
            .as_ref()
            .map(|location| location.field_path_elements.as_slice());
        elements
            .unwrap_or_default()
            .iter()
            .map(|FieldPathElement { field_name, index }| {
                let index = index.map(|index| format!("[{}]", vine::js::number_to_string(index)));
                format!(
                    "{}{}",
                    field_name.as_deref().unwrap_or_default(),
                    index.unwrap_or_default()
                )
            })
            .filter(|element| !element.is_empty())
            .collect::<Vec<_>>()
            .join(".")
    }

    fn code(&self) -> Option<&str> {
        self.error_code.values().next().and_then(Value::as_str)
    }

    fn describe(&self) -> String {
        let code = self.code();
        let message = self.message.as_deref();
        let location = self.location();
        let both = code.is_some_and(|code| !code.is_empty())
            && message.is_some_and(|message| !message.is_empty());
        [
            Some(message.or(code).unwrap_or("Unknown error").to_owned()),
            code.filter(|_| both).map(|code| format!("({code})")),
            (!location.is_empty()).then(|| format!("at {location}")),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

async fn failure_of(response: &mut FetchResponse) -> GoogleAdsFailure {
    // A body that is not JSON, or that was cut at its limit, explains nothing.
    let body: Option<Value> = response.json().await.ok();
    GOOGLE_ADS_FAILURE_VALIDATOR
        .validate_as::<FailureBody>(body.as_ref())
        .map(|failure| failure.error)
        .unwrap_or_default()
}

async fn google_ads_failure(response: &mut FetchResponse) -> BuiltinError {
    let failure = failure_of(response).await;
    let faults: Vec<&Fault> = failure
        .details
        .iter()
        .flat_map(|detail| &detail.errors)
        .collect();
    let codes = || faults.iter().filter_map(|fault| fault.code());

    // A token Google no longer knows gets a bare 401, without any of its own errors.
    if codes().any(|code| REJECTED_SIGN_IN.contains(&code))
        || (response.status().as_u16() == 401 && faults.is_empty())
    {
        return BuiltinError::authorization(
            "Google rejected the saved authorization. Re-authorize this MCP in MyMCPs.",
        );
    }

    let reasons = if faults.is_empty() {
        failure
            .message
            .clone()
            .unwrap_or_else(|| format!("HTTP {}", response.status().as_u16()))
    } else {
        faults
            .iter()
            .take(MAX_REPORTED_FAULTS)
            .map(|fault| fault.describe())
            .collect::<Vec<_>>()
            .join("; ")
    };
    let more = match faults.len().checked_sub(MAX_REPORTED_FAULTS) {
        Some(more) if more > 0 => format!(" (and {more} more)"),
        _ => String::new(),
    };
    let hint = codes()
        .find_map(setup_hint)
        .map(|hint| format!(" {hint}"))
        .unwrap_or_default();

    BuiltinError::tool(format!(
        "Google Ads refused the request: {}{hint}",
        js::slice_start(&format!("{reasons}{more}"), MAX_FAILURE_CHARS)
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Method {
    Get,
    #[default]
    Post,
}

/// How to call the Google Ads API. Left as it is, a `POST` without a body.
#[derive(Debug, Clone, Default)]
pub struct GoogleAdsRequest {
    pub method: Method,
    /// Sent as JSON.
    pub body: Option<Value>,
    /// Whether a timeout may have left a change behind.
    pub changes: bool,
}

impl GoogleAdsRequest {
    pub fn get() -> Self {
        Self {
            method: Method::Get,
            ..Self::default()
        }
    }

    /// A `POST` of this JSON.
    pub fn post(body: Value) -> Self {
        Self {
            body: Some(body),
            ..Self::default()
        }
    }

    /// Say that the request changes the account, so that a timeout is told for what it is.
    #[must_use]
    pub fn changing(mut self, changes: bool) -> Self {
        self.changes = changes;
        self
    }
}

/// Call the Google Ads API as the connected Google sign-in.
pub async fn google_ads_request(
    context: &BuiltinToolContext,
    path: &str,
    request: GoogleAdsRequest,
) -> BuiltinResult<Value> {
    let GoogleAdsRequest {
        method,
        body,
        changes,
    } = request;
    let url = Url::parse(&format!(
        "{GOOGLE_ADS_API_ORIGIN}/{GOOGLE_ADS_API_VERSION}{path}"
    ))
    .map_err(BuiltinError::internal)?;

    // No developer token: Google retired them in September 2026, and the access
    // level now belongs to the Google Cloud project of the OAuth client.
    let mut fetch = match method {
        Method::Get => FetchRequest::get(url),
        Method::Post => FetchRequest::post(url),
    }
    .header("Accept", "application/json")?
    .header("Authorization", &format!("Bearer {}", context.access_token))?;
    // Names the manager account the sign-in acts through to reach its client accounts.
    if let Some(manager) = context
        .settings
        .get("loginCustomerId")
        .filter(|manager| !manager.is_empty())
    {
        fetch = fetch.header("login-customer-id", manager)?;
    }
    if let Some(body) = &body {
        fetch = fetch
            .header("Content-Type", "application/json")?
            .body(serde_json::to_string(body)?);
    }

    let sent = context.env.fetcher.fetch_with_same_origin_redirects(
        fetch.timeout(REQUEST_TIMEOUT),
        "Google Ads API",
        UpstreamResponseLimits::default(),
    );
    let mut response = match sent.await {
        Err(FetchError::Timeout) => {
            return Err(BuiltinError::tool(if changes {
                "Google Ads did not respond in time. The change may still have been applied, so check before retrying."
            } else {
                "Google Ads did not respond in time. Try again, with a shorter period or a lower limit."
            }));
        }
        response => response?,
    };

    if !response.ok() {
        return Err(google_ads_failure(&mut response).await);
    }
    Ok(response.json().await?)
}

/// The accounts the MCP may act on: every one the sign-in reaches, unless the
/// administrator listed some. `None` when all are allowed.
pub fn allowed_customers(context: &BuiltinToolContext) -> Option<Vec<&str>> {
    let listed = context
        .settings
        .get("customerIds")
        .filter(|listed| !listed.is_empty())?;
    Some(listed.split(' ').collect())
}

/// The account a tool was asked to act on, once it is known to be one the MCP may use.
pub fn customer_of(context: &BuiltinToolContext, customer_id: &str) -> BuiltinResult<String> {
    let customer = customer_number(customer_id);
    if let Some(allowed) = allowed_customers(context)
        && !allowed.contains(&customer.as_str())
    {
        let allowed: Vec<String> = allowed.into_iter().map(customer_label).collect();
        return Err(BuiltinError::tool(format!(
            "This MCP may not use the Google Ads account {}. It is limited to: {}. An administrator can change that from the MCPs page in MyMCPs.",
            customer_label(&customer),
            allowed.join(", "),
        )));
    }
    Ok(customer)
}

/// A row of a report: one object for each resource the query selects from.
pub type GoogleAdsRow = Map<String, Value>;

/// What a report answered.
#[derive(Debug, Clone, PartialEq)]
pub struct GoogleAdsRows {
    pub rows: Vec<GoogleAdsRow>,
    /// Whether Google had more rows than were asked for.
    pub truncated: bool,
}

/// One page of a report, as `GOOGLE_ADS_ROWS_VALIDATOR` leaves it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowsPage {
    #[serde(default)]
    results: Vec<GoogleAdsRow>,
    next_page_token: Option<String>,
}

/// Run a Google Ads Query Language query and return at most `limit` rows.
/// `truncated` says whether Google had more, which a query that limits itself
/// only shows when it asks for one row more than `limit`.
pub async fn search_google_ads(
    context: &BuiltinToolContext,
    customer: &str,
    query: &str,
    limit: usize,
) -> BuiltinResult<GoogleAdsRows> {
    let mut rows: Vec<GoogleAdsRow> = Vec::new();
    let mut page_token: Option<String> = None;

    let truncated = loop {
        let mut body = Map::new();
        body.insert("query".to_owned(), Value::from(query));
        if let Some(page_token) = &page_token {
            body.insert("pageToken".to_owned(), Value::from(page_token.as_str()));
        }
        let answer = google_ads_request(
            context,
            &format!("/customers/{customer}/googleAds:search"),
            GoogleAdsRequest::post(Value::Object(body)),
        )
        .await?;
        let page = GOOGLE_ADS_ROWS_VALIDATOR
            .validate(&answer)
            .map_err(|_| BuiltinError::tool("Google Ads did not return the rows of a report"))?;
        let page: RowsPage = serde_json::from_value(page)?;

        rows.extend(page.results);
        page_token = page.next_page_token.filter(|token| !token.is_empty());
        if page_token.is_none() || rows.len() >= limit {
            break page_token.is_some() || rows.len() > limit;
        }
    };

    rows.truncate(limit);
    Ok(GoogleAdsRows { rows, truncated })
}

/// One change to one resource, such as `{ campaignOperation: { update, updateMask } }`.
pub type GoogleAdsOperation = Value;

/// What a mutate answered, as `GOOGLE_ADS_MUTATION_VALIDATOR` leaves it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutationAnswer {
    #[serde(default)]
    mutate_operation_responses: Vec<Map<String, Value>>,
}

/// Apply changes to an account together: either all of them are made, or none.
/// With `validate_only`, Google checks them and makes none. Returns the
/// resource name each change produced, in the order of the operations.
pub async fn mutate_google_ads(
    context: &BuiltinToolContext,
    customer: &str,
    operations: &[GoogleAdsOperation],
    validate_only: bool,
) -> BuiltinResult<Vec<Option<String>>> {
    let mut body = Map::new();
    body.insert(
        "mutateOperations".to_owned(),
        Value::Array(operations.to_vec()),
    );
    if validate_only {
        body.insert("validateOnly".to_owned(), Value::Bool(true));
    }
    let answer = google_ads_request(
        context,
        &format!("/customers/{customer}/googleAds:mutate"),
        GoogleAdsRequest::post(Value::Object(body)).changing(!validate_only),
    )
    .await?;
    let result = GOOGLE_ADS_MUTATION_VALIDATOR.validate(&answer).map_err(|_| {
        BuiltinError::tool(
            "Google Ads did not confirm the change. It may still have been applied, so check before retrying.",
        )
    })?;
    let result: MutationAnswer = serde_json::from_value(result)?;

    Ok(result
        .mutate_operation_responses
        .iter()
        .map(|response| {
            let changed = response.values().next();
            js::get(changed, "resourceName")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

/// The last number of a resource name: `456` in `customers/123/campaigns/456`, `789` in `…/adGroupAds/456~789`.
pub fn resource_id(resource_name: Option<&str>) -> Option<String> {
    let resource_name = resource_name?;
    let digits = resource_name
        .bytes()
        .rev()
        .take_while(u8::is_ascii_digit)
        .count();
    (digits > 0).then(|| resource_name[resource_name.len() - digits..].to_owned())
}
