//! The tools of the built-in Strava MCP.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use http::Method;
use mymcps_builtin::arguments::{NO_ARGUMENTS_VALIDATOR, NoArguments, to_iso};
use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinTool, BuiltinToolContext};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::{Params, StravaRequest, strava_get, strava_request};
use crate::payload::{activity_summaries, compact_strava_payload, downsample_streams};
use crate::validators::{
    ACTIVITY_PAGE_VALIDATOR, ACTIVITY_VALIDATOR, CREATE_ACTIVITY_VALIDATOR,
    EXPLORE_SEGMENTS_VALIDATOR, GEAR_VALIDATOR, GET_ACTIVITY_STREAMS_VALIDATOR,
    GET_ACTIVITY_VALIDATOR, LIST_ACTIVITIES_VALIDATOR, LIST_SEGMENT_EFFORTS_VALIDATOR,
    PAGE_VALIDATOR, ROUTE_VALIDATOR, SEGMENT_VALIDATOR, STAR_SEGMENT_VALIDATOR,
    STRAVA_ATHLETE_VALIDATOR, STRAVA_LIMITS, STRAVA_STREAM_KEYS, UPDATE_ACTIVITY_VALIDATOR,
    UPDATE_ATHLETE_WEIGHT_VALIDATOR,
};

const UNITS: &str = "Distances and elevations are in meters, durations in seconds, and speeds in meters per second.";

const ACTIVITY_SCOPES: [&str; 2] = ["activity:read", "activity:read_all"];

const DEFAULT_STREAM_KEYS: [&str; 7] = [
    "time",
    "distance",
    "altitude",
    "velocity_smooth",
    "heartrate",
    "cadence",
    "watts",
];

const SPORT_TYPE_HINT: &str = "Strava sport type in PascalCase, such as Run, TrailRun, Ride, GravelRide, MountainBikeRide, VirtualRide, Swim, Walk, Hike, WeightTraining, Workout, or Yoga.";

const DEFAULT_PAGE_SIZE: u64 = 30;
const DEFAULT_STREAM_POINTS: usize = 200;

type Context = Arc<BuiltinToolContext>;

fn pagination_properties() -> Value {
    json!({
        "page": { "type": "integer", "minimum": 1, "default": 1, "description": "Page number, starting at 1." },
        "per_page": {
            "type": "integer",
            "minimum": 1,
            "maximum": STRAVA_LIMITS.page_size,
            "default": DEFAULT_PAGE_SIZE,
            "description": "Number of items per page.",
        },
    })
}

fn activity_id_property() -> Value {
    json!({
        "activity_id": {
            "type": "integer",
            "description": "Activity identifier, as returned by list_activities.",
        },
    })
}

/// The properties of several objects as those of one, in the order they are given.
fn properties<const N: usize>(parts: [Value; N]) -> Value {
    let mut merged = Map::new();
    for part in parts {
        if let Value::Object(part) = part {
            merged.extend(part);
        }
    }
    Value::Object(merged)
}

fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

#[derive(Debug, Deserialize)]
struct Page {
    page: Option<u64>,
    per_page: Option<u64>,
}

fn pagination(page: &Page) -> Params {
    Params::new()
        .with("page", page.page.unwrap_or(1))
        .with("per_page", page.per_page.unwrap_or(DEFAULT_PAGE_SIZE))
}

#[derive(Debug, Deserialize)]
struct ListActivities {
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Debug, Deserialize)]
struct GetActivity {
    include_segment_efforts: Option<bool>,
    activity_id: u64,
}

