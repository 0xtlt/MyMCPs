//! The port of `tests/unit/builtin_strava.spec.ts`, and of the cases of
//! `tests/functional/builtin_strava_mcp.spec.ts` that are about the tools.
//!
//! The cases about the runtime are not here: what a sign-in may list or
//! call, write access turned off, and the renewal of tokens.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use http::{Method, StatusCode};
use mymcps_builtin::BuiltinRegistry;
use mymcps_builtin::oauth::{builtin_authorization_url, requested_builtin_scopes};
use mymcps_core::models::Mcp;
use mymcps_net::CannedResponse;
use mymcps_strava::payload::{compact_strava_payload, downsample_streams};
use serde_json::{Value, json};

use crate::support::{
    STRAVA, assert_json, connected_strava, default_strava, merge, mock_strava, pairs, provider,
    strava_fixtures, strava_json,
};

const READ_TOOLS: [&str; 17] = [
    "get_athlete",
    "get_athlete_stats",
    "get_athlete_zones",
    "list_activities",
    "get_activity",
    "get_activity_streams",
    "get_activity_zones",
    "list_activity_comments",
    "list_activity_kudos",
    "list_starred_segments",
    "get_segment",
    "explore_segments",
    "list_segment_efforts",
    "list_routes",
    "get_route",
    "list_clubs",
    "get_gear",
];

const WRITE_TOOLS: [&str; 4] = [
    "create_activity",
    "update_activity",
    "update_athlete_weight",
    "star_segment",
];

fn epoch_seconds(iso: &str) -> String {
    iso.parse::<DateTime<Utc>>()
        .unwrap()
        .timestamp()
        .to_string()
}

// Built-in Strava MCP: payloads

#[test]
fn builds_the_strava_authorization_url_with_comma_separated_scopes() {
    let oauth = STRAVA.oauth().unwrap();
    let url = builtin_authorization_url(
        oauth,
        "123456",
        "https://mcp.example.com/mcps/oauth/callback",
        "state-value",
        &oauth.scopes,
    )
    .unwrap();
    let url = url::Url::parse(&url).unwrap();

    assert_eq!(
        format!("{}{}", url.origin().ascii_serialization(), url.path()),
        "https://www.strava.com/oauth/authorize"
    );
    let parameters: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert_eq!(
        parameters,
        pairs([
            ("client_id", "123456"),
            (
                "redirect_uri",
                "https://mcp.example.com/mcps/oauth/callback"
            ),
            ("response_type", "code"),
            ("scope", "read,read_all,profile:read_all,activity:read_all"),
            ("state", "state-value"),
            ("approval_prompt", "force"),
        ])
    );
}

#[test]
fn only_knows_the_registered_built_in_keys() {
    let registry = BuiltinRegistry::new(vec![mymcps_strava::definition()]);

    let strava = registry.get(Some("strava")).unwrap();
    assert_eq!((strava.key(), strava.name()), ("strava", "Strava"));
    assert!(registry.get(Some("constructor")).is_none());
    assert!(registry.get(None).is_none());
}

#[test]
fn describes_the_api_application_the_admin_registers() {
    let oauth = STRAVA.oauth().unwrap();

    assert!(STRAVA.password().is_none());
    assert_eq!(oauth.issuer, "https://www.strava.com");
    assert_eq!(oauth.token_url, "https://www.strava.com/api/v3/oauth/token");
    // Strava does not document the redirect URI of a code exchange.
    assert!(!oauth.sends_redirect_uri_with_code);
    assert_eq!(
        oauth.client_id_hint,
        Some("The Strava Client ID is a number, such as 123456")
    );
    let pattern = oauth.client_id_pattern.as_ref().unwrap();
    assert!(pattern.is_match("123456"));
    // A secret pasted into the wrong field, and digits that are not ASCII.
    for refused in ["a1b2c3d4e5", "", "123 456", "12\n", "١٢٣"] {
        assert!(!pattern.is_match(refused), "{refused:?}");
    }
    assert!(STRAVA.settings().is_empty());
    assert!(!STRAVA.has_download() && !STRAVA.has_upload());
}

