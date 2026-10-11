//! The port of `tests/unit/vine_builtin_strava.spec.ts`.
//!
//! Two of its cases are elsewhere: the two shapes of a refused token request
//! are read by `mymcps_builtin::oauth`, and a refused renewal is explained by
//! the runtime that renews tokens.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use mymcps_builtin::BuiltinError;
use mymcps_builtin::arguments::to_iso;
use mymcps_builtin::tool_input::tool_input;
use mymcps_strava::validators::{
    ACTIVITY_PAGE_VALIDATOR, ACTIVITY_VALIDATOR, CREATE_ACTIVITY_VALIDATOR,
    EXPLORE_SEGMENTS_VALIDATOR, GEAR_VALIDATOR, GET_ACTIVITY_STREAMS_VALIDATOR,
    GET_ACTIVITY_VALIDATOR, LIST_ACTIVITIES_VALIDATOR, LIST_SEGMENT_EFFORTS_VALIDATOR,
    PAGE_VALIDATOR, ROUTE_VALIDATOR, SEGMENT_VALIDATOR, STAR_SEGMENT_VALIDATOR,
    STRAVA_ATHLETE_VALIDATOR, STRAVA_FAILURE_VALIDATOR, STRAVA_STREAM_KEYS,
    UPDATE_ACTIVITY_VALIDATOR, UPDATE_ATHLETE_WEIGHT_VALIDATOR,
};
use mymcps_vine::Validator;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::support::{
    assert_json, connected_strava, default_strava, merge, mock_strava, strava_json,
};

/// The arguments of a tool once they passed its validator.
fn input(validator: &Validator, arguments: &Value) -> Value {
    tool_input(validator, arguments).unwrap_or_else(|error| panic!("{arguments}: {error}"))
}

/// The sentence the agent reads when a tool refuses its arguments.
fn refusal(validator: &Validator, arguments: &Value) -> Option<String> {
    match tool_input::<Value>(validator, arguments) {
        Ok(_) => None,
        Err(BuiltinError::Tool(sentence)) => Some(sentence),
        Err(error) => panic!("{arguments}: {error:?}"),
    }
}

fn bounding_box() -> Value {
    json!({ "south_west_lat": 45.1, "south_west_lng": 4.5, "north_east_lat": 45.9, "north_east_lng": 5.2 })
}

fn activity() -> Value {
    json!({
        "name": "Ride",
        "sport_type": "Ride",
        "start_date_local": "2026-10-03T08:00:00",
        "elapsed_time": 1800,
    })
}

// Built-in Strava MCP: validators

#[test]
fn returns_the_arguments_of_each_tool_the_way_the_tool_uses_them() {
    let cases: Vec<(&Validator, Value, Value)> = vec![
        (&PAGE_VALIDATOR, json!({}), json!({})),
        (
            &PAGE_VALIDATOR,
            json!({ "page": "2", "per_page": 100, "other": 1 }),
            json!({ "page": 2, "per_page": 100 }),
        ),
        (
            &ACTIVITY_VALIDATOR,
            json!({ "activity_id": "15000000001" }),
            json!({ "activity_id": 15000000001_i64 }),
        ),
        (
            &ACTIVITY_PAGE_VALIDATOR,
            json!({ "activity_id": 7, "page": 3 }),
            json!({ "activity_id": 7, "page": 3 }),
        ),
        (
            &GET_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 7, "include_segment_efforts": "true" }),
            json!({ "include_segment_efforts": true, "activity_id": 7 }),
        ),
        (
            &GET_ACTIVITY_STREAMS_VALIDATOR,
            json!({ "activity_id": 7, "keys": ["time", "heartrate", "time"], "max_points": "3" }),
            json!({ "max_points": 3, "activity_id": 7, "keys": ["time", "heartrate"] }),
        ),
        (
            &GET_ACTIVITY_STREAMS_VALIDATOR,
            json!({ "activity_id": 7, "keys": null }),
            json!({ "activity_id": 7 }),
        ),
        (
            &SEGMENT_VALIDATOR,
            json!({ "segment_id": 229781 }),
            json!({ "segment_id": 229781 }),
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            merge(
                bounding_box(),
                json!({ "south_west_lat": "45.1", "activity_type": "running", "min_climb_category": 0 }),
            ),
            merge(
                bounding_box(),
                json!({ "activity_type": "running", "min_climb_category": 0 }),
            ),
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            merge(bounding_box(), json!({ "activity_type": "" })),
            bounding_box(),
        ),
        (
            &ROUTE_VALIDATOR,
            json!({ "route_id": 2984453279_i64 }),
            json!({ "route_id": "2984453279" }),
        ),
        (
            &ROUTE_VALIDATOR,
            json!({ "route_id": " 2984453279043962922 " }),
            json!({ "route_id": "2984453279043962922" }),
        ),
        (
            &GEAR_VALIDATOR,
            json!({ "gear_id": "b101" }),
            json!({ "gear_id": "b101" }),
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            merge(
                activity(),
                json!({ "name": "  Evening yoga ", "distance": "0", "description": "", "trainer": true }),
            ),
            merge(
                activity(),
                json!({
                    "name": "Evening yoga",
                    "start_date_local": "2026-10-03T08:00:00Z",
                    "distance": 0,
                    "description": "",
                    "trainer": true,
                }),
            ),
        ),
        (
            &UPDATE_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 7, "name": " Tempo run ", "description": "", "gear_id": "none", "commute": false }),
            json!({ "name": "Tempo run", "description": "", "gear_id": "none", "commute": false, "activity_id": 7 }),
        ),
        (
            &UPDATE_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 7, "sport_type": "", "gear_id": "" }),
            json!({ "activity_id": 7 }),
        ),
        (
            &UPDATE_ATHLETE_WEIGHT_VALIDATOR,
            json!({ "weight": "68.4" }),
            json!({ "weight": 68.4 }),
        ),
        (
            &STAR_SEGMENT_VALIDATOR,
            json!({ "segment_id": 229781 }),
            json!({ "segment_id": 229781 }),
        ),
        (
            &STAR_SEGMENT_VALIDATOR,
            json!({ "segment_id": 229781, "starred": "false" }),
            json!({ "segment_id": 229781, "starred": false }),
        ),
    ];

    for (validator, arguments, expected) in cases {
        assert_json(&input(validator, &arguments), &expected);
    }
}

