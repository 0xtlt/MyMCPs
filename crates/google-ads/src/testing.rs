//! A fake Google for tests: its token endpoint and the Google Ads API. The
//! port of `tests/helpers/google_ads.ts`, behind the `test-util` feature.
//!
//! ```
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! use mymcps_builtin::BuiltinEnv;
//! use mymcps_core::TestCore;
//! use mymcps_google_ads::testing::{FakeGoogleAds, GOOGLE_ADS_CUSTOMER, fixtures, google_ads_context, google_json};
//! use serde_json::{Map, json};
//!
//! // Answers the cases a test cares about, and leaves the rest to the defaults.
//! let google = FakeGoogleAds::responding(|request| {
//!     let query = request.json.as_ref()?.get("query")?.as_str()?;
//!     query.contains(" FROM campaign ").then(|| google_json(&json!({ "results": [fixtures::campaign()] }), 200))
//! });
//! let core = TestCore::new().await;
//! let env = BuiltinEnv::new(core.core.clone()).with_fetcher(google.fetcher());
//! let context = google_ads_context(env, &[("loginCustomerId", "9876543210")]);
//!
//! let definition = mymcps_google_ads::definition();
//! let mymcps_builtin::BuiltinMcpDefinition::Oauth { provider, .. } = &definition else { unreachable!() };
//! let mut arguments = Map::new();
//! arguments.insert("customer_id".into(), json!(GOOGLE_ADS_CUSTOMER));
//! let listed = provider.tool("list_campaigns").unwrap().run(arguments, context).await.unwrap();
//!
//! assert_eq!(listed["campaigns"][0]["name"], "Spring sale");
//! assert!(google.queries()[0].contains("segments.date DURING LAST_30_DAYS"));
//! assert_eq!(google.requests()[0].header("login-customer-id").as_deref(), Some("9876543210"));
//! # });
//! ```

use std::sync::{Arc, Mutex, PoisonError};

use http::{HeaderMap, Method, StatusCode};
use mymcps_builtin::{BuiltinEnv, BuiltinToolContext};
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use serde_json::{Map, Value, json};
use url::Url;

pub const GOOGLE_ADS_SCOPE: &str = "https://www.googleapis.com/auth/adwords";
pub const GOOGLE_ADS_CUSTOMER: &str = "1234567890";
/// The token the fake's token endpoint issues, and the one contexts are signed in with.
pub const GOOGLE_ADS_ACCESS_TOKEN: &str = "google-access-token";

const GOOGLE_ADS_ORIGIN: &str = "https://googleads.googleapis.com";
const GOOGLE_OAUTH_ORIGIN: &str = "https://oauth2.googleapis.com";

/// A request the fake received.
#[derive(Debug, Clone)]
pub struct GoogleAdsTestRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    /// The fields of a form body, such as a token request.
    pub form: Option<Vec<(String, String)>>,
    /// A JSON body.
    pub json: Option<Value>,
}

impl GoogleAdsTestRequest {
    fn of(request: &SentRequest) -> Self {
        let body = if request.method == Method::GET {
            String::new()
        } else {
            request.text()
        };
        let is_json = request
            .header("content-type")
            .is_some_and(|content_type| content_type.contains("application/json"));
        Self {
            method: request.method.clone(),
            url: request.url.clone(),
            headers: request.headers.clone(),
            form: (!body.is_empty() && !is_json).then(|| {
                url::form_urlencoded::parse(body.as_bytes())
                    .map(|(name, value)| (name.into_owned(), value.into_owned()))
                    .collect()
            }),
            json: (!body.is_empty() && is_json)
                .then(|| serde_json::from_str(&body).unwrap_or(Value::Null)),
        }
    }

    /// The value of a header, or `None` when it was not sent.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }

    /// The GAQL query of a report, or an empty text for any other request.
    pub fn query(&self) -> &str {
        self.json
            .as_ref()
            .and_then(|json| json.get("query"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    fn is_validation(&self) -> bool {
        self.json
            .as_ref()
            .and_then(|json| json.get("validateOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// A JSON answer of Google.
pub fn google_json(body: &Value, status: u16) -> CannedResponse {
    CannedResponse::json(StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body)
}

/// One mistake of a refused request: its kind and code, what Google says of
/// it, and where in the request it is.
#[derive(Debug, Clone, Default)]
pub struct GoogleAdsFault {
    /// Such as `("authorizationError", "USER_PERMISSION_DENIED")`.
    pub error_code: (&'static str, &'static str),
    pub message: &'static str,
    pub fields: Option<Vec<&'static str>>,
}

/// A refusal as the Google Ads API words it: one entry per mistake, each with its kind and code.
pub fn google_ads_failure(errors: &[GoogleAdsFault], status: u16) -> CannedResponse {
    let errors: Vec<Value> = errors
        .iter()
        .map(
            |GoogleAdsFault {
                 error_code: (kind, code),
                 message,
                 fields,
             }| {
                let mut error = Map::new();
                error.insert("errorCode".to_owned(), json!({ *kind: code }));
                error.insert("message".to_owned(), json!(message));
                if let Some(fields) = fields {
                    let elements: Vec<Value> = fields
                        .iter()
                        .map(|field| json!({ "fieldName": field }))
                        .collect();
                    error.insert(
                        "location".to_owned(),
                        json!({ "fieldPathElements": elements }),
                    );
                }
                Value::Object(error)
            },
        )
        .collect();
    google_json(
        &json!({
            "error": {
                "code": status,
                "message": "Request contains an invalid argument.",
                "status": "INVALID_ARGUMENT",
                "details": [{
                    "@type": "type.googleapis.com/google.ads.googleads.v25.errors.GoogleAdsFailure",
                    "errors": errors,
                    "requestId": "test-request-id",
                }],
            },
        }),
        status,
    )
}

/// Rows shaped like real Google Ads API v25 answers: camelCase keys, 64-bit numbers as text.
pub mod fixtures {
    use serde_json::{Value, json};

    use super::{GOOGLE_ADS_ACCESS_TOKEN, GOOGLE_ADS_CUSTOMER, GOOGLE_ADS_SCOPE};

    pub fn account() -> Value {
        json!({
            "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}"),
            "id": GOOGLE_ADS_CUSTOMER,
            "descriptiveName": "Acme Shoes",
            "currencyCode": "EUR",
            "timeZone": "Europe/Paris",
            "manager": false,
            "testAccount": false,
            "status": "ENABLED",
        })
    }

    pub fn campaign() -> Value {
        json!({
            "campaign": {
                "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaigns/111"),
                "id": "111",
                "name": "Spring sale",
                "status": "ENABLED",
                "primaryStatus": "ELIGIBLE",
                "advertisingChannelType": "SEARCH",
                "biddingStrategyType": "TARGET_SPEND",
                "startDateTime": "2026-03-01 00:00:00",
                "networkSettings": {
                    "targetGoogleSearch": true,
                    "targetSearchNetwork": false,
                    "targetContentNetwork": false,
                },
                "campaignBudget": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaignBudgets/222"),
            },
            "campaignBudget": {
                "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaignBudgets/222"),
                "id": "222",
                "amountMicros": "2500000",
                "referenceCount": "1",
            },
            "customer": account(),
            "metrics": {
                "impressions": "12000",
                "clicks": "300",
                "costMicros": "150000000",
                "ctr": 0.025,
                "conversions": 12,
                "conversionsValue": 960,
            },
        })
    }

    pub fn ad_group() -> Value {
        json!({
            "adGroup": {
                "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/adGroups/333"),
                "id": "333",
                "name": "Running shoes",
                "status": "ENABLED",
                "type": "SEARCH_STANDARD",
                "cpcBidMicros": "1200000",
            },
            "campaign": {
                "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaigns/111"),
                "id": "111",
                "name": "Spring sale",
                "status": "ENABLED",
                "advertisingChannelType": "SEARCH",
            },
            "customer": account(),
        })
    }

    pub fn tokens() -> Value {
        json!({
            "access_token": GOOGLE_ADS_ACCESS_TOKEN,
            "expires_in": 3599,
            "refresh_token": "google-refresh-token",
            "scope": GOOGLE_ADS_SCOPE,
            "token_type": "Bearer",
        })
    }
}

/// What a mutate answers for one operation: the resource it made or changed.
fn mutate_result(operation: &Value, index: usize) -> Value {
    let Some((kind, change)) = operation
        .as_object()
        .and_then(|operation| operation.iter().next())
    else {
        return json!({});
    };
    let resource = kind.strip_suffix("Operation").unwrap_or(kind);
    let collection = match resource.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{resource}s"),
    };
    let named = change
        .get("remove")
        .filter(|removed| !removed.is_null())
        .or_else(|| {
            change
                .get("update")
                .and_then(|update| update.get("resourceName"))
        })
        .filter(|named| !named.is_null())
        .cloned();
    let resource_name = named.unwrap_or_else(|| {
        json!(format!(
            "customers/{GOOGLE_ADS_CUSTOMER}/{collection}/{}",
            9000 + index
        ))
    });
    json!({ format!("{resource}Result"): { "resourceName": resource_name } })
}

/// What Google answers when a test has nothing to say about a request.
fn default_answer(request: &GoogleAdsTestRequest) -> CannedResponse {
    if request.url.origin().ascii_serialization() == GOOGLE_OAUTH_ORIGIN {
        return google_json(&fixtures::tokens(), 200);
    }
    let path = request.url.path();
    if path == "/v25/customers:listAccessibleCustomers" {
        return google_json(
            &json!({ "resourceNames": [format!("customers/{GOOGLE_ADS_CUSTOMER}")] }),
            200,
        );
    }
    if path.ends_with("/googleAds:mutate") {
        if request.is_validation() {
            return google_json(&json!({}), 200);
        }
        let operations = request
            .json
            .as_ref()
            .and_then(|json| json.get("mutateOperations"))
            .and_then(Value::as_array);
        let responses: Vec<Value> = operations
            .map(|operations| {
                operations
                    .iter()
                    .enumerate()
                    .map(|(index, operation)| mutate_result(operation, index))
                    .collect()
            })
            .unwrap_or_default();
        return google_json(&json!({ "mutateOperationResponses": responses }), 200);
    }
    if path.ends_with("/googleAds:search") {
        let query = request.query();
        let results = if query.contains(" FROM customer ") {
            json!([{ "customer": fixtures::account() }])
        } else if query.contains(" FROM campaign ") {
            json!([fixtures::campaign()])
        } else if query.contains(" FROM ad_group ") {
            json!([fixtures::ad_group()])
        } else {
            json!([])
        };
        return google_json(&json!({ "results": results }), 200);
    }
    google_json(
        &json!({ "error": { "code": 404, "message": "Not found", "status": "NOT_FOUND" } }),
        404,
    )
}

type Requests = Arc<Mutex<Vec<GoogleAdsTestRequest>>>;

/// A fake Google: its token endpoint and the Google Ads API. It answers the
/// requests of the fetcher it hands out, remembers them, and lets any other
/// request fail as one the network could not carry. (`mockGoogleAds`)
#[derive(Clone)]
pub struct FakeGoogleAds {
    requests: Requests,
    fetcher: Fetcher,
}

impl Default for FakeGoogleAds {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeGoogleAds {
    /// A Google that gives its default answers: one account, one campaign, one ad group.
    pub fn new() -> Self {
        Self::responding(|_| None)
    }