#[test]
fn drops_noise_nulls_and_imprecise_numeric_route_identifiers() {
    let compacted = compact_strava_payload(json!({
        "id": 2984453279043963000_i64,
        "id_str": "2984453279043962922",
        "name": "Col loop",
        "resource_state": 3,
        "description": null,
        "map": { "id": "r1", "summary_polyline": "encoded" },
        "athlete": { "id": 4242, "resource_state": 1, "profile": "https://images.example/a.jpg" },
        "segments": [{ "id": 7, "resource_state": 2, "map": {}, "average_grade": 5.5 }],
    }));

    assert_json(
        &compacted,
        &json!({
            "id": "2984453279043962922",
            "name": "Col loop",
            "athlete": { "id": 4242 },
            "segments": [{ "id": 7, "average_grade": 5.5 }],
        }),
    );
}

#[test]
fn downsamples_every_stream_at_the_same_evenly_spaced_positions() {
    let time: Vec<u32> = (0..1000).collect();
    let heartrate: Vec<u32> = time.iter().map(|second| 100 + second % 60).collect();
    let result = downsample_streams(
        &json!({
            "time": { "data": time, "original_size": 1000 },
            "heartrate": { "data": heartrate, "original_size": 1000 },
            "ignored": { "resolution": "high" },
        }),
        5,
    );

    assert_json(
        &result,
        &json!({
            "original_points": 1000,
            "returned_points": 5,
            "streams": {
                "time": [0, 250, 500, 749, 999],
                "heartrate": [100, 110, 120, 129, 139],
            },
        }),
    );

    let short = downsample_streams(&json!({ "time": { "data": [0, 1, 2] } }), 200);
    assert_json(
        &short,
        &json!({ "original_points": 3, "returned_points": 3, "streams": { "time": [0, 1, 2] } }),
    );
}

// Built-in Strava MCP: tools

/// What is left here of "lists tools without calling Strava and hides tools
/// whose scope was unchecked": the tools there are, and what each needs. The
/// runtime decides from it what a sign-in is shown.
#[test]
fn lists_its_tools_with_the_permission_each_needs() {
    let tools = STRAVA.tools();
    let named = |write: bool| -> Vec<&str> {
        tools
            .iter()
            .filter(|tool| tool.write == write)
            .map(|tool| tool.name)
            .collect()
    };
    assert_eq!(named(false), READ_TOOLS);
    assert_eq!(named(true), WRITE_TOOLS);

    for tool in &tools {
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        assert!(tool.description.len() > 20, "{}", tool.name);
        assert!(!tool.asks_approval, "{}", tool.name);
    }

    let needs = |name: &str| STRAVA.tool(name).unwrap().requires_any_scope.to_vec();
    let activity = ["activity:read", "activity:read_all"];
    // An authorization that only has `read` keeps the first four.
    for name in [
        "get_athlete",
        "get_athlete_stats",
        "list_starred_segments",
        "get_segment",
        "explore_segments",
        "list_routes",
        "get_route",
        "list_clubs",
        "get_gear",
    ] {
        assert!(needs(name).is_empty(), "{name}");
    }
    assert_eq!(needs("get_athlete_zones"), ["profile:read_all"]);
    for name in [
        "list_activities",
        "get_activity",
        "get_activity_streams",
        "get_activity_zones",
        "list_activity_comments",
        "list_activity_kudos",
        "list_segment_efforts",
    ] {
        assert_eq!(needs(name), activity, "{name}");
    }
    // The athlete may uncheck one of the two write permissions on Strava.
    assert_eq!(needs("create_activity"), ["activity:write"]);
    assert_eq!(needs("update_activity"), ["activity:write"]);
    assert_eq!(needs("update_athlete_weight"), ["profile:write"]);
    assert_eq!(needs("star_segment"), ["profile:write"]);
}