#[derive(Debug, Deserialize)]
struct GetActivityStreams {
    max_points: Option<usize>,
    activity_id: u64,
    keys: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct Activity {
    activity_id: u64,
}

#[derive(Debug, Deserialize)]
struct ActivityPage {
    activity_id: u64,
    #[serde(flatten)]
    page: Page,
}

#[derive(Debug, Deserialize)]
struct Segment {
    segment_id: u64,
}

#[derive(Debug, Deserialize)]
struct ExploreSegments {
    south_west_lat: f64,
    south_west_lng: f64,
    north_east_lat: f64,
    north_east_lng: f64,
    activity_type: Option<String>,
    min_climb_category: Option<u64>,
    max_climb_category: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ListSegmentEfforts {
    segment_id: u64,
    start_date: Option<DateTime<Utc>>,
    end_date: Option<DateTime<Utc>>,
    per_page: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Route {
    route_id: String,
}

#[derive(Debug, Deserialize)]
struct Gear {
    gear_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateActivity {
    name: String,
    sport_type: String,
    start_date_local: String,
    elapsed_time: u64,
    distance: Option<f64>,
    description: Option<String>,
    trainer: Option<bool>,
    commute: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UpdateActivity {
    activity_id: u64,
    /// The fields that were passed, in the order the validator lists them.
    #[serde(flatten)]
    changes: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateAthleteWeight {
    weight: f64,
}

#[derive(Debug, Deserialize)]
struct StarSegment {
    segment_id: u64,
    starred: Option<bool>,
}

/// The activity as get_activity returns it, without its segment efforts.
fn activity_result(activity: Value) -> Value {
    let mut compacted = compact_strava_payload(activity);
    if let Value::Object(activity) = &mut compacted {
        activity.shift_remove("segment_efforts");
    }
    compacted
}

async fn athlete_id(context: &BuiltinToolContext) -> BuiltinResult<String> {
    #[derive(Deserialize)]
    struct Athlete {
        id: Value,
    }

    let athlete = strava_get(context, "/athlete", Params::new()).await?;
    match STRAVA_ATHLETE_VALIDATOR.validate_as::<Athlete>(&athlete) {
        // Written into a path as JavaScript writes a number.
        Ok(athlete) => Ok(vine::js::to_string(&athlete.id)),
        Err(_) => Err(BuiltinError::tool(
            "Strava did not return the connected athlete",
        )),
    }
}

/// A moment without its milliseconds when they are zero: Luxon's
/// `toISO({ suppressMilliseconds: true })`.
fn iso_to_the_second(moment: DateTime<Utc>) -> String {
    let iso = to_iso(moment);
    match iso.strip_suffix(".000Z") {
        Some(seconds) => format!("{seconds}Z"),
        None => iso,
    }
}

pub fn strava_tools() -> Vec<BuiltinTool<BuiltinToolContext>> {
    vec![
        BuiltinTool::new(
            "get_athlete",
            "Get the connected Strava athlete's profile: name, location, weight, FTP, measurement preference, and their bikes and shoes with the distance on each.",
            no_arguments(),
            &NO_ARGUMENTS_VALIDATOR,
            |_: NoArguments, context: Context| async move {
                let athlete = strava_get(&context, "/athlete", Params::new()).await?;
                Ok(compact_strava_payload(athlete))
            },
        ),
        BuiltinTool::new(
            "get_athlete_stats",
            format!(
                "Get the connected athlete's ride, run, and swim totals for the last 4 weeks, the current year, and all time, plus their longest ride and biggest climb. Strava only counts activities visible to Everyone. {UNITS}"
            ),
            no_arguments(),
            &NO_ARGUMENTS_VALIDATOR,
            |_: NoArguments, context: Context| async move {
                let path = format!("/athletes/{}/stats", athlete_id(&context).await?);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, Params::new()).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "get_athlete_zones",
            "Get the connected athlete's heart rate and power zone boundaries.",
            no_arguments(),
            &NO_ARGUMENTS_VALIDATOR,
            |_: NoArguments, context: Context| async move {
                let zones = strava_get(&context, "/athlete/zones", Params::new()).await?;
                Ok(compact_strava_payload(zones))
            },
        )
        .requires_any_scope(&["profile:read_all"]),
        BuiltinTool::new(
            "list_activities",
            format!(
                "List the connected athlete's activities with summary metrics: distance, time, elevation, speed, heart rate, and power. Results are newest first, or oldest first when \"after\" is set. Call get_activity for the full detail of one activity. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": properties([
                    json!({
                        "after": {
                            "type": "string",
                            "description": "Only activities that started after this ISO 8601 date or datetime, such as 2026-01-31.",
                        },
                        "before": {
                            "type": "string",
                            "description": "Only activities that started before this ISO 8601 date or datetime.",
                        },
                    }),
                    pagination_properties(),
                ]),
                "additionalProperties": false,
            }),
            &LIST_ACTIVITIES_VALIDATOR,
            |input: ListActivities, context: Context| async move {
                let query = Params::new()
                    .with("after", input.after.map(|after| after.timestamp()))
                    .with("before", input.before.map(|before| before.timestamp()))
                    .and(pagination(&input.page));
                Ok(activity_summaries(
                    strava_get(&context, "/athlete/activities", query).await?,
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "get_activity",
            format!(
                "Get one activity in full: description, calories, device, gear, splits, laps, and best efforts. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": properties([
                    activity_id_property(),
                    json!({
                        "include_segment_efforts": {
                            "type": "boolean",
                            "default": false,
                            "description": "Also return every segment effort of the activity. This makes the result much larger.",
                        },
                    }),
                ]),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &GET_ACTIVITY_VALIDATOR,
            |input: GetActivity, context: Context| async move {
                let include_segment_efforts = input.include_segment_efforts.unwrap_or(false);
                let path = format!("/activities/{}", input.activity_id);
                let query = Params::new().with(
                    "include_all_efforts",
                    include_segment_efforts.then_some(true),
                );
                let activity = strava_get(&context, &path, query).await?;
                Ok(if include_segment_efforts {
                    compact_strava_payload(activity)
                } else {
                    activity_result(activity)
                })
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "get_activity_streams",
            format!(
                "Get the time series recorded during an activity, such as heart rate, power, cadence, speed, and altitude. Each stream is reduced to at most max_points evenly spaced samples that share the same positions, so index i of every stream is the same moment. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": properties([
                    activity_id_property(),
                    json!({
                        "keys": {
                            "type": "array",
                            "items": { "type": "string", "enum": STRAVA_STREAM_KEYS },
                            "default": DEFAULT_STREAM_KEYS,
                            "description": "Streams to return. Strava omits streams the activity did not record. \"latlng\" contains GPS coordinates.",
                        },
                        "max_points": {
                            "type": "integer",
                            "minimum": 2,
                            "maximum": STRAVA_LIMITS.stream_points,
                            "default": DEFAULT_STREAM_POINTS,
                            "description": "Maximum number of samples per stream.",
                        },
                    }),
                ]),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &GET_ACTIVITY_STREAMS_VALIDATOR,
            |input: GetActivityStreams, context: Context| async move {
                let keys = match &input.keys {
                    Some(keys) => keys.join(","),
                    None => DEFAULT_STREAM_KEYS.join(","),
                };
                let path = format!("/activities/{}/streams", input.activity_id);
                let query = Params::new().with("keys", keys).with("key_by_type", true);
                let streams = strava_get(&context, &path, query).await?;
                Ok(downsample_streams(
                    &streams,
                    input.max_points.unwrap_or(DEFAULT_STREAM_POINTS),
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "get_activity_zones",
            "Get the time spent in each heart rate and power zone during an activity, in seconds. Strava requires a subscription for this data.",
            json!({
                "type": "object",
                "properties": activity_id_property(),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &ACTIVITY_VALIDATOR,
            |input: Activity, context: Context| async move {
                let path = format!("/activities/{}/zones", input.activity_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, Params::new()).await?,
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "list_activity_comments",
            "List the comments on an activity.",
            json!({
                "type": "object",
                "properties": properties([activity_id_property(), pagination_properties()]),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &ACTIVITY_PAGE_VALIDATOR,
            |input: ActivityPage, context: Context| async move {
                let path = format!("/activities/{}/comments", input.activity_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, pagination(&input.page)).await?,
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "list_activity_kudos",
            "List the athletes who gave kudos to an activity.",
            json!({
                "type": "object",
                "properties": properties([activity_id_property(), pagination_properties()]),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &ACTIVITY_PAGE_VALIDATOR,
            |input: ActivityPage, context: Context| async move {
                let path = format!("/activities/{}/kudos", input.activity_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, pagination(&input.page)).await?,
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "list_starred_segments",
            format!("List the segments the connected athlete starred. {UNITS}"),
            json!({
                "type": "object",
                "properties": pagination_properties(),
                "additionalProperties": false,
            }),
            &PAGE_VALIDATOR,
            |page: Page, context: Context| async move {
                Ok(compact_strava_payload(
                    strava_get(&context, "/segments/starred", pagination(&page)).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "get_segment",
            format!(
                "Get a segment: distance, grade, elevation, effort counts, and the connected athlete's personal record and effort count on it. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": { "segment_id": { "type": "integer", "description": "Segment identifier." } },
                "required": ["segment_id"],
                "additionalProperties": false,
            }),
            &SEGMENT_VALIDATOR,
            |input: Segment, context: Context| async move {
                let path = format!("/segments/{}", input.segment_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, Params::new()).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "explore_segments",
            format!(
                "Find up to 10 popular segments inside a latitude/longitude bounding box. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "south_west_lat": { "type": "number", "minimum": -90, "maximum": 90 },
                    "south_west_lng": { "type": "number", "minimum": -180, "maximum": 180 },
                    "north_east_lat": { "type": "number", "minimum": -90, "maximum": 90 },
                    "north_east_lng": { "type": "number", "minimum": -180, "maximum": 180 },
                    "activity_type": { "type": "string", "enum": ["riding", "running"], "default": "riding" },
                    "min_climb_category": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "Lowest climb category to include, from 0 (uncategorized) to 5 (hardest).",
                    },
                    "max_climb_category": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "Highest climb category to include, from 0 (uncategorized) to 5 (hardest).",
                    },
                },
                "required": ["south_west_lat", "south_west_lng", "north_east_lat", "north_east_lng"],
                "additionalProperties": false,
            }),
            &EXPLORE_SEGMENTS_VALIDATOR,
            |input: ExploreSegments, context: Context| async move {
                let bounds = [
                    input.south_west_lat,
                    input.south_west_lng,
                    input.north_east_lat,
                    input.north_east_lng,
                ];
                let query = Params::new()
                    .with("bounds", bounds.map(vine::js::number_to_string).join(","))
                    .with("activity_type", input.activity_type)
                    .with("min_cat", input.min_climb_category)
                    .with("max_cat", input.max_climb_category);
                Ok(compact_strava_payload(
                    strava_get(&context, "/segments/explore", query).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "list_segment_efforts",
            format!(
                "List the connected athlete's efforts on one segment, optionally within a date range. Strava requires a subscription for this data. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "segment_id": { "type": "integer", "description": "Segment identifier." },
                    "start_date": {
                        "type": "string",
                        "description": "Only efforts on or after this ISO 8601 date or datetime.",
                    },
                    "end_date": {
                        "type": "string",
                        "description": "Only efforts on or before this ISO 8601 date or datetime.",
                    },
                    "per_page": pagination_properties()["per_page"],
                },
                "required": ["segment_id"],
                "additionalProperties": false,
            }),
            &LIST_SEGMENT_EFFORTS_VALIDATOR,
            |input: ListSegmentEfforts, context: Context| async move {
                let query = Params::new()
                    .with("segment_id", input.segment_id)
                    .with("start_date_local", input.start_date.map(iso_to_the_second))
                    .with("end_date_local", input.end_date.map(iso_to_the_second))
                    .with("per_page", input.per_page.unwrap_or(DEFAULT_PAGE_SIZE));
                Ok(compact_strava_payload(
                    strava_get(&context, "/segment_efforts", query).await?,
                ))
            },
        )
        .requires_any_scope(&ACTIVITY_SCOPES),
        BuiltinTool::new(
            "list_routes",
            format!("List the routes the connected athlete created. {UNITS}"),
            json!({
                "type": "object",
                "properties": pagination_properties(),
                "additionalProperties": false,
            }),
            &PAGE_VALIDATOR,
            |page: Page, context: Context| async move {
                let path = format!("/athletes/{}/routes", athlete_id(&context).await?);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, pagination(&page)).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "get_route",
            format!(
                "Get a route: distance, elevation gain, estimated moving time, and the segments along it. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "route_id": {
                        "type": "string",
                        "description": "Route identifier, as returned by list_routes. Pass it as a string.",
                    },
                },
                "required": ["route_id"],
                "additionalProperties": false,
            }),
            &ROUTE_VALIDATOR,
            |input: Route, context: Context| async move {
                let path = format!("/routes/{}", input.route_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, Params::new()).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "list_clubs",
            "List the clubs the connected athlete is a member of.",
            json!({
                "type": "object",
                "properties": pagination_properties(),
                "additionalProperties": false,
            }),
            &PAGE_VALIDATOR,
            |page: Page, context: Context| async move {
                Ok(compact_strava_payload(
                    strava_get(&context, "/athlete/clubs", pagination(&page)).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "get_gear",
            "Get a bike or pair of shoes: brand, model, and total distance in meters. Gear identifiers come from get_athlete and from the gear_id of an activity.",
            json!({
                "type": "object",
                "properties": {
                    "gear_id": { "type": "string", "description": "Gear identifier, such as b1234567 or g1234567." },
                },
                "required": ["gear_id"],
                "additionalProperties": false,
            }),
            &GEAR_VALIDATOR,
            |input: Gear, context: Context| async move {
                let path = format!("/gear/{}", input.gear_id);
                Ok(compact_strava_payload(
                    strava_get(&context, &path, Params::new()).await?,
                ))
            },
        ),
        BuiltinTool::new(
            "create_activity",
            format!(
                "Create a manual activity on the connected athlete's Strava account, such as a workout recorded without a device. It appears in their feed like any other activity. Strava has no API to delete an activity, so confirm the details with the user first. {UNITS}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "maxLength": STRAVA_LIMITS.name_length,
                        "description": "Activity title.",
                    },
                    "sport_type": { "type": "string", "description": SPORT_TYPE_HINT },
                    "start_date_local": {
                        "type": "string",
                        "description": "Start in the athlete's local time as an ISO 8601 date and time, such as 2026-01-31T18:00:00.",
                    },
                    "elapsed_time": { "type": "integer", "minimum": 1, "description": "Duration in seconds." },
                    "distance": { "type": "number", "minimum": 0, "description": "Distance in meters." },
                    "description": { "type": "string", "maxLength": STRAVA_LIMITS.description_length },
                    "trainer": { "type": "boolean", "description": "Recorded on an indoor trainer or treadmill." },
                    "commute": { "type": "boolean", "description": "Mark the activity as a commute." },
                },
                "required": ["name", "sport_type", "start_date_local", "elapsed_time"],
                "additionalProperties": false,
            }),
            &CREATE_ACTIVITY_VALIDATOR,
            |input: CreateActivity, context: Context| async move {
                // Strava reads these two flags from a form as 1 and 0.
                let flag = |value: Option<bool>| value.map(u64::from);
                let form = Params::new()
                    .with("name", input.name)
                    .with("sport_type", input.sport_type)
                    .with("start_date_local", input.start_date_local)
                    .with("elapsed_time", input.elapsed_time)
                    .with("distance", input.distance)
                    .with("description", input.description)
                    .with("trainer", flag(input.trainer))
                    .with("commute", flag(input.commute));
                let request = StravaRequest {
                    method: Method::POST,
                    form: Some(form),
                    ..StravaRequest::default()
                };
                Ok(activity_result(
                    strava_request(&context, "/activities", request).await?,
                ))
            },
        )
        .requires_any_scope(&["activity:write"])
        .write(),
        BuiltinTool::new(
            "update_activity",
            "Change an activity of the connected athlete: its title, description, sport type, gear, or its commute, trainer, and muted flags. Only the fields you pass are changed. Strava's API cannot change an activity's visibility, date, distance, or time.",
            json!({
                "type": "object",
                "properties": properties([
                    activity_id_property(),
                    json!({
                        "name": { "type": "string", "maxLength": STRAVA_LIMITS.name_length, "description": "New title." },
                        "description": {
                            "type": "string",
                            "maxLength": STRAVA_LIMITS.description_length,
                            "description": "New description. Pass an empty string to clear it.",
                        },
                        "sport_type": { "type": "string", "description": SPORT_TYPE_HINT },
                        "gear_id": {
                            "type": "string",
                            "description": "Gear identifier from get_athlete, such as b1234567, or \"none\" to remove the gear.",
                        },
                        "commute": { "type": "boolean" },
                        "trainer": { "type": "boolean" },
                        "hide_from_home": {
                            "type": "boolean",
                            "description": "Mute the activity so it stays out of followers' home feeds.",
                        },
                    }),
                ]),
                "required": ["activity_id"],
                "additionalProperties": false,
            }),
            &UPDATE_ACTIVITY_VALIDATOR,
            |input: UpdateActivity, context: Context| async move {
                if input.changes.is_empty() {
                    return Err(BuiltinError::tool("Pass at least one field to change"));
                }
                let path = format!("/activities/{}", input.activity_id);
                let request = StravaRequest {
                    method: Method::PUT,
                    json: Some(input.changes),
                    ..StravaRequest::default()
                };
                Ok(activity_result(
                    strava_request(&context, &path, request).await?,
                ))
            },
        )
        .requires_any_scope(&["activity:write"])
        .write(),
        BuiltinTool::new(
            "update_athlete_weight",
            "Set the connected athlete's weight on their Strava profile, in kilograms. Strava uses it to estimate power and calories.",
            json!({
                "type": "object",
                "properties": {
                    "weight": { "type": "number", "minimum": 20, "maximum": 400, "description": "Weight in kilograms." },
                },
                "required": ["weight"],
                "additionalProperties": false,
            }),
            &UPDATE_ATHLETE_WEIGHT_VALIDATOR,
            |input: UpdateAthleteWeight, context: Context| async move {
                let request = StravaRequest {
                    method: Method::PUT,
                    form: Some(Params::new().with("weight", input.weight)),
                    ..StravaRequest::default()
                };
                Ok(compact_strava_payload(
                    strava_request(&context, "/athlete", request).await?,
                ))
            },
        )
        .requires_any_scope(&["profile:write"])
        .write(),
        BuiltinTool::new(
            "star_segment",
            "Star or unstar a segment for the connected athlete.",
            json!({
                "type": "object",
                "properties": {
                    "segment_id": { "type": "integer", "description": "Segment identifier." },
                    "starred": {
                        "type": "boolean",
                        "default": true,
                        "description": "True to star the segment, false to unstar it.",
                    },
                },
                "required": ["segment_id"],
                "additionalProperties": false,
            }),
            &STAR_SEGMENT_VALIDATOR,
            |input: StarSegment, context: Context| async move {
                let path = format!("/segments/{}/starred", input.segment_id);
                let request = StravaRequest {
                    method: Method::PUT,
                    form: Some(Params::new().with("starred", input.starred.unwrap_or(true))),
                    ..StravaRequest::default()
                };
                Ok(compact_strava_payload(
                    strava_request(&context, &path, request).await?,
                ))
            },
        )
        .requires_any_scope(&["profile:write"])
        .write(),
    ]
}
