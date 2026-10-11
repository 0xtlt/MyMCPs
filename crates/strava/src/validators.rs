//! Vine schemas for the built-in Strava MCP: the arguments of its tools, and
//! the JSON Strava answers with. A schema lists the arguments in the order
//! they are checked: of several wrong ones, the agent is told about the first.

use std::sync::LazyLock;

use mymcps_builtin::arguments::{
    TOOL_VINE, VineArgument, argument, boolean, choice, integer, iso_date, local_timestamp, number,
    pattern, text, trimmed_text,
};
use mymcps_builtin::oauth::provider_faults;
use mymcps_vine as vine;
use serde_json::{Value, json};
use vine::{Rule, Schema, Validator};

/// The bounds the tools advertise, and the schemas below enforce.
#[derive(Debug, Clone, Copy)]
pub struct StravaLimits {
    pub page_size: i64,
    pub stream_points: i64,
    pub name_length: usize,
    pub description_length: usize,
}

pub const STRAVA_LIMITS: StravaLimits = StravaLimits {
    page_size: 100,
    stream_points: 1000,
    name_length: 255,
    description_length: 5000,
};

pub const STRAVA_STREAM_KEYS: [&str; 11] = [
    "time",
    "distance",
    "latlng",
    "altitude",
    "velocity_smooth",
    "heartrate",
    "cadence",
    "watts",
    "temp",
    "moving",
    "grade_smooth",
];

fn id() -> VineArgument {
    integer(1..)
}

fn pagination() -> Vec<(String, Schema)> {
    vine::properties! {
        "page" => integer(1..).optional(),
        "per_page" => integer(1..=STRAVA_LIMITS.page_size).optional(),
    }
}

fn sport_type() -> VineArgument {
    pattern(r"^[A-Z][A-Za-z]{1,39}$", "a Strava sport type like Run")
}

fn latitude() -> VineArgument {
    number(-90..=90)
}

fn longitude() -> VineArgument {
    number(-180..=180)
}

/// One sentence whatever is wrong with the list, so one rule for all of it.
fn stream_keys_rule() -> Rule {
    vine::rule(|value, field| {
        let known = |key: &Value| {
            key.as_str()
                .is_some_and(|key| STRAVA_STREAM_KEYS.contains(&key))
        };
        let keys = value
            .as_array()
            .filter(|keys| !keys.is_empty() && keys.iter().all(known));
        let Some(keys) = keys else {
            field.report_with(
                "{{ field }} must be a non-empty array of: {{ keys }}",
                "streamKeys",
                json!({ "keys": STRAVA_STREAM_KEYS.join(", ") }),
            );
            return;
        };
        // Each key once, where it first appears.
        let mut unique: Vec<Value> = Vec::with_capacity(keys.len());
        for key in keys {
            if !unique.contains(key) {
                unique.push(key.clone());
            }
        }
        field.mutate(unique);
    })
    .json_schema(|schema| {
        schema.insert("type".to_owned(), json!("array"));
        schema.insert(
            "items".to_owned(),
            json!({ "type": "string", "enum": STRAVA_STREAM_KEYS }),
        );
    })
}

fn not_blank_rule() -> Rule {
    vine::rule(|value, field| {
        let Some(text) = value.as_str() else { return };
        let trimmed = vine::js::trim(text);
        if trimmed.is_empty() {
            field.report("{{ field }} must not be empty", "notBlank");
            return;
        }
        field.mutate(trimmed);
    })
}

pub static LIST_ACTIVITIES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "after" => iso_date().optional(),
        "before" => iso_date().optional(),
        ..pagination(),
    })
});

pub static GET_ACTIVITY_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "include_segment_efforts" => boolean().optional(),
        "activity_id" => id(),
    })
});

pub static GET_ACTIVITY_STREAMS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "max_points" => integer(2..=STRAVA_LIMITS.stream_points).optional(),
        "activity_id" => id(),
        "keys" => argument(stream_keys_rule()).optional(),
    })
});

pub static ACTIVITY_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "activity_id" => id(),
    })
});

/// One page of what belongs to an activity, such as its comments.
pub static ACTIVITY_PAGE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "activity_id" => id(),
        ..pagination(),
    })
});

/// One page of what belongs to the athlete, such as their routes.
pub static PAGE_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| TOOL_VINE.create(vine::object(pagination())));

pub static SEGMENT_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "segment_id" => id(),
    })
});

pub static EXPLORE_SEGMENTS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "south_west_lat" => latitude(),
        "south_west_lng" => longitude(),
        "north_east_lat" => latitude(),
        "north_east_lng" => longitude(),
        "activity_type" => choice(["riding", "running"]).optional(),
        "min_climb_category" => integer(0..=5).optional(),
        "max_climb_category" => integer(0..=5).optional(),
    })
});

pub static LIST_SEGMENT_EFFORTS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "segment_id" => id(),
        "start_date" => iso_date().optional(),
        "end_date" => iso_date().optional(),
        // Only `per_page` is advertised and sent: Strava returns these efforts on
        // one page. A wrong `page` is refused all the same, as for the other lists.
        ..pagination(),
    })
});

pub static ROUTE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "route_id" => pattern(r"^\d{1,20}$", "a numeric route identifier"),
    })
});

pub static GEAR_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "gear_id" => pattern(r"^[bg]\d{1,20}$", "a gear identifier such as b1234567"),
    })
});

pub static CREATE_ACTIVITY_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "name" => trimmed_text(STRAVA_LIMITS.name_length),
        "sport_type" => sport_type(),
        "start_date_local" => local_timestamp(),
        "elapsed_time" => integer(1..=30 * 24 * 3600),
        "distance" => number(0..=10_000_000).optional(),
        "description" => text(STRAVA_LIMITS.description_length).optional(),
        "trainer" => boolean().optional(),
        "commute" => boolean().optional(),
    })
});

pub static UPDATE_ACTIVITY_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "name" => text(STRAVA_LIMITS.name_length).use_rule(not_blank_rule()).optional(),
        "description" => text(STRAVA_LIMITS.description_length).optional(),
        "sport_type" => sport_type().optional(),
        "gear_id" => pattern(
            r"^([bg]\d{1,20}|none)$",
            r#"a gear identifier such as b1234567, or "none""#,
        )
        .optional(),
        "commute" => boolean().optional(),
        "trainer" => boolean().optional(),
        "hide_from_home" => boolean().optional(),
        "activity_id" => id(),
    })
});

pub static UPDATE_ATHLETE_WEIGHT_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "weight" => number(20..=400),
    })
});

pub static STAR_SEGMENT_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "segment_id" => id(),
        "starred" => boolean().optional(),
    })
});

/// The athlete Strava answers with, of which only the identifier is read.
pub static STRAVA_ATHLETE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "id" => vine::number().strict(),
    })
});

/// The body of a response Strava sent with an error status. The reasons it
/// gives are the faults `mymcps_builtin::oauth` also reads from its token
/// endpoint.
pub static STRAVA_FAILURE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "message" => vine::string().optional(),
        "errors" => provider_faults().optional(),
    })
});