#[tokio::test]
async fn lists_activities_with_iso_dates_converted_to_epoch_seconds_and_trimmed_summaries() {
    let mcp = connected_strava(default_strava()).await;
    let result = mcp
        .call(
            "list_activities",
            json!({ "after": "2026-09-01", "before": "2026-10-01T12:00:00+02:00", "per_page": "5" }),
        )
        .await
        .unwrap();

    let requests = mcp.strava.api_requests();
    let request = &requests[0];
    assert_eq!(request.path(), "/api/v3/athlete/activities");
    assert_eq!(
        request.query(),
        pairs([
            ("after", &epoch_seconds("2026-09-01T00:00:00Z")),
            ("before", &epoch_seconds("2026-10-01T10:00:00Z")),
            ("page", "1"),
            ("per_page", "5"),
        ])
    );
    assert_eq!(
        request.authorization.as_deref(),
        Some("Bearer strava-access-token")
    );

    let activity = &result[0];
    assert_eq!(activity["id"], 15000000001_i64);
    assert_eq!(activity["name"], "Morning Run");
    assert_eq!(activity["average_heartrate"], 152.3);
    for dropped in ["map", "external_id", "average_watts"] {
        assert!(activity.get(dropped).is_none(), "{dropped}");
    }
    assert!(!result.to_string().contains("encoded-polyline"));
    assert_json(
        &result,
        &json!([{
            "id": 15000000001_i64,
            "name": "Morning Run",
            "sport_type": "Run",
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
            "pr_count": 1,
            "achievement_count": 2,
            "kudos_count": 7,
            "comment_count": 1,
            "gear_id": "g202",
            "trainer": false,
            "commute": false,
            "manual": false,
            "private": false,
        }]),
    );
}

#[tokio::test]
async fn returns_one_activity_without_segment_efforts_unless_they_are_requested() {
    let detail = merge(
        strava_fixtures::activity(),
        json!({
            "resource_state": 3,
            "description": "Felt great",
            "calories": 640,
            "laps": [{ "id": 1, "resource_state": 2, "lap_index": 1, "distance": 1000 }],
            "segment_efforts": [{ "id": 99, "name": "Riverside sprint", "resource_state": 2 }],
        }),
    );
    let strava = mock_strava(move |request| {
        (request.path() == "/api/v3/activities/15000000001")
            .then(|| strava_json(detail.clone(), 200))
    });
    let mcp = connected_strava(strava).await;

    let compact = mcp
        .call("get_activity", json!({ "activity_id": 15000000001_i64 }))
        .await
        .unwrap();
    assert_eq!(compact["description"], "Felt great");
    assert_json(
        &compact["laps"],
        &json!([{ "id": 1, "lap_index": 1, "distance": 1000 }]),
    );
    assert!(compact.get("segment_efforts").is_none());
    assert!(
        mcp.strava.api_requests()[0]
            .query_value("include_all_efforts")
            .is_none()
    );
    assert_eq!(mcp.strava.api_requests()[0].url.query(), None);

    let full = mcp
        .call(
            "get_activity",
            json!({ "activity_id": "15000000001", "include_segment_efforts": true }),
        )
        .await
        .unwrap();
    assert_json(
        &full["segment_efforts"],
        &json!([{ "id": 99, "name": "Riverside sprint" }]),
    );
    assert_eq!(
        mcp.strava.api_requests()[1]
            .query_value("include_all_efforts")
            .as_deref(),
        Some("true")
    );
}

#[tokio::test]
async fn requests_streams_by_type_and_returns_downsampled_series() {
    let data: Vec<u32> = (0..600).collect();
    let strava = mock_strava(move |request| {
        (request.path() == "/api/v3/activities/7/streams").then(|| {
            strava_json(
                json!({
                    "time": { "data": data, "original_size": 600, "resolution": "high", "series_type": "distance" },
                    "heartrate": { "data": vec![150; 600], "original_size": 600 },
                }),
                200,
            )
        })
    });
    let mcp = connected_strava(strava).await;
    let result = mcp
        .call(
            "get_activity_streams",
            json!({ "activity_id": 7, "keys": ["time", "heartrate", "time"], "max_points": 3 }),
        )
        .await
        .unwrap();

    let requests = mcp.strava.api_requests();
    assert_eq!(
        requests[0].query(),
        pairs([("keys", "time,heartrate"), ("key_by_type", "true")])
    );
    assert_json(
        &result,
        &json!({
            "original_points": 600,
            "returned_points": 3,
            "streams": { "time": [0, 300, 599], "heartrate": [150, 150, 150] },
        }),
    );
}

