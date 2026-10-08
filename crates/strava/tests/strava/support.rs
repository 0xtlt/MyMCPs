//! A fake Strava and an MCP connected to it: the port of
//! `tests/helpers/strava.ts`.
//!
//! The TypeScript tests call a tool through `callBuiltinTool(mcp, name, args)`,
//! which reads the sign-in from the `mcps` row. That runtime is another
//! crate's: here a tool runs with the context the runtime would hand it.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};

use http::{Method, StatusCode};
use mymcps_builtin::{
    BuiltinEnv, BuiltinMcpDefinition, BuiltinProvider, BuiltinResult, BuiltinToolContext,
};
use mymcps_core::TestCore;
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use serde_json::{Map, Value, json};
use url::Url;

pub static STRAVA: LazyLock<BuiltinMcpDefinition> = LazyLock::new(mymcps_strava::definition);

pub fn provider() -> &'static BuiltinProvider<BuiltinToolContext> {
    match &*STRAVA {
        BuiltinMcpDefinition::Oauth { provider, .. } => provider,
        BuiltinMcpDefinition::Password { .. } => panic!("Strava signs in with OAuth"),
    }
}

/// `{ ...base, ...extra }`
pub fn merge(base: Value, extra: Value) -> Value {
    let (Value::Object(mut merged), Value::Object(extra)) = (base, extra) else {
        panic!("two objects to merge");
    };
    merged.extend(extra);
    Value::Object(merged)
}

/// The same JSON, with its keys in the same order.
#[track_caller]
pub fn assert_json(actual: &Value, expected: &Value) {
    assert_eq!(actual.to_string(), expected.to_string());
}

#[derive(Debug, Clone)]
pub struct StravaTestRequest {
    pub method: Method,
    pub url: Url,
    pub authorization: Option<String>,
    pub form: Option<Vec<(String, String)>>,
    pub json: Option<Value>,
}

impl StravaTestRequest {
    fn of(sent: &SentRequest) -> Self {
        let body = if sent.method == Method::GET {
            String::new()
        } else {
            sent.text()
        };
        let is_json = sent
            .header("Content-Type")
            .is_some_and(|content_type| content_type.contains("application/json"));
        let form = |body: &str| {
            url::form_urlencoded::parse(body.as_bytes())
                .into_owned()
                .collect()
        };
        Self {
            method: sent.method.clone(),
            url: sent.url.clone(),
            authorization: sent.header("Authorization"),
            form: (!body.is_empty() && !is_json).then(|| form(&body)),
            json: (!body.is_empty() && is_json)
                .then(|| serde_json::from_str(&body).expect("a JSON body")),
        }
    }

    pub fn path(&self) -> &str {
        self.url.path()
    }

    /// The parameters of the query string, in the order they were sent.
    pub fn query(&self) -> Vec<(String, String)> {
        self.url.query_pairs().into_owned().collect()
    }

    pub fn query_value(&self, name: &str) -> Option<String> {
        self.query()
            .into_iter()
            .find_map(|(key, value)| (key == name).then_some(value))
    }
}

/// Names and values as the pairs a request is compared with.
pub fn pairs<const N: usize>(pairs: [(&str, &str); N]) -> Vec<(String, String)> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

pub fn strava_json(body: Value, status: u16) -> CannedResponse {
    CannedResponse::json(StatusCode::from_u16(status).expect("a status"), &body)
}

/// Shapes follow real Strava API v3 responses, including the fields agents never need.
pub mod strava_fixtures {
    use serde_json::{Value, json};

    pub fn athlete() -> Value {
        json!({
            "id": 4242,
            "resource_state": 3,
            "firstname": "Test",
            "lastname": "Athlete",
            "city": "Lyon",
            "country": "France",
            "weight": 70.5,
            "ftp": null,
            "profile": "https://images.example/large.jpg",
            "profile_medium": "https://images.example/medium.jpg",
            "badge_type_id": 1,
            "measurement_preference": "meters",
            "bikes": [{ "id": "b101", "name": "Road bike", "distance": 1250000, "resource_state": 2 }],
            "shoes": [],
        })
    }