#[test]
fn reads_the_dates_of_a_list_as_moments_in_time() {
    #[derive(Deserialize)]
    struct ListActivities {
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
        #[serde(flatten)]
        page: Map<String, Value>,
    }

    let arguments =
        json!({ "after": "2026-09-01", "before": "2026-10-01T12:00:00+02:00", "per_page": "5" });
    let input: ListActivities = tool_input(&LIST_ACTIVITIES_VALIDATOR, &arguments).unwrap();

    let first_of_september: DateTime<Utc> = "2026-09-01T00:00:00Z".parse().unwrap();
    assert_eq!(
        input.after.unwrap().timestamp(),
        first_of_september.timestamp()
    );
    assert_eq!(to_iso(input.before.unwrap()), "2026-10-01T10:00:00.000Z");
    assert_json(&Value::Object(input.page), &json!({ "per_page": 5 }));
}

#[test]
fn tells_the_agent_which_argument_is_wrong_and_what_it_must_be() {
    let boxed = |extra: Value| merge(bounding_box(), extra);
    let with = |extra: Value| merge(activity(), extra);
    let cases: Vec<(&Validator, Value, &str)> = vec![
        (
            &PAGE_VALIDATOR,
            json!({ "page": 0 }),
            "page must be an integer of at least 1",
        ),
        (
            &PAGE_VALIDATOR,
            json!({ "per_page": 101 }),
            "per_page must be an integer between 1 and 100",
        ),
        (&ACTIVITY_VALIDATOR, json!({}), "activity_id is required"),
        (
            &ACTIVITY_VALIDATOR,
            json!({ "activity_id": 0 }),
            "activity_id must be an integer of at least 1",
        ),
        (
            &ACTIVITY_VALIDATOR,
            json!({ "activity_id": "7/.." }),
            "activity_id must be an integer of at least 1",
        ),
        (
            &GET_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 7, "include_segment_efforts": "yes" }),
            "include_segment_efforts must be true or false",
        ),
        (
            &GET_ACTIVITY_STREAMS_VALIDATOR,
            json!({ "activity_id": 7, "max_points": 1 }),
            "max_points must be an integer between 2 and 1000",
        ),
        (
            &SEGMENT_VALIDATOR,
            json!({ "segment_id": "abc" }),
            "segment_id must be an integer of at least 1",
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            json!({ "south_west_lat": 45 }),
            "south_west_lng is required",
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            boxed(json!({ "south_west_lat": 120 })),
            "south_west_lat must be a number between -90 and 90",
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            boxed(json!({ "north_east_lng": 181 })),
            "north_east_lng must be a number between -180 and 180",
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            boxed(json!({ "activity_type": "walking" })),
            "activity_type must be one of: riding, running",
        ),
        (
            &EXPLORE_SEGMENTS_VALIDATOR,
            boxed(json!({ "max_climb_category": 6 })),
            "max_climb_category must be an integer between 0 and 5",
        ),
        (
            &LIST_SEGMENT_EFFORTS_VALIDATOR,
            json!({ "segment_id": 1, "start_date": "last week" }),
            "start_date must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z",
        ),
        (
            &ROUTE_VALIDATOR,
            json!({ "route_id": "12?x=1" }),
            "route_id must be a numeric route identifier",
        ),
        (&ROUTE_VALIDATOR, json!({}), "route_id is required"),
        (
            &GEAR_VALIDATOR,
            json!({ "gear_id": "../athlete" }),
            "gear_id must be a gear identifier such as b1234567",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "name": "   " })),
            "name is required",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "name": "x".repeat(256) })),
            "name must be text of at most 255 characters",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "sport_type": "ride; DROP" })),
            "sport_type must be a Strava sport type like Run",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "start_date_local": "tomorrow" })),
            "start_date_local must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "elapsed_time": 30 * 24 * 3600 + 1 })),
            "elapsed_time must be an integer between 1 and 2592000",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "distance": -5 })),
            "distance must be a number between 0 and 10000000",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "description": "x".repeat(5001) })),
            "description must be text of at most 5000 characters",
        ),
        (
            &CREATE_ACTIVITY_VALIDATOR,
            with(json!({ "commute": 1 })),
            "commute must be true or false",
        ),
        (
            &UPDATE_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 1, "name": " " }),
            "name must not be empty",
        ),
        (
            &UPDATE_ACTIVITY_VALIDATOR,
            json!({ "activity_id": 1, "gear_id": "../x" }),
            "gear_id must be a gear identifier such as b1234567, or \"none\"",
        ),
        (
            &UPDATE_ACTIVITY_VALIDATOR,
            json!({ "name": "No id" }),
            "activity_id is required",
        ),
        (
            &UPDATE_ATHLETE_WEIGHT_VALIDATOR,
            json!({ "weight": 4 }),
            "weight must be a number between 20 and 400",
        ),
        (
            &UPDATE_ATHLETE_WEIGHT_VALIDATOR,
            json!({}),
            "weight is required",
        ),
        (
            &STAR_SEGMENT_VALIDATOR,
            json!({ "segment_id": 1, "starred": "no" }),
            "starred must be true or false",
        ),
    ];

    for (validator, arguments, sentence) in cases {
        assert_eq!(
            refusal(validator, &arguments).as_deref(),
            Some(sentence),
            "{arguments}"
        );
    }
}