#[tokio::test]
async fn resolves_the_athlete_before_athlete_scoped_endpoints() {
    let strava = mock_strava(|request| match request.path() {
        "/api/v3/athletes/4242/stats" => Some(strava_json(
            json!({ "all_run_totals": { "count": 310, "distance": 3100000 } }),
            200,
        )),
        "/api/v3/athletes/4242/routes" => Some(strava_json(
            json!([{ "id": 2984453279043963000_i64, "id_str": "2984453279043962922", "name": "Loop" }]),
            200,
        )),
        _ => None,
    });
    let mcp = connected_strava(strava).await;

    let stats = mcp.call("get_athlete_stats", json!({})).await.unwrap();
    assert_json(
        &stats,
        &json!({ "all_run_totals": { "count": 310, "distance": 3100000 } }),
    );

    let routes = mcp.call("list_routes", json!({})).await.unwrap();
    assert_json(
        &routes,
        &json!([{ "id": "2984453279043962922", "name": "Loop" }]),
    );
    let requests = mcp.strava.api_requests();
    let paths: Vec<&str> = requests.iter().map(|request| request.path()).collect();
    assert_eq!(
        paths,
        [
            "/api/v3/athlete",
            "/api/v3/athletes/4242/stats",
            "/api/v3/athlete",
            "/api/v3/athletes/4242/routes",
        ]
    );
    assert_eq!(
        requests[3].query(),
        pairs([("page", "1"), ("per_page", "30")])
    );
}

#[tokio::test]
async fn rejects_invalid_arguments_before_calling_strava() {
    let mcp = connected_strava(default_strava()).await;
    let cases = [
        ("get_activity", json!({}), "activity_id is required"),
        (
            "get_activity",
            json!({ "activity_id": "12/../../athlete" }),
            "activity_id must be an integer",
        ),
        (
            "get_activity",
            json!({ "activity_id": 0 }),
            "activity_id must be an integer of at least 1",
        ),
        (
            "get_gear",
            json!({ "gear_id": "../athlete" }),
            "gear_id must be a gear identifier",
        ),
        (
            "get_route",
            json!({ "route_id": "12?x=1" }),
            "route_id must be a numeric route identifier",
        ),
        (
            "list_activities",
            json!({ "after": "last week" }),
            "after must be an ISO 8601 date",
        ),
        (
            "list_activities",
            json!({ "per_page": 500 }),
            "per_page must be an integer between 1 and 100",
        ),
        (
            "get_activity_streams",
            json!({ "activity_id": 1, "keys": ["pace"] }),
            "keys must be a non-empty",
        ),
        (
            "explore_segments",
            json!({ "south_west_lat": 45 }),
            "south_west_lng is required",
        ),
        (
            "explore_segments",
            json!({ "south_west_lat": 120 }),
            "south_west_lat must be a number between",
        ),
    ];

    for (tool, arguments, message) in cases {
        let refusal = mcp.refusal(tool, arguments.clone()).await;
        assert!(refusal.contains(message), "{tool} {arguments}: {refusal}");
    }
    // "Unknown Strava tool: delete_everything" is what the runtime answers.
    assert!(provider().tool("delete_everything").is_none());
    assert!(mcp.strava.requests().is_empty());
}