    /// `respond` handles the cases a test cares about and returns `None` to
    /// fall back to the default answers.
    pub fn responding(
        respond: impl Fn(&GoogleAdsTestRequest) -> Option<CannedResponse> + Send + Sync + 'static,
    ) -> Self {
        let requests = Requests::default();
        let fetcher = Fetcher::offline().answering({
            let requests = requests.clone();
            move |sent| {
                let origin = sent.url.origin().ascii_serialization();
                if origin != GOOGLE_ADS_ORIGIN && origin != GOOGLE_OAUTH_ORIGIN {
                    return None;
                }
                let request = GoogleAdsTestRequest::of(sent);
                requests
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(request.clone());
                Some(respond(&request).unwrap_or_else(|| default_answer(&request)))
            }
        });
        Self { requests, fetcher }
    }

    /// What reaches this Google instead of the network: give it to `BuiltinEnv::with_fetcher`.
    pub fn fetcher(&self) -> Fetcher {
        self.fetcher.clone()
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<GoogleAdsTestRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn token_requests(&self) -> Vec<GoogleAdsTestRequest> {
        let is_token = |request: &GoogleAdsTestRequest| {
            request.url.origin().ascii_serialization() == GOOGLE_OAUTH_ORIGIN
        };
        self.requests().into_iter().filter(is_token).collect()
    }

    fn to(&self, suffix: &str) -> Vec<GoogleAdsTestRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.url.path().ends_with(suffix))
            .collect()
    }

    /// The GAQL queries sent, in order.
    pub fn queries(&self) -> Vec<String> {
        self.to("/googleAds:search")
            .iter()
            .map(|request| request.query().to_owned())
            .collect()
    }

    fn operations(&self, validations: bool) -> Vec<Value> {
        self.to("/googleAds:mutate")
            .into_iter()
            .filter(|request| request.is_validation() == validations)
            .map(|request| {
                request
                    .json
                    .and_then(|mut json| json.get_mut("mutateOperations").map(Value::take))
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The changes sent to be made, without the ones only sent to be checked.
    pub fn mutations(&self) -> Vec<Value> {
        self.operations(false)
    }

    /// The changes sent to be checked only.
    pub fn validations(&self) -> Vec<Value> {
        self.operations(true)
    }
}

/// What a tool of a connected Google Ads MCP runs with. `settings` is what the
/// admin entered, such as `("loginCustomerId", "9876543210")` for the manager
/// account the sign-in acts through, or `("customerIds", "1234567890 2345678901")`
/// for the accounts agents may use.
pub fn google_ads_context(env: BuiltinEnv, settings: &[(&str, &str)]) -> Arc<BuiltinToolContext> {
    Arc::new(BuiltinToolContext {
        env,
        mcp_id: 1,
        access_token: GOOGLE_ADS_ACCESS_TOKEN.to_owned(),
        granted_scopes: Some(vec![GOOGLE_ADS_SCOPE.to_owned()]),
        settings: settings
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    })
}