#[test]
fn names_the_first_wrong_argument_in_the_order_each_schema_lists_them() {
    assert_eq!(
        refusal(
            &GET_ACTIVITY_VALIDATOR,
            &json!({ "include_segment_efforts": "x" })
        )
        .as_deref(),
        Some("include_segment_efforts must be true or false")
    );
    assert_eq!(
        refusal(
            &GET_ACTIVITY_STREAMS_VALIDATOR,
            &json!({ "max_points": 1, "keys": [] })
        )
        .as_deref(),
        Some("max_points must be an integer between 2 and 1000")
    );
    assert_eq!(
        refusal(
            &UPDATE_ACTIVITY_VALIDATOR,
            &json!({ "name": 5, "description": 5 })
        )
        .as_deref(),
        Some("name must be text of at most 255 characters")
    );
    assert_eq!(
        refusal(
            &LIST_SEGMENT_EFFORTS_VALIDATOR,
            &json!({ "end_date": "x", "per_page": 0 })
        )
        .as_deref(),
        Some("segment_id is required")
    );
}

#[test]
fn accepts_the_streams_strava_records_once_each_and_names_them_otherwise() {
    let sentence = format!(
        "keys must be a non-empty array of: {}",
        STRAVA_STREAM_KEYS.join(", ")
    );
    let keys = |value: Value| {
        let arguments = json!({ "activity_id": 1, "keys": value });
        input(&GET_ACTIVITY_STREAMS_VALIDATOR, &arguments)["keys"].clone()
    };

    assert_eq!(keys(json!(STRAVA_STREAM_KEYS)), json!(STRAVA_STREAM_KEYS));
    assert_eq!(
        keys(json!(["watts", "watts", "latlng"])),
        json!(["watts", "latlng"])
    );
    let left_out = input(
        &GET_ACTIVITY_STREAMS_VALIDATOR,
        &json!({ "activity_id": 1 }),
    );
    assert!(left_out.get("keys").is_none());
    for value in [
        json!([]),
        json!(["pace"]),
        json!(["time", null]),
        json!(["time", 1]),
        json!("time"),
        json!(""),
        json!({}),
        json!(5),
    ] {
        let arguments = json!({ "activity_id": 1, "keys": value });
        assert_eq!(
            refusal(&GET_ACTIVITY_STREAMS_VALIDATOR, &arguments).as_deref(),
            Some(sentence.as_str()),
            "{value}"
        );
    }
}