#[tokio::test]
async fn explains_strava_failures_to_the_agent_without_leaking_credentials() {
    let strava = mock_strava(|request| {
        Some(match request.path() {
            "/api/v3/activities/401" => strava_json(
                json!({
                    "message": "Authorization Error",
                    "errors": [{ "resource": "Athlete", "field": "access_token", "code": "invalid" }],
                }),
                401,
            ),
            "/api/v3/activities/4011" => strava_json(
                json!({
                    "message": "Authorization Error",
                    "errors": [{ "resource": "AccessToken", "field": "activity:read_permission", "code": "missing" }],
                }),
                401,
            ),
            "/api/v3/activities/402/zones" => {
                strava_json(json!({ "message": "Payment Required", "errors": [] }), 402)
            }
            "/api/v3/activities/404" => strava_json(
                json!({
                    "message": "Record Not Found",
                    "errors": [{ "resource": "Activity", "field": "", "code": "not found" }],
                }),
                404,
            ),
            "/api/v3/activities/429" => strava_json(
                json!({ "message": "Rate Limit Exceeded", "errors": [] }),
                429,
            )
            .header("X-ReadRateLimit-Limit", "100,1000")
            .unwrap()
            .header("X-ReadRateLimit-Usage", "101,420")
            .unwrap(),
            "/api/v3/activities/500" => {
                CannedResponse::new(StatusCode::BAD_GATEWAY).body("<html>Bad gateway</html>")
            }
            _ => return None,
        })
    });
    let mcp = connected_strava(strava).await;
    let call = async |tool: &str, id: u32| {
        let error = mcp
            .call(tool, json!({ "activity_id": id }))
            .await
            .unwrap_err();
        assert!(error.is_tool_error(), "{error:?}");
        assert!(!error.to_string().contains("strava-access-token"));
        error
    };

    let rejected = call("get_activity", 401).await;
    assert_eq!(
        rejected.to_string(),
        "Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs."
    );
    assert!(rejected.is_authorization_error());

    let missing = call("get_activity", 4011).await;
    assert_eq!(
        missing.to_string(),
        "Strava permission \"activity:read\" was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked."
    );
    // The authorization still works for what was granted.
    assert!(!missing.is_authorization_error());

    assert_eq!(
        call("get_activity_zones", 402).await.to_string(),
        "Strava only returns this data to athletes with a subscription."
    );
    assert_eq!(
        call("get_activity", 404).await.to_string(),
        "Strava could not find this resource: Record Not Found (Activity not found)"
    );
    assert_eq!(
        call("get_activity", 429).await.to_string(),
        "Strava rate limit reached (101 of 100 requests in 15 minutes, 420 of 1000 today). The 15-minute window resets on the quarter hour and the daily window at midnight UTC."
    );
    assert_eq!(
        call("get_activity", 500).await.to_string(),
        "Strava API returned HTTP 502"
    );
}

// Built-in Strava MCP: authorization lifecycle

/// What is left here of "reports connection health from one authenticated
/// Strava request": the request that proves the sign-in, and how it fails.
#[tokio::test]
async fn verifies_the_sign_in_with_one_authenticated_strava_request() {
    let athlete_status = Arc::new(Mutex::new(200));
    let strava = mock_strava({
        let athlete_status = Arc::clone(&athlete_status);
        move |request| {
            let status = *athlete_status.lock().unwrap();
            (request.path() == "/api/v3/athlete" && status != 200).then(|| {
                strava_json(
                    json!({
                        "message": "Authorization Error",
                        "errors": [{ "resource": "Athlete", "field": "access_token", "code": "invalid" }],
                    }),
                    status,
                )
            })
        }
    });
    let mcp = connected_strava(strava).await;

    provider().verify(Arc::clone(&mcp.context)).await.unwrap();
    let requests = mcp.strava.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(
        requests[0].url.as_str(),
        "https://www.strava.com/api/v3/athlete"
    );
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer strava-access-token")
    );

    *athlete_status.lock().unwrap() = 401;
    let rejected = provider()
        .verify(Arc::clone(&mcp.context))
        .await
        .unwrap_err();
    assert!(rejected.is_authorization_error());
    assert_eq!(
        rejected.to_string(),
        "Strava rejected the saved authorization. Re-authorize this MCP in MyMCPs."
    );

    // Strava being down says nothing about the authorization.
    *athlete_status.lock().unwrap() = 503;
    let unavailable = provider()
        .verify(Arc::clone(&mcp.context))
        .await
        .unwrap_err();
    assert!(!unavailable.is_authorization_error());
    assert_eq!(
        unavailable.to_string(),
        "Strava API returned HTTP 503: Authorization Error (Athlete access_token invalid)"
    );
}

// Built-in Strava MCP: write tools