    pub fn activity() -> Value {
        json!({
            "resource_state": 2,
            "athlete": { "id": 4242, "resource_state": 1 },
            "id": 15000000001_i64,
            "name": "Morning Run",
            "sport_type": "Run",
            "type": "Run",
            "start_date": "2026-09-30T05:30:00Z",
            "start_date_local": "2026-09-30T07:30:00Z",
            "timezone": "(GMT+01:00) Europe/Paris",
            "distance": 10012.4,
            "moving_time": 2890,
            "elapsed_time": 2950,
            "total_elevation_gain": 84.2,
            "average_speed": 3.464,
            "max_speed": 5.1,
            "average_heartrate": 152.3,
            "max_heartrate": 178,
            "average_watts": null,
            "kudos_count": 7,
            "comment_count": 1,
            "achievement_count": 2,
            "pr_count": 1,
            "trainer": false,
            "commute": false,
            "manual": false,
            "private": false,
            "gear_id": "g202",
            "upload_id": 16000000001_i64,
            "upload_id_str": "16000000001",
            "external_id": "garmin_ping_123",
            "has_kudoed": false,
            "map": { "id": "a15000000001", "summary_polyline": "encoded-polyline", "resource_state": 2 },
        })
    }
}

/// A fake Strava, and the requests it received.
#[derive(Clone)]
pub struct FakeStrava {
    requests: Arc<Mutex<Vec<StravaTestRequest>>>,
    fetcher: Fetcher,
}

impl FakeStrava {
    pub fn requests(&self) -> Vec<StravaTestRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn api_requests(&self) -> Vec<StravaTestRequest> {
        let mut requests = self.requests();
        requests.retain(|request| request.path() != "/api/v3/oauth/token");
        requests
    }
}

/// Answer the requests to Strava in place of the network. `respond` handles
/// the cases a test cares about and returns `None` to fall back to the
/// defaults below. A request to anywhere else fails.
pub fn mock_strava(
    respond: impl Fn(&StravaTestRequest) -> Option<CannedResponse> + Send + Sync + 'static,
) -> FakeStrava {
    let requests: Arc<Mutex<Vec<StravaTestRequest>>> = Arc::default();
    let fetcher = Fetcher::offline().answering({
        let requests = Arc::clone(&requests);
        move |sent| {
            if sent.url.origin().ascii_serialization() != "https://www.strava.com" {
                return None;
            }
            let request = StravaTestRequest::of(sent);
            requests.lock().unwrap().push(request.clone());

            let custom = respond(&request);
            Some(custom.unwrap_or_else(|| match request.path() {
                "/api/v3/athlete" => strava_json(strava_fixtures::athlete(), 200),
                "/api/v3/athlete/activities" => {
                    strava_json(json!([strava_fixtures::activity()]), 200)
                }
                _ => strava_json(
                    json!({
                        "message": "Record Not Found",
                        "errors": [{ "resource": "Resource", "field": "", "code": "not found" }],
                    }),
                    404,
                ),
            }))
        }
    });
    FakeStrava { requests, fetcher }
}

/// A fake Strava that only gives its default answers.
pub fn default_strava() -> FakeStrava {
    mock_strava(|_| None)
}

/// A built-in Strava MCP that is connected, as its tools see it.
pub struct ConnectedStrava {
    pub strava: FakeStrava,
    pub context: Arc<BuiltinToolContext>,
    _core: TestCore,
}

impl ConnectedStrava {
    /// Run a tool with the arguments an agent passed.
    pub async fn call(&self, tool: &str, arguments: Value) -> BuiltinResult<Value> {
        let tool = provider()
            .tool(tool)
            .unwrap_or_else(|| panic!("Strava has no tool {tool}"));
        let arguments: Map<String, Value> = arguments
            .as_object()
            .cloned()
            .expect("arguments are an object");
        tool.run(arguments, Arc::clone(&self.context)).await
    }

    /// What the agent reads when a tool refuses a call or fails.
    pub async fn refusal(&self, tool: &str, arguments: Value) -> String {
        let label = format!("{tool} {arguments}");
        match self.call(tool, arguments).await {
            Ok(result) => panic!("{label} returned {result}"),
            Err(error) => {
                assert!(error.is_tool_error(), "{label} failed with {error:?}");
                error.to_string()
            }
        }
    }
}

pub async fn connected_strava(strava: FakeStrava) -> ConnectedStrava {
    let core = TestCore::new().await;
    let env = BuiltinEnv::new(core.core.clone()).with_fetcher(strava.fetcher.clone());
    let granted = [
        "read",
        "read_all",
        "profile:read_all",
        "activity:read_all",
        "activity:write",
        "profile:write",
    ];
    let context = BuiltinToolContext {
        env,
        mcp_id: 1,
        access_token: "strava-access-token".to_owned(),
        granted_scopes: Some(granted.map(str::to_owned).to_vec()),
        settings: BTreeMap::new(),
    };
    ConnectedStrava {
        strava,
        context: Arc::new(context),
        _core: core,
    }
}