#[test]
fn refuses_a_wrong_page_of_segment_efforts_although_strava_returns_a_single_one() {
    assert_json(
        &input(
            &LIST_SEGMENT_EFFORTS_VALIDATOR,
            &json!({ "segment_id": 1, "page": 3 }),
        ),
        &json!({ "segment_id": 1, "page": 3 }),
    );
    assert_eq!(
        refusal(
            &LIST_SEGMENT_EFFORTS_VALIDATOR,
            &json!({ "segment_id": 1, "page": "x" })
        )
        .as_deref(),
        Some("page must be an integer of at least 1")
    );
}

// Built-in Strava MCP: what Strava answers

#[test]
fn reads_the_identifier_of_the_athlete_and_nothing_that_is_not_a_number() {
    assert_json(
        &STRAVA_ATHLETE_VALIDATOR
            .validate(&json!({ "id": 4242, "firstname": "Test", "ftp": null }))
            .unwrap(),
        &json!({ "id": 4242 }),
    );
    for body in [
        json!({ "id": "4242" }),
        json!({ "id": null }),
        json!({}),
        json!([]),
        json!(null),
        json!("athlete"),
        json!(4242),
    ] {
        assert!(
            STRAVA_ATHLETE_VALIDATOR.try_validate(&body).is_err(),
            "{body}"
        );
    }
}

#[test]
fn reads_what_strava_says_about_a_failure_and_nothing_from_another_body() {
    let failure = json!({
        "message": "Bad Request",
        "errors": [{ "resource": "Activity", "field": "sport_type", "code": "invalid", "extra": 1 }],
        "documentation_url": "https://developers.strava.com",
    });
    assert_json(
        &STRAVA_FAILURE_VALIDATOR.validate(&failure).unwrap(),
        &json!({
            "message": "Bad Request",
            "errors": [{ "resource": "Activity", "field": "sport_type", "code": "invalid" }],
        }),
    );
    assert_json(
        &STRAVA_FAILURE_VALIDATOR
            .validate(&json!({ "message": null, "errors": null }))
            .unwrap(),
        &json!({}),
    );

    for body in [
        json!(null),
        json!([]),
        json!("<html>"),
        json!({ "message": 5 }),
        json!({ "errors": "none" }),
        json!({ "errors": [5] }),
    ] {
        assert!(
            STRAVA_FAILURE_VALIDATOR.try_validate(&body).is_err(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn refuses_to_guess_the_athlete_when_strava_does_not_name_one() {
    for athlete in [json!({ "id": "4242" }), json!({}), json!(null), json!([])] {
        let strava = mock_strava({
            let athlete = athlete.clone();
            move |request| {
                (request.path() == "/api/v3/athlete").then(|| strava_json(athlete.clone(), 200))
            }
        });
        let mcp = connected_strava(strava).await;

        assert_eq!(
            mcp.refusal("get_athlete_stats", json!({})).await,
            "Strava did not return the connected athlete",
            "{athlete}"
        );
        assert_eq!(mcp.strava.api_requests().len(), 1);
    }
}

#[tokio::test]
async fn explains_a_failure_from_the_status_alone_when_its_body_is_not_the_documented_one() {
    let bodies = VecDeque::from([
        json!({ "message": "Bad Request", "errors": [null] }),
        json!({ "message": { "text": "Bad Request" } }),
        json!({ "errors": [{ "resource": "Activity", "code": 400 }] }),
        json!(["Bad Request"]),
    ]);
    let count = bodies.len();
    let bodies = Mutex::new(bodies);
    let strava = mock_strava(move |request| {
        (request.path() == "/api/v3/activities/7")
            .then(|| bodies.lock().unwrap().pop_front())
            .flatten()
            .map(|body| strava_json(body, 400))
    });
    let mcp = connected_strava(strava).await;

    for _ in 0..count {
        assert_eq!(
            mcp.refusal("get_activity", json!({ "activity_id": 7 }))
                .await,
            "Strava API returned HTTP 400"
        );
    }
    assert_eq!(mcp.strava.api_requests().len(), count);
}

#[tokio::test]
async fn refuses_wrong_arguments_before_asking_strava_for_anything() {
    let mcp = connected_strava(default_strava()).await;
    let cases = [
        (
            "list_routes",
            json!({ "per_page": 500 }),
            "per_page must be an integer between 1 and 100",
        ),
        (
            "list_segment_efforts",
            json!({ "segment_id": 1, "page": 0 }),
            "page must be an integer of at least 1",
        ),
        (
            "update_activity",
            json!({ "activity_id": 1 }),
            "Pass at least one field to change",
        ),
        (
            "update_activity",
            json!({ "activity_id": 1, "sport_type": "" }),
            "Pass at least one field to change",
        ),
    ];

    for (tool, arguments, sentence) in cases {
        assert_eq!(mcp.refusal(tool, arguments).await, sentence, "{tool}");
    }
    assert!(mcp.strava.requests().is_empty());
}