#[test]
fn only_requests_write_scopes_once_write_access_is_allowed() {
    let oauth = STRAVA.oauth().unwrap();
    let read_only = Mcp::default();
    let writable = Mcp {
        builtin_write_enabled: true,
        ..Mcp::default()
    };

    assert_eq!(
        requested_builtin_scopes(oauth, &read_only),
        ["read", "read_all", "profile:read_all", "activity:read_all"]
    );
    assert_eq!(
        requested_builtin_scopes(oauth, &writable),
        [
            "read",
            "read_all",
            "profile:read_all",
            "activity:read_all",
            "activity:write",
            "profile:write",
        ]
    );
}

#[tokio::test]
async fn creates_a_manual_activity_with_the_local_start_time_kept_as_written() {
    let strava = mock_strava(|request| {
        (request.method == Method::POST && request.path() == "/api/v3/activities").then(|| {
            strava_json(
                merge(
                    strava_fixtures::activity(),
                    json!({ "id": 15000000099_i64, "name": "Evening yoga", "segment_efforts": [] }),
                ),
                201,
            )
        })
    });
    let mcp = connected_strava(strava).await;
    let created = mcp
        .call(
            "create_activity",
            json!({
                "name": "  Evening yoga ",
                "sport_type": "Yoga",
                "start_date_local": "2026-10-03T19:30:00+02:00",
                "elapsed_time": 3600,
                "distance": 0,
                "description": "Hip mobility",
                "trainer": true,
            }),
        )
        .await
        .unwrap();

    let requests = mcp.strava.api_requests();
    let request = &requests[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(
        request.authorization.as_deref(),
        Some("Bearer strava-access-token")
    );
    assert_eq!(request.url.query(), None);
    assert_eq!(
        request.form,
        Some(pairs([
            ("name", "Evening yoga"),
            ("sport_type", "Yoga"),
            ("start_date_local", "2026-10-03T19:30:00Z"),
            ("elapsed_time", "3600"),
            ("distance", "0"),
            ("description", "Hip mobility"),
            ("trainer", "1"),
        ]))
    );

    assert_eq!(created["id"], 15000000099_i64);
    assert!(created.get("map").is_none());
    assert!(created.get("segment_efforts").is_none());
}

#[tokio::test]
async fn updates_only_the_activity_fields_that_were_passed() {
    let strava = mock_strava(|request| {
        (request.method == Method::PUT && request.path() == "/api/v3/activities/15000000001").then(
            || {
                strava_json(
                    merge(
                        strava_fixtures::activity(),
                        json!({ "name": "Tempo run", "description": "" }),
                    ),
                    200,
                )
            },
        )
    });
    let mcp = connected_strava(strava).await;
    let updated = mcp
        .call(
            "update_activity",
            json!({
                "activity_id": 15000000001_i64,
                "name": "Tempo run",
                "description": "",
                "gear_id": "none",
                "commute": false,
            }),
        )
        .await
        .unwrap();

    let requests = mcp.strava.api_requests();
    let request = &requests[0];
    assert_eq!(request.method, Method::PUT);
    assert!(request.form.is_none());
    assert_json(
        request.json.as_ref().unwrap(),
        &json!({ "name": "Tempo run", "description": "", "gear_id": "none", "commute": false }),
    );
    assert_eq!(updated["name"], "Tempo run");
}

#[tokio::test]
async fn updates_the_athlete_weight_and_stars_or_unstars_a_segment() {
    let strava = mock_strava(|request| {
        if request.method != Method::PUT {
            return None;
        }
        match request.path() {
            "/api/v3/athlete" => Some(strava_json(
                merge(strava_fixtures::athlete(), json!({ "weight": 68.4 })),
                200,
            )),
            "/api/v3/segments/229781/starred" => Some(strava_json(
                json!({ "id": 229781, "name": "Hawk Hill", "starred": false, "resource_state": 3 }),
                200,
            )),
            _ => None,
        }
    });
    let mcp = connected_strava(strava).await;

    let athlete = mcp
        .call("update_athlete_weight", json!({ "weight": 68.4 }))
        .await
        .unwrap();
    assert_eq!(athlete["weight"], 68.4);

    let segment = mcp
        .call(
            "star_segment",
            json!({ "segment_id": 229781, "starred": false }),
        )
        .await
        .unwrap();
    assert_json(
        &segment,
        &json!({ "id": 229781, "name": "Hawk Hill", "starred": false }),
    );

    let requests = mcp.strava.api_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::PUT);
    assert_eq!(requests[0].path(), "/api/v3/athlete");
    assert_eq!(requests[0].form, Some(pairs([("weight", "68.4")])));
    assert_eq!(requests[1].method, Method::PUT);
    assert_eq!(requests[1].path(), "/api/v3/segments/229781/starred");
    assert_eq!(requests[1].form, Some(pairs([("starred", "false")])));
}

#[tokio::test]
async fn rejects_invalid_write_arguments_before_calling_strava() {
    let mcp = connected_strava(default_strava()).await;
    let activity = json!({
        "name": "Ride",
        "sport_type": "Ride",
        "start_date_local": "2026-10-03T08:00:00",
        "elapsed_time": 1800,
    });
    let with = |extra: Value| merge(activity.clone(), extra);
    let cases = [
        (
            "create_activity",
            with(json!({ "name": "   " })),
            "name is required",
        ),
        (
            "create_activity",
            with(json!({ "sport_type": "ride; DROP" })),
            "sport_type must be",
        ),
        (
            "create_activity",
            with(json!({ "start_date_local": "tomorrow" })),
            "start_date_local must",
        ),
        (
            "create_activity",
            with(json!({ "elapsed_time": 0 })),
            "elapsed_time must be an integer",
        ),
        (
            "create_activity",
            with(json!({ "distance": -5 })),
            "distance must be a number",
        ),
        (
            "update_activity",
            json!({ "activity_id": 1 }),
            "Pass at least one field to change",
        ),
        (
            "update_activity",
            json!({ "activity_id": 1, "name": " " }),
            "name must not be empty",
        ),
        (
            "update_activity",
            json!({ "activity_id": 1, "gear_id": "../x" }),
            "gear_id must be",
        ),
        (
            "update_activity",
            json!({ "name": "No id" }),
            "activity_id is required",
        ),
        (
            "update_athlete_weight",
            json!({ "weight": 4 }),
            "weight must be a number between 20 and 400",
        ),
        (
            "star_segment",
            json!({ "segment_id": "abc" }),
            "segment_id must be an integer",
        ),
    ];

    for (tool, arguments, message) in cases {
        let refusal = mcp.refusal(tool, arguments.clone()).await;
        assert!(refusal.contains(message), "{tool} {arguments}: {refusal}");
    }
    assert!(mcp.strava.requests().is_empty());
}

#[tokio::test]
async fn explains_write_failures_reported_by_strava() {
    let responses = VecDeque::from([
        strava_json(
            json!({
                "message": "Authorization Error",
                "errors": [{ "resource": "AccessToken", "field": "activity:write_permission", "code": "missing" }],
            }),
            401,
        ),
        strava_json(
            json!({
                "message": "Bad Request",
                "errors": [{ "resource": "Activity", "field": "sport_type", "code": "invalid" }],
            }),
            400,
        ),
        strava_json(
            json!({ "message": "Rate Limit Exceeded", "errors": [] }),
            429,
        )
        .header("X-RateLimit-Limit", "200,2000")
        .unwrap()
        .header("X-RateLimit-Usage", "201,640")
        .unwrap()
        .header("X-ReadRateLimit-Limit", "100,1000")
        .unwrap()
        .header("X-ReadRateLimit-Usage", "12,340")
        .unwrap(),
    ]);
    let responses = Mutex::new(responses);
    let strava = mock_strava(move |request| {
        (request.method == Method::PUT)
            .then(|| responses.lock().unwrap().pop_front())
            .flatten()
    });
    let mcp = connected_strava(strava).await;
    let rename = async || {
        mcp.refusal(
            "update_activity",
            json!({ "activity_id": 7, "name": "New" }),
        )
        .await
    };

    assert_eq!(
        rename().await,
        "Strava permission \"activity:write\" was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked."
    );
    assert_eq!(
        rename().await,
        "Strava API returned HTTP 400: Bad Request (Activity sport_type invalid)"
    );
    // A write only counts against the overall limit, not the one of reads.
    assert_eq!(
        rename().await,
        "Strava rate limit reached (201 of 200 requests in 15 minutes, 640 of 2000 today). The 15-minute window resets on the quarter hour and the daily window at midnight UTC."
    );
}

// Built-in Strava MCP: gateway (`tests/functional/builtin_strava_mcp.spec.ts`)

/// The calls the gateway cases make, as the tools answer them.
#[tokio::test]
async fn answers_the_calls_the_gateway_passes_on() {
    let strava = mock_strava(|request| {
        (request.method == Method::PUT && request.path() == "/api/v3/activities/15000000001").then(
            || {
                strava_json(
                    json!({ "id": 15000000001_i64, "name": "Renamed by an agent" }),
                    200,
                )
            },
        )
    });
    let mcp = connected_strava(strava).await;

    let listed = mcp
        .call("list_activities", json!({ "per_page": 1 }))
        .await
        .unwrap();
    assert_eq!(listed[0]["name"], "Morning Run");

    assert_eq!(
        mcp.refusal("get_activity", json!({ "activity_id": 404 }))
            .await,
        "Strava could not find this resource: Record Not Found (Resource not found)"
    );

    let athlete = mcp.call("get_athlete", json!({})).await.unwrap();
    assert_json(
        &athlete,
        &json!({
            "id": 4242,
            "firstname": "Test",
            "lastname": "Athlete",
            "city": "Lyon",
            "country": "France",
            "weight": 70.5,
            "measurement_preference": "meters",
            "bikes": [{ "id": "b101", "name": "Road bike", "distance": 1250000 }],
            "shoes": [],
        }),
    );

    let renamed = mcp
        .call(
            "update_activity",
            json!({ "activity_id": 15000000001_i64, "name": "Renamed by an agent" }),
        )
        .await
        .unwrap();
    assert_eq!(renamed["name"], "Renamed by an agent");
    let requests = mcp.strava.api_requests();
    let update = requests
        .iter()
        .find(|request| request.method == Method::PUT)
        .unwrap();
    assert_json(
        update.json.as_ref().unwrap(),
        &json!({ "name": "Renamed by an agent" }),
    );
}

// Where the port differs from the TypeScript

/// JavaScript reads every number as a double, so an identifier above 2^53
/// came back rounded: `3145986239223939600` here. Rust reads it whole.
#[tokio::test]
async fn hands_back_the_numbers_of_strava_whole_also_those_javascript_rounds() {
    let strava = mock_strava(|request| {
        (request.path() == "/api/v3/segment_efforts").then(|| {
            CannedResponse::new(StatusCode::OK).body(
                r#"[{"id":3145986239223939584,"elapsed_time":553.0,"segment":{"id":229781,"distance":2684.82}}]"#,
            )
        })
    });
    let mcp = connected_strava(strava).await;

    let efforts = mcp
        .call("list_segment_efforts", json!({ "segment_id": 229781 }))
        .await
        .unwrap();
    // A whole number Strava wrote with a fraction is written without, as JavaScript does.
    assert_eq!(
        efforts.to_string(),
        r#"[{"id":3145986239223939584,"elapsed_time":553,"segment":{"id":229781,"distance":2684.82}}]"#
    );
}

/// The TypeScript failed on an activity that is `null`, unless its segment
/// efforts were asked for. Here it is handed back either way.
#[tokio::test]
async fn hands_back_an_activity_strava_answers_nothing_for() {
    let strava = mock_strava(|request| {
        (request.path() == "/api/v3/activities/7").then(|| strava_json(Value::Null, 200))
    });
    let mcp = connected_strava(strava).await;

    for arguments in [
        json!({ "activity_id": 7 }),
        json!({ "activity_id": 7, "include_segment_efforts": true }),
    ] {
        assert_eq!(
            mcp.call("get_activity", arguments).await.unwrap(),
            Value::Null
        );
    }
}
